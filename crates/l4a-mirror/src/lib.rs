mod pool;
mod redirect;
mod rules;
#[cfg(test)]
mod tests;

pub use redirect::{RedirectHop, RedirectRequestOptions, RedirectSendError, RedirectedResponse};
pub use rules::{MirrorRule, ResolvedRequest};

use pool::ClientPool;
use rules::CompiledRule;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use thiserror::Error;

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

    pub fn set_https_only(&self, enabled: bool) {
        self.https_only.store(enabled, Ordering::Relaxed);
    }

    /// 仅供集成测试使用：使缓存客户端禁用环境代理 (`no_proxy`)，避免本机 `HTTP_PROXY` 干扰 loopback 夹具。
    pub fn set_no_proxy_for_testing(&self, enabled: bool) {
        self.no_proxy_for_testing.store(enabled, Ordering::Relaxed);
    }
}
