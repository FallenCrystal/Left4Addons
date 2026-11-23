use super::L4aEngine;
use crate::error::CoreError;
use crate::lifecycle::sync_lifecycle;
use crate::tasks::{create_task, update_task};
use l4a_mirror::MirrorRouter;
use l4a_protocol::{
    AppConfig, CoreEvent, SearchQuery, SearchResponse, SyncReport, TaskKind, TaskStatus,
    WorkshopFetchOutcome, WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use l4a_steam::{BridgeError, SteamBridge};
use l4a_workshop::{
    CatalogBackendProvider, SourceRoutingPolicy, SteamBridgeProvider, SteamWebApiProvider,
    WebScraperPlaceholderProvider, WorkshopError, WorkshopProvider, WorkshopResolver,
};
use std::sync::Arc;

impl L4aEngine {
    pub async fn sync_workshop(&self) -> Result<SyncReport, CoreError> {
        self.hydration_backoff.lock().unwrap().clear();
        let resolver = self.resolver.read().unwrap().clone();

        let mut task = create_task(&self.db, TaskKind::Sync, "workshop-sync", None)?;
        update_task(&self.db, &mut task, TaskStatus::Running, 10, None, None)?;

        match sync_lifecycle(&self.db, &resolver).await {
            Ok(report) => {
                if let Ok(_workspace) = self.lock_workspace() {
                    self.refresh_bundles_logged();
                }
                update_task(
                    &self.db,
                    &mut task,
                    TaskStatus::Completed,
                    100,
                    Some(format!("同步完成，检查 {} 个条目", report.checked_items)),
                    None,
                )?;
                self.events.emit(CoreEvent::SnapshotUpdated {
                    external_change: false,
                });
                Ok(report)
            }
            Err(err) => {
                update_task(
                    &self.db,
                    &mut task,
                    TaskStatus::Failed,
                    0,
                    None,
                    Some(err.to_string()),
                )?;
                Err(err)
            }
        }
    }

    pub async fn search_workshop(&self, query: &SearchQuery) -> Result<SearchResponse, CoreError> {
        let resolver = self.resolver.read().unwrap().clone();
        let (resp, _) = resolver.search(query).await?;
        Ok(resp)
    }

    pub async fn fetch_workshop_item(
        &self,
        workshop_id: WorkshopId,
    ) -> Result<WorkshopItemRecord, CoreError> {
        let resolver = self.resolver.read().unwrap().clone();
        fetch_workshop_item(&resolver, workshop_id).await
    }

    pub async fn subscribed_workshop_ids(&self) -> Result<Vec<WorkshopId>, CoreError> {
        let items = self.steam_bridge.subscribed_items().await.map_err(|err| {
            let message = err.to_string();
            match err {
                BridgeError::Unavailable(_) => WorkshopError::Unsupported(message),
                _ => WorkshopError::Provider(message),
            }
        })?;
        Ok(items.into_iter().map(|item| item.workshop_id).collect())
    }
}

pub(super) async fn fetch_workshop_item(
    resolver: &WorkshopResolver,
    workshop_id: WorkshopId,
) -> Result<WorkshopItemRecord, CoreError> {
    let (outcomes, _) = resolver.fetch_items(&[workshop_id]).await?;
    outcomes
        .into_iter()
        .find_map(|outcome| match outcome {
            WorkshopFetchOutcome::Found { item }
                if item.workshop_id == workshop_id
                    && !matches!(
                        item.status,
                        WorkshopItemStatus::Removed | WorkshopItemStatus::PrivateOrUnavailable
                    ) =>
            {
                Some(item)
            }
            _ => None,
        })
        .ok_or_else(|| CoreError::NotFound(format!("工坊条目不可用或已下架: {workshop_id}")))
}

pub(super) fn build_resolver_from_config(
    config: &AppConfig,
    mirror: Arc<MirrorRouter>,
    steam_bridge: Arc<dyn SteamBridge>,
) -> Arc<WorkshopResolver> {
    let mut providers: Vec<Arc<dyn WorkshopProvider>> = Vec::new();

    let catalog = config
        .catalog_server_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|url| Arc::new(CatalogBackendProvider::new(url, mirror.clone())));

    for source in &config.source_order {
        match source.as_str() {
            "catalog-server" => {
                if let Some(ref c) = catalog {
                    providers.push(c.clone());
                }
            }
            "steamworks-sdk" => {
                if !config.disable_steamworks_sdk {
                    providers.push(Arc::new(SteamBridgeProvider::new(steam_bridge.clone())));
                }
            }
            "steam-web-api" => {
                providers.push(Arc::new(SteamWebApiProvider::new(mirror.clone())));
            }
            "web-scraper" => {
                providers.push(Arc::new(WebScraperPlaceholderProvider::placeholder()));
            }
            _ => {}
        }
    }

    Arc::new(WorkshopResolver::new(
        providers,
        SourceRoutingPolicy::default(),
    ))
}
