use async_trait::async_trait;
use l4a_protocol::ScraperPlaceholderStatus;

/// 服务端后台主动爬虫契约占位。
#[async_trait]
pub trait CatalogScraper: Send + Sync {
    async fn status(&self, queued_jobs: u64) -> ScraperPlaceholderStatus;
}

#[derive(Debug, Default)]
pub struct PlaceholderCatalogScraper;

#[async_trait]
impl CatalogScraper for PlaceholderCatalogScraper {
    async fn status(&self, queued_jobs: u64) -> ScraperPlaceholderStatus {
        ScraperPlaceholderStatus {
            implemented: false,
            provider_name: "placeholder-server-scraper".to_string(),
            queued_jobs,
            message: "服务端主动创意工坊爬虫当前为占位实现 (数据可通过可信玩家提交或后续接入独立 Scraper Adapter)".to_string(),
        }
    }
}
