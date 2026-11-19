use crate::error::WorkshopError;
use async_trait::async_trait;
use l4a_protocol::{
    MetadataSource, SearchQuery, SearchResponse, WorkshopCollectionRecord, WorkshopFetchOutcome,
    WorkshopId,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub fetch_items: bool,
    pub fetch_collection: bool,
    pub search: bool,
    pub provides_file_url: bool,
    pub provides_dependencies: bool,
    pub supports_status_tombstone: bool,
}

#[async_trait]
pub trait WorkshopProvider: Send + Sync {
    fn name(&self) -> &str;
    fn source_kind(&self) -> MetadataSource;
    fn capabilities(&self) -> ProviderCapabilities;

    async fn fetch_items(
        &self,
        ids: &[WorkshopId],
    ) -> Result<Vec<WorkshopFetchOutcome>, WorkshopError>;

    async fn fetch_collection(
        &self,
        _collection_id: WorkshopId,
    ) -> Result<WorkshopCollectionRecord, WorkshopError> {
        Err(WorkshopError::Unsupported(format!(
            "{} 不支持查询合集",
            self.name()
        )))
    }

    async fn search(&self, _query: &SearchQuery) -> Result<SearchResponse, WorkshopError> {
        Err(WorkshopError::Unsupported(format!(
            "{} 不支持关键词搜索",
            self.name()
        )))
    }
}
