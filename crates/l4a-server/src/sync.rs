use crate::config::CatalogConfigStore;
use crate::db::{
    CatalogIndexPolicy, ServerRepo, StoredItemMeta, STATE_BACKFILL_CURSOR,
    STATE_BACKFILL_SYNCED_TOTAL, STATE_INCREMENTAL_SCAN, STATE_REMOTE_WORKSHOP_TOTAL,
};
use crate::upstream::{
    WorkshopObservation, WorkshopUpstream, DETAILS_BATCH_SIZE, MATCHING_FILE_TYPE_COLLECTIONS,
    MATCHING_FILE_TYPE_ITEMS, QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE,
    QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
};
use chrono::{DateTime, Utc};
use l4a_protocol::{
    AdminFetchUpstreamResponse, BatchItemsResponse, CatalogSyncStatus, MetadataSource,
    WorkshopCollectionRecord, WorkshopFetchOutcome, WorkshopFileType, WorkshopId,
    WorkshopItemRecord,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::{broadcast, Mutex, Notify};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

// The desktop HTTP client gives the Catalog 15 seconds, including transfer.
const PUBLIC_REFRESH_BUDGET: Duration = Duration::from_secs(10);
const LISTING_RECHECK_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Default, Deserialize, Serialize)]
struct IncrementalScan {
    checkpoint: Option<i64>,
    pending: Option<IncrementalPage>,
}

#[derive(Deserialize, Serialize)]
struct IncrementalPage {
    cursor: String,
    newest: Option<i64>,
}

fn load_incremental_scan(repo: &ServerRepo) -> anyhow::Result<IncrementalScan> {
    match repo.get_sync_state(STATE_INCREMENTAL_SCAN)? {
        Some(value) => Ok(serde_json::from_str(&value)?),
        None => Ok(IncrementalScan::default()),
    }
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
    inflight: Mutex<HashMap<WorkshopId, broadcast::Receiver<Result<(), String>>>>,
    metrics: StdMutex<SyncRuntimeMetrics>,
    incremental_lock: Mutex<()>,
    backfill_lock: Arc<Mutex<()>>,
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
            metrics: StdMutex::new(SyncRuntimeMetrics::default()),
            incremental_lock: Mutex::new(()),
            backfill_lock: Arc::new(Mutex::new(())),
            wake_notify: Notify::new(),
        })
    }

    pub fn notify_config_changed(&self) {
        self.wake_notify.notify_waiters();
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
        self: &Arc<Self>,
        wid: WorkshopId,
        force_refresh: bool,
    ) -> anyhow::Result<Option<WorkshopItemRecord>> {
        self.resolve_single_until(wid, force_refresh, Instant::now() + PUBLIC_REFRESH_BUDGET)
            .await
    }

    async fn resolve_single_until(
        self: &Arc<Self>,
        wid: WorkshopId,
        force_refresh: bool,
        deadline: Instant,
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
            self.repo
                .negative_status(wid, sync_cfg.item_ttl_secs)
                .is_none()
        };

        if should_fetch {
            let refresh = self.singleflight_refresh(&[wid], deadline).await;
            let current = self.db_call(move |r| r.get_item(wid)).await?;
            if let Err(err) = refresh {
                tracing::warn!(workshop_id = %wid, error = %err, "Steam CM 上游刷新失败");
                if force_refresh && current.is_none() {
                    return Err(err);
                }
            }
            return Ok(current.map(|m| with_catalog_source(m.record)));
        }

        Ok(cached.map(|m| with_catalog_source(m.record)))
    }

    /// 供 `POST /api/v1/items/batch` 使用：批量返回缓存条目，对过期或缺失条目自动合并 Singleflight 上游拉取。
    pub async fn resolve_batch(
        self: &Arc<Self>,
        ids: &[WorkshopId],
        force_refresh: bool,
    ) -> anyhow::Result<BatchItemsResponse> {
        self.resolve_batch_until(ids, force_refresh, Instant::now() + PUBLIC_REFRESH_BUDGET)
            .await
    }

    async fn resolve_batch_until(
        self: &Arc<Self>,
        ids: &[WorkshopId],
        force_refresh: bool,
        deadline: Instant,
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
                        .repo
                        .negative_status(*wid, sync_cfg.item_ttl_secs)
                        .is_none()
                {
                    to_refresh.push(*wid);
                }
            }
        }

        if !to_refresh.is_empty() {
            if let Err(err) = self.singleflight_refresh(&to_refresh, deadline).await {
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
            if let Some(neg_status) = self.repo.negative_status(wid, sync_cfg.item_ttl_secs) {
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
        self: &Arc<Self>,
        collection_id: WorkshopId,
        force_refresh: bool,
    ) -> anyhow::Result<Option<WorkshopCollectionRecord>> {
        let deadline = Instant::now() + PUBLIC_REFRESH_BUDGET;
        self.resolve_single_until(collection_id, force_refresh, deadline)
            .await?;
        let col_opt = self
            .db_call(move |r| r.get_collection(collection_id))
            .await?;
        let Some(col) = col_opt else {
            return Ok(None);
        };

        if !col.child_ids.is_empty() {
            self.resolve_batch_until(&col.child_ids, force_refresh, deadline)
                .await?;
            return self.db_call(move |r| r.get_collection(collection_id)).await;
        }

        Ok(Some(col))
    }

    /// 管理员手动强制拉取指定 ID（若包含合集则一并拉取合集内全部子条目），并将公开条目纳入目录索引。
    pub async fn admin_force_fetch(
        &self,
        ids: &[WorkshopId],
    ) -> anyhow::Result<AdminFetchUpstreamResponse> {
        self.admin_force_fetch_with_policy(ids, CatalogIndexPolicy::IndexInCatalog)
            .await
    }

    /// 管理员强制重刷已缓存条目，保留原有目录收录状态（避免将仅按 ID 穿透缓存的条目暴露到公开首页）。
    pub async fn admin_refresh_cached(
        &self,
        ids: &[WorkshopId],
    ) -> anyhow::Result<AdminFetchUpstreamResponse> {
        self.admin_force_fetch_with_policy(ids, CatalogIndexPolicy::CacheOnly)
            .await
    }

    async fn admin_force_fetch_with_policy(
        &self,
        ids: &[WorkshopId],
        index_policy: CatalogIndexPolicy,
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

        let mut child_ids = Vec::new();
        let mut removed_ids = Vec::new();
        let mut unavailable_ids = Vec::new();
        for observation in &outcomes {
            match &observation.outcome {
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

        self.apply_outcomes(outcomes, index_policy).await?;
        if !child_ids.is_empty() {
            if let Ok(child_outcomes) = self.upstream.fetch_details(&child_ids).await {
                let _ = self.apply_outcomes(child_outcomes, index_policy).await;
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

    async fn singleflight_refresh(
        self: &Arc<Self>,
        ids: &[WorkshopId],
        deadline: Instant,
    ) -> anyhow::Result<()> {
        let mut receivers = Vec::new();
        let mut leaders = Vec::new();
        {
            let mut map = self.inflight.lock().await;
            for &wid in ids {
                if let Some(rx) = map.get(&wid).filter(|rx| !rx.is_closed()) {
                    receivers.push(rx.resubscribe());
                } else {
                    let (tx, rx) = broadcast::channel(1);
                    receivers.push(rx.resubscribe());
                    map.insert(wid, rx);
                    leaders.push((wid, tx));
                }
            }
        }

        for group in leaders.chunks(DETAILS_BATCH_SIZE) {
            let coordinator = Arc::clone(self);
            let group = group.to_vec();
            // Apply successful chunks independently of other chunks and of the HTTP deadline.
            tokio::spawn(async move {
                let ids: Vec<_> = group.iter().map(|(wid, _)| *wid).collect();
                let result = async {
                    let outcomes = coordinator.upstream.fetch_details(&ids).await?;
                    let verified: HashSet<_> =
                        outcomes.iter().map(|o| o.outcome.workshop_id()).collect();
                    coordinator
                        .apply_outcomes(outcomes, CatalogIndexPolicy::CacheOnly)
                        .await?;
                    Ok::<_, anyhow::Error>(verified)
                }
                .await
                .map_err(|err| err.to_string());

                let mut map = coordinator.inflight.lock().await;
                for (wid, sender) in group {
                    map.remove(&wid);
                    let signal = match &result {
                        Ok(verified) if verified.contains(&wid) => Ok(()),
                        Ok(_) => Err(format!("Steam CM 未核验条目 {wid}")),
                        Err(err) => Err(err.clone()),
                    };
                    let _ = sender.send(signal);
                }
            });
        }
        drop(leaders);

        tokio::time::timeout_at(deadline, async {
            let mut first_error = None;
            for mut rx in receivers {
                if let Err(message) = rx.recv().await? {
                    first_error.get_or_insert_with(|| anyhow::Error::msg(message));
                }
            }
            first_error.map_or(Ok(()), Err)
        })
        .await?
    }

    async fn apply_outcomes(
        &self,
        outcomes: Vec<WorkshopObservation>,
        index_policy: CatalogIndexPolicy,
    ) -> anyhow::Result<u64> {
        let summary = self
            .db_call(move |repo| repo.apply_upstream_outcomes_with_policy(&outcomes, index_policy))
            .await?;
        Ok(summary.active_written())
    }

    async fn recheck_listings(
        self: &Arc<Self>,
        cursor: &mut Option<WorkshopId>,
        ttl_secs: u64,
    ) -> anyhow::Result<()> {
        let after = *cursor;
        let recheck_age = i64::try_from(ttl_secs - ttl_secs / 3).unwrap_or(i64::MAX);
        let verified_before = Utc::now().timestamp().saturating_sub(recheck_age);
        let ids = self
            .db_call(move |repo| {
                repo.listing_recheck_ids(after, verified_before, DETAILS_BATCH_SIZE)
            })
            .await?;
        // Continue past failures so one inaccessible item never starves the rest of the cache.
        *cursor = if ids.len() == DETAILS_BATCH_SIZE {
            ids.last().copied()
        } else {
            None
        };
        if ids.is_empty() {
            return Ok(());
        }
        self.singleflight_refresh(&ids, Instant::now() + PUBLIC_REFRESH_BUDGET)
            .await
    }

    pub async fn run_incremental_once(self: &Arc<Self>) -> anyhow::Result<u64> {
        let coordinator = Arc::clone(self);
        tokio::spawn(async move { coordinator.run_incremental_job().await }).await?
    }

    async fn run_incremental_job(&self) -> anyhow::Result<u64> {
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
        let mut scan = self.db_call(load_incremental_scan).await?;
        let mut pending = scan.pending.take().unwrap_or(IncrementalPage {
            cursor: "*".into(),
            newest: None,
        });
        let mut total_upserted = 0;

        for _ in 0..sync_cfg.incremental_pages_per_tick {
            let page = self
                .upstream
                .query_files(
                    QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE,
                    MATCHING_FILE_TYPE_ITEMS,
                    &pending.cursor,
                    50,
                )
                .await?;

            let page_newest = page
                .outcomes
                .iter()
                .filter_map(|observation| match &observation.outcome {
                    WorkshopFetchOutcome::Found { item } => item.time_updated,
                    _ => None,
                })
                .max();
            // Keep the scan's upper bound once observed so newer arrivals are revisited.
            pending.newest = pending.newest.or(page_newest);
            // Scan timestamp ties too, since an item's update timestamp has second precision.
            let past_boundary = page_newest
                .zip(scan.checkpoint)
                .is_some_and(|(newest, checkpoint)| newest < checkpoint);

            total_upserted += self
                .apply_outcomes(page.outcomes, CatalogIndexPolicy::IndexInCatalog)
                .await?;

            let next = page.next_cursor.filter(|_| !past_boundary);
            let stalled =
                next.as_deref() == Some(pending.cursor.as_str()) || next.as_deref() == Some("*");
            let complete = next.is_none();
            if complete {
                scan.checkpoint = scan.checkpoint.max(pending.newest);
            }
            if let Some(next) = next.filter(|_| !stalled) {
                scan.pending = Some(IncrementalPage {
                    cursor: next,
                    newest: pending.newest,
                });
            }

            let value = serde_json::to_string(&scan)?;
            self.db_call(move |r| {
                if page.total > 0 {
                    r.set_sync_states(&[
                        (STATE_INCREMENTAL_SCAN, &value),
                        (STATE_REMOTE_WORKSHOP_TOTAL, &page.total.to_string()),
                    ])
                } else {
                    r.set_sync_state(STATE_INCREMENTAL_SCAN, &value)
                }
            })
            .await?;
            if stalled {
                // Retry from the committed checkpoint on the next tick.
                anyhow::bail!("Steam CM 增量同步游标未前进");
            }
            match scan.pending.take() {
                Some(next) => pending = next,
                None => break,
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
            total_upserted += self
                .apply_outcomes(col_page.outcomes, CatalogIndexPolicy::IndexInCatalog)
                .await?;
        }

        Ok(total_upserted)
    }

    pub async fn run_backfill_step_once(self: &Arc<Self>) -> anyhow::Result<u64> {
        let coordinator = Arc::clone(self);
        tokio::spawn(async move { coordinator.run_backfill_job().await }).await?
    }

    async fn run_backfill_job(&self) -> anyhow::Result<u64> {
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
            match &res {
                Ok(Some(_)) => {
                    m.last_sync_error = None;
                }
                Ok(None) => {}
                Err(err) => {
                    m.last_sync_error = Some(err.to_string());
                }
            }
        }
        res.map(|count| count.unwrap_or(0))
    }

    async fn run_backfill_inner(&self) -> anyhow::Result<Option<u64>> {
        let batch_size = self
            .config_store
            .get()
            .sync
            .backfill_batch_size
            .clamp(1, 100);
        let cursor = self
            .db_call(|r| r.get_sync_state(STATE_BACKFILL_CURSOR))
            .await?
            .unwrap_or_else(|| "*".to_string());

        if cursor == "DONE" {
            return Ok(None);
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

        let next_cursor = page
            .next_cursor
            .filter(|c| c != &cursor && c != "*" && !page.outcomes.is_empty())
            .unwrap_or_else(|| "DONE".to_string());
        let written = self
            .db_call(move |r| r.apply_backfill_page(&page.outcomes, &next_cursor, page.total))
            .await?;
        Ok(Some(written))
    }

    pub async fn reset_backfill_cursor(&self) -> anyhow::Result<()> {
        let guard = self.backfill_lock.clone().lock_owned().await;
        self.db_call(move |r| {
            let _guard = guard;
            r.set_sync_states(&[
                (STATE_BACKFILL_CURSOR, "*"),
                (STATE_BACKFILL_SYNCED_TOTAL, "0"),
            ])
        })
        .await
    }

    pub async fn status(&self) -> anyhow::Result<CatalogSyncStatus> {
        let cfg_dto = self.config_store.get().sync.to_dto();
        let (high_water, cursor, synced_total, remote_total) = self
            .db_call(|r| {
                let hw = load_incremental_scan(r)?.checkpoint;
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
        let feed_token = token.clone();

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
                    _ = feed_token.cancelled() => break,
                    _ = coordinator.wake_notify.notified() => {},
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {},
                }
            }
        });

        let coordinator = self.clone();
        let listing_token = token.clone();
        tokio::spawn(async move {
            let mut cursor = None;
            let mut interval = tokio::time::interval(LISTING_RECHECK_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                tokio::select! {
                    _ = listing_token.cancelled() => break,
                    _ = interval.tick() => {},
                }

                let ttl_secs = coordinator.config_store.get().sync.item_ttl_secs;
                if let Err(err) = coordinator.recheck_listings(&mut cursor, ttl_secs).await {
                    tracing::warn!(error = %err, "后台公开条目核验未完成");
                }
            }
        });

        token
    }
}

fn is_stale(last_verified_at: DateTime<Utc>, ttl_secs: u64) -> bool {
    (Utc::now() - last_verified_at).num_seconds().max(0) as u64 >= ttl_secs
}

pub(crate) fn with_catalog_source(mut record: WorkshopItemRecord) -> WorkshopItemRecord {
    if record.source != MetadataSource::AdminOverride {
        record.source = MetadataSource::CatalogServer;
    }
    record
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CatalogConfig;
    use crate::upstream::MockWorkshopUpstream;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn public_deadline_returns_cache_while_the_refresh_is_waiting_to_write() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Arc::new(ServerRepo::open(&dir.path().join("catalog.db")).unwrap());
        repo.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO workshop_items
                    (workshop_id, file_type, status, title, file_url, source, fetched_at, last_verified_at)
                 VALUES (1, 'item', 'active', 'Map', 'cached-link', 'steam_kit', ?1, ?1)",
                [Utc::now().to_rfc3339()],
            )?;
            Ok(())
        }).unwrap();
        let coordinator = CatalogSyncCoordinator::new(
            Arc::new(CatalogConfigStore::new(None, CatalogConfig::default())),
            Arc::clone(&repo),
            Arc::new(MockWorkshopUpstream::new()),
        );

        let (locked_tx, locked_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let hold_writer = tokio::task::spawn_blocking(move || {
            repo.with_transaction(|_| {
                let _ = locked_tx.send(());
                let _ = release_rx.blocking_recv();
                Ok(())
            })
        });
        locked_rx.await.unwrap();
        let item = tokio::time::timeout(
            Duration::from_secs(2),
            coordinator.resolve_single_until(
                WorkshopId(1),
                true,
                Instant::now() + Duration::from_millis(50),
            ),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(item.file_url.as_deref(), Some("cached-link"));
        release_tx.send(()).unwrap();
        hold_writer.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn backfill_recovery_clears_last_sync_error_without_clearing_on_done_noop() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Arc::new(ServerRepo::open(&dir.path().join("catalog.db")).unwrap());
        let upstream = Arc::new(MockWorkshopUpstream::new());
        let coordinator = CatalogSyncCoordinator::new(
            Arc::new(CatalogConfigStore::new(None, CatalogConfig::default())),
            Arc::clone(&repo),
            upstream.clone(),
        );

        upstream.set_fail_fetches(true);
        assert!(coordinator.run_backfill_step_once().await.is_err());
        assert!(coordinator
            .status()
            .await
            .unwrap()
            .last_sync_error
            .is_some());

        upstream.set_fail_fetches(false);
        assert_eq!(coordinator.run_backfill_step_once().await.unwrap(), 0);
        assert!(coordinator
            .status()
            .await
            .unwrap()
            .last_sync_error
            .is_none());

        // Cursor is now DONE; an incremental error must not be wiped by a no-op backfill tick.
        upstream.set_fail_fetches(true);
        assert!(coordinator.run_incremental_once().await.is_err());
        assert!(coordinator
            .status()
            .await
            .unwrap()
            .last_sync_error
            .is_some());
        upstream.set_fail_fetches(false);
        assert_eq!(coordinator.run_backfill_step_once().await.unwrap(), 0);
        assert!(coordinator
            .status()
            .await
            .unwrap()
            .last_sync_error
            .is_some());
    }
}
