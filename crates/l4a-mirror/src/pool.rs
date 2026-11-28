use crate::redirect::{urls_same_origin, DEFAULT_MAX_REDIRECTS};
use crate::rules::apply_resolved_headers;
use crate::{MirrorError, MirrorRouter, ResolvedRequest};
use reqwest::{Client, Method, RequestBuilder};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::time::Duration;

const MAX_STREAMING_CLIENTS: usize = 16;
pub(crate) const MAX_CUSTOM_TIMEOUT_CLIENTS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RedirectMode {
    Follow,
    SameOriginOnly,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ClientPoolKey {
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
pub(crate) struct ClientPool {
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

impl MirrorRouter {
    /// 统一推导有效的客户端池 Key（集中约束 `insecure_tls && !https_only` 以及镜像跨源重定向隔离）。
    pub(crate) fn pool_key_for_resolved(
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

    pub(crate) fn get_or_build_client(&self, key: ClientPoolKey) -> Result<Client, MirrorError> {
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
    pub(crate) fn cached_client_count(&self) -> usize {
        self.clients
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .total_len()
    }

    #[cfg(test)]
    pub(crate) fn cached_streaming_client_count(&self) -> usize {
        self.clients
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .streaming
            .len()
    }
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
