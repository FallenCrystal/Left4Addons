use crate::error::WorkshopError;
use crate::provider::{ProviderCapabilities, WorkshopProvider};
use async_trait::async_trait;
use l4a_protocol::{
    MetadataSource, WorkshopCollectionRecord, WorkshopFetchOutcome, WorkshopId,
};
use l4a_steam::SteamBridge;
use std::sync::Arc;

pub struct SteamBridgeProvider {
    bridge: Arc<dyn SteamBridge>,
}

impl SteamBridgeProvider {
    pub fn new(bridge: Arc<dyn SteamBridge>) -> Self {
        Self { bridge }
    }
}

#[async_trait]
impl WorkshopProvider for SteamBridgeProvider {
    fn name(&self) -> &str {
        "steamworks-sdk"
    }

    fn source_kind(&self) -> MetadataSource {
        MetadataSource::SteamworksSdk
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            fetch_items: true,
            fetch_collection: true,
            search: false,
            provides_file_url: false,
            provides_dependencies: true,
            supports_status_tombstone: false,
        }
    }

    async fn fetch_items(
        &self,
        ids: &[WorkshopId],
    ) -> Result<Vec<WorkshopFetchOutcome>, WorkshopError> {
        self.bridge
            .fetch_items(ids)
            .await
            .map_err(|e| WorkshopError::Provider(e.to_string()))
    }

    async fn fetch_collection(
        &self,
        collection_id: WorkshopId,
    ) -> Result<WorkshopCollectionRecord, WorkshopError> {
        self.bridge
            .fetch_collection(collection_id)
            .await
            .map_err(|e| WorkshopError::Provider(e.to_string()))
    }
}
