use crate::pool::RedirectMode;
use crate::rules::{apply_resolved_headers, ensure_https_if_required, url_authority};
use crate::{MirrorError, MirrorRouter, ResolvedRequest};
use futures_util::FutureExt;
use reqwest::header::{HeaderValue, LOCATION, REFERER};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use thiserror::Error;

pub(crate) const DEFAULT_MAX_REDIRECTS: usize = 10;
const MAX_REDIRECT_BODY_DRAIN_BYTES: usize = 4096;
pub(crate) const PAYLOAD_HEADERS: &[&str] = &[
    "content-type",
    "content-length",
    "content-encoding",
    "transfer-encoding",
];

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

struct NextHopTransition {
    next_url: url::Url,
    next_resolved: ResolvedRequest,
    next_method: Method,
    is_same_origin: bool,
    referer: Option<HeaderValue>,
}

impl MirrorRouter {
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
        let initial_https_only = self.effective_https_only(options);
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
            let req = self.build_hop_request(
                hop,
                &current_method,
                &current_resolved,
                referer.take(),
                options,
                started_at,
                &mut configure,
            )?;

            let mut response = send(req, hop).await?;
            let status = response.status();
            let Some(location_value) = is_redirect_status(status)
                .then(|| response.headers().get(LOCATION))
                .flatten()
                .cloned()
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

            let transition = self.resolve_next_hop(
                hop,
                status,
                &location_value,
                response.url(),
                &current_method,
                &current_resolved,
                &mirror_sources,
                &mut used_https,
                &mut drop_payload_headers,
                options,
                &mut validate_hop,
            )?;

            referer = transition.referer;
            drain_ready_redirect_body(&mut response);
            current_method = transition.next_method;
            if !transition.is_same_origin
                && transition
                    .next_resolved
                    .rewrites_authority(&transition.next_url)
            {
                mirror_sources.push(transition.next_url);
            }
            current_resolved = transition.next_resolved;
        }

        Err(RedirectSendError::Mirror(MirrorError::TooManyRedirects))
    }

    fn effective_https_only(&self, options: RedirectRequestOptions) -> bool {
        options
            .https_only_override
            .unwrap_or_else(|| self.https_only.load(Ordering::Relaxed))
    }

    #[allow(clippy::too_many_arguments)]
    fn build_hop_request<FBuild, E>(
        &self,
        hop: usize,
        current_method: &Method,
        current_resolved: &ResolvedRequest,
        referer: Option<HeaderValue>,
        options: RedirectRequestOptions,
        started_at: Instant,
        configure: &mut FBuild,
    ) -> Result<RequestBuilder, RedirectSendError<E>>
    where
        FBuild: FnMut(RequestBuilder, usize, &Method) -> RequestBuilder,
    {
        let https_only = self.effective_https_only(options);
        ensure_https_if_required(&current_resolved.url, https_only)?;

        let key = self.pool_key_for_resolved(
            current_resolved,
            https_only,
            RedirectMode::None,
            None,
            options.connect_timeout,
            options.read_timeout,
        );
        let client = self.get_or_build_client(key)?;
        let req = apply_resolved_headers(
            client.request(current_method.clone(), &current_resolved.url),
            current_resolved,
        );
        let mut req = configure(req, hop, current_method);
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
        Ok(req)
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_next_hop<FValidate, E>(
        &self,
        hop: usize,
        status: StatusCode,
        location_value: &HeaderValue,
        current_url: &url::Url,
        current_method: &Method,
        current_resolved: &ResolvedRequest,
        mirror_sources: &[url::Url],
        used_https: &mut bool,
        drop_payload_headers: &mut bool,
        options: RedirectRequestOptions,
        validate_hop: &mut FValidate,
    ) -> Result<NextHopTransition, RedirectSendError<E>>
    where
        FValidate: FnMut(&RedirectHop<'_>) -> Result<(), E>,
    {
        let location = std::str::from_utf8(location_value.as_bytes())
            .map_err(|_| MirrorError::InvalidRedirect("Location 头包含无效的 UTF-8".to_string()))?;
        *used_https |= current_url.scheme() == "https";
        let next_url = current_url
            .join(location)
            .map_err(|e| MirrorError::InvalidRedirect(e.to_string()))?;
        if !matches!(next_url.scheme(), "http" | "https") {
            return Err(RedirectSendError::Mirror(MirrorError::InvalidRedirect(
                format!("不支持的重定向协议: {}", next_url.scheme()),
            )));
        }

        let next_https_only = self.effective_https_only(options);
        let is_same_origin = urls_same_origin(current_url, &next_url);
        let mut next_resolved = if is_same_origin {
            let mut resolved_url = next_url.clone();
            if resolved_url.username().is_empty() && resolved_url.password().is_none() {
                // Reqwest converts URL credentials to Authorization and removes them from
                // the response URL used to resolve Location.
                let previous = url::Url::parse(&current_resolved.url).ok();
                if let Some(previous) = previous.filter(|url| urls_same_origin(url, current_url)) {
                    let _ = resolved_url.set_username(previous.username());
                    let _ = resolved_url.set_password(previous.password());
                }
            }
            ResolvedRequest {
                url: resolved_url.into(),
                ..current_resolved.clone()
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

        let (next_method, drops_payload) = next_redirect_method(status, current_method.clone());
        // Later hops can match another rule after the request body has been discarded.
        *drop_payload_headers |= drops_payload;
        if *drop_payload_headers {
            next_resolved.headers.retain(|name, _| {
                !PAYLOAD_HEADERS
                    .iter()
                    .any(|header| name.eq_ignore_ascii_case(header))
            });
        }
        ensure_https_if_required(&next_resolved.url, next_https_only)?;
        if *used_https
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

        let referer = is_same_origin
            .then(|| redirect_referer(current_url))
            .flatten();

        Ok(NextHopTransition {
            next_url,
            next_resolved,
            next_method,
            is_same_origin,
            referer,
        })
    }
}

pub(crate) fn urls_same_origin(a: &url::Url, b: &url::Url) -> bool {
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
