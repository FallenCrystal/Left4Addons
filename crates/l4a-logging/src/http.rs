//! Audited outbound HTTP.
//!
//! Wrap `reqwest::RequestBuilder::send()` with [`send`] (or [`send_quiet`] for high-volume
//! requests such as thumbnails) so every network request is recorded under the `l4a::net`
//! target with method, redacted URL, status, time-to-response-headers and size. Request headers
//! and bodies are never logged.

use crate::redact::{error_chain, redact_url};
use reqwest::{RequestBuilder, Response};
use std::time::Instant;

/// Log target used for all outbound network records.
pub const NET_TARGET: &str = "l4a::net";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SuccessLevel {
    Info,
    Debug,
}

/// Sends the request and logs the outcome at `INFO` (failures and HTTP errors at `WARN`).
///
/// `purpose` is a short, stable label such as `"workshop.details"`.
pub async fn send(builder: RequestBuilder, purpose: &str) -> reqwest::Result<Response> {
    send_inner(builder, purpose, SuccessLevel::Info).await
}

/// Like [`send`], but successful responses are logged at `DEBUG` to keep logs readable for
/// frequent requests (cover images, polling). Failures are still logged at `WARN`.
pub async fn send_quiet(builder: RequestBuilder, purpose: &str) -> reqwest::Result<Response> {
    send_inner(builder, purpose, SuccessLevel::Debug).await
}

/// Method-style access to [`send`] / [`send_quiet`]: replace `.send()` with
/// `.send_audited("purpose")` in a request builder chain.
pub trait AuditedSend {
    /// See [`send`].
    fn send_audited(
        self,
        purpose: &'static str,
    ) -> impl std::future::Future<Output = reqwest::Result<Response>> + Send;

    /// See [`send_quiet`].
    fn send_audited_quiet(
        self,
        purpose: &'static str,
    ) -> impl std::future::Future<Output = reqwest::Result<Response>> + Send;
}

impl AuditedSend for RequestBuilder {
    fn send_audited(
        self,
        purpose: &'static str,
    ) -> impl std::future::Future<Output = reqwest::Result<Response>> + Send {
        send_inner(self, purpose, SuccessLevel::Info)
    }

    fn send_audited_quiet(
        self,
        purpose: &'static str,
    ) -> impl std::future::Future<Output = reqwest::Result<Response>> + Send {
        send_inner(self, purpose, SuccessLevel::Debug)
    }
}

async fn send_inner(
    builder: RequestBuilder,
    purpose: &str,
    level: SuccessLevel,
) -> reqwest::Result<Response> {
    let (client, request) = builder.build_split();
    let request = match request {
        Ok(request) => request,
        Err(err) => {
            tracing::warn!(
                target: NET_TARGET,
                purpose,
                error = %error_chain(&err),
                "Failed to build HTTP request"
            );
            return Err(err);
        }
    };
    let method = request.method().clone();
    let url = redact_url(request.url());
    let started = Instant::now();
    match client.execute(request).await {
        Ok(response) => {
            let elapsed_ms = started.elapsed().as_millis() as u64;
            let status = response.status();
            let bytes = response.content_length();
            if status.is_client_error() || status.is_server_error() {
                tracing::warn!(
                    target: NET_TARGET,
                    purpose,
                    %method,
                    url = %url,
                    status = status.as_u16(),
                    elapsed_ms,
                    bytes,
                    "HTTP request returned error status"
                );
            } else if level == SuccessLevel::Info {
                tracing::info!(
                    target: NET_TARGET,
                    purpose,
                    %method,
                    url = %url,
                    status = status.as_u16(),
                    elapsed_ms,
                    bytes,
                    "HTTP request completed"
                );
            } else {
                tracing::debug!(
                    target: NET_TARGET,
                    purpose,
                    %method,
                    url = %url,
                    status = status.as_u16(),
                    elapsed_ms,
                    bytes,
                    "HTTP request completed"
                );
            }
            Ok(response)
        }
        Err(err) => {
            tracing::warn!(
                target: NET_TARGET,
                purpose,
                %method,
                url = %url,
                elapsed_ms = started.elapsed().as_millis() as u64,
                timeout = err.is_timeout(),
                error = %error_chain(&err),
                "HTTP request failed"
            );
            Err(err)
        }
    }
}
