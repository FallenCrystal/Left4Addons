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
    Workshop(#[from] l4a_workshop::WorkshopError),
    #[error("实体未找到: {0}")]
    NotFound(String),
    #[error("操作冲突: {0}")]
    Conflict(String),
    #[error("参数无效: {0}")]
    InvalidInput(String),
}
