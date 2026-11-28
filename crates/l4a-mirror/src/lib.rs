use futures_util::FutureExt;
use regex::Regex;
use reqwest::header::{HeaderValue, LOCATION, REFERER};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use thiserror::Error;

const DEFAULT_MAX_REDIRECTS: usize = 10;
const MAX_STREAMING_CLIENTS: usize = 16;
const MAX_CUSTOM_TIMEOUT_CLIENTS: usize = 32;
const MAX_REDIRECT_BODY_DRAIN_BYTES: usize = 4096;
const PAYLOAD_HEADERS: &[&str] = &[
    "content-type",
    "content-length",
    "content-encoding",
    "transfer-encoding",
];

#[derive(Debug, Error)]
pub enum MirrorError {
    #[error("读取镜像配置文件失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("解析镜像配置 JSON 失败: {0}")]
    Json(#[from] serde_json::Error),
    #[error("构建 HTTP 客户端失败: {0}")]
    ClientBuild(#[from] reqwest::Error),
    #[error("已启用仅 HTTPS 连接，请使用 HTTPS 地址")]
    HttpsRequired,
    #[error("重定向次数过多")]
    TooManyRedirects,
    #[error("无效的重定向目标地址: {0}")]
    InvalidRedirect(String),
}

#[derive(Debug, Error)]
pub enum RedirectSendError<E> {
    #[error(transparent)]
    Mirror(#[from] MirrorError),
    #[error(transparent)]
    Request(#[from] reqwest::Error),
    #[error(transparent)]
    Rejected(E),
    #[error("HTTP 请求超时")]
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MirrorRule {
    #[serde(default)]
    pub rules: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub subdomains: Option<Vec<String>>,
    #[serde(rename = "allow-www", default)]
    pub allow_www: bool,
    #[serde(default)]
    pub regex: Option<String>,
    pub target: String,
    #[serde(rename = "keep-host", default)]
    pub keep_host: bool,
    #[serde(rename = "insecure-tls", default)]
    pub insecure_tls: bool,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone)]
struct CompiledRule {
    regex: Regex,
    raw: MirrorRule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRequest {
    pub url: String,
    pub original_host: Option<String>,
    pub headers: HashMap<String, String>,
    pub insecure_tls: bool,
    /// A configured rule rewrote the authority or supplied request headers.
    pub matched_rule: bool,
}

impl ResolvedRequest {
    fn has_mirror_overrides(&self, effective_insecure_tls: bool) -> bool {
        effective_insecure_tls || self.original_host.is_some() || !self.headers.is_empty()
    }

    fn rewrites_authority(&self, source: &url::Url) -> bool {
        self.matched_rule
            && url::Url::parse(&self.url)
                .is_ok_and(|target| url_authority(&target) != url_authority(source))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RedirectRequestOptions {
    pub timeout: Option<Duration>,
    pub connect_timeout: Option<Duration>,
    pub read_timeout: Option<Duration>,
    pub https_only_override: Option<bool>,
    pub max_redirects: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct RedirectHop<'a> {
    pub hop: usize,
    pub url: &'a url::Url,
    pub same_origin: bool,
    pub matched_mirror_rule: bool,
    pub resolved: &'a ResolvedRequest,
}

#[derive(Debug)]
pub struct RedirectedResponse {
    pub response: Response,
    pub initial_resolved: ResolvedRequest,
    pub final_resolved: ResolvedRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RedirectMode {
    Follow,
    SameOriginOnly,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ClientPoolKey {
    https_only: bool,
    insecure_tls: bool,
    redirect_mode: RedirectMode,
    http1_only: bool,
    no_proxy: bool,
    timeout: Option<Duration>,
    connect_timeout: Option<Duration>,
    read_timeout: Option<Duration>,
}

impl ClientPoolKey {
    fn bucket(&self) -> PoolBucket {
        if self.timeout.is_some() {
            PoolBucket::CustomTimeout
        } else if self.connect_timeout.is_some() || self.read_timeout.is_some() {
            PoolBucket::Streaming
        } else {
            PoolBucket::Core
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PoolBucket {
    Core,
    Streaming,
    CustomTimeout,
}

#[derive(Debug, Default)]
struct ClientPool {
    core: HashMap<ClientPoolKey, Client>,
    streaming: HashMap<ClientPoolKey, Client>,
    streaming_order: VecDeque<ClientPoolKey>,
    custom_timeout: HashMap<ClientPoolKey, Client>,
    custom_timeout_order: VecDeque<ClientPoolKey>,
}

impl ClientPool {
    fn get(&self, key: &ClientPoolKey) -> Option<&Client> {
        match key.bucket() {
            PoolBucket::Core => self.core.get(key),
            PoolBucket::Streaming => self.streaming.get(key),
            PoolBucket::CustomTimeout => self.custom_timeout.get(key),
        }
    }

    fn insert(&mut self, key: ClientPoolKey, client: Client) {
        match key.bucket() {
            PoolBucket::Core => {
                self.core.insert(key, client);
            }
            PoolBucket::Streaming => {
                insert_bounded(
                    &mut self.streaming,
                    &mut self.streaming_order,
                    key,
                    client,
                    MAX_STREAMING_CLIENTS,
                );
            }
            PoolBucket::CustomTimeout => {
                insert_bounded(
                    &mut self.custom_timeout,
                    &mut self.custom_timeout_order,
                    key,
                    client,
                    MAX_CUSTOM_TIMEOUT_CLIENTS,
                );
            }
        }
    }

    #[cfg(test)]
    fn total_len(&self) -> usize {
        self.core.len() + self.streaming.len() + self.custom_timeout.len()
    }
}

fn insert_bounded(
    map: &mut HashMap<ClientPoolKey, Client>,
    order: &mut VecDeque<ClientPoolKey>,
    key: ClientPoolKey,
    client: Client,
    capacity: usize,
) {
    if let std::collections::hash_map::Entry::Occupied(mut existing) = map.entry(key) {
        existing.insert(client);
        return;
    }
    while map.len() >= capacity {
        let Some(oldest) = order.pop_front() else {
            break;
        };
        map.remove(&oldest);
    }
    order.push_back(key);
    map.insert(key, client);
}

/// 可注入、无全局状态的镜像路由器。
///
/// 规则配置加载时即预编译正则表达式，内部按 TLS / 重定向 / 流式超时策略懒加载并复用
/// `reqwest::Client`（共享底层连接池与 TLS 配置），避免每次请求重复构建客户端。
#[derive(Debug, Clone, Default)]
pub struct MirrorRouter {
    rules: Vec<CompiledRule>,
    rule_fingerprint: [u8; 32],
    https_only: Arc<AtomicBool>,
    no_proxy_for_testing: Arc<AtomicBool>,
    clients: Arc<RwLock<ClientPool>>,
}

impl MirrorRouter {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_rules(rules: Vec<MirrorRule>) -> Self {
        let mut compiled = Vec::with_capacity(rules.len());
        for rule in rules {
            let Some(pattern) = rule_pattern(&rule) else {
                continue;
            };
            match Regex::new(&pattern) {
                Ok(regex) => compiled.push(CompiledRule { regex, raw: rule }),
                Err(err) => {
                    tracing::warn!(pattern = %pattern, error = %err, "Ignoring invalid mirror rule regex");
                }
            }
        }
        let rule_fingerprint = if compiled.is_empty() {
            [0; 32]
        } else {
            let mut hasher = Sha256::new();
            for rule in &compiled {
                let raw = &rule.raw;
                let headers = raw
                    .headers
                    .as_ref()
                    .map(|headers| headers.iter().collect::<BTreeMap<_, _>>());
                let encoded = serde_json::to_vec(&(
                    rule.regex.as_str(),
                    &raw.target,
                    raw.keep_host,
                    raw.insecure_tls,
                    headers,
                ))
                .expect("mirror rules are serializable");
                hasher.update(encoded);
            }
            hasher.finalize().into()
        };
        Self {
            rules: compiled,
            rule_fingerprint,
            ..Self::default()
        }
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, MirrorError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::empty());
        }
        let content = fs::read_to_string(path)?;
        let rules: Vec<MirrorRule> = serde_json::from_str(&content)?;
        Ok(Self::from_rules(rules))
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Stable identifier for the effective, ordered rules of this router.
    pub fn rule_fingerprint(&self) -> &[u8; 32] {
        &self.rule_fingerprint
    }

    pub fn set_https_only(&self, enabled: bool) {
        self.https_only.store(enabled, Ordering::Relaxed);
    }

    /// 仅供集成测试使用：使缓存客户端禁用环境代理 (`no_proxy`)，避免本机 `HTTP_PROXY` 干扰 loopback 夹具。
    pub fn set_no_proxy_for_testing(&self, enabled: bool) {
        self.no_proxy_for_testing.store(enabled, Ordering::Relaxed);
    }

    pub fn resolve(&self, url: &str) -> ResolvedRequest {
        let https_only = self.https_only.load(Ordering::Relaxed);
        self.resolve_with_https_only(url, https_only)
    }

    pub fn resolve_with_https_only(&self, url: &str, https_only: bool) -> ResolvedRequest {
        Self::resolve_with_rules(url, https_only, &self.rules)
    }

    fn resolve_with_rules(url: &str, https_only: bool, rules: &[CompiledRule]) -> ResolvedRequest {
        let direct = |url: String| ResolvedRequest {
            url,
            original_host: None,
            headers: HashMap::new(),
            insecure_tls: false,
            matched_rule: false,
        };

        let Ok(mut parsed_url) = url::Url::parse(url) else {
            return direct(url.to_string());
        };

        let Some(host) = parsed_url
            .host_str()
            .map(|hostname| match parsed_url.port() {
                Some(port) => format!("{hostname}:{port}"),
                None => hostname.to_string(),
            })
        else {
            return direct(url.to_string());
        };

        let upgrade_cdn = is_legacy_steam_cdn_url(&parsed_url);
        for compiled in rules {
            let Some(captures) = compiled.regex.captures(&host) else {
                continue;
            };
            let target = replace_captures(&compiled.raw.target, &captures);
            let mut routed_url = parsed_url.clone();
            // 允许 HTTP 时镜像沿用原始协议。仅 HTTPS 模式先升级，再应用镜像端口。
            if upgrade_cdn && https_only {
                let _ = routed_url.set_scheme("https");
            }
            if set_target_host(&mut routed_url, &target) {
                let original_host = compiled.raw.keep_host.then_some(host);
                let headers = compiled.raw.headers.clone().unwrap_or_default();
                let matched_rule = url_authority(&routed_url) != url_authority(&parsed_url)
                    || original_host.is_some()
                    || !headers.is_empty();
                return ResolvedRequest {
                    url: routed_url.into(),
                    original_host,
                    headers,
                    insecure_tls: compiled.raw.insecure_tls,
                    matched_rule,
                };
            }
        }

        if upgrade_cdn && parsed_url.set_scheme("https").is_ok() {
            direct(parsed_url.into())
        } else {
            direct(url.to_string())
        }
    }

    pub fn validate_url(&self, url: &str) -> Result<ResolvedRequest, MirrorError> {
        let https_only = self.https_only.load(Ordering::Relaxed);
        self.validate_url_with_policy(url, https_only)
    }

    pub fn validate_url_with_https_only(
        &self,
        url: &str,
        https_only: bool,
    ) -> Result<ResolvedRequest, MirrorError> {
        self.validate_url_with_policy(url, https_only)
    }

    fn validate_url_with_policy(
        &self,
        url: &str,
        https_only: bool,
    ) -> Result<ResolvedRequest, MirrorError> {
        let resolved = self.resolve_with_https_only(url, https_only);
        ensure_https_if_required(&resolved.url, https_only)?;
        Ok(resolved)
    }

    /// 统一推导有效的客户端池 Key（集中约束 `insecure_tls && !https_only` 以及镜像跨源重定向隔离）。
    fn pool_key_for_resolved(
        &self,
        resolved: &ResolvedRequest,
        https_only: bool,
        requested_redirect: RedirectMode,
        timeout: Option<Duration>,
        connect_timeout: Option<Duration>,
        read_timeout: Option<Duration>,
    ) -> ClientPoolKey {
        let insecure_tls = resolved.insecure_tls && !https_only;
        let redirect_mode = match requested_redirect {
            RedirectMode::Follow if resolved.has_mirror_overrides(insecure_tls) => {
                RedirectMode::SameOriginOnly
            }
            other => other,
        };
        // Preserve the configured HTTP/1.1 Host override when TLS could negotiate HTTP/2.
        let http1_only = resolved.url.starts_with("https://")
            && (resolved.original_host.is_some()
                || resolved
                    .headers
                    .keys()
                    .any(|key| key.eq_ignore_ascii_case("host")));
        ClientPoolKey {
            https_only,
            insecure_tls,
            redirect_mode,
            http1_only,
            no_proxy: self.no_proxy_for_testing.load(Ordering::Relaxed),
            timeout,
            connect_timeout,
            read_timeout,
        }
    }

    fn resolve_and_client(
        &self,
        url: &str,
        https_only: bool,
        requested_redirect: RedirectMode,
        timeout: Option<Duration>,
        connect_timeout: Option<Duration>,
        read_timeout: Option<Duration>,
    ) -> Result<(Client, ResolvedRequest), MirrorError> {
        let resolved = self.validate_url_with_policy(url, https_only)?;
        let key = self.pool_key_for_resolved(
            &resolved,
            https_only,
            requested_redirect,
            timeout,
            connect_timeout,
            read_timeout,
        );
        let client = self.get_or_build_client(key)?;
        Ok((client, resolved))
    }

    /// 返回按策略缓存复用的常规 `reqwest::Client`（不绑定单次总超时）。
    ///
    /// 若规则带有 `insecure-tls`、`keep-host` 或自定义 `headers`，默认限制为仅同源重定向，
    /// 避免镜像配置随跨源重定向泄露；需要安全跨源重定向请使用 [`Self::send_with_redirects`]。
    pub fn client_for(&self, url: &str) -> Result<(Client, ResolvedRequest), MirrorError> {
        let https_only = self.https_only.load(Ordering::Relaxed);
        self.resolve_and_client(url, https_only, RedirectMode::Follow, None, None, None)
    }

    /// 返回按策略缓存复用的流式下载 `reqwest::Client`（绑定 `connect_timeout` 与 `read_timeout`）。
    pub fn streaming_client_for(
        &self,
        url: &str,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> Result<(Client, ResolvedRequest), MirrorError> {
        let https_only = self.https_only.load(Ordering::Relaxed);
        self.resolve_and_client(
            url,
            https_only,
            RedirectMode::Follow,
            None,
            Some(connect_timeout),
            Some(read_timeout),
        )
    }

    /// 一次性完成镜像校验、客户端复用与请求构造，并按单次请求应用可选 `timeout`。
    pub fn request(
        &self,
        method: Method,
        url: &str,
        timeout: Option<Duration>,
    ) -> Result<(RequestBuilder, ResolvedRequest), MirrorError> {
        let https_only = self.https_only.load(Ordering::Relaxed);
        self.build_request_with_policy(method, url, timeout, https_only, RedirectMode::Follow)
    }

    pub fn get(
        &self,
        url: &str,
        timeout: Option<Duration>,
    ) -> Result<(RequestBuilder, ResolvedRequest), MirrorError> {
        self.request(Method::GET, url, timeout)
    }

    pub fn post(
        &self,
        url: &str,
        timeout: Option<Duration>,
    ) -> Result<(RequestBuilder, ResolvedRequest), MirrorError> {
        self.request(Method::POST, url, timeout)
    }

    /// 与 [`Self::request`] 相同，但复用禁用自动重定向 (`Policy::none()`) 的客户端池。
    pub fn request_no_redirect(
        &self,
        method: Method,
        url: &str,
        timeout: Option<Duration>,
    ) -> Result<(RequestBuilder, ResolvedRequest), MirrorError> {
        let https_only = self.https_only.load(Ordering::Relaxed);
        self.build_request_with_policy(method, url, timeout, https_only, RedirectMode::None)
    }

    pub fn get_no_redirect(
        &self,
        url: &str,
        timeout: Option<Duration>,
    ) -> Result<(RequestBuilder, ResolvedRequest), MirrorError> {
        self.request_no_redirect(Method::GET, url, timeout)
    }

    pub fn post_no_redirect(
        &self,
        url: &str,
        timeout: Option<Duration>,
    ) -> Result<(RequestBuilder, ResolvedRequest), MirrorError> {
        self.request_no_redirect(Method::POST, url, timeout)
    }

    /// 使用调用方显式提供的 `https_only` 策略构造禁止自动重定向的请求，避免共享路由器时被其他引擎状态覆盖。
    pub fn request_no_redirect_with_policy(
        &self,
        method: Method,
        url: &str,
        timeout: Option<Duration>,
        https_only: bool,
    ) -> Result<(RequestBuilder, ResolvedRequest), MirrorError> {
        self.build_request_with_policy(method, url, timeout, https_only, RedirectMode::None)
    }

    pub fn get_no_redirect_with_policy(
        &self,
        url: &str,
        timeout: Option<Duration>,
        https_only: bool,
    ) -> Result<(RequestBuilder, ResolvedRequest), MirrorError> {
        self.request_no_redirect_with_policy(Method::GET, url, timeout, https_only)
    }

    pub fn post_no_redirect_with_policy(
        &self,
        url: &str,
        timeout: Option<Duration>,
        https_only: bool,
    ) -> Result<(RequestBuilder, ResolvedRequest), MirrorError> {
        self.request_no_redirect_with_policy(Method::POST, url, timeout, https_only)
    }

    /// Preserve same-origin redirects for insecure mirrors used by existing routed endpoints.
    pub fn request_with_insecure_mirror_redirects(
        &self,
        method: Method,
        url: &str,
        timeout: Option<Duration>,
        https_only: bool,
    ) -> Result<(RequestBuilder, ResolvedRequest), MirrorError> {
        let resolved = self.validate_url_with_policy(url, https_only)?;
        let redirect_mode = if resolved.insecure_tls && !https_only {
            RedirectMode::SameOriginOnly
        } else {
            RedirectMode::None
        };
        self.build_resolved_request(method, resolved, timeout, https_only, redirect_mode)
    }

    fn build_request_with_policy(
        &self,
        method: Method,
        url: &str,
        timeout: Option<Duration>,
        https_only: bool,
        redirect_mode: RedirectMode,
    ) -> Result<(RequestBuilder, ResolvedRequest), MirrorError> {
        let resolved = self.validate_url_with_policy(url, https_only)?;
        self.build_resolved_request(method, resolved, timeout, https_only, redirect_mode)
    }

    fn build_resolved_request(
        &self,
        method: Method,
        resolved: ResolvedRequest,
        timeout: Option<Duration>,
        https_only: bool,
        redirect_mode: RedirectMode,
    ) -> Result<(RequestBuilder, ResolvedRequest), MirrorError> {
        let key =
            self.pool_key_for_resolved(&resolved, https_only, redirect_mode, None, None, None);
        let client = self.get_or_build_client(key)?;
        let mut req = apply_resolved_headers(client.request(method, &resolved.url), &resolved);
        if let Some(t) = timeout {
            req = req.timeout(t);
        }
        Ok((req, resolved))
    }

    /// 逐跳安全执行 HTTP 请求并处理重定向：
    /// - 底层统一复用 `Policy::none()` 的缓存 `reqwest::Client`；
    /// - 每一跳重新读取生效的 `https_only` 策略；
    /// - 同源重定向保留 URL 凭据与当前镜像的 `Host` / 自定义请求头 / TLS 配置，并发送 `Referer`；
    /// - 跨源重定向丢弃上一跳镜像特权与自定义头，按新目标重新匹配规则并切换至受校验客户端；
    /// - 经过 HTTPS 后，拒绝向后续 HTTP 跳转发送 URL 凭据或镜像自定义头；
    /// - 镜像显式重定向回源站时直连源站，并继续执行逐跳校验；
    /// - 每次重定向发起前回调 `validate_hop` 供调用方执行来源白名单等策略校验。
    pub async fn send_with_redirects<FBuild, FValidate, FSend, Fut, E>(
        &self,
        method: Method,
        url: &str,
        options: RedirectRequestOptions,
        mut configure: FBuild,
        mut validate_hop: FValidate,
        mut send: FSend,
    ) -> Result<RedirectedResponse, RedirectSendError<E>>
    where
        FBuild: FnMut(RequestBuilder, usize, &Method) -> RequestBuilder,
        FValidate: FnMut(&RedirectHop<'_>) -> Result<(), E>,
        FSend: FnMut(RequestBuilder, usize) -> Fut,
        Fut: Future<Output = reqwest::Result<Response>>,
    {
        let max_redirects = options.max_redirects.unwrap_or(DEFAULT_MAX_REDIRECTS);
        let started_at = Instant::now();
        let mut current_method = method;
        let initial_https_only = options
            .https_only_override
            .unwrap_or_else(|| self.https_only.load(Ordering::Relaxed));
        let initial_resolved = self.validate_url_with_policy(url, initial_https_only)?;
        let mut current_resolved = initial_resolved.clone();
        let mut mirror_sources = Vec::new();
        let mut referer = None;
        let mut used_https = false;
        let mut drop_payload_headers = false;
        if initial_resolved.matched_rule {
            if let Some(source) = url::Url::parse(url)
                .ok()
                .filter(|source| initial_resolved.rewrites_authority(source))
            {
                mirror_sources.push(source);
            }
        }

        for hop in 0..=max_redirects {
            let https_only = options
                .https_only_override
                .unwrap_or_else(|| self.https_only.load(Ordering::Relaxed));
            ensure_https_if_required(&current_resolved.url, https_only)?;

            let key = self.pool_key_for_resolved(
                &current_resolved,
                https_only,
                RedirectMode::None,
                None,
                options.connect_timeout,
                options.read_timeout,
            );
            let client = self.get_or_build_client(key)?;
            let req = apply_resolved_headers(
                client.request(current_method.clone(), &current_resolved.url),
                &current_resolved,
            );
            let mut req = configure(req, hop, &current_method);
            if let Some(value) = referer {
                req = req.headers([(REFERER, value)].into_iter().collect());
            }
            if let Some(total_timeout) = options.timeout {
                let hop_timeout = total_timeout
                    .checked_sub(started_at.elapsed())
                    .filter(|remaining| !remaining.is_zero())
                    .ok_or(RedirectSendError::Timeout)?;
                req = req.timeout(hop_timeout);
            }

            let mut response = send(req, hop).await?;
            let status = response.status();
            let Some(location_value) = is_redirect_status(status)
                .then(|| response.headers().get(LOCATION))
                .flatten()
            else {
                return Ok(RedirectedResponse {
                    response,
                    initial_resolved,
                    final_resolved: current_resolved,
                });
            };

            if hop == max_redirects {
                return Err(RedirectSendError::Mirror(MirrorError::TooManyRedirects));
            }

            let location = std::str::from_utf8(location_value.as_bytes()).map_err(|_| {
                MirrorError::InvalidRedirect("Location 头包含无效的 UTF-8".to_string())
            })?;
            let current_url = response.url();
            used_https |= current_url.scheme() == "https";
            let next_url = current_url
                .join(location)
                .map_err(|e| MirrorError::InvalidRedirect(e.to_string()))?;
            if !matches!(next_url.scheme(), "http" | "https") {
                return Err(RedirectSendError::Mirror(MirrorError::InvalidRedirect(
                    format!("不支持的重定向协议: {}", next_url.scheme()),
                )));
            }

            let next_https_only = options
                .https_only_override
                .unwrap_or_else(|| self.https_only.load(Ordering::Relaxed));
            let is_same_origin = urls_same_origin(current_url, &next_url);
            let mut next_resolved = if is_same_origin {
                let mut resolved_url = next_url.clone();
                if resolved_url.username().is_empty() && resolved_url.password().is_none() {
                    // Reqwest converts URL credentials to Authorization and removes them from
                    // the response URL used to resolve Location.
                    let previous = url::Url::parse(&current_resolved.url).ok();
                    if let Some(previous) =
                        previous.filter(|url| urls_same_origin(url, current_url))
                    {
                        let _ = resolved_url.set_username(previous.username());
                        let _ = resolved_url.set_password(previous.password());
                    }
                }
                ResolvedRequest {
                    url: resolved_url.into(),
                    ..current_resolved
                }
            } else {
                // Keep direct requests direct across scheme changes, and honor returns to
                // any authority that was rewritten earlier in this chain.
                let current = url_authority(current_url);
                let target = url_authority(&next_url);
                let was_direct = !current_resolved.matched_rule && !current_resolved.insecure_tls;
                let use_direct_route = (was_direct && current == target)
                    || mirror_sources
                        .iter()
                        .any(|source| url_authority(source) == target);
                let rules = if use_direct_route {
                    &[]
                } else {
                    self.rules.as_slice()
                };
                Self::resolve_with_rules(next_url.as_str(), next_https_only, rules)
            };
            let (next_method, drops_payload) = next_redirect_method(status, current_method);
            // Later hops can match another rule after the request body has been discarded.
            drop_payload_headers |= drops_payload;
            if drop_payload_headers {
                next_resolved.headers.retain(|name, _| {
                    !PAYLOAD_HEADERS
                        .iter()
                        .any(|header| name.eq_ignore_ascii_case(header))
                });
            }
            ensure_https_if_required(&next_resolved.url, next_https_only)?;
            if used_https
                && next_resolved.url.starts_with("http://")
                && (!next_resolved.headers.is_empty()
                    || !next_url.username().is_empty()
                    || next_url.password().is_some())
            {
                return Err(MirrorError::InvalidRedirect(
                    "HTTPS 重定向到 HTTP 时禁止发送凭据或镜像自定义请求头".into(),
                )
                .into());
            }

            let hop_info = RedirectHop {
                hop: hop + 1,
                url: &next_url,
                same_origin: is_same_origin,
                matched_mirror_rule: next_resolved.matched_rule,
                resolved: &next_resolved,
            };
            validate_hop(&hop_info).map_err(RedirectSendError::Rejected)?;

            referer = is_same_origin
                .then(|| redirect_referer(current_url))
                .flatten();
            drain_ready_redirect_body(&mut response);
            current_method = next_method;
            if !is_same_origin && next_resolved.rewrites_authority(&next_url) {
                mirror_sources.push(next_url);
            }
            current_resolved = next_resolved;
        }

        Err(RedirectSendError::Mirror(MirrorError::TooManyRedirects))
    }

    /// 兼容旧调用点：返回按策略（含可选客户端默认超时）缓存复用的 `Client`。
    pub fn build_client_for(
        &self,
        url: &str,
        timeout: Option<Duration>,
    ) -> Result<(Client, ResolvedRequest), MirrorError> {
        let https_only = self.https_only.load(Ordering::Relaxed);
        self.resolve_and_client(url, https_only, RedirectMode::Follow, timeout, None, None)
    }

    /// 兼容旧调用点：等价于 [`Self::streaming_client_for`]。
    pub fn build_streaming_client_for(
        &self,
        url: &str,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> Result<(Client, ResolvedRequest), MirrorError> {
        self.streaming_client_for(url, connect_timeout, read_timeout)
    }

    fn get_or_build_client(&self, key: ClientPoolKey) -> Result<Client, MirrorError> {
        if let Some(client) = self
            .clients
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .get(&key)
            .cloned()
        {
            return Ok(client);
        }

        let mut builder = Client::builder()
            .https_only(key.https_only)
            .danger_accept_invalid_certs(key.insecure_tls);
        if key.no_proxy {
            builder = builder.no_proxy();
        }
        if key.http1_only {
            builder = builder.http1_only();
        }
        match key.redirect_mode {
            RedirectMode::Follow => {}
            RedirectMode::None => {
                builder = builder.redirect(reqwest::redirect::Policy::none());
            }
            RedirectMode::SameOriginOnly => {
                builder = builder.redirect(same_origin_redirect_policy(DEFAULT_MAX_REDIRECTS));
            }
        }
        if let Some(timeout) = key.timeout {
            builder = builder.timeout(timeout);
        }
        if let Some(connect_timeout) = key.connect_timeout {
            builder = builder.connect_timeout(connect_timeout);
        }
        if let Some(read_timeout) = key.read_timeout {
            builder = builder.read_timeout(read_timeout);
        }
        let built = builder.build()?;

        let mut guard = self.clients.write().unwrap_or_else(|err| err.into_inner());
        if let Some(existing) = guard.get(&key) {
            return Ok(existing.clone());
        }
        if key.insecure_tls {
            tracing::warn!("Mirror rule enables insecure-tls; certificate verification is skipped");
        }
        guard.insert(key, built.clone());
        Ok(built)
    }

    pub fn prepare_resolved_get(
        &self,
        client: &Client,
        resolved: &ResolvedRequest,
    ) -> RequestBuilder {
        apply_resolved_headers(client.get(&resolved.url), resolved)
    }

    pub fn prepare_resolved_post(
        &self,
        client: &Client,
        resolved: &ResolvedRequest,
    ) -> RequestBuilder {
        apply_resolved_headers(client.post(&resolved.url), resolved)
    }

    pub fn prepare_get(&self, client: &Client, url: &str) -> (RequestBuilder, ResolvedRequest) {
        let resolved = self.resolve(url);
        let req = self.prepare_resolved_get(client, &resolved);
        (req, resolved)
    }

    pub fn prepare_post(&self, client: &Client, url: &str) -> (RequestBuilder, ResolvedRequest) {
        let resolved = self.resolve(url);
        let req = self.prepare_resolved_post(client, &resolved);
        (req, resolved)
    }

    #[cfg(test)]
    fn cached_client_count(&self) -> usize {
        self.clients
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .total_len()
    }

    #[cfg(test)]
    fn cached_streaming_client_count(&self) -> usize {
        self.clients
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .streaming
            .len()
    }
}

fn ensure_https_if_required(url: &str, https_only: bool) -> Result<(), MirrorError> {
    if https_only && !url::Url::parse(url).is_ok_and(|parsed| parsed.scheme() == "https") {
        return Err(MirrorError::HttpsRequired);
    }
    Ok(())
}

fn url_authority(url: &url::Url) -> (Option<&str>, Option<u16>) {
    (url.host_str(), url.port())
}

fn urls_same_origin(a: &url::Url, b: &url::Url) -> bool {
    a.scheme().eq_ignore_ascii_case(b.scheme())
        && a.host_str()
            .zip(b.host_str())
            .is_some_and(|(ha, hb)| ha.eq_ignore_ascii_case(hb))
        && a.port_or_known_default() == b.port_or_known_default()
}

fn redirect_referer(url: &url::Url) -> Option<HeaderValue> {
    let mut url = url.clone();
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_fragment(None);
    HeaderValue::from_str(url.as_str()).ok()
}

fn same_origin_redirect_policy(max_redirects: usize) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() > max_redirects {
            return attempt.error("too many redirects");
        }
        let Some(prev) = attempt.previous().last() else {
            return attempt.stop();
        };
        if urls_same_origin(prev, attempt.url()) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

fn is_redirect_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn next_redirect_method(status: StatusCode, current: Method) -> (Method, bool) {
    match status {
        StatusCode::SEE_OTHER if current == Method::HEAD => (Method::HEAD, true),
        StatusCode::SEE_OTHER => (Method::GET, true),
        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND if current == Method::POST => {
            (Method::GET, true)
        }
        _ => (current, false),
    }
}

fn drain_ready_redirect_body(response: &mut Response) {
    if response
        .content_length()
        .is_some_and(|len| len > MAX_REDIRECT_BODY_DRAIN_BYTES as u64)
    {
        return;
    }
    let mut drained = 0usize;
    // An already buffered body can free an HTTP/1.1 connection. Never wait for it.
    while drained < MAX_REDIRECT_BODY_DRAIN_BYTES {
        let Some(Ok(Some(chunk))) = response.chunk().now_or_never() else {
            break;
        };
        drained = drained.saturating_add(chunk.len());
    }
}

fn is_legacy_steam_cdn_url(url: &url::Url) -> bool {
    url.scheme() == "http"
        && url.port().is_none()
        && url.host_str().is_some_and(|host| {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            let is_cloud_host = host.strip_suffix(".steampowered.com").is_some_and(|sub| {
                !sub.contains('.')
                    && (sub == "cloud"
                        || sub.starts_with("cloud-")
                        || sub.strip_prefix("cloud").is_some_and(|digits| {
                            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
                        }))
            });
            host == "steamusercontent.com"
                || host.ends_with(".steamusercontent.com")
                || host == "steamstatic.com"
                || host.ends_with(".steamstatic.com")
                || host == "media.steampowered.com"
                || (is_cloud_host && url.path().starts_with("/ugc/"))
                || host.strip_suffix(".akamaihd.net").is_some_and(|prefix| {
                    !prefix.contains('.')
                        && (matches!(prefix, "steamuserimages" | "steamcdn")
                            || prefix.starts_with("steamuserimages-")
                            || prefix.starts_with("steamcdn-"))
                })
        })
}

fn apply_resolved_headers(mut req: RequestBuilder, resolved: &ResolvedRequest) -> RequestBuilder {
    if let Some(ref host) = resolved.original_host {
        req = req.header(reqwest::header::HOST, host);
    }
    for (k, v) in &resolved.headers {
        req = req.header(k, v);
    }
    req
}

fn rule_pattern(rule: &MirrorRule) -> Option<String> {
    if let Some(regex) = &rule.regex {
        return Some(regex.clone());
    }

    if let Some(domain) = &rule.domain {
        let mut pattern = String::from("^");
        if rule.allow_www {
            pattern.push_str(r"(?:www\.)?");
        }
        if let Some(subdomains) = &rule.subdomains {
            if subdomains.len() == 1 && subdomains[0] == "*" {
                pattern.push_str(r"(.*)\.");
            } else if !subdomains.is_empty() {
                let alternatives = subdomains
                    .iter()
                    .map(|subdomain| regex::escape(subdomain))
                    .collect::<Vec<_>>();
                pattern.push_str(&format!("({})\\.", alternatives.join("|")));
            }
        }
        pattern.push_str(&regex::escape(domain));
        pattern.push('$');
        return Some(pattern);
    }

    rule.rules.as_deref().and_then(string_rule_pattern)
}

fn string_rule_pattern(rule: &str) -> Option<String> {
    let patterns = rule
        .split(';')
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
        .map(string_rule_pattern_fragment)
        .collect::<Vec<_>>();

    match patterns.as_slice() {
        [] => None,
        [pattern] => Some(format!("^{pattern}$")),
        _ => Some(format!("^(?:{})$", patterns.join("|"))),
    }
}

fn string_rule_pattern_fragment(rule: &str) -> String {
    let mut pattern = String::new();
    let mut in_group = false;
    let mut group = String::new();
    let chars: Vec<char> = rule.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        match chars[index] {
            '[' => {
                in_group = true;
                pattern.push('(');
            }
            ']' => {
                in_group = false;
                pattern.push_str(
                    &group
                        .split(',')
                        .map(regex::escape)
                        .collect::<Vec<_>>()
                        .join("|"),
                );
                group.clear();
                pattern.push(')');
                if chars.get(index + 1) == Some(&'?') {
                    pattern.push('?');
                    index += 1;
                }
            }
            character if in_group => group.push(character),
            '*' => pattern.push_str("(.*?)"),
            character => pattern.push_str(&regex::escape(&character.to_string())),
        }
        index += 1;
    }

    pattern
}

fn replace_captures(target: &str, captures: &regex::Captures<'_>) -> String {
    let mut replaced = target.to_string();
    for (index, capture) in captures.iter().enumerate().skip(1) {
        if let Some(capture) = capture {
            replaced = replaced.replace(&format!("${index}"), capture.as_str());
        }
    }
    if replaced.contains('*') {
        let first_capture = captures.get(1).map_or("", |capture| capture.as_str());
        replaced = replaced.replace('*', first_capture);
    }
    replaced
}

fn set_target_host(url: &mut url::Url, target: &str) -> bool {
    let Ok(target_url) = url::Url::parse(&format!("l4a://{target}")) else {
        return false;
    };
    let Some(target_host) = target_url.host_str() else {
        return false;
    };
    if url.set_host(Some(target_host)).is_err() {
        return false;
    }
    if url.set_port(target_url.port()).is_err() {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    use tempfile::tempdir;

    // Self-signed EC P-256 test certificate (SAN: IP:127.0.0.1, DNS:localhost) for TLS isolation tests.
    const TEST_TLS_CERT_DER: &[u8] = &[
        0x30, 0x82, 0x01, 0x9c, 0x30, 0x82, 0x01, 0x41, 0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x14,
        0x72, 0xf1, 0x13, 0x79, 0x50, 0x84, 0xf2, 0x3b, 0xaf, 0x53, 0xd8, 0xb1, 0x72, 0x9a, 0x80,
        0xc2, 0xe7, 0x2e, 0x80, 0xc5, 0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04,
        0x03, 0x02, 0x30, 0x14, 0x31, 0x12, 0x30, 0x10, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x09,
        0x6c, 0x6f, 0x63, 0x61, 0x6c, 0x68, 0x6f, 0x73, 0x74, 0x30, 0x20, 0x17, 0x0d, 0x32, 0x36,
        0x31, 0x31, 0x32, 0x37, 0x30, 0x38, 0x34, 0x30, 0x35, 0x35, 0x5a, 0x18, 0x0f, 0x32, 0x31,
        0x32, 0x36, 0x31, 0x31, 0x30, 0x33, 0x30, 0x38, 0x34, 0x30, 0x35, 0x35, 0x5a, 0x30, 0x14,
        0x31, 0x12, 0x30, 0x10, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x09, 0x6c, 0x6f, 0x63, 0x61,
        0x6c, 0x68, 0x6f, 0x73, 0x74, 0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce,
        0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42,
        0x00, 0x04, 0x53, 0xaf, 0x03, 0xb6, 0x54, 0xe3, 0xb8, 0x40, 0x09, 0x55, 0x62, 0x72, 0x53,
        0x1d, 0x4c, 0xcc, 0xa0, 0x31, 0x01, 0x08, 0x62, 0xfe, 0xd1, 0xff, 0x7d, 0x85, 0x53, 0x26,
        0x5c, 0x4e, 0xcf, 0x3c, 0x6c, 0x04, 0xf7, 0x0a, 0xfb, 0xe6, 0x07, 0xa6, 0xa7, 0x6a, 0x65,
        0x56, 0xae, 0xe5, 0x06, 0xc4, 0xa6, 0x70, 0x65, 0xe4, 0x09, 0x29, 0xcf, 0xa9, 0x1a, 0x60,
        0x02, 0xda, 0x99, 0xc1, 0xa5, 0xa0, 0xa3, 0x6f, 0x30, 0x6d, 0x30, 0x1d, 0x06, 0x03, 0x55,
        0x1d, 0x0e, 0x04, 0x16, 0x04, 0x14, 0x5f, 0x04, 0x7c, 0x64, 0x4e, 0xd5, 0xee, 0x24, 0x16,
        0xc3, 0x1b, 0x2d, 0x5f, 0x8c, 0xf5, 0x86, 0xd8, 0x59, 0xe0, 0x4b, 0x30, 0x1f, 0x06, 0x03,
        0x55, 0x1d, 0x23, 0x04, 0x18, 0x30, 0x16, 0x80, 0x14, 0x5f, 0x04, 0x7c, 0x64, 0x4e, 0xd5,
        0xee, 0x24, 0x16, 0xc3, 0x1b, 0x2d, 0x5f, 0x8c, 0xf5, 0x86, 0xd8, 0x59, 0xe0, 0x4b, 0x30,
        0x0f, 0x06, 0x03, 0x55, 0x1d, 0x13, 0x01, 0x01, 0xff, 0x04, 0x05, 0x30, 0x03, 0x01, 0x01,
        0xff, 0x30, 0x1a, 0x06, 0x03, 0x55, 0x1d, 0x11, 0x04, 0x13, 0x30, 0x11, 0x87, 0x04, 0x7f,
        0x00, 0x00, 0x01, 0x82, 0x09, 0x6c, 0x6f, 0x63, 0x61, 0x6c, 0x68, 0x6f, 0x73, 0x74, 0x30,
        0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02, 0x03, 0x49, 0x00, 0x30,
        0x46, 0x02, 0x21, 0x00, 0xf5, 0xca, 0x1c, 0x3f, 0x13, 0xe8, 0x5d, 0x04, 0xc9, 0xea, 0x4e,
        0xec, 0x5c, 0xf6, 0x52, 0xf9, 0x22, 0xda, 0xa8, 0x14, 0xba, 0xec, 0x60, 0x06, 0xcb, 0x1d,
        0x1f, 0xf8, 0x36, 0x41, 0x22, 0x02, 0x02, 0x21, 0x00, 0xee, 0xfb, 0xa1, 0xeb, 0xb1, 0x73,
        0xfb, 0x43, 0xf8, 0x9a, 0xfa, 0xbc, 0x8d, 0xbc, 0x2c, 0xe8, 0x28, 0x13, 0x67, 0x48, 0xc3,
        0xa4, 0x17, 0xae, 0x1b, 0x78, 0xc8, 0xa9, 0xba, 0xc6, 0x23, 0x42,
    ];
    const TEST_TLS_KEY_DER: &[u8] = &[
        0x30, 0x81, 0x87, 0x02, 0x01, 0x00, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d,
        0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x04, 0x6d, 0x30,
        0x6b, 0x02, 0x01, 0x01, 0x04, 0x20, 0x03, 0xd3, 0x30, 0xec, 0x10, 0xbd, 0xd5, 0xb6, 0xbd,
        0xe6, 0x6c, 0x37, 0xf7, 0x99, 0x33, 0x80, 0x5e, 0xe2, 0xa7, 0xfc, 0xe2, 0x47, 0x61, 0x87,
        0x35, 0x6d, 0xe6, 0xdf, 0x1f, 0xa5, 0xcd, 0x83, 0xa1, 0x44, 0x03, 0x42, 0x00, 0x04, 0x53,
        0xaf, 0x03, 0xb6, 0x54, 0xe3, 0xb8, 0x40, 0x09, 0x55, 0x62, 0x72, 0x53, 0x1d, 0x4c, 0xcc,
        0xa0, 0x31, 0x01, 0x08, 0x62, 0xfe, 0xd1, 0xff, 0x7d, 0x85, 0x53, 0x26, 0x5c, 0x4e, 0xcf,
        0x3c, 0x6c, 0x04, 0xf7, 0x0a, 0xfb, 0xe6, 0x07, 0xa6, 0xa7, 0x6a, 0x65, 0x56, 0xae, 0xe5,
        0x06, 0xc4, 0xa6, 0x70, 0x65, 0xe4, 0x09, 0x29, 0xcf, 0xa9, 0x1a, 0x60, 0x02, 0xda, 0x99,
        0xc1, 0xa5, 0xa0,
    ];

    fn base_rule(target: &str) -> MirrorRule {
        MirrorRule {
            rules: None,
            domain: None,
            subdomains: None,
            allow_www: false,
            regex: None,
            target: target.to_string(),
            keep_host: false,
            insecure_tls: false,
            headers: None,
        }
    }

    #[test]
    fn object_rule_rewrites_subdomain_host_and_headers() {
        let mut r = base_rule("$1.mirror.example.com");
        r.domain = Some("steamstatic.com".to_string());
        r.subdomains = Some(vec!["store".to_string(), "media".to_string()]);
        r.allow_www = true;
        r.keep_host = true;
        r.insecure_tls = true;
        r.headers = Some(HashMap::from([("X-Mirror".into(), "on".into())]));

        let router = MirrorRouter::from_rules(vec![r]);
        let res = router.resolve("https://www.media.steamstatic.com/img.jpg?v=1");
        assert_eq!(res.url, "https://media.mirror.example.com/img.jpg?v=1");
        assert_eq!(
            res.original_host.as_deref(),
            Some("www.media.steamstatic.com")
        );
        assert_eq!(res.headers.get("X-Mirror").map(String::as_str), Some("on"));
        assert!(res.insecure_tls);
        assert!(res.matched_rule);
    }

    #[test]
    fn string_rule_handles_optional_group_wildcard_and_port() {
        let mut r = base_rule("$2.proxy.local:8443");
        r.rules = Some("[www.]?*.steamcommunity.com".to_string());

        let router = MirrorRouter::from_rules(vec![r]);
        let res = router.resolve("https://www.workshop.steamcommunity.com/sharedfiles?id=1");
        assert_eq!(
            res.url,
            "https://workshop.proxy.local:8443/sharedfiles?id=1"
        );
        assert_eq!(res.original_host, None);
    }

    #[test]
    fn semicolon_rules_and_invalid_regex_are_handled_gracefully() {
        let mut invalid = base_rule("broken.local");
        invalid.regex = Some("[unclosed".to_string());

        let mut multi = base_rule("cdn.mirror.local");
        multi.rules = Some("cdn.steamstatic.com; community.steamstatic.com".to_string());

        let router = MirrorRouter::from_rules(vec![invalid, multi]);
        assert_eq!(router.rule_count(), 1);
        assert_eq!(
            router.resolve("https://community.steamstatic.com/a").url,
            "https://cdn.mirror.local/a"
        );
        assert_eq!(
            router.resolve("https://untouched.example.com/b").url,
            "https://untouched.example.com/b"
        );
    }

    #[test]
    fn loads_rules_from_json_file_and_handles_missing_file() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing.json");
        assert_eq!(MirrorRouter::from_file(&missing).unwrap().rule_count(), 0);

        let file = dir.path().join("mirrors.json");
        std::fs::write(
            &file,
            r#"[{"domain":"steampowered.com","subdomains":["api"],"target":"127.0.0.1:9000","keep-host":true}]"#,
        )
        .unwrap();

        let router = MirrorRouter::from_file(&file).unwrap();
        assert_eq!(router.rule_count(), 1);
        let res = router.resolve("https://api.steampowered.com/ISteamRemoteStorage/");
        assert_eq!(res.url, "https://127.0.0.1:9000/ISteamRemoteStorage/");
        assert_eq!(res.original_host.as_deref(), Some("api.steampowered.com"));
    }

    #[test]
    fn rewrites_ports_cleanly_including_clearing_and_port_80() {
        let mut clear_port = base_rule("cdn.mirror.local");
        clear_port.rules = Some("api.example.com:9000".to_string());

        let mut keep_80 = base_rule("proxy.local:80");
        keep_80.rules = Some("cdn.example.com".to_string());

        let router = MirrorRouter::from_rules(vec![clear_port, keep_80]);
        assert_eq!(
            router.resolve("https://api.example.com:9000/a.vpk").url,
            "https://cdn.mirror.local/a.vpk"
        );
        assert_eq!(
            router.resolve("https://cdn.example.com/b.vpk").url,
            "https://proxy.local:80/b.vpk"
        );
    }

    #[test]
    fn routes_legacy_steam_cdn_links_according_to_mirror_and_https_policy() {
        let mut rule = base_rule("mirror.local:8080");
        rule.rules = Some("cloud.steampowered.com".to_string());
        rule.keep_host = true;
        let router = MirrorRouter::from_rules(vec![rule]);
        let url = "http://cloud.steampowered.com/ugc/123/cover.jpg";

        let direct = MirrorRouter::empty();
        direct.set_https_only(true);
        assert_eq!(
            direct.validate_url(url).unwrap().url,
            "https://cloud.steampowered.com/ugc/123/cover.jpg"
        );
        assert_eq!(
            direct
                .validate_url("http://cloud4.steampowered.com/ugc/123/cover.jpg")
                .unwrap()
                .url,
            "https://cloud4.steampowered.com/ugc/123/cover.jpg"
        );
        let mirrored = router.validate_url(url).unwrap();
        assert_eq!(mirrored.url, "http://mirror.local:8080/ugc/123/cover.jpg");
        assert_eq!(
            mirrored.original_host.as_deref(),
            Some("cloud.steampowered.com")
        );

        router.set_https_only(true);
        assert_eq!(
            router.validate_url(url).unwrap().url,
            "https://mirror.local:8080/ugc/123/cover.jpg"
        );
        for other in [
            "http://evil-steamuserimages.akamaihd.net/cover.jpg",
            "http://cloud.steampowered.com/other/cover.jpg",
        ] {
            assert!(matches!(
                router.validate_url(other),
                Err(MirrorError::HttpsRequired)
            ));
        }
    }

    #[derive(Debug, Clone)]
    struct RecordedRequest {
        conn_id: usize,
        method: String,
        path: String,
        headers: HashMap<String, String>,
    }

    async fn serve_http_stream<S, F>(
        mut stream: S,
        conn_id: usize,
        log: Arc<Mutex<Vec<RecordedRequest>>>,
        handler: Arc<F>,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
        F: Fn(&RecordedRequest) -> (String, String, String) + Send + Sync + 'static,
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut buf = vec![0u8; 4096];
        let mut pending = Vec::new();
        loop {
            let header_end = loop {
                if let Some(pos) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(pos + 4);
                }
                match stream.read(&mut buf).await {
                    Ok(0) | Err(_) => break None,
                    Ok(n) => pending.extend_from_slice(&buf[..n]),
                }
            };
            let Some(end) = header_end else {
                break;
            };
            let header_text = String::from_utf8_lossy(&pending[..end]).into_owned();
            let mut lines = header_text.lines();
            let req_line = lines.next().unwrap_or_default();
            let mut parts = req_line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_string();
            let path = parts.next().unwrap_or_default().to_string();
            let mut headers = HashMap::new();
            for line in lines {
                if let Some((k, v)) = line.split_once(':') {
                    headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                }
            }
            let content_length = headers
                .get("content-length")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            pending.drain(..end);
            while pending.len() < content_length {
                match stream.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => pending.extend_from_slice(&buf[..n]),
                }
            }
            pending.drain(..content_length);

            let recorded = RecordedRequest {
                conn_id,
                method,
                path,
                headers,
            };
            log.lock().unwrap().push(recorded.clone());
            let (status_line, extra_headers, body) = handler(&recorded);
            let response = format!(
                "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n{extra_headers}\r\n{body}",
                body.len()
            );
            if stream.write_all(response.as_bytes()).await.is_err() {
                break;
            }
        }
    }

    async fn start_keep_alive_server<F>(handler: F) -> (String, Arc<Mutex<Vec<RecordedRequest>>>)
    where
        F: Fn(&RecordedRequest) -> (String, String, String) + Send + Sync + 'static,
    {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let next_conn_id = Arc::new(AtomicUsize::new(1));
        let handler = Arc::new(handler);

        let log_bg = Arc::clone(&log);
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let conn_id = next_conn_id.fetch_add(1, Ordering::SeqCst);
                let log_conn = Arc::clone(&log_bg);
                let handler_conn = Arc::clone(&handler);
                tokio::spawn(async move {
                    serve_http_stream(socket, conn_id, log_conn, handler_conn).await;
                });
            }
        });

        (format!("http://{addr}"), log)
    }

    fn test_tls_config() -> rustls::ServerConfig {
        use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

        let cert = CertificateDer::from(TEST_TLS_CERT_DER.to_vec());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(TEST_TLS_KEY_DER.to_vec()));
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap()
    }

    async fn start_self_signed_tls_server<F>(
        handler: F,
    ) -> (String, Arc<Mutex<Vec<RecordedRequest>>>)
    where
        F: Fn(&RecordedRequest) -> (String, String, String) + Send + Sync + 'static,
    {
        use tokio::net::TcpListener;
        use tokio_rustls::TlsAcceptor;

        let acceptor = TlsAcceptor::from(Arc::new(test_tls_config()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let next_conn_id = Arc::new(AtomicUsize::new(1));
        let handler = Arc::new(handler);

        let log_bg = Arc::clone(&log);
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let conn_id = next_conn_id.fetch_add(1, Ordering::SeqCst);
                let acceptor_conn = acceptor.clone();
                let log_conn = Arc::clone(&log_bg);
                let handler_conn = Arc::clone(&handler);
                tokio::spawn(async move {
                    if let Ok(tls_stream) = acceptor_conn.accept(socket).await {
                        serve_http_stream(tls_stream, conn_id, log_conn, handler_conn).await;
                    }
                });
            }
        });

        (format!("https://{addr}"), log)
    }

    fn distinct_serving_conns(log: &Mutex<Vec<RecordedRequest>>) -> usize {
        log.lock()
            .unwrap()
            .iter()
            .map(|r| r.conn_id)
            .collect::<HashSet<_>>()
            .len()
    }

    #[tokio::test]
    async fn reuses_http2_connection_and_preserves_host_overrides_over_http1() {
        use tokio::net::TcpListener;
        use tokio_rustls::TlsAcceptor;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap().to_string();
        let mut tls_config = test_tls_config();
        tls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(tls_config));
        let h2_connections = Arc::new(Mutex::new(Vec::new()));
        let h1_requests = Arc::new(Mutex::new(Vec::new()));
        let h2_connections_bg = Arc::clone(&h2_connections);
        let h1_requests_bg = Arc::clone(&h1_requests);
        tokio::spawn(async move {
            let mut conn_id = 0;
            while let Ok((socket, _)) = listener.accept().await {
                conn_id += 1;
                let acceptor = acceptor.clone();
                let h2_connections = Arc::clone(&h2_connections_bg);
                let h1_requests = Arc::clone(&h1_requests_bg);
                tokio::spawn(async move {
                    let stream = acceptor.accept(socket).await.unwrap();
                    if stream.get_ref().1.alpn_protocol() == Some(b"h2") {
                        let mut connection = h2::server::handshake(stream).await.unwrap();
                        while let Some(Ok((_, mut respond))) = connection.accept().await {
                            h2_connections.lock().unwrap().push(conn_id);
                            respond
                                .send_response(http::Response::new(()), true)
                                .unwrap();
                        }
                    } else {
                        serve_http_stream(
                            stream,
                            conn_id,
                            h1_requests,
                            Arc::new(|_: &RecordedRequest| {
                                ("200 OK".into(), String::new(), String::new())
                            }),
                        )
                        .await;
                    }
                });
            }
        });

        let mut h2_rule = base_rule(&target);
        h2_rule.rules = Some("h2.mirror.test".into());
        h2_rule.insecure_tls = true;
        let mut keep_host_rule = h2_rule.clone();
        keep_host_rule.rules = Some("keep-host.mirror.test".into());
        keep_host_rule.keep_host = true;
        let router = MirrorRouter::from_rules(vec![h2_rule, keep_host_rule]);
        router.set_no_proxy_for_testing(true);

        for timeout in [Duration::from_secs(4), Duration::from_secs(5)] {
            let (request, _) = router
                .get("https://h2.mirror.test/item", Some(timeout))
                .unwrap();
            let response = request.send().await.unwrap();
            assert_eq!(response.version(), reqwest::Version::HTTP_2);
            assert_eq!(response.status(), StatusCode::OK);
            response.bytes().await.unwrap();
        }
        assert_eq!(*h2_connections.lock().unwrap(), [1, 1]);

        let (request, _) = router
            .get(
                "https://keep-host.mirror.test/item",
                Some(Duration::from_secs(5)),
            )
            .unwrap();
        let response = request.send().await.unwrap();
        assert_eq!(response.version(), reqwest::Version::HTTP_11);
        let requests = h1_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].headers["host"], "keep-host.mirror.test");
    }

    #[tokio::test]
    async fn follows_location_without_waiting_for_optional_response_body() {
        use reqwest::ResponseBuilderExt;

        let router = MirrorRouter::empty();
        let redirected = tokio::time::timeout(
            Duration::from_secs(2),
            router.send_with_redirects(
                Method::GET,
                "https://mirror.example.com/start",
                RedirectRequestOptions::default(),
                |request, _, _| request,
                |_| Ok::<(), ()>(()),
                |request, hop| async move {
                    let url = request.build()?.url().clone();
                    let response = http::Response::builder().url(url);
                    let response = if hop == 0 {
                        let body = reqwest::Body::wrap_stream(futures_util::stream::pending::<
                            std::io::Result<&'static [u8]>,
                        >());
                        response
                            .status(StatusCode::FOUND)
                            .header(reqwest::header::LOCATION, "/item")
                            .body(body)
                    } else {
                        response.body(reqwest::Body::from("ok"))
                    };
                    Ok(response.unwrap().into())
                },
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            redirected.final_resolved.url,
            "https://mirror.example.com/item"
        );
        assert_eq!(redirected.response.text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn follows_utf8_location_paths() {
        let (base_url, requests) = start_keep_alive_server(|req| {
            if req.path == "/start" {
                (
                    "302 Found".into(),
                    "Location: /封面.jpg\r\n".into(),
                    String::new(),
                )
            } else {
                ("200 OK".into(), String::new(), "cover".into())
            }
        })
        .await;
        let router = MirrorRouter::empty();
        router.set_no_proxy_for_testing(true);

        let redirected = router
            .send_with_redirects(
                Method::GET,
                &format!("{base_url}/start"),
                RedirectRequestOptions {
                    timeout: Some(Duration::from_secs(5)),
                    ..Default::default()
                },
                |req, _, _| req,
                |_| Ok::<(), ()>(()),
                |req, _| req.send(),
            )
            .await
            .unwrap();

        assert_eq!(redirected.response.text().await.unwrap(), "cover");
        assert_eq!(requests.lock().unwrap()[1].path, "/%E5%B0%81%E9%9D%A2.jpg");
    }

    #[tokio::test]
    async fn redirect_strips_mirror_payload_headers_when_post_becomes_get() {
        let (base_url, requests) = start_keep_alive_server(|req| match req.path.as_str() {
            "/start" => (
                "307 Temporary Redirect".into(),
                "Location: /post\r\n".into(),
                String::new(),
            ),
            "/post" => (
                "303 See Other".into(),
                "Location: /final\r\n".into(),
                String::new(),
            ),
            _ => ("200 OK".into(), String::new(), "done".into()),
        })
        .await;
        let mut rule = base_rule(base_url.strip_prefix("http://").unwrap());
        rule.rules = Some("catalog.example.test".into());
        rule.headers = Some(HashMap::from([
            ("Content-Length".into(), "2".into()),
            ("Content-Type".into(), "application/json".into()),
            ("Content-Encoding".into(), "identity".into()),
            ("X-Mirror".into(), "present".into()),
        ]));
        let router = MirrorRouter::from_rules(vec![rule]);
        router.set_no_proxy_for_testing(true);

        let response = router
            .send_with_redirects(
                Method::POST,
                "http://catalog.example.test/start",
                RedirectRequestOptions {
                    timeout: Some(Duration::from_secs(5)),
                    ..Default::default()
                },
                |request, _, method| {
                    if *method == Method::POST {
                        request.body("{}")
                    } else {
                        request
                    }
                },
                |_| Ok::<(), ()>(()),
                |request, _| request.send(),
            )
            .await
            .unwrap();
        assert_eq!(response.response.text().await.unwrap(), "done");

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1].method, "POST");
        assert_eq!(requests[1].headers["content-length"], "2");
        assert_eq!(requests[2].method, "GET");
        assert_eq!(requests[2].headers["x-mirror"], "present");
        for header in PAYLOAD_HEADERS {
            assert!(!requests[2].headers.contains_key(*header));
        }
    }

    #[tokio::test]
    async fn exhausted_timeout_stops_before_sending() {
        let result = MirrorRouter::empty()
            .send_with_redirects(
                Method::GET,
                "https://mirror.example.com/item",
                RedirectRequestOptions {
                    timeout: Some(Duration::ZERO),
                    ..Default::default()
                },
                |request, _, _| request,
                |_| Ok::<(), ()>(()),
                |_, _| async { panic!("the deadline is exhausted") },
            )
            .await;
        assert!(matches!(result, Err(RedirectSendError::Timeout)));
    }

    #[tokio::test]
    async fn reuses_client_and_tcp_connection_across_different_request_timeouts() {
        let (base_url, log) =
            start_keep_alive_server(|_| ("200 OK".into(), String::new(), "ok".into())).await;
        let router = MirrorRouter::empty();
        router.set_no_proxy_for_testing(true);

        for (method_is_post, timeout) in [
            (false, Some(Duration::from_secs(5))),
            (true, Some(Duration::from_secs(15))),
            (false, Some(Duration::from_secs(30))),
            (true, None),
        ] {
            let (req, _) = if method_is_post {
                router.post(&format!("{base_url}/post"), timeout).unwrap()
            } else {
                router.get(&format!("{base_url}/get"), timeout).unwrap()
            };
            let resp = req.body("payload").send().await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert_eq!(resp.text().await.unwrap(), "ok");
            tokio::task::yield_now().await;
        }

        assert_eq!(log.lock().unwrap().len(), 4);
        assert_eq!(router.cached_client_count(), 1);
        assert_eq!(
            distinct_serving_conns(&log),
            1,
            "sequential requests with different per-request timeouts should reuse one serving TCP connection"
        );
    }

    #[tokio::test]
    async fn cloned_router_shares_cached_client_pool_and_tcp_connection() {
        let (base_url, log) =
            start_keep_alive_server(|_| ("200 OK".into(), String::new(), "shared".into())).await;
        let router = MirrorRouter::empty();
        router.set_no_proxy_for_testing(true);
        let cloned = router.clone();

        let (req1, _) = router
            .get(&format!("{base_url}/a"), Some(Duration::from_secs(10)))
            .unwrap();
        assert_eq!(req1.send().await.unwrap().text().await.unwrap(), "shared");
        tokio::task::yield_now().await;

        let (req2, _) = cloned
            .get(&format!("{base_url}/b"), Some(Duration::from_secs(20)))
            .unwrap();
        assert_eq!(req2.send().await.unwrap().text().await.unwrap(), "shared");

        assert_eq!(log.lock().unwrap().len(), 2);
        assert_eq!(router.cached_client_count(), 1);
        assert_eq!(cloned.cached_client_count(), 1);
        assert_eq!(distinct_serving_conns(&log), 1);
    }

    #[tokio::test]
    async fn streaming_client_reuses_client_and_tcp_connection() {
        let (base_url, log) =
            start_keep_alive_server(|_| ("200 OK".into(), String::new(), "chunk-data".into()))
                .await;
        let router = MirrorRouter::empty();
        router.set_no_proxy_for_testing(true);

        for path in ["/file1.vpk", "/file2.vpk"] {
            let (client, resolved) = router
                .streaming_client_for(
                    &format!("{base_url}{path}"),
                    Duration::from_secs(15),
                    Duration::from_secs(60),
                )
                .unwrap();
            let resp = router
                .prepare_resolved_get(&client, &resolved)
                .send()
                .await
                .unwrap();
            assert_eq!(resp.text().await.unwrap(), "chunk-data");
            tokio::task::yield_now().await;
        }

        assert_eq!(log.lock().unwrap().len(), 2);
        assert_eq!(router.cached_client_count(), 1);
        assert_eq!(distinct_serving_conns(&log), 1);
    }

    #[tokio::test]
    async fn no_redirect_get_and_post_preserve_policy_and_reuse_connection_for_insecure_rules() {
        let (base_url, log) = start_keep_alive_server(|_| {
            (
                "302 Found".into(),
                "Location: http://127.0.0.1:1/never-follow\r\n".into(),
                String::new(),
            )
        })
        .await;
        let target = base_url.strip_prefix("http://").unwrap().to_string();
        let mut rule = base_rule(&target);
        rule.rules = Some("filter.mirror.test".to_string());
        rule.insecure_tls = true;
        rule.keep_host = true;

        let router = MirrorRouter::from_rules(vec![rule]);
        router.set_no_proxy_for_testing(true);

        let (get_req, resolved) = router
            .get_no_redirect(
                "http://filter.mirror.test/info",
                Some(Duration::from_secs(10)),
            )
            .unwrap();
        assert_eq!(
            resolved.original_host.as_deref(),
            Some("filter.mirror.test")
        );
        assert!(resolved.insecure_tls);
        let resp = get_req.send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
        let _ = resp.bytes().await.unwrap();
        tokio::task::yield_now().await;

        let (post_req, _) = router
            .post_no_redirect(
                "http://filter.mirror.test/predict",
                Some(Duration::from_secs(10)),
            )
            .unwrap();
        let resp = post_req.body("{}").send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
        let _ = resp.bytes().await.unwrap();

        let entries = log.lock().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].method, "GET");
        assert_eq!(entries[1].method, "POST");
        assert_eq!(
            entries[0].headers.get("host").map(String::as_str),
            Some("filter.mirror.test")
        );
        assert_eq!(
            entries[1].headers.get("host").map(String::as_str),
            Some("filter.mirror.test")
        );
        drop(entries);
        assert_eq!(router.cached_client_count(), 1);
        assert_eq!(distinct_serving_conns(&log), 1);
    }

    #[tokio::test]
    async fn real_tls_isolates_verified_and_insecure_pools_and_blocks_cross_origin_insecure_leak() {
        // Server B: untrusted self-signed TLS server NOT covered by any mirror rule.
        let (server_b_url, server_b_log) =
            start_self_signed_tls_server(|_| ("200 OK".into(), String::new(), "secret-b".into()))
                .await;

        // Server A: self-signed TLS mirror server. `/ok` returns 200, `/cross` redirects to Server B.
        let server_b_redirect = format!("Location: {server_b_url}/secret\r\n");
        let (server_a_url, server_a_log) = start_self_signed_tls_server(move |req| {
            if req.path == "/cross" {
                ("302 Found".into(), server_b_redirect.clone(), String::new())
            } else {
                ("200 OK".into(), String::new(), "ok-a".into())
            }
        })
        .await;
        let target_a = server_a_url.strip_prefix("https://").unwrap().to_string();

        let mut insecure_rule = base_rule(&target_a);
        insecure_rule.rules = Some("mirror.insecure.test".to_string());
        insecure_rule.insecure_tls = true;
        insecure_rule.keep_host = true;

        let mut strict_rule = base_rule(&target_a);
        strict_rule.rules = Some("mirror.strict.test".to_string());
        strict_rule.insecure_tls = false;

        let router = MirrorRouter::from_rules(vec![insecure_rule, strict_rule]);
        router.set_no_proxy_for_testing(true);

        // 1. Strict route to the same self-signed authority MUST fail TLS verification.
        let strict_err = router
            .send_with_redirects(
                Method::GET,
                "https://mirror.strict.test/ok",
                RedirectRequestOptions {
                    timeout: Some(Duration::from_secs(5)),
                    ..Default::default()
                },
                |r, _, _| r,
                |_| Ok::<(), ()>(()),
                |r, _| r.send(),
            )
            .await;
        assert!(
            matches!(strict_err, Err(RedirectSendError::Request(_))),
            "strict route must reject self-signed TLS cert, got {strict_err:?}"
        );

        // 2. Insecure mirror route succeeds when https_only == false and reuses the TLS connection.
        for path in ["/ok", "/ok2"] {
            let res = router
                .send_with_redirects(
                    Method::GET,
                    &format!("https://mirror.insecure.test{path}"),
                    RedirectRequestOptions {
                        timeout: Some(Duration::from_secs(5)),
                        ..Default::default()
                    },
                    |r, _, _| r,
                    |_| Ok::<(), ()>(()),
                    |r, _| r.send(),
                )
                .await
                .expect("insecure mirror route should succeed when https_only=false");
            assert_eq!(res.response.text().await.unwrap(), "ok-a");
            tokio::task::yield_now().await;
        }
        assert_eq!(distinct_serving_conns(&server_a_log), 1);

        // 3. Cross-origin redirect from insecure mirror to untrusted Server B MUST fail TLS verification.
        let cross_err = router
            .send_with_redirects(
                Method::GET,
                "https://mirror.insecure.test/cross",
                RedirectRequestOptions {
                    timeout: Some(Duration::from_secs(5)),
                    ..Default::default()
                },
                |r, _, _| r,
                |_| Ok::<(), ()>(()),
                |r, _| r.send(),
            )
            .await;
        assert!(
            matches!(cross_err, Err(RedirectSendError::Request(_))),
            "cross-origin redirect from insecure mirror must use verified client and fail self-signed TLS, got {cross_err:?}"
        );
        assert_eq!(
            server_b_log.lock().unwrap().len(),
            0,
            "Server B must not receive an HTTP request after TLS rejection"
        );

        // 4. Direct raw `.get().send()` on insecure mirror stops on cross-origin redirect instead of leaking TLS bypass.
        let (raw_req, _) = router
            .get(
                "https://mirror.insecure.test/cross",
                Some(Duration::from_secs(5)),
            )
            .unwrap();
        let raw_resp = raw_req.send().await.unwrap();
        assert_eq!(raw_resp.status(), StatusCode::FOUND);
        assert_eq!(server_b_log.lock().unwrap().len(), 0);

        // 5. Enabling https_only on the SAME router immediately enforces certificate verification.
        router.set_https_only(true);
        let upgraded_err = router
            .send_with_redirects(
                Method::GET,
                "https://mirror.insecure.test/ok",
                RedirectRequestOptions {
                    timeout: Some(Duration::from_secs(5)),
                    ..Default::default()
                },
                |r, _, _| r,
                |_| Ok::<(), ()>(()),
                |r, _| r.send(),
            )
            .await;
        assert!(
            matches!(upgraded_err, Err(RedirectSendError::Request(_))),
            "enabling https_only must disable insecure-tls and fail self-signed TLS, got {upgraded_err:?}"
        );
    }

    #[tokio::test]
    async fn redirects_preserve_mirror_headers_same_origin_and_strip_cross_origin_without_pool_leak(
    ) {
        let (origin_b_url, origin_b_log) =
            start_keep_alive_server(|_| ("200 OK".into(), String::new(), "from-b".into())).await;
        let redirect_to_b = format!("Location: {origin_b_url}/final\r\n");

        let (origin_a_url, origin_a_log) =
            start_keep_alive_server(move |req| match req.path.as_str() {
                "/same-hop" => (
                    "302 Found".into(),
                    "Location: /same-final\r\n".into(),
                    String::new(),
                ),
                "/cross-hop" => ("302 Found".into(), redirect_to_b.clone(), String::new()),
                _ => ("200 OK".into(), String::new(), "from-a".into()),
            })
            .await;
        let target_a = origin_a_url.strip_prefix("http://").unwrap().to_string();

        let mut rule = base_rule(&target_a);
        rule.rules = Some("mirrored.example.test".to_string());
        rule.keep_host = true;
        rule.headers = Some(HashMap::from([(
            "X-Mirror-Secret".to_string(),
            "top-secret".to_string(),
        )]));

        let router = MirrorRouter::from_rules(vec![rule]);
        router.set_no_proxy_for_testing(true);

        // 1. Same-origin redirect keeps Host and mirror headers.
        let same = router
            .send_with_redirects(
                Method::GET,
                "http://mirrored.example.test/same-hop",
                RedirectRequestOptions::default(),
                |r, _, _| r,
                |_| Ok::<(), ()>(()),
                |r, _| r.send(),
            )
            .await
            .unwrap();
        assert_eq!(same.response.text().await.unwrap(), "from-a");
        tokio::task::yield_now().await;

        // 2. Cross-origin redirect drops Host and mirror headers before contacting Origin B.
        let cross = router
            .send_with_redirects(
                Method::GET,
                "http://mirrored.example.test/cross-hop",
                RedirectRequestOptions::default(),
                |r, _, _| r,
                |_| Ok::<(), ()>(()),
                |r, _| r.send(),
            )
            .await
            .unwrap();
        assert_eq!(cross.response.text().await.unwrap(), "from-b");
        tokio::task::yield_now().await;

        // 3. Direct request to Origin A (without mirror rule) on the SAME pooled client sends no mirror headers.
        let direct = router
            .send_with_redirects(
                Method::GET,
                &format!("{origin_a_url}/plain"),
                RedirectRequestOptions::default(),
                |r, _, _| r,
                |_| Ok::<(), ()>(()),
                |r, _| r.send(),
            )
            .await
            .unwrap();
        assert_eq!(direct.response.text().await.unwrap(), "from-a");

        let a_entries = origin_a_log.lock().unwrap();
        assert_eq!(a_entries.len(), 4);
        assert_eq!(
            a_entries[1].headers.get("host").map(String::as_str),
            Some("mirrored.example.test")
        );
        assert_eq!(
            a_entries[1]
                .headers
                .get("x-mirror-secret")
                .map(String::as_str),
            Some("top-secret")
        );
        assert_eq!(a_entries[3].path, "/plain");
        assert_eq!(a_entries[3].headers.get("x-mirror-secret"), None);
        assert_ne!(
            a_entries[3].headers.get("host").map(String::as_str),
            Some("mirrored.example.test")
        );
        drop(a_entries);
        assert_eq!(distinct_serving_conns(&origin_a_log), 1);

        let b_entries = origin_b_log.lock().unwrap();
        assert_eq!(b_entries.len(), 1);
        assert_eq!(b_entries[0].headers.get("x-mirror-secret"), None);
        assert_ne!(
            b_entries[0].headers.get("host").map(String::as_str),
            Some("mirrored.example.test")
        );
    }

    #[tokio::test]
    async fn redirects_keep_credentials_and_send_a_sanitized_referer_on_the_same_origin() {
        let (other_origin, other_requests) =
            start_keep_alive_server(|_| ("200 OK".into(), String::new(), "done".into())).await;
        let (origin, requests) = start_keep_alive_server(move |req| {
            let location = if req.path == "/start" {
                "/next".to_string()
            } else {
                format!("{other_origin}/final")
            };
            (
                "307 Temporary Redirect".into(),
                format!("Location: {location}\r\n"),
                String::new(),
            )
        })
        .await;
        let mut url = url::Url::parse(&format!("{origin}/start")).unwrap();
        url.set_username("fixture").unwrap();
        url.set_password(Some("test :@")).unwrap();
        url.set_fragment(Some("not-for-the-server"));
        let router = MirrorRouter::empty();
        router.set_no_proxy_for_testing(true);

        let result = router
            .send_with_redirects(
                Method::GET,
                url.as_str(),
                RedirectRequestOptions {
                    timeout: Some(Duration::from_secs(5)),
                    ..Default::default()
                },
                |req, _, _| req,
                |_| Ok::<(), ()>(()),
                |req, _| req.send(),
            )
            .await
            .unwrap();

        assert_eq!(result.response.text().await.unwrap(), "done");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let authorization = &requests[0].headers["authorization"];
        assert!(authorization.starts_with("Basic "));
        assert_eq!(requests[1].path, "/next");
        assert_eq!(
            requests[1].headers.get("authorization"),
            Some(authorization)
        );
        assert!(!requests[0].headers.contains_key("referer"));
        assert_eq!(requests[1].headers["referer"], format!("{origin}/start"));
        let other_requests = other_requests.lock().unwrap();
        assert_eq!(other_requests.len(), 1);
        assert!(!other_requests[0].headers.contains_key("authorization"));
        assert!(!other_requests[0].headers.contains_key("referer"));
    }

    #[tokio::test]
    async fn mirror_can_return_to_source_after_switching_rules() {
        let (source_url, source_requests) =
            start_keep_alive_server(|_| ("200 OK".into(), String::new(), "from-source".into()))
                .await;
        let source_authority = source_url.strip_prefix("http://").unwrap();
        let source_item = format!("{source_url}/item");
        let redirect_to_source = format!("Location: {source_item}\r\n");
        let (mirror_url, mirror_requests) = start_keep_alive_server(move |req| {
            let location = if req.path == "/item" {
                "Location: /mirror-step\r\n".into()
            } else {
                redirect_to_source.clone()
            };
            ("302 Found".into(), location, String::new())
        })
        .await;
        let target = mirror_url.strip_prefix("http://").unwrap();
        let entry_rule = MirrorRule {
            rules: Some("entry.example.test".into()),
            ..base_rule(target)
        };
        let source_rule = MirrorRule {
            rules: Some(source_authority.into()),
            keep_host: true,
            insecure_tls: true,
            headers: Some(HashMap::from([("X-Mirror-Route".into(), "source".into())])),
            ..base_rule(target)
        };
        let router = MirrorRouter::from_rules(vec![entry_rule, source_rule]);
        router.set_no_proxy_for_testing(true);
        let mut matched_mirror = Vec::new();

        let redirected = router
            .send_with_redirects(
                Method::GET,
                "http://entry.example.test/start",
                RedirectRequestOptions {
                    timeout: Some(Duration::from_secs(5)),
                    ..Default::default()
                },
                |req, _, _| req,
                |hop| {
                    matched_mirror.push(hop.matched_mirror_rule);
                    Ok::<(), ()>(())
                },
                |req, _| req.send(),
            )
            .await
            .unwrap();

        assert_eq!(redirected.final_resolved.url, source_item);
        assert!(redirected.final_resolved.original_host.is_none());
        assert!(!redirected.final_resolved.insecure_tls);
        assert_eq!(redirected.response.text().await.unwrap(), "from-source");
        assert_eq!(matched_mirror, [true, true, false]);

        let mirror = mirror_requests.lock().unwrap();
        let paths: Vec<_> = mirror.iter().map(|req| req.path.as_str()).collect();
        assert_eq!(paths, ["/start", "/item", "/mirror-step"]);
        assert_eq!(
            mirror[1].headers.get("x-mirror-route").map(String::as_str),
            Some("source")
        );
        let source = source_requests.lock().unwrap();
        assert_eq!(source.len(), 1);
        assert_eq!(source[0].headers.get("x-mirror-route"), None);
    }

    #[tokio::test]
    async fn mirror_can_return_to_an_earlier_source_in_the_chain() {
        use reqwest::ResponseBuilderExt;

        let router = MirrorRouter::from_rules(vec![
            MirrorRule {
                rules: Some("first.example.test".into()),
                ..base_rule("first.mirror.test")
            },
            MirrorRule {
                rules: Some("second.example.test".into()),
                ..base_rule("second.mirror.test")
            },
        ]);
        let steps = [
            (
                "https://first.mirror.test/start",
                Some("https://second.example.test/edge"),
            ),
            (
                "https://second.mirror.test/edge",
                Some("https://first.example.test/file"),
            ),
            ("https://first.example.test/file", None),
        ];
        let redirected = router
            .send_with_redirects(
                Method::GET,
                "https://first.example.test/start",
                RedirectRequestOptions::default(),
                |req, _, _| req,
                |_| Ok::<(), ()>(()),
                |req, hop| async move {
                    let url = req.build()?.url().clone();
                    let (expected, location) = steps[hop];
                    assert_eq!(url.as_str(), expected);
                    let mut response = http::Response::builder().url(url);
                    if let Some(location) = location {
                        response = response
                            .status(StatusCode::FOUND)
                            .header(reqwest::header::LOCATION, location);
                    }
                    Ok(response.body(reqwest::Body::from("")).unwrap().into())
                },
            )
            .await
            .unwrap();

        assert_eq!(redirected.final_resolved.url, steps[2].0);
        assert!(!redirected.final_resolved.matched_rule);
    }

    #[tokio::test]
    async fn direct_mirror_fallback_obeys_https_policy_and_stays_direct_on_upgrade() {
        use reqwest::ResponseBuilderExt;

        let rule = MirrorRule {
            rules: Some("source.example.test".into()),
            ..base_rule("mirror.example.test")
        };
        let router = MirrorRouter::from_rules(vec![rule]);
        let source_http = "http://source.example.test/item";
        let source_https = "https://source.example.test/item";
        let steps = [
            ("https://mirror.example.test/item", Some(source_http)),
            (source_http, Some(source_https)),
            (source_https, None),
        ];

        for https_only in [false, true] {
            let redirected = router
                .send_with_redirects(
                    Method::GET,
                    source_https,
                    RedirectRequestOptions {
                        https_only_override: Some(https_only),
                        ..Default::default()
                    },
                    |req, _, _| req,
                    |_| Ok::<(), ()>(()),
                    |req, hop| async move {
                        let url = req.build()?.url().clone();
                        let (expected_url, location) = steps[hop];
                        assert_eq!(url.as_str(), expected_url);
                        assert!(!https_only || hop == 0);
                        let mut response = http::Response::builder().url(url);
                        if let Some(location) = location {
                            response = response
                                .status(StatusCode::FOUND)
                                .header(reqwest::header::LOCATION, location);
                        }
                        Ok(response.body(reqwest::Body::from("")).unwrap().into())
                    },
                )
                .await;

            if https_only {
                assert!(matches!(
                    redirected,
                    Err(RedirectSendError::Mirror(MirrorError::HttpsRequired))
                ));
            } else {
                assert_eq!(redirected.unwrap().final_resolved.url, source_https);
            }
        }
    }

    #[tokio::test]
    async fn https_downgrade_rejects_mirror_headers_and_inline_credentials() {
        use reqwest::ResponseBuilderExt;

        let mut rule = base_rule("source.example.test");
        rule.rules = Some("source.example.test".into());
        rule.headers = Some(HashMap::from([(
            "Authorization".into(),
            "Bearer fixture-only".into(),
        )]));

        for (router, intermediate, location) in [
            (
                MirrorRouter::from_rules(vec![rule.clone()]),
                None,
                "http://source.example.test/final",
            ),
            (
                MirrorRouter::from_rules(vec![rule]),
                Some("http://fallback.example.test/next"),
                "http://source.example.test/final",
            ),
            (
                MirrorRouter::empty(),
                None,
                "http://fixture:fixture-only@source.example.test/final",
            ),
        ] {
            let result = router
                .send_with_redirects(
                    Method::GET,
                    "https://source.example.test/start",
                    RedirectRequestOptions::default(),
                    |req, _, _| req,
                    |_| Ok::<(), ()>(()),
                    |req, hop| async move {
                        let location = match (hop, intermediate) {
                            (0, Some(intermediate)) => intermediate,
                            (0, None) | (1, Some(_)) => location,
                            _ => panic!("the credentialed HTTP hop should not be requested"),
                        };
                        let response = http::Response::builder()
                            .url(req.build()?.url().clone())
                            .status(StatusCode::FOUND)
                            .header(LOCATION, location)
                            .body(reqwest::Body::from(""))
                            .unwrap();
                        Ok(response.into())
                    },
                )
                .await;
            assert!(matches!(
                result,
                Err(RedirectSendError::Mirror(MirrorError::InvalidRedirect(_)))
            ));
        }
    }

    #[test]
    fn custom_timeout_cache_is_bounded_and_cannot_crowd_out_streaming_or_core_pools() {
        let router = MirrorRouter::empty();
        assert_eq!(router.cached_client_count(), 0);

        // Fill legacy custom-timeout bucket past its limit.
        for secs in 1..=(MAX_CUSTOM_TIMEOUT_CLIENTS as u64 + 10) {
            let _ = router
                .build_client_for("https://example.com/d", Some(Duration::from_secs(secs)))
                .unwrap();
        }
        assert_eq!(router.cached_client_count(), MAX_CUSTOM_TIMEOUT_CLIENTS);

        // Streaming clients live in their own bucket and are never crowded out.
        let _ = router
            .streaming_client_for(
                "https://example.com/file.vpk",
                Duration::from_secs(15),
                Duration::from_secs(60),
            )
            .unwrap();
        assert_eq!(router.cached_streaming_client_count(), 1);
        let _ = router
            .streaming_client_for(
                "https://example.com/file2.vpk",
                Duration::from_secs(15),
                Duration::from_secs(60),
            )
            .unwrap();
        assert_eq!(router.cached_streaming_client_count(), 1);

        // Core clients live in their own bucket and remain cached.
        let _ = router
            .get("https://example.com/core", Some(Duration::from_secs(5)))
            .unwrap();
        let count_after_core = router.cached_client_count();
        let _ = router
            .post("https://example.com/core2", Some(Duration::from_secs(30)))
            .unwrap();
        assert_eq!(router.cached_client_count(), count_after_core);
    }
}
