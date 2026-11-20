use crate::bridge::{
    BridgeDownloadStatus, BridgeError, SteamBridge, SteamCapabilities, SubscribedItemInfo,
};
use async_trait::async_trait;
use l4a_protocol::{
    WorkshopCollectionRecord, WorkshopFetchOutcome, WorkshopId, WorkshopItemRecord,
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Default)]
struct MockState {
    available: bool,
    outcomes: HashMap<WorkshopId, WorkshopFetchOutcome>,
    collections: HashMap<WorkshopId, WorkshopCollectionRecord>,
    subscribed: Vec<SubscribedItemInfo>,
}

/// 内存态 Mock Steamworks Bridge，供单元测试与集成测试无 Steam 客户端运行。
#[derive(Clone, Default)]
pub struct MockSteamBridge {
    state: Arc<RwLock<MockState>>,
}

impl MockSteamBridge {
    pub fn new(available: bool) -> Self {
        Self {
            state: Arc::new(RwLock::new(MockState {
                available,
                ..Default::default()
            })),
        }
    }

    pub fn insert_item(&self, item: WorkshopItemRecord) {
        let id = item.workshop_id;
        self.state
            .write()
            .unwrap()
            .outcomes
            .insert(id, WorkshopFetchOutcome::Found { item });
    }

    pub fn insert_outcome(&self, outcome: WorkshopFetchOutcome) {
        let id = outcome.workshop_id();
        self.state.write().unwrap().outcomes.insert(id, outcome);
    }

    pub fn insert_collection(&self, collection: WorkshopCollectionRecord) {
        let id = collection.collection_id;
        self.state
            .write()
            .unwrap()
            .collections
            .insert(id, collection);
    }

    pub fn set_subscribed(&self, items: Vec<SubscribedItemInfo>) {
        self.state.write().unwrap().subscribed = items;
    }
}

#[async_trait]
impl SteamBridge for MockSteamBridge {
    async fn capabilities(&self) -> SteamCapabilities {
        let available = self.state.read().unwrap().available;
        SteamCapabilities {
            bridge_available: available,
            bridge_initialized: available,
            bridge_version: available.then(|| "mock-2.0".to_string()),
            current_user_steam_id: available.then(|| "76561198000000001".to_string()),
            last_error: (!available).then(|| "mock unavailable".to_string()),
        }
    }

    async fn fetch_items(
        &self,
        ids: &[WorkshopId],
    ) -> Result<Vec<WorkshopFetchOutcome>, BridgeError> {
        let guard = self.state.read().unwrap();
        if !guard.available {
            return Err(BridgeError::Unavailable("mock unavailable".into()));
        }
        Ok(ids
            .iter()
            .filter_map(|id| guard.outcomes.get(id).cloned())
            .collect())
    }

    async fn fetch_collection(
        &self,
        collection_id: WorkshopId,
    ) -> Result<WorkshopCollectionRecord, BridgeError> {
        let guard = self.state.read().unwrap();
        if !guard.available {
            return Err(BridgeError::Unavailable("mock unavailable".into()));
        }
        guard
            .collections
            .get(&collection_id)
            .cloned()
            .ok_or_else(|| BridgeError::CallFailed("collection not found".into()))
    }

    async fn subscribed_items(&self) -> Result<Vec<SubscribedItemInfo>, BridgeError> {
        let guard = self.state.read().unwrap();
        if !guard.available {
            return Err(BridgeError::Unavailable("mock unavailable".into()));
        }
        Ok(guard.subscribed.clone())
    }

    async fn request_download(&self, _id: WorkshopId) -> Result<(), BridgeError> {
        let guard = self.state.read().unwrap();
        if !guard.available {
            return Err(BridgeError::Unavailable("mock unavailable".into()));
        }
        Ok(())
    }

    async fn download_status(&self, _id: WorkshopId) -> Result<BridgeDownloadStatus, BridgeError> {
        let guard = self.state.read().unwrap();
        if !guard.available {
            return Err(BridgeError::Unavailable("mock unavailable".into()));
        }
        Ok(BridgeDownloadStatus::default())
    }

    async fn shutdown(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::NullSteamBridge;

    #[tokio::test]
    async fn null_bridge_reports_unavailable() {
        let bridge = NullSteamBridge::new("disabled by user");
        let caps = bridge.capabilities().await;
        assert!(!caps.bridge_available);
        assert!(bridge
            .fetch_items(&[WorkshopId::new(1).unwrap()])
            .await
            .is_err());
    }
}
