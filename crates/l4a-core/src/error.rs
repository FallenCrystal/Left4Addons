use l4a_protocol::{DesktopError, DesktopErrorCode};
use l4a_workshop::WorkshopError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("配置解析/序列化错误: {0}")]
    Config(String),
    #[error("VPK 错误: {0}")]
    Vpk(String),
    #[error("创意工坊服务错误: {0}")]
    Workshop(#[from] WorkshopError),
    #[error("网络请求错误: {0}")]
    Network(String),
    #[error("当前环境不支持: {0}")]
    Unsupported(String),
    #[error("实体未找到: {0}")]
    NotFound(String),
    #[error("操作冲突: {0}")]
    Conflict(String),
    #[error("参数无效: {0}")]
    InvalidInput(String),
    #[error("操作回滚未完成: {0}")]
    RecoveryRequired(String),
}

impl From<CoreError> for DesktopError {
    fn from(error: CoreError) -> Self {
        use DesktopErrorCode as Code;

        let code = match &error {
            CoreError::Db(_) => Code::Database,
            CoreError::Io(_) | CoreError::Workshop(WorkshopError::Io(_)) => Code::Io,
            CoreError::Config(_) => Code::Config,
            CoreError::Vpk(_) => Code::Vpk,
            CoreError::Workshop(WorkshopError::Cancelled) => Code::Cancelled,
            CoreError::Network(_)
            | CoreError::Workshop(WorkshopError::Network(_) | WorkshopError::HttpStatus { .. }) => {
                Code::Network
            }
            CoreError::Unsupported(_)
            | CoreError::Workshop(
                WorkshopError::Unsupported(_) | WorkshopError::ScraperNotImplemented(_),
            ) => Code::Unsupported,
            CoreError::Workshop(_) => Code::Workshop,
            CoreError::NotFound(_) => Code::NotFound,
            CoreError::Conflict(_) => Code::Conflict,
            CoreError::InvalidInput(_) => Code::InvalidInput,
            CoreError::RecoveryRequired(_) => Code::RecoveryRequired,
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}
