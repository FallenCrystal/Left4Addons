use crate::config::CatalogConfigStore;
use crate::db::{ServerRepo, StoredItemMeta};
use crate::upstream::{
    WorkshopUpstream, MATCHING_FILE_TYPE_COLLECTIONS, MATCHING_FILE_TYPE_ITEMS,
    QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE, QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
};
use chrono::{DateTime, Utc};
use l4a_protocol::{
    AdminFetchUpstreamResponse, BatchItemsResponse, CatalogSyncStatus, MetadataSource,
    WorkshopCollectionRecord, WorkshopFetchOutcome, WorkshopFileType, WorkshopId,
    WorkshopItemRecord, WorkshopItemStatus,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::{broadcast, Mutex, Notify};
use tokio_util::sync::CancellationToken;

const STATE_HIGH_WATER_UPDATED: &str = "high_water_time_updated";
const STATE_BACKFILL_CURSOR: &str = "backfill_cursor";
const STATE_BACKFILL_SYNCED_TOTAL: &str = "backfill_synced_total";
const STATE_REMOTE_WORKSHOP_TOTAL: &str = "remote_workshop_total";
const MAX_NEGATIVE_CACHE_ENTRIES: usize = 4096;

#[derive(Clone)]
struct NegativeCacheEntry {
    status: WorkshopItemStatus,
    verified_at: DateTime<Utc>,
}

#[derive(Default)]
struct SyncRuntimeMetrics {
    incremental_running: bool,
    last_incremental_at: Option<DateTime<Utc>>,
    last_incremental_upserted: u64,
    backfill_running: bool,
    last_sync_error: Option<String>,
}

pub struct CatalogSyncCoordinator {
    config_store: Arc<CatalogConfigStore>,
    repo: Arc<ServerRepo>,
    upstream: Arc<dyn WorkshopUpstream>,
    inflight: Mutex<HashMap<WorkshopId, broadcast::Sender<Result<(), String>>>>,
    negative_cache: StdMutex<HashMap<WorkshopId, NegativeCacheEntry>>,
    metrics: StdMutex<SyncRuntimeMetrics>,
    incremental_lock: Mutex<()>,
    backfill_lock: Mutex<()>,
    wake_notify: Notify,
}

impl CatalogSyncCoordinator {
    pub fn new(
        config_store: Arc<CatalogConfigStore>,
        repo: Arc<ServerRepo>,
        upstream: Arc<dyn WorkshopUpstream>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config_store,
            repo,
            upstream,
            inflight: Mutex::new(HashMap::new()),
            negative_cache: StdMutex::new(HashMap::new()),
            metrics: StdMutex::new(SyncRuntimeMetrics::default()),
            incremental_lock: Mutex::new(()),
            backfill_lock: Mutex::new(()),
            wake_notify: Notify::new(),
        })
    }

    pub fn notify_config_changed(&self) {
        self.wake_notify.notify_waiters();
    }

    fn check_negative_cache(&self, wid: WorkshopId, ttl_secs: u64) -> Option<WorkshopItemStatus> {
        let now = Utc::now();
        let guard = self.negative_cache.lock().unwrap();
        let entry = guard.get(&wid)?;
        if (now - entry.verified_at).num_seconds() < ttl_secs as i64 {
            Some(entry.status)
        } else {
            None
        }
    }

    fn put_negative_cache(&self, wid: WorkshopId, status: WorkshopItemStatus) {
        let mut guard = self.negative_cache.lock().unwrap();
        if guard.len() >= MAX_NEGATIVE_CACHE_ENTRIES {
            guard.clear();
        }
        guard.insert(
            wid,
            NegativeCacheEntry {
                status,
                verified_at: Utc::now(),
            },
        );
    }

    fn clear_negative_cache(&self, wid: WorkshopId) {
        self.negative_cache.lock().unwrap().remove(&wid);
    }

    async fn db_call<T: Send + 'static>(
        &self,
        op: impl FnOnce(&ServerRepo) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let repo = self.repo.clone();
        tokio::task::spawn_blocking(move || op(&repo)).await?
    }

    /// 供 `GET /api/v1/items/{id}` 使用：解析单条工坊详情（支持 Read-Through、TTL 过期重刷与 `?refresh=true`）。
    pub async fn resolve_single_item(
        &self,
        wid: WorkshopId,
        force_refresh: bool,
    ) -> anyhow::Result<Option<WorkshopItemRecord>> {
        let sync_cfg = self.config_store.get().sync;
        let cached = self.db_call(move |r| r.get_item(wid)).await?;

        let should_fetch = if cached.as_ref().is_some_and(|m| m.is_admin_override) {
            false
        } else if force_refresh {
            true
        } else if !sync_cfg.enable_read_through {
            false
        } else if let Some(ref meta) = cached {
            is_stale(meta.last_verified_at, sync_cfg.item_ttl_secs)
        } else {
            self.check_negative_cache(wid, sync_cfg.item_ttl_secs)
                .is_none()
        };

        if should_fetch {
            if let Err(err) = self.singleflight_refresh(&[wid]).await {
                if let Some(meta) = cached {
                    tracing::warn!(
                        workshop_id = %wid,
                        error = %err,
                        "Steam CM 上游刷新失败，回退返回本地已有缓存快照"
                    );
                    return Ok(Some(with_catalog_source(meta.record)));
                }
                if force_refresh {
                    return Err(err);
                }
            }
            let refreshed = self.db_call(move |r| r.get_item(wid)).await?;
            return Ok(refreshed.map(|m| with_catalog_source(m.record)));
        }

        Ok(cached.map(|m| with_catalog_source(m.record)))
    }

    /// 供 `POST /api/v1/items/batch` 使用：批量返回缓存条目，对过期或缺失条目自动合并 Singleflight 上游拉取。
    pub async fn resolve_batch(
        &self,
        ids: &[WorkshopId],
        force_refresh: bool,
    ) -> anyhow::Result<BatchItemsResponse> {
        if ids.is_empty() {
            return Ok(BatchItemsResponse {
                items: Vec::new(),
                missing_ids: Vec::new(),
            });
        }

        let sync_cfg = self.config_store.get().sync;
        let ids_vec = ids.to_vec();
        let (cached_list, _) = self.db_call(move |r| r.get_batch(&ids_vec)).await?;
        let cached_map: HashMap<WorkshopId, StoredItemMeta> = cached_list
            .into_iter()
            .map(|m| (m.record.workshop_id, m))
            .collect();

        let mut to_refresh = Vec::new();
        if force_refresh || sync_cfg.enable_read_through {
            for wid in ids {
                if let Some(meta) = cached_map.get(wid) {
                    if !meta.is_admin_override
                        && (force_refresh
                            || is_stale(meta.last_verified_at, sync_cfg.item_ttl_secs))
                    {
                        to_refresh.push(*wid);
                    }
                } else if force_refresh
                    || self
                        .check_negative_cache(*wid, sync_cfg.item_ttl_secs)
                        .is_none()
                {
                    to_refresh.push(*wid);
                }
            }
        }

        if !to_refresh.is_empty() {
            if let Err(err) = self.singleflight_refresh(&to_refresh).await {
                tracing::warn!(error = %err, count = to_refresh.len(), "批量穿透刷新部分失败，回退使用本地缓存");
            }
        }

        let ids_reload = ids.to_vec();
        let (final_cached, still_missing) = self.db_call(move |r| r.get_batch(&ids_reload)).await?;
        let mut items: Vec<WorkshopItemRecord> = final_cached
            .into_iter()
            .map(|m| with_catalog_source(m.record))
            .collect();

        let mut truly_missing = Vec::new();
        let now = Utc::now();
        for wid in still_missing {
            if let Some(neg_status) = self.check_negative_cache(wid, sync_cfg.item_ttl_secs) {
                items.push(WorkshopItemRecord {
                    workshop_id: wid,
                    file_type: WorkshopFileType::Item,
                    status: neg_status,
                    title: format!("Workshop #{wid}"),
                    description: None,
                    creator_steam_id: None,
                    creator_name: None,
                    preview_url: None,
                    previews: Vec::new(),
                    file_url: None,
                    file_size: None,
                    time_created: None,
                    time_updated: None,
                    tags: Vec::new(),
                    dependencies: Vec::new(),
                    collection_children: Vec::new(),
                    subscriptions: None,
                    favorites: None,
                    source: MetadataSource::CatalogServer,
                    fetched_at: now,
                });
            } else {
                truly_missing.push(wid);
            }
        }

        Ok(BatchItemsResponse {
            items,
            missing_ids: truly_missing,
        })
    }

    /// 供 `GET /api/v1/collections/{id}` 使用：拉取合集并自动补全子条目详情。
    pub async fn resolve_collection(
        &self,
        collection_id: WorkshopId,
        force_refresh: bool,
    ) -> anyhow::Result<Option<WorkshopCollectionRecord>> {
        let _ = self
            .resolve_single_item(collection_id, force_refresh)
            .await?;
        let col_opt = self
            .db_call(move |r| r.get_collection(collection_id))
            .await?;
        let Some(col) = col_opt else {
            return Ok(None);
        };

        if !col.child_ids.is_empty() {
            let _ = self.resolve_batch(&col.child_ids, force_refresh).await?;
            return self.db_call(move |r| r.get_collection(collection_id)).await;
        }

        Ok(Some(col))
    }

    /// 管理员手动强制拉取指定 ID（若包含合集则一并拉取合集内全部子条目）。
    pub async fn admin_force_fetch(
        &self,
        ids: &[WorkshopId],
    ) -> anyhow::Result<AdminFetchUpstreamResponse> {
        let mut seen = HashSet::new();
        let deduped: Vec<WorkshopId> = ids.iter().copied().filter(|id| seen.insert(*id)).collect();
        if deduped.is_empty() {
            return Ok(AdminFetchUpstreamResponse {
                items: Vec::new(),
                removed_ids: Vec::new(),
                unavailable_ids: Vec::new(),
            });
        }

        let outcomes = self.upstream.fetch_details(&deduped).await?;
        self.apply_and_index_outcomes(&outcomes).await?;

        let mut child_ids = Vec::new();
        let mut removed_ids = Vec::new();
        let mut unavailable_ids = Vec::new();
        for o in &outcomes {
            match o {
                WorkshopFetchOutcome::Found { item } => {
                    if item.file_type == WorkshopFileType::Collection {
                        for cid in &item.collection_children {
                            if seen.insert(*cid) {
                                child_ids.push(*cid);
                            }
                        }
                    }
                }
                WorkshopFetchOutcome::Removed { workshop_id, .. } => {
                    removed_ids.push(*workshop_id);
                }
                WorkshopFetchOutcome::PrivateOrUnavailable { workshop_id, .. } => {
                    unavailable_ids.push(*workshop_id);
                }
            }
        }

        if !child_ids.is_empty() {
            if let Ok(child_outcomes) = self.upstream.fetch_details(&child_ids).await {
                let _ = self.apply_and_index_outcomes(&child_outcomes).await;
            }
        }

        let lookup_ids = deduped;
        let (stored, _) = self.db_call(move |r| r.get_batch(&lookup_ids)).await?;
        let items = stored.into_iter().map(|m| m.record).collect();

        Ok(AdminFetchUpstreamResponse {
            items,
            removed_ids,
            unavailable_ids,
        })
    }

    async fn singleflight_refresh(&self, ids: &[WorkshopId]) -> anyhow::Result<()> {
        let mut leader_ids = Vec::new();
        let mut leader_senders = Vec::new();
        let mut follower_receivers = Vec::new();

        {
            let mut map = self.inflight.lock().await;
            for wid in ids {
                if let Some(tx) = map.get(wid) {
                    follower_receivers.push(tx.subscribe());
                } else {
                    let (tx, _) = broadcast::channel(1);
                    map.insert(*wid, tx.clone());
                    leader_ids.push(*wid);
                    leader_senders.push((*wid, tx));
                }
            }
        }

        let mut leader_err: Option<anyhow::Error> = None;
        if !leader_ids.is_empty() {
            let res = async {
                let outcomes = self.upstream.fetch_details(&leader_ids).await?;
                self.apply_and_index_outcomes(&outcomes).await?;
                Ok::<(), anyhow::Error>(())
            }
            .await;

            let broadcast_msg = match &res {
                Ok(()) => Ok(()),
                Err(e) => Err(e.to_string()),
            };

            {
                let mut map = self.inflight.lock().await;
                for (wid, tx) in leader_senders {
                    map.remove(&wid);
                    let _ = tx.send(broadcast_msg.clone());
                }
            }

            if let Err(err) = res {
                leader_err = Some(err);
            }
        }

        for mut rx in follower_receivers {
            if let Ok(Err(msg)) = rx.recv().await {
                if leader_err.is_none() {
                    leader_err = Some(anyhow::anyhow!(msg));
                }
            }
        }

        if let Some(err) = leader_err {
            return Err(err);
        }
        Ok(())
    }

    async fn apply_and_index_outcomes(
        &self,
        outcomes: &[WorkshopFetchOutcome],
    ) -> anyhow::Result<u64> {
        for o in outcomes {
            match o {
                WorkshopFetchOutcome::Found { item } => {
                    self.clear_negative_cache(item.workshop_id);
                }
                WorkshopFetchOutcome::Removed { workshop_id, .. } => {
                    self.put_negative_cache(*workshop_id, WorkshopItemStatus::Removed);
                }
                WorkshopFetchOutcome::PrivateOrUnavailable { workshop_id, .. } => {
                    self.put_negative_cache(*workshop_id, WorkshopItemStatus::PrivateOrUnavailable);
                }
            }
        }

        let owned = outcomes.to_vec();
        let summary = self
            .db_call(move |r| r.apply_upstream_outcomes(&owned))
            .await?;

        if let Some(max_ts) = summary.max_time_updated {
            let _ = self.advance_high_water_updated(max_ts).await;
        }
        Ok(summary.active_upserted)
    }

    async fn advance_high_water_updated(&self, candidate: i64) -> anyhow::Result<()> {
        self.db_call(move |r| {
            let prev = r
                .get_sync_state(STATE_HIGH_WATER_UPDATED)?
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0);
            if candidate > prev {
                r.set_sync_state(STATE_HIGH_WATER_UPDATED, &candidate.to_string())?;
            }
            Ok(())
        })
        .await
    }

    pub async fn run_incremental_once(&self) -> anyhow::Result<u64> {
        let Ok(_guard) = self.incremental_lock.try_lock() else {
            return Ok(0);
        };
        {
            let mut m = self.metrics.lock().unwrap();
            m.incremental_running = true;
        }

        let res = self.run_incremental_inner().await;

        {
            let mut m = self.metrics.lock().unwrap();
            m.incremental_running = false;
            match &res {
                Ok(count) => {
                    m.last_incremental_at = Some(Utc::now());
                    m.last_incremental_upserted = *count;
                    m.last_sync_error = None;
                }
                Err(err) => {
                    m.last_sync_error = Some(err.to_string());
                }
            }
        }

        res
    }

    async fn run_incremental_inner(&self) -> anyhow::Result<u64> {
        let sync_cfg = self.config_store.get().sync;
        let prev_high_water = self
            .db_call(|r| {
                Ok(r.get_sync_state(STATE_HIGH_WATER_UPDATED)?
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0))
            })
            .await?;

        let mut total_upserted = 0u64;
        let mut cursor = "*".to_string();

        for _ in 0..sync_cfg.incremental_pages_per_tick.clamp(1, 20) {
            let page = self
                .upstream
                .query_files(
                    QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE,
                    MATCHING_FILE_TYPE_ITEMS,
                    &cursor,
                    50,
                )
                .await?;

            if page.total > 0 {
                let total_str = page.total.to_string();
                let _ = self
                    .db_call(move |r| r.set_sync_state(STATE_REMOTE_WORKSHOP_TOTAL, &total_str))
                    .await;
            }

            if page.items.is_empty() {
                break;
            }

            let reached_known_waterline = prev_high_water > 0
                && page
                    .items
                    .iter()
                    .all(|i| i.time_updated.unwrap_or(0) <= prev_high_water);

            let outcomes: Vec<WorkshopFetchOutcome> = page
                .items
                .into_iter()
                .map(|item| WorkshopFetchOutcome::Found { item })
                .collect();
            total_upserted += self.apply_and_index_outcomes(&outcomes).await?;

            if reached_known_waterline {
                break;
            }
            match page.next_cursor {
                Some(next) if next != cursor => cursor = next,
                _ => break,
            }
        }

        // 同步最新更新的合集第一页
        if let Ok(col_page) = self
            .upstream
            .query_files(
                QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE,
                MATCHING_FILE_TYPE_COLLECTIONS,
                "*",
                30,
            )
            .await
        {
            if !col_page.items.is_empty() {
                let outcomes: Vec<WorkshopFetchOutcome> = col_page
                    .items
                    .into_iter()
                    .map(|item| WorkshopFetchOutcome::Found { item })
                    .collect();
                total_upserted += self.apply_and_index_outcomes(&outcomes).await?;
            }
        }

        Ok(total_upserted)
    }

    pub async fn run_backfill_step_once(&self) -> anyhow::Result<u64> {
        let Ok(_guard) = self.backfill_lock.try_lock() else {
            return Ok(0);
        };
        {
            let mut m = self.metrics.lock().unwrap();
            m.backfill_running = true;
        }

        let res = self.run_backfill_inner().await;

        {
            let mut m = self.metrics.lock().unwrap();
            m.backfill_running = false;
            if let Err(ref err) = res {
                m.last_sync_error = Some(err.to_string());
            }
        }
        res
    }

    async fn run_backfill_inner(&self) -> anyhow::Result<u64> {
        let batch_size = self
            .config_store
            .get()
            .sync
            .backfill_batch_size
            .clamp(1, 100);
        let (cursor, prev_total) = self
            .db_call(|r| {
                let cur = r
                    .get_sync_state(STATE_BACKFILL_CURSOR)?
                    .unwrap_or_else(|| "*".to_string());
                let synced = r
                    .get_sync_state(STATE_BACKFILL_SYNCED_TOTAL)?
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                Ok((cur, synced))
            })
            .await?;

        if cursor == "DONE" {
            return Ok(0);
        }

        let page = self
            .upstream
            .query_files(
                QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
                MATCHING_FILE_TYPE_ITEMS,
                &cursor,
                batch_size,
            )
            .await?;

        if page.total > 0 {
            let total_str = page.total.to_string();
            let _ = self
                .db_call(move |r| r.set_sync_state(STATE_REMOTE_WORKSHOP_TOTAL, &total_str))
                .await;
        }

        if page.items.is_empty() {
            self.db_call(|r| r.set_sync_state(STATE_BACKFILL_CURSOR, "DONE"))
                .await?;
            return Ok(0);
        }

        let outcomes: Vec<WorkshopFetchOutcome> = page
            .items
            .into_iter()
            .map(|item| WorkshopFetchOutcome::Found { item })
            .collect();
        let count = self.apply_and_index_outcomes(&outcomes).await?;
        let next_cursor = page
            .next_cursor
            .filter(|c| c != &cursor)
            .unwrap_or_else(|| "DONE".to_string());
        let next_total = prev_total + count;

        self.db_call(move |r| {
            r.set_sync_state(STATE_BACKFILL_CURSOR, &next_cursor)?;
            r.set_sync_state(STATE_BACKFILL_SYNCED_TOTAL, &next_total.to_string())?;
            Ok(())
        })
        .await?;

        Ok(count)
    }

    pub async fn reset_backfill_cursor(&self) -> anyhow::Result<()> {
        self.db_call(|r| {
            r.set_sync_state(STATE_BACKFILL_CURSOR, "*")?;
            r.set_sync_state(STATE_BACKFILL_SYNCED_TOTAL, "0")?;
            Ok(())
        })
        .await
    }

    pub async fn status(&self) -> anyhow::Result<CatalogSyncStatus> {
        let cfg_dto = self.config_store.get().sync.to_dto();
        let (high_water, cursor, synced_total, remote_total) = self
            .db_call(|r| {
                let hw = r
                    .get_sync_state(STATE_HIGH_WATER_UPDATED)?
                    .and_then(|v| v.parse::<i64>().ok());
                let cur = r.get_sync_state(STATE_BACKFILL_CURSOR)?;
                let synced = r
                    .get_sync_state(STATE_BACKFILL_SYNCED_TOTAL)?
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                let remote = r
                    .get_sync_state(STATE_REMOTE_WORKSHOP_TOTAL)?
                    .and_then(|v| v.parse::<u64>().ok());
                Ok((hw, cur, synced, remote))
            })
            .await?;

        let m = self.metrics.lock().unwrap();
        Ok(CatalogSyncStatus {
            config: cfg_dto,
            incremental_running: m.incremental_running,
            last_incremental_at: m.last_incremental_at,
            last_incremental_upserted: m.last_incremental_upserted,
            high_water_time_updated: high_water,
            backfill_running: m.backfill_running,
            backfill_cursor: cursor,
            backfill_synced_total: synced_total,
            remote_workshop_total: remote_total,
            last_sync_error: m.last_sync_error.clone(),
        })
    }

    pub fn spawn_background_workers(self: &Arc<Self>) -> CancellationToken {
        let token = CancellationToken::new();
        let coordinator = self.clone();
        let child_token = token.clone();

        tokio::spawn(async move {
            let mut last_inc = tokio::time::Instant::now()
                .checked_sub(Duration::from_secs(3600))
                .unwrap_or_else(tokio::time::Instant::now);
            let mut last_bf = last_inc;

            loop {
                let sync_cfg = coordinator.config_store.get().sync;
                let now = tokio::time::Instant::now();

                if sync_cfg.enable_incremental_sync
                    && now.duration_since(last_inc)
                        >= Duration::from_secs(sync_cfg.incremental_interval_secs.max(30))
                {
                    if let Err(err) = coordinator.run_incremental_once().await {
                        tracing::warn!(error = %err, "后台增量同步执行失败");
                    }
                    last_inc = tokio::time::Instant::now();
                }

                if sync_cfg.enable_backfill
                    && now.duration_since(last_bf)
                        >= Duration::from_secs(sync_cfg.backfill_interval_secs.max(2))
                {
                    if let Err(err) = coordinator.run_backfill_step_once().await {
                        tracing::warn!(error = %err, "后台全量游标回填执行失败");
                    }
                    last_bf = tokio::time::Instant::now();
                }

                tokio::select! {
                    _ = child_token.cancelled() => break,
                    _ = coordinator.wake_notify.notified() => {},
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {},
                }
            }
        });

        token
    }
}

fn is_stale(last_verified_at: DateTime<Utc>, ttl_secs: u64) -> bool {
    (Utc::now() - last_verified_at).num_seconds() >= ttl_secs as i64
}

fn with_catalog_source(mut record: WorkshopItemRecord) -> WorkshopItemRecord {
    if record.source != MetadataSource::AdminOverride {
        record.source = MetadataSource::CatalogServer;
    }
    record
}
