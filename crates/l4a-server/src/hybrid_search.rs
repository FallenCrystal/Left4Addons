use crate::config::{CatalogConfigStore, SyncSection};
use crate::db::{CatalogIndexPolicy, RankedCatalogMatch, ServerRepo};
use crate::search::{self, MatchRank, ParsedQuery};
use crate::sync::with_catalog_source;
use crate::upstream::{
    SteamSearchRequest, SteamSearchTarget, WorkshopObservation, WorkshopQueryPage,
    WorkshopUpstream, MATCHING_FILE_TYPE_COLLECTIONS, MATCHING_FILE_TYPE_ITEMS,
    QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE, QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
    QUERY_TYPE_RANKED_BY_TEXT_SEARCH, QUERY_TYPE_RANKED_BY_TOTAL_UNIQUE_SUBSCRIPTIONS,
    QUERY_TYPE_RANKED_BY_TREND, QUERY_TYPE_RANKED_BY_VOTE,
};
use chrono::Utc;
use futures_util::future::join_all;
use l4a_protocol::{
    MetadataSource, SearchQuery, SearchResponse, WorkshopFetchOutcome, WorkshopFileType,
    WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, Mutex, OnceCell};

const MAX_SEARCH_CACHE_ENTRIES: usize = 512;
const MAX_INFLIGHT_SEARCHES: usize = 256;
const MAX_WARM_QUEUE_BATCHES: usize = 64;
const MAX_STEAM_MERGE_PAGES: u64 = 24;
const STEAM_SCAN_PAGE_SIZE: u32 = 100;
const MAX_STEAM_MERGE_ITEMS: u64 = MAX_STEAM_MERGE_PAGES * STEAM_SCAN_PAGE_SIZE as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchExecutionMode {
    LocalOnly,
    Hybrid,
    LocalFallback,
}

impl SearchExecutionMode {
    pub fn as_header_value(self) -> &'static str {
        match self {
            Self::LocalOnly => "local",
            Self::Hybrid => "hybrid",
            Self::LocalFallback => "local-fallback",
        }
    }
}

#[derive(Debug, Clone)]
pub struct HybridSearchOutput {
    pub response: SearchResponse,
    pub mode: SearchExecutionMode,
}

struct SharedSteamPage {
    raw: WorkshopQueryPage,
    restrictions_saved: OnceCell<()>,
}

struct CachedSteamPage {
    page: Arc<SharedSteamPage>,
    cached_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeterministicSort {
    BrowseRelevance,
    Subscriptions,
    Updated,
    Newest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeStrategy {
    Relevance,
    SteamOrder,
    Deterministic(DeterministicSort),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SteamSortPlan {
    query_type: u32,
    days: Option<u32>,
    merge: MergeStrategy,
}

#[derive(Clone, Copy)]
struct SteamFetchCtx {
    app_id: u32,
    cache_ttl: Duration,
    steam_timeout: Duration,
    deadline: Instant,
    cache_steam_results: bool,
}

#[derive(Debug, Clone, thiserror::Error)]
enum SteamSearchError {
    #[error("{0}")]
    Unavailable(String),
    #[error("保存 Steam 可见性失败: {0}")]
    Visibility(String),
}

enum SteamResults {
    Complete(Vec<Arc<SharedSteamPage>>),
    NativePage {
        page: Arc<SharedSteamPage>,
        total: u64,
    },
    NativePageEmpty {
        total: u64,
    },
}

enum ExactLookup {
    Local,
    Steam(Option<Arc<WorkshopObservation>>),
    Unavailable,
}

#[derive(Clone, Copy)]
struct Pagination {
    page: u32,
    size: u32,
    offset: usize,
}

impl Pagination {
    fn new(query: &SearchQuery) -> Self {
        let page = query.page.max(1);
        let size = query.page_size.clamp(1, 100);
        let offset = usize::try_from(u64::from(page - 1) * u64::from(size)).unwrap_or(usize::MAX);
        Self { page, size, offset }
    }

    fn output(
        self,
        items: Vec<WorkshopItemRecord>,
        total: u64,
        mode: SearchExecutionMode,
    ) -> HybridSearchOutput {
        HybridSearchOutput {
            response: SearchResponse {
                items: items.into_iter().map(with_catalog_source).collect(),
                total,
                page: self.page,
                page_size: self.size,
                source: MetadataSource::CatalogServer,
            },
            mode,
        }
    }
}

#[derive(Clone, Copy)]
enum SearchHit<'a> {
    Catalog(RankedCatalogMatch),
    Steam(&'a WorkshopItemRecord),
}

impl SearchHit<'_> {
    fn id(self) -> WorkshopId {
        match self {
            Self::Catalog(matched) => matched.id,
            Self::Steam(item) => item.workshop_id,
        }
    }

    fn should_promote(self, steam_best_tier: Option<u8>) -> bool {
        let Self::Catalog(matched) = self else {
            // An exact ID fetched separately from Steam participates as a tier-zero match.
            return true;
        };
        matched.is_active
            && matched.rank.is_some_and(|rank| {
                rank.tier == 0
                    || (matched.is_admin_override && rank.tier <= 3)
                    || (rank.tier <= 5 && steam_best_tier.is_none_or(|best| rank.tier < best))
            })
    }

    fn sort_key(self, sort: DeterministicSort) -> (Reverse<i128>, Reverse<i128>, u64) {
        let primary = match (self, sort) {
            (
                Self::Catalog(matched),
                DeterministicSort::Subscriptions | DeterministicSort::BrowseRelevance,
            ) => i128::from(matched.sort_value.max(0)),
            (Self::Catalog(matched), _) => i128::from(matched.sort_value),
            (
                Self::Steam(item),
                DeterministicSort::Subscriptions | DeterministicSort::BrowseRelevance,
            ) => i128::from(item.subscriptions.unwrap_or(0)),
            (Self::Steam(item), DeterministicSort::Updated) => {
                i128::from(item.time_updated.unwrap_or(0))
            }
            (Self::Steam(item), DeterministicSort::Newest) => {
                i128::from(item.time_created.unwrap_or(0))
            }
        };
        let secondary = match (self, sort) {
            (Self::Catalog(matched), DeterministicSort::BrowseRelevance) => {
                i128::from(matched.secondary_sort_value)
            }
            (Self::Steam(item), DeterministicSort::BrowseRelevance) => {
                i128::from(item.time_updated.unwrap_or(0))
            }
            _ => 0,
        };
        let id = self.id().as_u64();
        let tie_breaker = match sort {
            DeterministicSort::Subscriptions | DeterministicSort::BrowseRelevance => id,
            DeterministicSort::Updated | DeterministicSort::Newest => u64::MAX - id,
        };
        (Reverse(primary), Reverse(secondary), tie_breaker)
    }
}

type SharedPageResult = Result<Arc<SharedSteamPage>, SteamSearchError>;
type SharedExactResult = Result<Option<Arc<WorkshopObservation>>, SteamSearchError>;

struct WarmBatch {
    observations: Vec<WorkshopObservation>,
    policy: CatalogIndexPolicy,
}

#[derive(Default)]
struct SearchCacheState {
    cache: HashMap<SteamSearchRequest, CachedSteamPage>,
    inflight: HashMap<SteamSearchRequest, broadcast::Receiver<SharedPageResult>>,
    inflight_exact: HashMap<WorkshopId, broadcast::Receiver<SharedExactResult>>,
}

struct PageInflightGuard {
    state: Arc<Mutex<SearchCacheState>>,
    key: Option<SteamSearchRequest>,
}

impl PageInflightGuard {
    fn disarm(&mut self) {
        self.key = None;
    }
}

impl Drop for PageInflightGuard {
    fn drop(&mut self) {
        let Some(key) = self.key.take() else {
            return;
        };
        if let Ok(mut guard) = self.state.try_lock() {
            guard.inflight.remove(&key);
        } else if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let state = Arc::clone(&self.state);
            handle.spawn(async move {
                state.lock().await.inflight.remove(&key);
            });
        }
    }
}

struct ExactInflightGuard {
    state: Arc<Mutex<SearchCacheState>>,
    key: Option<WorkshopId>,
}

impl ExactInflightGuard {
    fn disarm(&mut self) {
        self.key = None;
    }
}

impl Drop for ExactInflightGuard {
    fn drop(&mut self) {
        let Some(key) = self.key.take() else {
            return;
        };
        if let Ok(mut guard) = self.state.try_lock() {
            guard.inflight_exact.remove(&key);
        } else if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let state = Arc::clone(&self.state);
            handle.spawn(async move {
                state.lock().await.inflight_exact.remove(&key);
            });
        }
    }
}

pub struct HybridSearchService {
    config_store: Arc<CatalogConfigStore>,
    repo: Arc<ServerRepo>,
    upstream: Arc<dyn WorkshopUpstream>,
    cache_state: Arc<Mutex<SearchCacheState>>,
    warm_tx: mpsc::Sender<WarmBatch>,
    warm_rx: StdMutex<Option<mpsc::Receiver<WarmBatch>>>,
}

impl HybridSearchService {
    pub fn new(
        config_store: Arc<CatalogConfigStore>,
        repo: Arc<ServerRepo>,
        upstream: Arc<dyn WorkshopUpstream>,
    ) -> Self {
        let (warm_tx, warm_rx) = mpsc::channel(MAX_WARM_QUEUE_BATCHES);
        Self {
            config_store,
            repo,
            upstream,
            cache_state: Arc::new(Mutex::new(SearchCacheState::default())),
            warm_tx,
            warm_rx: StdMutex::new(Some(warm_rx)),
        }
    }

    async fn db_call<T: Send + 'static>(
        &self,
        op: impl FnOnce(&ServerRepo) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let repo = self.repo.clone();
        tokio::task::spawn_blocking(move || op(&repo)).await?
    }

    pub async fn search(
        self: &Arc<Self>,
        query: &SearchQuery,
        accept_language: Option<&str>,
    ) -> anyhow::Result<HybridSearchOutput> {
        let pagination = Pagination::new(query);
        let cfg = self.config_store.get();
        let search_cfg = &cfg.search;

        let parsed = query.query.as_deref().and_then(search::parse_query);
        let is_pure_url = parsed
            .as_ref()
            .is_some_and(|p| p.exact_workshop_id.is_some() && p.numeric_prefix.is_none());
        let explicit_file_type = query.file_type.is_some();
        let mut query = query.clone();
        if !is_pure_url && query.file_type.is_none() {
            query.file_type = Some(WorkshopFileType::Item);
        }

        if !search_cfg.enable_hybrid_search || should_use_local_only(&query) {
            return self
                .run_local_only(&query, SearchExecutionMode::LocalOnly)
                .await;
        }

        let steam_timeout = Duration::from_millis(search_cfg.steam_timeout_ms);
        let ctx = SteamFetchCtx {
            app_id: cfg.steam.app_id,
            cache_ttl: Duration::from_secs(search_cfg.cache_ttl_secs),
            steam_timeout,
            deadline: Instant::now() + steam_timeout,
            cache_steam_results: search_cfg.cache_steam_results,
        };
        let search_target = parsed_steam_target(query.search_target.as_deref());

        let mut exact_observation = None;
        if is_pure_url || search_target != Some(SteamSearchTarget::Description) {
            if let Some(wid) = parsed
                .as_ref()
                .filter(|p| {
                    p.numeric_prefix
                        .as_ref()
                        .is_none_or(|prefix| prefix.len() >= 6)
                })
                .and_then(|p| p.exact_workshop_id)
            {
                match self.lookup_exact(WorkshopId(wid), ctx, &cfg.sync).await? {
                    ExactLookup::Local if is_pure_url => {
                        return self
                            .run_local_only(&query, SearchExecutionMode::LocalOnly)
                            .await;
                    }
                    ExactLookup::Unavailable if is_pure_url => {
                        return self
                            .run_local_only(&query, SearchExecutionMode::LocalFallback)
                            .await;
                    }
                    ExactLookup::Steam(observation) if is_pure_url => {
                        let items = self
                            .apply_local_overlay(&query, ctx.app_id, observation.as_deref(), false)
                            .await?;
                        let total = items.len() as u64;
                        let items = items.into_iter().skip(pagination.offset).collect();
                        return Ok(pagination.output(items, total, SearchExecutionMode::Hybrid));
                    }
                    ExactLookup::Steam(observation) => exact_observation = observation,
                    ExactLookup::Local | ExactLookup::Unavailable => {}
                }
            }
        }

        let search_text = query
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let sort_plan = resolve_sort_plan(query.sort.as_deref(), search_text.is_some());
        let required_tags = query
            .tag
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|tag| vec![tag.to_owned()])
            .unwrap_or_default();
        let request = SteamSearchRequest {
            query_type: sort_plan.query_type,
            file_type: match query.file_type {
                Some(WorkshopFileType::Collection) => MATCHING_FILE_TYPE_COLLECTIONS,
                _ => MATCHING_FILE_TYPE_ITEMS,
            },
            page: pagination.page,
            num_per_page: pagination.size,
            search_text_target: search_text.as_ref().and(search_target),
            search_text: search_text.clone(),
            required_tags,
            language: resolve_steam_language(
                query.language.as_deref(),
                accept_language,
                &search_cfg.default_language,
            ),
            days: sort_plan.days,
        };

        let force_native_paging =
            sort_plan.merge == MergeStrategy::SteamOrder && search_text.is_none();
        let steam = self
            .fetch_steam_results(&request, ctx, force_native_paging)
            .await;
        let exact_query = if !explicit_file_type {
            let mut q = query.clone();
            q.file_type = None;
            q
        } else {
            query.clone()
        };
        let exact = self
            .apply_local_overlay(
                &exact_query,
                ctx.app_id,
                exact_observation.as_deref(),
                false,
            )
            .await?
            .pop();

        match steam {
            Err(SteamSearchError::Unavailable(err)) => {
                tracing::warn!(error = %err, "Steam CM 混合检索失败，回退至本地索引");
                self.local_with_exact(
                    &query,
                    pagination,
                    exact.as_ref(),
                    sort_plan.merge,
                    SearchExecutionMode::LocalFallback,
                )
                .await
            }
            Err(err @ SteamSearchError::Visibility(_)) => Err(err.into()),
            Ok(SteamResults::Complete(pages)) if !pages.is_empty() => {
                let observations: Vec<_> = pages
                    .iter()
                    .flat_map(|page| page.raw.outcomes.iter())
                    .collect();
                let steam_items = self
                    .apply_local_overlay(&query, ctx.app_id, observations.iter().copied(), true)
                    .await?;
                let local = self.catalog_matches(&query).await?;
                let hits = merge_matches(
                    &local,
                    &steam_items,
                    &observations,
                    pagination.size as usize,
                    exact.as_ref(),
                    sort_plan.merge,
                    parsed.as_ref(),
                    search_target,
                );
                self.finish_hits(&query, pagination, hits, SearchExecutionMode::Hybrid)
                    .await
            }
            Ok(SteamResults::NativePage { page, total }) => {
                let mut items = self
                    .apply_local_overlay(&query, ctx.app_id, page.raw.outcomes.iter(), true)
                    .await?;
                let mut seen = HashSet::new();
                items.retain(|item| seen.insert(item.workshop_id));
                if let Some(exact_item) = exact {
                    if sort_plan.merge == MergeStrategy::Relevance {
                        items.retain(|item| item.workshop_id != exact_item.workshop_id);
                        if pagination.page == 1 {
                            items.insert(0, exact_item);
                            items.truncate(pagination.size as usize);
                        }
                    }
                }
                Ok(pagination.output(items, total, SearchExecutionMode::Hybrid))
            }
            Ok(SteamResults::NativePageEmpty { total }) => {
                Ok(pagination.output(Vec::new(), total, SearchExecutionMode::Hybrid))
            }
            Ok(_) => {
                self.local_with_exact(
                    &query,
                    pagination,
                    exact.as_ref(),
                    sort_plan.merge,
                    SearchExecutionMode::Hybrid,
                )
                .await
            }
        }
    }

    async fn lookup_exact(
        self: &Arc<Self>,
        wid: WorkshopId,
        ctx: SteamFetchCtx,
        sync: &SyncSection,
    ) -> anyhow::Result<ExactLookup> {
        if !sync.enable_read_through {
            return Ok(ExactLookup::Local);
        }
        let stored = self.db_call(move |r| r.get_item(wid)).await?;
        if let Some(meta) = stored {
            if meta.is_in_public_catalog()
                || meta.is_admin_override
                || meta.record.status == WorkshopItemStatus::PrivateOrUnavailable
                || meta.is_listed == Some(false)
            {
                return Ok(ExactLookup::Local);
            }
            let age_secs = (Utc::now() - meta.last_verified_at).num_seconds().max(0) as u64;
            if age_secs < sync.item_ttl_secs {
                if meta.record.status == WorkshopItemStatus::Removed {
                    return Ok(ExactLookup::Local);
                }
                if meta.record.status == WorkshopItemStatus::Active && meta.is_listed == Some(true)
                {
                    return Ok(ExactLookup::Steam(Some(Arc::new(WorkshopObservation {
                        outcome: WorkshopFetchOutcome::Found { item: meta.record },
                        listed: true,
                        consumer_appid: Some(ctx.app_id),
                    }))));
                }
            }
        }
        if matches!(
            self.repo.negative_status(wid, sync.item_ttl_secs),
            Some(WorkshopItemStatus::PrivateOrUnavailable | WorkshopItemStatus::Removed)
        ) {
            return Ok(ExactLookup::Local);
        }

        match self.fetch_exact_singleflight(wid, ctx).await {
            Ok(observation) => Ok(ExactLookup::Steam(observation)),
            Err(SteamSearchError::Unavailable(err)) => {
                tracing::warn!(workshop_id = %wid, error = %err, "精确 ID Steam CM 检索失败");
                Ok(ExactLookup::Unavailable)
            }
            Err(err @ SteamSearchError::Visibility(_)) => Err(err.into()),
        }
    }

    async fn fetch_exact_singleflight(
        self: &Arc<Self>,
        wid: WorkshopId,
        ctx: SteamFetchCtx,
    ) -> SharedExactResult {
        let wait_budget = remaining_timeout(ctx.deadline)
            .map_err(|err| SteamSearchError::Unavailable(err.to_string()))?;

        let mut rx = {
            let mut state = self.cache_state.lock().await;
            if let Some(existing) = state.inflight_exact.get(&wid).filter(|rx| !rx.is_closed()) {
                existing.resubscribe()
            } else {
                if state.inflight_exact.len() >= MAX_INFLIGHT_SEARCHES {
                    return Err(SteamSearchError::Unavailable(
                        "Steam CM 精确检索并发已达上限".to_owned(),
                    ));
                }
                let (tx, rx) = broadcast::channel(1);
                state.inflight_exact.insert(wid, rx.resubscribe());
                let svc = Arc::clone(self);
                let cache_state = Arc::clone(&self.cache_state);
                tokio::spawn(async move {
                    let mut guard = ExactInflightGuard {
                        state: cache_state,
                        key: Some(wid),
                    };
                    let fetch_result =
                        tokio::time::timeout(ctx.steam_timeout, svc.upstream.fetch_details(&[wid]))
                            .await
                            .map_err(|_| {
                                SteamSearchError::Unavailable("精确 ID Steam CM 检索超时".into())
                            })
                            .and_then(|res| {
                                res.map_err(|err| SteamSearchError::Unavailable(err.to_string()))
                            });

                    let final_result: SharedExactResult = match fetch_result {
                        Ok(mut observations) => {
                            observations.retain(|obs| obs.outcome.workshop_id() == wid);
                            match svc
                                .persist_confirmed_restrictions(&observations, ctx.app_id)
                                .await
                            {
                                Ok(()) => {
                                    if ctx.cache_steam_results {
                                        svc.warm_public_results(&observations, ctx.app_id, 0);
                                    }
                                    Ok(observations.into_iter().next().map(Arc::new))
                                }
                                Err(err) => Err(SteamSearchError::Visibility(err.to_string())),
                            }
                        }
                        Err(err) => Err(err),
                    };

                    {
                        let mut state = svc.cache_state.lock().await;
                        state.inflight_exact.remove(&wid);
                        guard.disarm();
                        let _ = tx.send(final_result);
                    }
                });
                rx
            }
        };

        tokio::time::timeout(wait_budget, rx.recv())
            .await
            .map_err(|_| SteamSearchError::Unavailable("精确 ID Steam CM 检索等待超时".into()))?
            .map_err(|err| {
                SteamSearchError::Unavailable(format!("Steam CM 精确检索并发通道异常: {err}"))
            })?
    }

    async fn run_local_only(
        &self,
        query: &SearchQuery,
        mode: SearchExecutionMode,
    ) -> anyhow::Result<HybridSearchOutput> {
        let query = query.clone();
        let resp = self.db_call(move |r| r.search(&query, true)).await?;
        Ok(HybridSearchOutput {
            response: normalize_response(resp),
            mode,
        })
    }

    async fn local_with_exact(
        &self,
        query: &SearchQuery,
        pagination: Pagination,
        exact: Option<&WorkshopItemRecord>,
        strategy: MergeStrategy,
        mode: SearchExecutionMode,
    ) -> anyhow::Result<HybridSearchOutput> {
        let Some(exact) = exact else {
            return self.run_local_only(query, mode).await;
        };
        let local = self.catalog_matches(query).await?;
        let mut hits: Vec<_> = std::iter::once(SearchHit::Steam(exact))
            .chain(
                local
                    .into_iter()
                    .filter(|matched| matched.id != exact.workshop_id)
                    .map(SearchHit::Catalog),
            )
            .collect();
        if let MergeStrategy::Deterministic(sort) = strategy {
            hits.sort_unstable_by_key(|hit| hit.sort_key(sort));
        }
        self.finish_hits(query, pagination, hits, mode).await
    }

    async fn catalog_matches(
        &self,
        query: &SearchQuery,
    ) -> anyhow::Result<Vec<RankedCatalogMatch>> {
        let query = query.clone();
        self.db_call(move |r| r.catalog_search_matches(&query))
            .await
    }

    async fn finish_hits(
        &self,
        query: &SearchQuery,
        pagination: Pagination,
        hits: Vec<SearchHit<'_>>,
        mode: SearchExecutionMode,
    ) -> anyhow::Result<HybridSearchOutput> {
        let mut total = hits.len() as u64;
        let page: Vec<_> = hits
            .into_iter()
            .skip(pagination.offset)
            .take(pagination.size as usize)
            .collect();
        let ids: Vec<_> = page
            .iter()
            .filter_map(|hit| match hit {
                SearchHit::Catalog(matched) => Some(matched.id),
                SearchHit::Steam(_) => None,
            })
            .collect();
        let mut local = HashMap::new();
        if !ids.is_empty() {
            let (stored, _) = self.db_call(move |r| r.get_batch(&ids)).await?;
            // A sync or admin update can change visibility/status between ranking and loading the page.
            local.extend(
                stored
                    .into_iter()
                    .filter(|meta| {
                        meta.is_in_public_catalog() && matches_filters(&meta.record, query, true)
                    })
                    .map(|meta| (meta.record.workshop_id, meta.record)),
            );
        }
        let mut dropped = 0u64;
        let mut items = Vec::with_capacity(page.len());
        for hit in page {
            match hit {
                SearchHit::Catalog(matched) => {
                    if let Some(item) = local.remove(&matched.id) {
                        items.push(item);
                    } else {
                        dropped += 1;
                    }
                }
                SearchHit::Steam(item) => items.push(item.clone()),
            }
        }
        total = total.saturating_sub(dropped);
        Ok(pagination.output(items, total, mode))
    }

    async fn fetch_steam_results(
        self: &Arc<Self>,
        request: &SteamSearchRequest,
        ctx: SteamFetchCtx,
        force_native_paging: bool,
    ) -> Result<SteamResults, SteamSearchError> {
        let mut scan_request = request.clone();
        scan_request.page = 1;
        let first = self.fetch_steam_page_cached(&scan_request, ctx).await?;
        let total = steam_total(&first.raw);
        // For very broad searches or Steam-only browse sorts (trend/toprated), keep native Steam paging.
        if total > MAX_STEAM_MERGE_ITEMS || (force_native_paging && total > 0) {
            return self.resolve_native_page(request, first, total, ctx).await;
        }

        if total.div_ceil(u64::from(request.num_per_page)) > MAX_STEAM_MERGE_PAGES {
            scan_request.num_per_page = STEAM_SCAN_PAGE_SIZE;
        }
        let num_pages = total.div_ceil(u64::from(scan_request.num_per_page)) as u32;
        let pages = join_all((1..=num_pages).map(|page| {
            let mut req = scan_request.clone();
            req.page = page;
            let first = &first;
            async move {
                if page == 1 && req.num_per_page == request.num_per_page {
                    return Ok(first.clone());
                }
                self.fetch_steam_page_cached(&req, ctx).await
            }
        }))
        .await;
        // Wait for all fetched restrictions to be saved before allowing a local fallback.
        if let Some(err) = pages.iter().find_map(|page| match page {
            Err(err @ SteamSearchError::Visibility(_)) => Some(err),
            _ => None,
        }) {
            return Err(err.clone());
        }
        let mut pages = pages.into_iter().collect::<Result<Vec<_>, _>>()?;
        if scan_request.num_per_page != request.num_per_page {
            if let Some(repaged_first) = pages.first() {
                let updated_total = steam_total(&repaged_first.raw);
                if updated_total == 0 {
                    return Err(SteamSearchError::Unavailable(
                        "Steam CM 检索结果在重新分页时发生变化".to_owned(),
                    ));
                }
                if updated_total > MAX_STEAM_MERGE_ITEMS {
                    return self
                        .resolve_native_page(request, first, updated_total, ctx)
                        .await;
                }
                let updated_num_pages =
                    updated_total.div_ceil(u64::from(scan_request.num_per_page)) as u32;
                if updated_num_pages > num_pages {
                    let extra = join_all(((num_pages + 1)..=updated_num_pages).map(|page| {
                        let mut req = scan_request.clone();
                        req.page = page;
                        async move { self.fetch_steam_page_cached(&req, ctx).await }
                    }))
                    .await;
                    if let Some(err) = extra.iter().find_map(|page| match page {
                        Err(err @ SteamSearchError::Visibility(_)) => Some(err),
                        _ => None,
                    }) {
                        return Err(err.clone());
                    }
                    pages.extend(extra.into_iter().collect::<Result<Vec<_>, _>>()?);
                }
            }
        }
        if pages.iter().all(|page| page.raw.outcomes.is_empty()) {
            pages.clear();
        }
        Ok(SteamResults::Complete(pages))
    }

    async fn resolve_native_page(
        self: &Arc<Self>,
        request: &SteamSearchRequest,
        first: Arc<SharedSteamPage>,
        total: u64,
        ctx: SteamFetchCtx,
    ) -> Result<SteamResults, SteamSearchError> {
        if request.page == 1 {
            return Ok(SteamResults::NativePage { page: first, total });
        }
        let offset = u64::from(request.page - 1).saturating_mul(u64::from(request.num_per_page));
        if offset >= total {
            return Ok(SteamResults::NativePageEmpty { total });
        }
        let page = self.fetch_steam_page_cached(request, ctx).await?;
        Ok(SteamResults::NativePage { page, total })
    }

    async fn apply_local_overlay<'a>(
        &self,
        query: &SearchQuery,
        app_id: u32,
        observations: impl IntoIterator<Item = &'a WorkshopObservation>,
        steam_enforced_tags: bool,
    ) -> anyhow::Result<Vec<WorkshopItemRecord>> {
        let obs_list: Vec<&WorkshopObservation> = observations.into_iter().collect();
        if obs_list.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<WorkshopId> = obs_list
            .iter()
            .map(|obs| obs.outcome.workshop_id())
            .collect();
        let meta_map = self.db_call(move |r| r.get_overlay_batch(&ids)).await?;

        let mut out = Vec::with_capacity(obs_list.len());
        for obs in obs_list {
            let wid = obs.outcome.workshop_id();
            if let Some(meta) = meta_map.get(&wid) {
                if meta.is_admin_override {
                    if !meta.is_in_public_catalog() {
                        continue;
                    }
                    if let Some(ref record) = meta.override_record {
                        if query.status.is_none()
                            && matches!(
                                record.status,
                                WorkshopItemStatus::PrivateOrUnavailable
                                    | WorkshopItemStatus::Removed
                            )
                        {
                            continue;
                        }
                        if matches_filters(record, query, true) {
                            out.push(record.clone());
                        }
                    }
                    continue;
                }
            }

            if !is_public_active_for_app(obs, app_id) {
                continue;
            }
            let WorkshopFetchOutcome::Found { item } = &obs.outcome else {
                continue;
            };
            if !matches_filters(item, query, !steam_enforced_tags) {
                continue;
            }

            let mut item = item.clone();
            if let Some(meta) = meta_map.get(&wid) {
                if (matches!(
                    meta.status,
                    WorkshopItemStatus::PrivateOrUnavailable | WorkshopItemStatus::Removed
                ) || meta.is_listed == Some(false))
                    && meta.last_verified_at >= item.fetched_at
                {
                    continue;
                }
                if item.creator_name.is_none() {
                    item.creator_name = meta.creator_name.clone();
                }
            }
            if let Some((neg_status, neg_at)) = self.repo.negative_observed_at(wid) {
                if matches!(
                    neg_status,
                    WorkshopItemStatus::PrivateOrUnavailable | WorkshopItemStatus::Removed
                ) && neg_at >= item.fetched_at
                {
                    continue;
                }
            }
            out.push(item);
        }
        Ok(out)
    }

    async fn fetch_steam_page_cached(
        self: &Arc<Self>,
        request: &SteamSearchRequest,
        ctx: SteamFetchCtx,
    ) -> SharedPageResult {
        let wait_budget = remaining_timeout(ctx.deadline)
            .map_err(|err| SteamSearchError::Unavailable(err.to_string()))?;

        let mut rx = {
            let mut state = self.cache_state.lock().await;
            if !ctx.cache_ttl.is_zero() {
                match state.cache.get(request) {
                    Some(entry) if entry.cached_at.elapsed() < ctx.cache_ttl => {
                        let page = entry.page.clone();
                        drop(state);
                        self.save_page_restrictions(&page, ctx.app_id).await?;
                        return Ok(page);
                    }
                    Some(_) => {
                        state.cache.remove(request);
                    }
                    None => {}
                }
            }

            if let Some(existing) = state.inflight.get(request).filter(|rx| !rx.is_closed()) {
                existing.resubscribe()
            } else {
                if state.inflight.len() >= MAX_INFLIGHT_SEARCHES {
                    return Err(SteamSearchError::Unavailable(
                        "Steam CM 实时搜索并发已达上限".to_owned(),
                    ));
                }
                let (tx, rx) = broadcast::channel(1);
                state.inflight.insert(request.clone(), rx.resubscribe());
                let svc = Arc::clone(self);
                let cache_state = Arc::clone(&self.cache_state);
                let req = request.clone();
                tokio::spawn(async move {
                    let mut guard = PageInflightGuard {
                        state: cache_state,
                        key: Some(req.clone()),
                    };
                    let result =
                        tokio::time::timeout(ctx.steam_timeout, svc.upstream.search_files(&req))
                            .await
                            .map_err(|_| {
                                SteamSearchError::Unavailable("Steam CM 实时搜索超时".to_owned())
                            })
                            .and_then(|res| {
                                res.map_err(|err| SteamSearchError::Unavailable(err.to_string()))
                            })
                            .map(|raw| {
                                Arc::new(SharedSteamPage {
                                    raw,
                                    restrictions_saved: OnceCell::new(),
                                })
                            });

                    {
                        let mut state = svc.cache_state.lock().await;
                        if let Ok(page) = &result {
                            if !ctx.cache_ttl.is_zero() {
                                insert_cached_page_locked(
                                    &mut state.cache,
                                    req.clone(),
                                    page.clone(),
                                    ctx.cache_ttl,
                                );
                            }
                        }
                        state.inflight.remove(&req);
                        guard.disarm();
                        let _ = tx.send(result.clone());
                    }
                    // Save restrictions even if the original HTTP request has gone away.
                    if let Ok(page) = result {
                        match svc.save_page_restrictions(&page, ctx.app_id).await {
                            Ok(()) if ctx.cache_steam_results => {
                                svc.warm_public_results(
                                    &page.raw.outcomes,
                                    ctx.app_id,
                                    req.language,
                                );
                            }
                            Ok(()) => {}
                            Err(err) => tracing::error!(error = %err, "保存搜索结果的可见性失败"),
                        }
                    }
                });
                rx
            }
        };

        let page = tokio::time::timeout(wait_budget, rx.recv())
            .await
            .map_err(|_| SteamSearchError::Unavailable("Steam CM 实时搜索等待超时".to_owned()))?
            .map_err(|err| {
                SteamSearchError::Unavailable(format!("Steam CM 搜索并发通道异常: {err}"))
            })??;
        self.save_page_restrictions(&page, ctx.app_id).await?;
        Ok(page)
    }

    async fn save_page_restrictions(
        &self,
        page: &SharedSteamPage,
        app_id: u32,
    ) -> Result<(), SteamSearchError> {
        page.restrictions_saved
            .get_or_try_init(|| self.persist_confirmed_restrictions(&page.raw.outcomes, app_id))
            .await
            .map_err(|err| SteamSearchError::Visibility(err.to_string()))?;
        Ok(())
    }

    async fn persist_confirmed_restrictions(
        &self,
        observations: &[WorkshopObservation],
        app_id: u32,
    ) -> anyhow::Result<()> {
        let restricted: Vec<_> = observations
            .iter()
            .filter(|obs| {
                obs.consumer_appid.is_none_or(|id| id == app_id)
                    && match &obs.outcome {
                        WorkshopFetchOutcome::Found { item } => {
                            item.status == WorkshopItemStatus::PrivateOrUnavailable
                                || (item.status == WorkshopItemStatus::Active && !obs.listed)
                        }
                        WorkshopFetchOutcome::PrivateOrUnavailable { .. }
                        | WorkshopFetchOutcome::Removed { .. } => true,
                    }
            })
            .cloned()
            .collect();
        if !restricted.is_empty() {
            self.db_call(move |repo| repo.persist_confirmed_restrictions(&restricted))
                .await?;
        }
        Ok(())
    }

    fn ensure_warm_worker(&self) {
        let rx = self.warm_rx.lock().unwrap().take();
        let Some(mut rx) = rx else {
            return;
        };
        let repo = Arc::clone(&self.repo);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                while let Some(batch) = rx.recv().await {
                    let repo_clone = Arc::clone(&repo);
                    let res = tokio::task::spawn_blocking(move || {
                        repo_clone
                            .apply_upstream_outcomes_with_policy(&batch.observations, batch.policy)
                    })
                    .await;
                    match res {
                        Ok(Ok(_)) => {}
                        Ok(Err(err)) => {
                            tracing::warn!(error = %err, "Steam 检索暖缓存写入失败");
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "Steam 检索暖缓存任务异常退出");
                        }
                    }
                }
            });
        }
    }

    fn warm_public_results(
        &self,
        observations: &[WorkshopObservation],
        app_id: u32,
        language: i32,
    ) {
        let observations: Vec<_> = observations
            .iter()
            .filter(|obs| is_public_active_for_app(obs, app_id))
            .cloned()
            .collect();
        if observations.is_empty() {
            return;
        }
        self.ensure_warm_worker();
        let policy = if language == 0 {
            CatalogIndexPolicy::RefreshUnindexedCache
        } else {
            CatalogIndexPolicy::InsertOnlyCache
        };
        if self
            .warm_tx
            .try_send(WarmBatch {
                observations,
                policy,
            })
            .is_err()
        {
            tracing::debug!("Steam 检索暖缓存队列已满，跳过本批预热写入");
        }
    }
}

fn insert_cached_page_locked(
    cache: &mut HashMap<SteamSearchRequest, CachedSteamPage>,
    request: SteamSearchRequest,
    page: Arc<SharedSteamPage>,
    cache_ttl: Duration,
) {
    cache.retain(|_, entry| entry.cached_at.elapsed() < cache_ttl);
    if !cache.contains_key(&request) && cache.len() >= MAX_SEARCH_CACHE_ENTRIES {
        if let Some(oldest_key) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.cached_at)
            .map(|(k, _)| k.clone())
        {
            cache.remove(&oldest_key);
        }
    }
    cache.insert(
        request,
        CachedSteamPage {
            page,
            cached_at: Instant::now(),
        },
    );
}

fn remaining_timeout(deadline: Instant) -> anyhow::Result<Duration> {
    let now = Instant::now();
    if now >= deadline {
        anyhow::bail!("Steam CM 请求超出检索超时预算");
    }
    Ok(deadline - now)
}

fn steam_total(page: &WorkshopQueryPage) -> u64 {
    page.total.max(page.outcomes.len() as u64)
}

#[allow(clippy::too_many_arguments)]
fn merge_matches<'a>(
    local: &[RankedCatalogMatch],
    steam: &'a [WorkshopItemRecord],
    observations: &[&WorkshopObservation],
    first_page_len: usize,
    exact: Option<&'a WorkshopItemRecord>,
    strategy: MergeStrategy,
    parsed: Option<&ParsedQuery>,
    target: Option<SteamSearchTarget>,
) -> Vec<SearchHit<'a>> {
    let mut steam_by_id: HashMap<WorkshopId, &'a WorkshopItemRecord> =
        HashMap::with_capacity(steam.len());
    let steam: Vec<_> = steam
        .iter()
        .filter(|item| steam_by_id.insert(item.workshop_id, *item).is_none())
        .collect();
    // A cached Steam hit can be suppressed by a newer local tombstone. Keep that ID out of the local tail as well.
    let suppressed: HashSet<_> = observations
        .iter()
        .map(|obs| obs.outcome.workshop_id())
        .filter(|id| !steam_by_id.contains_key(id))
        .collect();

    let local = exact.into_iter().map(SearchHit::Steam).chain(
        local
            .iter()
            .filter(|matched| {
                !suppressed.contains(&matched.id)
                    && exact.is_none_or(|item| item.workshop_id != matched.id)
            })
            .copied()
            .map(SearchHit::Catalog),
    );

    let mut hits = if strategy == MergeStrategy::Relevance {
        let first_page_ids: HashSet<_> = steam
            .iter()
            .take(first_page_len)
            .map(|item| item.workshop_id)
            .collect();
        let steam_best_tier = parsed.and_then(|parsed| {
            steam
                .iter()
                .take(5)
                .filter_map(|item| score_for_target(parsed, target, item))
                .map(|rank| rank.tier)
                .min()
        });
        let (promoted_raw, tail): (Vec<_>, Vec<_>) = local.partition(|hit| {
            !first_page_ids.contains(&hit.id()) && hit.should_promote(steam_best_tier)
        });
        let mut promoted: Vec<SearchHit<'a>> = promoted_raw
            .into_iter()
            .map(|hit| match hit {
                SearchHit::Catalog(matched) => steam_by_id
                    .get(&matched.id)
                    .copied()
                    .map(SearchHit::Steam)
                    .unwrap_or(SearchHit::Catalog(matched)),
                steam_hit => steam_hit,
            })
            .collect();
        let promoted_ids: HashSet<_> = promoted.iter().map(|hit| hit.id()).collect();
        promoted.extend(
            steam
                .into_iter()
                .filter(|item| !promoted_ids.contains(&item.workshop_id))
                .map(SearchHit::Steam),
        );
        promoted.extend(
            tail.into_iter()
                .filter(|hit| !steam_by_id.contains_key(&hit.id())),
        );
        promoted
    } else {
        steam
            .into_iter()
            .map(SearchHit::Steam)
            .chain(local.filter(|hit| !steam_by_id.contains_key(&hit.id())))
            .collect()
    };
    if let MergeStrategy::Deterministic(sort) = strategy {
        hits.sort_unstable_by_key(|hit| hit.sort_key(sort));
    }
    hits
}

fn normalize_response(mut resp: SearchResponse) -> SearchResponse {
    resp.items = resp.items.into_iter().map(with_catalog_source).collect();
    resp
}

fn should_use_local_only(query: &SearchQuery) -> bool {
    if query
        .status
        .is_some_and(|status| status != WorkshopItemStatus::Active)
    {
        return true;
    }
    matches!(
        query
            .sort
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("updated_asc" | "title")
    )
}

fn resolve_sort_plan(sort: Option<&str>, has_query: bool) -> SteamSortPlan {
    match sort.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("trend") => SteamSortPlan {
            query_type: QUERY_TYPE_RANKED_BY_TREND,
            days: Some(7),
            merge: MergeStrategy::SteamOrder,
        },
        Some("toprated") => SteamSortPlan {
            query_type: QUERY_TYPE_RANKED_BY_VOTE,
            days: None,
            merge: MergeStrategy::SteamOrder,
        },
        Some("subscriptions" | "totalprofiles" | "popular") => SteamSortPlan {
            query_type: QUERY_TYPE_RANKED_BY_TOTAL_UNIQUE_SUBSCRIPTIONS,
            days: None,
            merge: MergeStrategy::Deterministic(DeterministicSort::Subscriptions),
        },
        Some("updated" | "lastupdated") => SteamSortPlan {
            query_type: QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE,
            days: None,
            merge: MergeStrategy::Deterministic(DeterministicSort::Updated),
        },
        Some("newest" | "mostrecent") => SteamSortPlan {
            query_type: QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
            days: None,
            merge: MergeStrategy::Deterministic(DeterministicSort::Newest),
        },
        _ if has_query => SteamSortPlan {
            query_type: QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            days: None,
            merge: MergeStrategy::Relevance,
        },
        _ => SteamSortPlan {
            query_type: QUERY_TYPE_RANKED_BY_TOTAL_UNIQUE_SUBSCRIPTIONS,
            days: None,
            merge: MergeStrategy::Deterministic(DeterministicSort::BrowseRelevance),
        },
    }
}

fn parsed_steam_target(raw: Option<&str>) -> Option<SteamSearchTarget> {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("title" | "1") => Some(SteamSearchTarget::Title),
        Some("description" | "desc" | "2") => Some(SteamSearchTarget::Description),
        _ => None,
    }
}

fn is_public_active_for_app(obs: &WorkshopObservation, app_id: u32) -> bool {
    obs.listed
        && obs.consumer_appid.is_none_or(|cid| cid == app_id)
        && matches!(
            &obs.outcome,
            WorkshopFetchOutcome::Found { item } if item.status == WorkshopItemStatus::Active
        )
}

fn matches_filters(item: &WorkshopItemRecord, query: &SearchQuery, check_tag: bool) -> bool {
    if let Some(status) = query.status {
        if item.status != status {
            return false;
        }
    }
    if let Some(ft) = query.file_type {
        if item.file_type != ft {
            return false;
        }
    }
    if check_tag {
        if let Some(tag) = query
            .tag
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if !item.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                return false;
            }
        }
    }
    true
}

fn score_for_target(
    parsed: &ParsedQuery,
    target: Option<SteamSearchTarget>,
    item: &WorkshopItemRecord,
) -> Option<MatchRank> {
    match target {
        Some(SteamSearchTarget::Title) => search::score_title_only_candidate(parsed, &item.title),
        Some(SteamSearchTarget::Description) => item
            .description
            .as_deref()
            .and_then(|d| search::score_desc_candidate(parsed, d)),
        _ => search::score_primary_candidate(
            parsed,
            item.workshop_id.as_u64(),
            &item.title,
            item.creator_name.as_deref(),
        ),
    }
}

pub fn resolve_steam_language(
    query_lang: Option<&str>,
    accept_language: Option<&str>,
    default_lang: &str,
) -> i32 {
    if let Some(code) = query_lang.and_then(parse_steam_language_tag) {
        return code;
    }
    if let Some(code) = accept_language.and_then(parse_accept_language) {
        return code;
    }
    parse_steam_language_tag(default_lang).unwrap_or(0)
}

pub fn parse_accept_language(header: &str) -> Option<i32> {
    let mut candidates: Vec<(u32, usize, i32)> = Vec::new();
    for (idx, part) in header.split(',').enumerate() {
        let mut segments = part.split(';');
        let tag = segments.next().map(str::trim).unwrap_or("");
        if tag.is_empty() || tag == "*" {
            continue;
        }
        let mut q_milli: u32 = 1000;
        for param in segments {
            let trimmed = param.trim();
            if let Some(val) = trimmed
                .strip_prefix("q=")
                .or_else(|| trimmed.strip_prefix("Q="))
            {
                if let Ok(parsed) = val.trim().parse::<f32>() {
                    if parsed.is_finite() {
                        q_milli = (parsed.clamp(0.0, 1.0) * 1000.0).round() as u32;
                    }
                }
            }
        }
        if q_milli == 0 {
            continue;
        }
        if let Some(lang) = parse_steam_language_tag(tag) {
            candidates.push((q_milli, idx, lang));
        }
    }
    candidates.sort_by_key(|&(q, idx, _)| (Reverse(q), idx));
    candidates.first().map(|&(_, _, lang)| lang)
}

pub fn parse_steam_language_tag(raw: &str) -> Option<i32> {
    let normalized = raw.trim().to_ascii_lowercase().replace('_', "-");
    if normalized.is_empty() {
        return None;
    }
    if let Some(exact) = match normalized.as_str() {
        "en" | "english" => Some(0),
        "de" | "german" => Some(1),
        "fr" | "french" => Some(2),
        "it" | "italian" => Some(3),
        "ko" | "kr" | "koreana" | "korean" => Some(4),
        "es-419" | "es-mx" | "es-ar" | "es-cl" | "es-co" | "es-pe" | "latam" => Some(27),
        "es" | "spanish" => Some(5),
        "zh-tw" | "zh-hk" | "zh-mo" | "zh-hant" | "tchinese" => Some(7),
        "zh" | "zh-cn" | "zh-sg" | "zh-hans" | "schinese" => Some(6),
        "ru" | "russian" => Some(8),
        "th" | "thai" => Some(9),
        "ja" | "jp" | "japanese" => Some(10),
        "pt-br" | "brazilian" => Some(22),
        "pt" | "portuguese" => Some(11),
        "pl" | "polish" => Some(12),
        "da" | "danish" => Some(13),
        "nl" | "dutch" => Some(14),
        "fi" | "finnish" => Some(15),
        "no" | "nb" | "nn" | "norwegian" => Some(16),
        "sv" | "swedish" => Some(17),
        "hu" | "hungarian" => Some(18),
        "cs" | "czech" => Some(19),
        "ro" | "romanian" => Some(20),
        "tr" | "turkish" => Some(21),
        "bg" | "bulgarian" => Some(23),
        "el" | "greek" => Some(24),
        "uk" | "ukrainian" => Some(25),
        "vi" | "vietnamese" => Some(28),
        "id" | "indonesian" => Some(30),
        _ => None,
    } {
        return Some(exact);
    }

    let mut parts = normalized.split('-');
    let base = parts.next().unwrap_or("");
    if base == "zh" {
        let subtags: Vec<&str> = parts.collect();
        if subtags.contains(&"hans") {
            return Some(6);
        }
        if subtags
            .iter()
            .any(|s| matches!(*s, "hant" | "tw" | "hk" | "mo"))
        {
            return Some(7);
        }
        return Some(6);
    }

    match base {
        "en" => Some(0),
        "de" => Some(1),
        "fr" => Some(2),
        "it" => Some(3),
        "ko" => Some(4),
        "es" => Some(5),
        "ru" => Some(8),
        "th" => Some(9),
        "ja" => Some(10),
        "pt" => Some(11),
        "pl" => Some(12),
        "da" => Some(13),
        "nl" => Some(14),
        "fi" => Some(15),
        "no" | "nb" | "nn" => Some(16),
        "sv" => Some(17),
        "hu" => Some(18),
        "cs" => Some(19),
        "ro" => Some(20),
        "tr" => Some(21),
        "bg" => Some(23),
        "el" => Some(24),
        "uk" => Some(25),
        "vi" => Some(28),
        "id" => Some(30),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn resolves_steam_language_from_query_accept_header_and_default() {
        assert_eq!(parse_steam_language_tag("en"), Some(0));
        assert_eq!(parse_steam_language_tag("en-US"), Some(0));
        assert_eq!(parse_steam_language_tag("en-HK"), Some(0));
        assert_eq!(parse_steam_language_tag("en-TW"), Some(0));
        assert_eq!(parse_steam_language_tag("pt-MO"), Some(11));
        assert_eq!(parse_steam_language_tag("zh"), Some(6));
        assert_eq!(parse_steam_language_tag("zh-CN"), Some(6));
        assert_eq!(parse_steam_language_tag("zh-Hans-HK"), Some(6));
        assert_eq!(parse_steam_language_tag("schinese"), Some(6));
        assert_eq!(parse_steam_language_tag("zh-TW"), Some(7));
        assert_eq!(parse_steam_language_tag("zh-Hant"), Some(7));
        assert_eq!(parse_steam_language_tag("zh-Hant-US"), Some(7));
        assert_eq!(parse_steam_language_tag("zh_HK"), Some(7));
        assert_eq!(parse_steam_language_tag("tchinese"), Some(7));
        assert_eq!(parse_steam_language_tag("ja"), Some(10));
        assert_eq!(parse_steam_language_tag("ko"), Some(4));
        assert_eq!(parse_steam_language_tag("ru"), Some(8));
        assert_eq!(parse_steam_language_tag("pt-BR"), Some(22));
        assert_eq!(parse_steam_language_tag("es-419"), Some(27));

        assert_eq!(
            resolve_steam_language(Some("zh-CN"), Some("en-US,en;q=0.9"), "english"),
            6
        );
        assert_eq!(
            resolve_steam_language(None, Some("en-US;q=0.5, zh-TW;q=0.9, ja;q=0.8"), "english"),
            7
        );
        assert_eq!(
            resolve_steam_language(None, Some("xx-INVALID"), "schinese"),
            6
        );
        assert_eq!(resolve_steam_language(None, None, "unknown"), 0);
    }

    #[test]
    fn maps_search_targets_and_sort_plans() {
        assert_eq!(parsed_steam_target(None), None);
        assert_eq!(parsed_steam_target(Some("all")), None);
        assert_eq!(
            parsed_steam_target(Some("title")),
            Some(SteamSearchTarget::Title)
        );
        assert_eq!(
            parsed_steam_target(Some("1")),
            Some(SteamSearchTarget::Title)
        );
        assert_eq!(
            parsed_steam_target(Some("description")),
            Some(SteamSearchTarget::Description)
        );
        assert_eq!(
            parsed_steam_target(Some("2")),
            Some(SteamSearchTarget::Description)
        );

        let text_rel = resolve_sort_plan(None, true);
        assert_eq!(text_rel.query_type, QUERY_TYPE_RANKED_BY_TEXT_SEARCH);
        assert_eq!(text_rel.merge, MergeStrategy::Relevance);

        let browse_rel = resolve_sort_plan(None, false);
        assert_eq!(
            browse_rel.query_type,
            QUERY_TYPE_RANKED_BY_TOTAL_UNIQUE_SUBSCRIPTIONS
        );
        assert_eq!(
            browse_rel.merge,
            MergeStrategy::Deterministic(DeterministicSort::BrowseRelevance)
        );

        let trend = resolve_sort_plan(Some("trend"), true);
        assert_eq!(trend.query_type, QUERY_TYPE_RANKED_BY_TREND);
        assert_eq!(trend.days, Some(7));
        assert_eq!(trend.merge, MergeStrategy::SteamOrder);

        let toprated = resolve_sort_plan(Some("toprated"), false);
        assert_eq!(toprated.query_type, QUERY_TYPE_RANKED_BY_VOTE);
        assert_eq!(toprated.merge, MergeStrategy::SteamOrder);

        let updated = resolve_sort_plan(Some("updated"), true);
        assert_eq!(updated.query_type, QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE);
        assert_eq!(
            updated.merge,
            MergeStrategy::Deterministic(DeterministicSort::Updated)
        );

        let newest = resolve_sort_plan(Some("newest"), false);
        assert_eq!(newest.query_type, QUERY_TYPE_RANKED_BY_PUBLICATION_DATE);
        assert_eq!(
            newest.merge,
            MergeStrategy::Deterministic(DeterministicSort::Newest)
        );
    }

    fn unit_sample_item(id: u64, title: &str) -> WorkshopItemRecord {
        WorkshopItemRecord {
            workshop_id: WorkshopId(id),
            file_type: WorkshopFileType::Item,
            status: WorkshopItemStatus::Active,
            title: title.to_string(),
            description: Some(format!("Description for {title}")),
            creator_steam_id: None,
            creator_name: None,
            preview_url: None,
            previews: Vec::new(),
            file_url: Some(format!("https://cdn.steamusercontent.com/ugc/{id}.vpk")),
            file_size: Some(1024),
            time_created: Some(1_700_000_000),
            time_updated: Some(1_700_000_100),
            tags: vec!["Campaigns".to_string()],
            dependencies: Vec::new(),
            collection_children: Vec::new(),
            subscriptions: Some(100),
            favorites: Some(10),
            source: MetadataSource::SteamKit,
            fetched_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn cached_steam_page_hit_does_not_retrigger_cache_warming() {
        use crate::config::CatalogConfig;
        use crate::upstream::MockWorkshopUpstream;

        let repo = Arc::new(ServerRepo::open_in_memory().unwrap());
        let config_store = Arc::new(CatalogConfigStore::new(None, CatalogConfig::default()));
        let upstream = Arc::new(MockWorkshopUpstream::new());
        upstream.insert_search_page(
            SteamSearchRequest {
                query_type: QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
                file_type: MATCHING_FILE_TYPE_ITEMS,
                page: 1,
                num_per_page: 30,
                search_text: Some("WarmOnce".to_string()),
                required_tags: Vec::new(),
                language: 0,
                days: None,
                search_text_target: None,
            },
            WorkshopQueryPage {
                total: 1,
                next_cursor: None,
                outcomes: vec![WorkshopObservation::from(WorkshopFetchOutcome::Found {
                    item: unit_sample_item(7901, "WarmOnce Map"),
                })],
            },
        );

        let svc = Arc::new(HybridSearchService::new(
            config_store,
            repo.clone(),
            upstream.clone(),
        ));
        let query = SearchQuery {
            query: Some("WarmOnce".to_string()),
            tag: None,
            status: None,
            sort: None,
            page: 1,
            page_size: 30,
            language: Some("en".to_string()),
            search_target: None,
            file_type: None,
        };

        let first = svc.search(&query, None).await.unwrap();
        assert_eq!(first.response.total, 1);

        tokio::time::timeout(Duration::from_secs(2), async {
            while repo.get_item(WorkshopId(7901)).unwrap().is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        repo.with_transaction(|tx| {
            tx.execute("DELETE FROM workshop_items WHERE workshop_id = 7901", [])?;
            Ok(())
        })
        .unwrap();

        let second = svc.search(&query, None).await.unwrap();
        assert_eq!(second.response.total, 1);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(repo.get_item(WorkshopId(7901)).unwrap().is_none());
        assert_eq!(upstream.recorded_searches().len(), 1);
    }

    #[test]
    fn insert_only_cache_skips_existing_rows_atomically() {
        let repo = ServerRepo::open_in_memory().unwrap();
        let mut canonical = unit_sample_item(7902, "English Canonical");
        canonical.description = Some("Full English Description".to_string());
        repo.upsert_item(&canonical, false, true).unwrap();

        let mut localized = unit_sample_item(7902, "中文译名");
        localized.fetched_at = Utc::now() + chrono::Duration::seconds(60);
        let fresh = unit_sample_item(7903, "新入库条目");

        let summary = repo
            .apply_upstream_outcomes_with_policy(
                &[
                    WorkshopObservation::from(WorkshopFetchOutcome::Found { item: localized }),
                    WorkshopObservation::from(WorkshopFetchOutcome::Found { item: fresh }),
                ],
                CatalogIndexPolicy::InsertOnlyCache,
            )
            .unwrap();

        assert_eq!(summary.skipped_stale, 1);
        assert_eq!(summary.active_upserted, 1);
        assert_eq!(
            repo.get_item(WorkshopId(7902))
                .unwrap()
                .unwrap()
                .record
                .title,
            "English Canonical"
        );
        assert_eq!(
            repo.get_item(WorkshopId(7903))
                .unwrap()
                .unwrap()
                .record
                .title,
            "新入库条目"
        );
    }
}
