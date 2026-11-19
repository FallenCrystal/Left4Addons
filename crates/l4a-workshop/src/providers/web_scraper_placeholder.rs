use crate::error::WorkshopError;
use crate::provider::{ProviderCapabilities, WorkshopProvider};
use async_trait::async_trait;
use l4a_protocol::{
    MetadataSource, SearchQuery, SearchResponse, WorkshopCollectionRecord, WorkshopFetchOutcome,
    WorkshopId, WorkshopItemRecord,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCategory {
    pub category: String,
    pub tags: Vec<String>,
}

/// 未来接入 Steam Community 网页端抓取的显式扩展契约。
///
/// 当前阶段按架构约束故意保留为占位接口；后续可在独立 crate 或本 trait 实现类中接入 HTML SSR 解析，
/// 而不影响 `l4a-core` 与 `WorkshopResolver` 的任何调用代码。
#[async_trait]
pub trait WorkshopWebScraper: Send + Sync {
    async fn scrape_item_page(
        &self,
        workshop_id: WorkshopId,
    ) -> Result<WorkshopItemRecord, WorkshopError>;

    async fn scrape_browse_page(
        &self,
        query: &SearchQuery,
    ) -> Result<SearchResponse, WorkshopError>;

    async fn scrape_collection_page(
        &self,
        collection_id: WorkshopId,
    ) -> Result<WorkshopCollectionRecord, WorkshopError>;

    async fn scrape_tag_tree(&self) -> Result<Vec<TagCategory>, WorkshopError>;
}

/// 默认网页抓取占位实现：所有方法均明确返回 [`WorkshopError::ScraperNotImplemented`]。
#[derive(Debug, Default)]
pub struct PlaceholderWebScraper;

#[async_trait]
impl WorkshopWebScraper for PlaceholderWebScraper {
    async fn scrape_item_page(
        &self,
        workshop_id: WorkshopId,
    ) -> Result<WorkshopItemRecord, WorkshopError> {
        Err(WorkshopError::ScraperNotImplemented(format!(
            "scrape_item_page({workshop_id})"
        )))
    }

    async fn scrape_browse_page(
        &self,
        _query: &SearchQuery,
    ) -> Result<SearchResponse, WorkshopError> {
        Err(WorkshopError::ScraperNotImplemented(
            "scrape_browse_page".into(),
        ))
    }

    async fn scrape_collection_page(
        &self,
        collection_id: WorkshopId,
    ) -> Result<WorkshopCollectionRecord, WorkshopError> {
        Err(WorkshopError::ScraperNotImplemented(format!(
            "scrape_collection_page({collection_id})"
        )))
    }

    async fn scrape_tag_tree(&self) -> Result<Vec<TagCategory>, WorkshopError> {
        Err(WorkshopError::ScraperNotImplemented(
            "scrape_tag_tree".into(),
        ))
    }
}

/// 将 [`WorkshopWebScraper`] 适配进统一 [`WorkshopProvider`] 路由管线。
pub struct WebScraperPlaceholderProvider {
    scraper: Arc<dyn WorkshopWebScraper>,
}

impl WebScraperPlaceholderProvider {
    pub fn new(scraper: Arc<dyn WorkshopWebScraper>) -> Self {
        Self { scraper }
    }

    pub fn placeholder() -> Self {
        Self::new(Arc::new(PlaceholderWebScraper))
    }
}

#[async_trait]
impl WorkshopProvider for WebScraperPlaceholderProvider {
    fn name(&self) -> &str {
        "web-scraper-placeholder"
    }

    fn source_kind(&self) -> MetadataSource {
        MetadataSource::WebScraper
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            fetch_items: false,
            fetch_collection: false,
            search: false,
            provides_file_url: false,
            provides_dependencies: false,
            supports_status_tombstone: false,
        }
    }

    async fn fetch_items(
        &self,
        ids: &[WorkshopId],
    ) -> Result<Vec<WorkshopFetchOutcome>, WorkshopError> {
        let Some(&first) = ids.first() else {
            return Ok(Vec::new());
        };
        let item = self.scraper.scrape_item_page(first).await?;
        Ok(vec![WorkshopFetchOutcome::Found { item }])
    }

    async fn fetch_collection(
        &self,
        collection_id: WorkshopId,
    ) -> Result<WorkshopCollectionRecord, WorkshopError> {
        self.scraper.scrape_collection_page(collection_id).await
    }

    async fn search(&self, query: &SearchQuery) -> Result<SearchResponse, WorkshopError> {
        self.scraper.scrape_browse_page(query).await
    }
}
