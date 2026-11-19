use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkshopError {
    #[error("网络请求失败: {0}")]
    Network(String),
    #[error("远端服务返回错误状态 {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("响应解析失败: {0}")]
    Parse(String),
    #[error("能力不支持: {0}")]
    Unsupported(String),
    #[error("创意工坊 Web 抓取模块当前为占位实现，尚未接入具体抓取逻辑: {0}")]
    ScraperNotImplemented(String),
    #[error("下载任务已被取消")]
    Cancelled,
    #[error("断点续传校验不一致: {0}")]
    IntegrityMismatch(String),
    #[error("本地文件操作失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("底层提供商报错: {0}")]
    Provider(String),
}
