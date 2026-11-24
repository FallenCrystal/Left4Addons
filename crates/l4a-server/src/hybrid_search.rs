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
use futures_util::future::join_all;
use l4a_protocol::{
    MetadataSource, SearchQuery, SearchResponse, WorkshopFetchOutcome, WorkshopFileType,
    WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Mutex, OnceCell};

const MAX_SEARCH_CACHE_ENTRIES: usize = 512;
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
}

enum ExactLookup {
    Local,
    Steam(Option<Box<WorkshopObservation>>),
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

    fn sort_key(self, sort: DeterministicSort) -> (Reverse<i128>, u64) {
        let sort_value = match (self, sort) {
            (Self::Catalog(matched), DeterministicSort::Subscriptions) => {
                i128::from(matched.sort_value as u64)
            }
            (Self::Catalog(matched), _) => i128::from(matched.sort_value),
            (Self::Steam(item), DeterministicSort::Subscriptions) => {
                i128::from(item.subscriptions.unwrap_or(0))
            }
            (Self::Steam(item), DeterministicSort::Updated) => {
                i128::from(item.time_updated.unwrap_or(0))
            }
            (Self::Steam(item), DeterministicSort::Newest) => {
                i128::from(item.time_created.unwrap_or(0))
            }
        };
        let id = self.id().as_u64();
        let tie_breaker = match sort {
            DeterministicSort::Subscriptions => id,
            DeterministicSort::Updated | DeterministicSort::Newest => u64::MAX - id,
        };
        (Reverse(sort_value), tie_breaker)
    }
}

type SharedPageResult = Result<Arc<SharedSteamPage>, SteamSearchError>;

pub struct HybridSearchService {
    config_store: Arc<CatalogConfigStore>,
    repo: Arc<ServerRepo>,
    upstream: Arc<dyn WorkshopUpstream>,
    cache: Mutex<HashMap<SteamSearchRequest, CachedSteamPage>>,
    inflight: Mutex<HashMap<SteamSearchRequest, broadcast::Receiver<SharedPageResult>>>,
}

impl HybridSearchService {
    pub fn new(
        config_store: Arc<CatalogConfigStore>,
        repo: Arc<ServerRepo>,
        upstream: Arc<dyn WorkshopUpstream>,
    ) -> Self {
        Self {
            config_store,
            repo,
            upstream,
            cache: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
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
        let mut query = query.clone();
        if !is_pure_url && query.file_type.is_none() {
            query.file_type = Some(WorkshopFileType::Item);
        }

        if !search_cfg.enable_hybrid_search || should_use_local_only(&query) {
            return self
                .run_local_only(&query, SearchExecutionMode::LocalOnly)
                .await;
        }

        let ctx = SteamFetchCtx {
            app_id: cfg.steam.app_id,
            cache_ttl: Duration::from_secs(search_cfg.cache_ttl_secs),
            deadline: Instant::now() + Duration::from_millis(search_cfg.steam_timeout_ms),
            cache_steam_results: search_cfg.cache_steam_results,
        };

        let mut exact_observation = None;
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

        let search_text = query
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let sort_plan = resolve_sort_plan(query.sort.as_deref(), search_text.is_some());
        let search_target = parsed_steam_target(query.search_target.as_deref());
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
            search_text,
            required_tags,
            language: resolve_steam_language(
                query.language.as_deref(),
                accept_language,
                &search_cfg.default_language,
            ),
            days: sort_plan.days,
        };

        let steam = self.fetch_steam_results(&request, ctx).await;
        let exact = self
            .apply_local_overlay(&query, ctx.app_id, exact_observation.as_deref(), false)
            .await?
            .pop();
        match steam {
            Err(SteamSearchError::Unavailable(err)) => {
                tracing::warn!(error = %err, "Steam CM 混合检索失败，回退至本地索引");
                self.local_with_exact(
                    &query,
                    pagination,
                    exact.as_ref(),
                    SearchExecutionMode::LocalFallback,
                )
                .await
            }
            Err(err @ SteamSearchError::Visibility(_)) => Err(err.into()),
            Ok(SteamResults::Complete(pages)) if !pages.is_empty() => {
                let first_page_len = pages[0].raw.outcomes.len().min(pagination.size as usize);
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
                    first_page_len,
                    exact.as_ref(),
                    sort_plan.merge,
                    parsed.as_ref(),
                    search_target,
                );
                self.finish_hits(pagination, hits, SearchExecutionMode::Hybrid)
                    .await
            }
            Ok(SteamResults::NativePage { page, total }) if exact.is_none() => {
                let mut items = self
                    .apply_local_overlay(&query, ctx.app_id, page.raw.outcomes.iter(), true)
                    .await?;
                let mut seen = HashSet::new();
                items.retain(|item| seen.insert(item.workshop_id));
                Ok(pagination.output(items, total, SearchExecutionMode::Hybrid))
            }
            Ok(_) => {
                self.local_with_exact(
                    &query,
                    pagination,
                    exact.as_ref(),
                    SearchExecutionMode::Hybrid,
                )
                .await
            }
        }
    }

    async fn lookup_exact(
        &self,
        wid: WorkshopId,
        ctx: SteamFetchCtx,
        sync: &SyncSection,
    ) -> anyhow::Result<ExactLookup> {
        if !sync.enable_read_through {
            return Ok(ExactLookup::Local);
        }
        let stored = self.db_call(move |r| r.get_item(wid)).await?;
        if stored.as_ref().is_some_and(|meta| {
            meta.is_in_public_catalog()
                || meta.is_admin_override
                || meta.record.status == WorkshopItemStatus::PrivateOrUnavailable
                || meta.is_listed == Some(false)
        }) || matches!(
            self.repo.negative_status(wid, sync.item_ttl_secs),
            Some(WorkshopItemStatus::PrivateOrUnavailable | WorkshopItemStatus::Removed)
        ) {
            return Ok(ExactLookup::Local);
        }

        let result = match remaining_timeout(ctx.deadline) {
            Ok(budget) => tokio::time::timeout(budget, self.upstream.fetch_details(&[wid]))
                .await
                .map_err(|_| anyhow::anyhow!("精确 ID Steam CM 检索超时"))
                .and_then(|res| res),
            Err(err) => Err(err),
        };
        match result {
            Ok(mut observations) => {
                observations.retain(|obs| obs.outcome.workshop_id() == wid);
                self.persist_confirmed_restrictions(&observations, ctx.app_id)
                    .await?;
                if ctx.cache_steam_results {
                    self.warm_public_results(&observations, ctx.app_id, 0);
                }
                Ok(ExactLookup::Steam(
                    observations.into_iter().next().map(Box::new),
                ))
            }
            Err(err) => {
                tracing::warn!(workshop_id = %wid, error = %err, "精确 ID Steam CM 检索失败");
                Ok(ExactLookup::Unavailable)
            }
        }
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
        mode: SearchExecutionMode,
    ) -> anyhow::Result<HybridSearchOutput> {
        let Some(exact) = exact else {
            return self.run_local_only(query, mode).await;
        };
        let local = self.catalog_matches(query).await?;
        let hits = std::iter::once(SearchHit::Steam(exact))
            .chain(
                local
                    .into_iter()
                    .filter(|matched| matched.id != exact.workshop_id)
                    .map(SearchHit::Catalog),
            )
            .collect();
        self.finish_hits(pagination, hits, mode).await
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
        pagination: Pagination,
        hits: Vec<SearchHit<'_>>,
        mode: SearchExecutionMode,
    ) -> anyhow::Result<HybridSearchOutput> {
        let total = hits.len() as u64;
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
            // A sync can change listing status between ranking and loading the page.
            local.extend(
                stored
                    .into_iter()
                    .filter(|meta| meta.is_in_public_catalog())
                    .map(|meta| (meta.record.workshop_id, meta.record)),
            );
        }
        let items = page
            .into_iter()
            .filter_map(|hit| match hit {
                SearchHit::Catalog(matched) => local.remove(&matched.id),
                SearchHit::Steam(item) => Some(item.clone()),
            })
            .collect();
        Ok(pagination.output(items, total, mode))
    }

    async fn fetch_steam_results(
        self: &Arc<Self>,
        request: &SteamSearchRequest,
        ctx: SteamFetchCtx,
    ) -> Result<SteamResults, SteamSearchError> {
        let mut scan_request = request.clone();
        scan_request.page = 1;
        let first = self.fetch_steam_page_cached(&scan_request, ctx).await?;
        let total = steam_total(&first.raw);
        // For very broad searches, keep native Steam paging for the entire query.
        // An exact union would require scanning every upstream page before returning any page.
        if total > MAX_STEAM_MERGE_ITEMS {
            let page = if request.page == 1 {
                first
            } else {
                self.fetch_steam_page_cached(request, ctx).await?
            };
            return Ok(SteamResults::NativePage { page, total });
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
        if scan_request.num_per_page != request.num_per_page
            && pages
                .first()
                .is_some_and(|page| steam_total(&page.raw) == 0)
        {
            return Err(SteamSearchError::Unavailable(
                "Steam CM 检索结果在重新分页时发生变化".to_owned(),
            ));
        }
        if pages.iter().all(|page| page.raw.outcomes.is_empty()) {
            pages.clear();
        }
        Ok(SteamResults::Complete(pages))
    }

    async fn apply_local_overlay<'a>(
        &self,
        query: &SearchQuery,
        app_id: u32,
        observations: impl IntoIterator<Item = &'a WorkshopObservation>,
        steam_enforced_tags: bool,
    ) -> anyhow::Result<Vec<WorkshopItemRecord>> {
        let mut candidates = Vec::new();
        let mut ids = Vec::new();
        for obs in observations {
            if !is_public_active_for_app(obs, app_id) {
                continue;
            }
            if let WorkshopFetchOutcome::Found { item } = &obs.outcome {
                if matches_filters(item, query, !steam_enforced_tags) {
                    ids.push(item.workshop_id);
                    candidates.push(item.clone());
                }
            }
        }
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let (stored, _) = self.db_call(move |r| r.get_batch(&ids)).await?;
        let meta_map: HashMap<_, _> = stored
            .into_iter()
            .map(|meta| (meta.record.workshop_id, meta))
            .collect();

        let mut out = Vec::with_capacity(candidates.len());
        for mut item in candidates {
            if let Some(meta) = meta_map.get(&item.workshop_id) {
                if meta.is_admin_override {
                    if meta.is_listed == Some(false) || !matches_filters(&meta.record, query, true)
                    {
                        continue;
                    }
                    out.push(meta.record.clone());
                    continue;
                }
                if (matches!(
                    meta.record.status,
                    WorkshopItemStatus::PrivateOrUnavailable | WorkshopItemStatus::Removed
                ) || meta.is_listed == Some(false))
                    && meta.last_verified_at >= item.fetched_at
                {
                    continue;
                }
                if item.creator_name.is_none() {
                    item.creator_name = meta.record.creator_name.clone();
                }
            }
            if let Some((neg_status, neg_at)) = self.repo.negative_observed_at(item.workshop_id) {
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
        if !ctx.cache_ttl.is_zero() {
            let cached = {
                let mut cache = self.cache.lock().await;
                match cache.get(request) {
                    Some(entry) if entry.cached_at.elapsed() < ctx.cache_ttl => {
                        Some(entry.page.clone())
                    }
                    _ => {
                        cache.remove(request);
                        None
                    }
                }
            };
            if let Some(page) = cached {
                self.save_page_restrictions(&page, ctx.app_id).await?;
                return Ok(page);
            }
        }

        let mut map = self.inflight.lock().await;
        let wait_budget = remaining_timeout(ctx.deadline)
            .map_err(|err| SteamSearchError::Unavailable(err.to_string()))?;
        let mut rx = if let Some(existing) = map.get(request).filter(|rx| !rx.is_closed()) {
            existing.resubscribe()
        } else {
            let (tx, rx) = broadcast::channel(1);
            map.insert(request.clone(), rx.resubscribe());
            let svc = Arc::clone(self);
            let req = request.clone();
            tokio::spawn(async move {
                let result = tokio::time::timeout(wait_budget, svc.upstream.search_files(&req))
                    .await
                    .map_err(|_| SteamSearchError::Unavailable("Steam CM 实时搜索超时".to_owned()))
                    .and_then(|res| {
                        res.map_err(|err| SteamSearchError::Unavailable(err.to_string()))
                    })
                    .map(|raw| {
                        Arc::new(SharedSteamPage {
                            raw,
                            restrictions_saved: OnceCell::new(),
                        })
                    });

                if let Ok(page) = &result {
                    if !ctx.cache_ttl.is_zero() {
                        svc.insert_cached_page(req.clone(), page.clone(), ctx.cache_ttl)
                            .await;
                    }
                }
                {
                    let mut map = svc.inflight.lock().await;
                    map.remove(&req);
                    let _ = tx.send(result.clone());
                }
                // Save restrictions even if the original HTTP request has gone away.
                if let Ok(page) = result {
                    match svc.save_page_restrictions(&page, ctx.app_id).await {
                        Ok(()) if ctx.cache_steam_results => {
                            svc.warm_public_results(&page.raw.outcomes, ctx.app_id, req.language);
                        }
                        Ok(()) => {}
                        Err(err) => tracing::error!(error = %err, "保存搜索结果的可见性失败"),
                    }
                }
            });
            rx
        };
        drop(map);

        let page = tokio::time::timeout(wait_budget, rx.recv())
            .await
            .map_err(|_| SteamSearchError::Unavailable("Steam CM 实时搜索等待超时".to_owned()))?
            .map_err(|err| {
                SteamSearchError::Unavailable(format!("Steam CM 搜索并发通道异常: {err}"))
            })??;
        self.save_page_restrictions(&page, ctx.app_id).await?;
        Ok(page)
    }

    async fn insert_cached_page(
        &self,
        request: SteamSearchRequest,
        page: Arc<SharedSteamPage>,
        cache_ttl: Duration,
    ) {
        let mut cache = self.cache.lock().await;
        cache.retain(|_, entry| entry.cached_at.elapsed() < cache_ttl);
        if cache.len() >= MAX_SEARCH_CACHE_ENTRIES {
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
            self.db_call(move |repo| {
                repo.apply_upstream_outcomes_with_policy(
                    &restricted,
                    CatalogIndexPolicy::CacheOnly,
                )?;
                Ok(())
            })
            .await?;
        }
        Ok(())
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
        let repo = Arc::clone(&self.repo);
        let policy = if language == 0 {
            CatalogIndexPolicy::RefreshUnindexedCache
        } else {
            CatalogIndexPolicy::InsertOnlyCache
        };
        tokio::task::spawn_blocking(move || {
            if let Err(err) = repo.apply_upstream_outcomes_with_policy(&observations, policy) {
                tracing::warn!(error = %err, "Steam 检索暖缓存写入失败");
            }
        });
    }
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
    let mut steam_ids = HashSet::new();
    let steam: Vec<_> = steam
        .iter()
        .filter(|item| steam_ids.insert(item.workshop_id))
        .collect();
    // A cached Steam hit can be suppressed by a newer local tombstone. Keep that ID out of the local tail as well.
    let suppressed: HashSet<_> = observations
        .iter()
        .map(|obs| obs.outcome.workshop_id())
        .filter(|id| !steam_ids.contains(id))
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
        let first_page_ids: HashSet<_> = observations[..first_page_len]
            .iter()
            .map(|obs| obs.outcome.workshop_id())
            .collect();
        let steam_best_tier = parsed.and_then(|parsed| {
            steam
                .iter()
                .filter(|item| first_page_ids.contains(&item.workshop_id))
                .take(5)
                .filter_map(|item| score_for_target(parsed, target, item))
                .map(|rank| rank.tier)
                .min()
        });
        let (mut promoted, tail): (Vec<_>, Vec<_>) = local.partition(|hit| {
            !first_page_ids.contains(&hit.id()) && hit.should_promote(steam_best_tier)
        });
        let promoted_ids: HashSet<_> = promoted.iter().map(|hit| hit.id()).collect();
        promoted.extend(
            steam
                .into_iter()
                .filter(|item| !promoted_ids.contains(&item.workshop_id))
                .map(SearchHit::Steam),
        );
        promoted.extend(
            tail.into_iter()
                .filter(|hit| !steam_ids.contains(&hit.id())),
        );
        promoted
    } else {
        steam
            .into_iter()
            .map(SearchHit::Steam)
            .chain(local.filter(|hit| !steam_ids.contains(&hit.id())))
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
    if matches!(
        query.status,
        Some(WorkshopItemStatus::Removed | WorkshopItemStatus::PrivateOrUnavailable)
    ) {
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
            merge: MergeStrategy::Deterministic(DeterministicSort::Subscriptions),
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
        Some(SteamSearchTarget::Title) => {
            search::score_title_only_candidate(parsed, item.workshop_id.as_u64(), &item.title)
        }
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
            MergeStrategy::Deterministic(DeterministicSort::Subscriptions)
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
