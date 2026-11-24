use crate::config::CatalogConfigStore;
use crate::db::{CatalogIndexPolicy, RankedLocalItem, RankedLocalSearch, ServerRepo};
use crate::search::{self, MatchRank, ParsedQuery};
use crate::sync::with_catalog_source;
use crate::upstream::{
    SteamSearchRequest, SteamSearchTarget, WorkshopObservation, WorkshopQueryPage,
    WorkshopUpstream, MATCHING_FILE_TYPE_COLLECTIONS, MATCHING_FILE_TYPE_ITEMS,
    QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE, QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
    QUERY_TYPE_RANKED_BY_TEXT_SEARCH, QUERY_TYPE_RANKED_BY_TOTAL_UNIQUE_SUBSCRIPTIONS,
    QUERY_TYPE_RANKED_BY_TREND, QUERY_TYPE_RANKED_BY_VOTE,
};
use chrono::{DateTime, Utc};
use l4a_protocol::{
    MetadataSource, SearchQuery, SearchResponse, WorkshopFetchOutcome, WorkshopFileType,
    WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinSet;

const MAX_SEARCH_CACHE_ENTRIES: usize = 512;
const LOCAL_WINDOW_LIMIT: u32 = 100;
const OVERLAY_BATCH_SIZE: usize = 100;
const MAX_FULL_STEAM_DEDUP_ITEMS: u64 = 240;
const MAX_FULL_STEAM_DEDUP_PAGES: u32 = 12;
const MAX_DETERMINISTIC_MERGE_ITEMS: u64 = 300;
const MAX_DETERMINISTIC_MERGE_PAGES: u32 = 12;
const MAX_FULL_LOCAL_PRELOAD: u64 = 500;
const MAX_DETERMINISTIC_LOCAL_PRELOAD: u64 = 2000;

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

#[derive(Clone)]
struct CachedSteamPage {
    page: WorkshopQueryPage,
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
    steam_timeout: Duration,
    deadline: Instant,
    cache_steam_results: bool,
}

enum MergeOutcome {
    Ready(SearchResponse),
    SteamUnavailable(anyhow::Error),
}

struct FullSteamCorpus {
    steam_p1: WorkshopQueryPage,
    all_overlaid: Vec<WorkshopItemRecord>,
    raw_steam_ids: HashSet<WorkshopId>,
    suppressed_steam_ids: HashSet<WorkshopId>,
    contradicted: HashMap<WorkshopId, DateTime<Utc>>,
}

struct LocalTailCursor<'a> {
    query: &'a SearchQuery,
    in_memory: &'a [WorkshopItemRecord],
    raw_local_loaded: u64,
    local_total: u64,
    contradicted: &'a HashMap<WorkshopId, DateTime<Utc>>,
    excluded_ids: &'a HashSet<WorkshopId>,
    to_skip: usize,
    page_size: usize,
}

pub struct HybridSearchService {
    config_store: Arc<CatalogConfigStore>,
    repo: Arc<ServerRepo>,
    upstream: Arc<dyn WorkshopUpstream>,
    cache: Mutex<HashMap<SteamSearchRequest, CachedSteamPage>>,
    inflight:
        Mutex<HashMap<SteamSearchRequest, broadcast::Receiver<Result<WorkshopQueryPage, String>>>>,
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

    async fn fetch_local_range(
        &self,
        query: &SearchQuery,
        offset: u64,
        limit: u32,
    ) -> anyhow::Result<RankedLocalSearch> {
        let q = query.clone();
        self.db_call(move |r| r.search_ranked(&q, true, offset, limit))
            .await
    }

    pub async fn search(
        self: &Arc<Self>,
        query: &SearchQuery,
        accept_language: Option<&str>,
    ) -> anyhow::Result<HybridSearchOutput> {
        let page = query.page.max(1);
        let page_size = query.page_size.clamp(1, 100);
        let offset = u64::from(page - 1) * u64::from(page_size);

        let cfg = self.config_store.get();
        let search_cfg = cfg.search.clone();
        let app_id = cfg.steam.app_id;
        let item_ttl_secs = cfg.sync.item_ttl_secs;
        let enable_read_through = cfg.sync.enable_read_through;

        let parsed = query.query.as_deref().and_then(search::parse_query);
        let is_pure_url = parsed
            .as_ref()
            .is_some_and(|p| p.exact_workshop_id.is_some() && p.numeric_prefix.is_none());

        let mut effective_query = query.clone();
        if !is_pure_url && effective_query.file_type.is_none() {
            effective_query.file_type = Some(WorkshopFileType::Item);
        }

        if !search_cfg.enable_hybrid_search || should_use_local_only(&effective_query) {
            return self
                .run_local_only(&effective_query, SearchExecutionMode::LocalOnly)
                .await;
        }

        let steam_timeout = Duration::from_millis(search_cfg.steam_timeout_ms);
        let deadline = Instant::now() + steam_timeout;
        let cache_ttl = Duration::from_secs(search_cfg.cache_ttl_secs);

        // Handle direct Steam Workshop URL or 6+ digit Workshop ID lookup.
        let mut exact_steam_item: Option<WorkshopItemRecord> = None;
        if let Some(wid) = parsed.as_ref().and_then(|p| p.exact_workshop_id) {
            let is_long_numeric = parsed
                .as_ref()
                .and_then(|p| p.numeric_prefix.as_deref())
                .is_some_and(|p| p.len() >= 6);

            if is_pure_url || is_long_numeric {
                let wid_typed = WorkshopId(wid);
                let q_clone = effective_query.clone();
                let local_check = self.db_call(move |r| r.search(&q_clone, true)).await?;
                let found_in_local = if is_pure_url {
                    local_check.total > 0
                } else {
                    local_check
                        .items
                        .iter()
                        .any(|item| item.workshop_id == wid_typed)
                };

                if is_pure_url && found_in_local {
                    return Ok(HybridSearchOutput {
                        response: normalize_response(local_check),
                        mode: SearchExecutionMode::LocalOnly,
                    });
                }

                if !found_in_local {
                    if !enable_read_through {
                        if is_pure_url {
                            return Ok(HybridSearchOutput {
                                response: normalize_response(local_check),
                                mode: SearchExecutionMode::LocalOnly,
                            });
                        }
                    } else {
                        let stored = self.db_call(move |r| r.get_item(wid_typed)).await?;
                        let negative = self.repo.negative_status(wid_typed, item_ttl_secs);
                        let blocked_by_local = stored.as_ref().is_some_and(|meta| {
                            meta.is_admin_override
                                || meta.record.status == WorkshopItemStatus::PrivateOrUnavailable
                                || meta.is_listed == Some(false)
                        }) || matches!(
                            negative,
                            Some(
                                WorkshopItemStatus::PrivateOrUnavailable
                                    | WorkshopItemStatus::Removed
                            )
                        );

                        if !blocked_by_local {
                            let exact_res = match remaining_timeout(deadline) {
                                Ok(budget) => tokio::time::timeout(
                                    budget,
                                    self.upstream.fetch_details(&[wid_typed]),
                                )
                                .await
                                .map_err(|_| anyhow::anyhow!("精确 ID Steam CM 检索超时"))
                                .and_then(|res| res),
                                Err(err) => Err(err),
                            };
                            match exact_res {
                                Ok(observations) => {
                                    if search_cfg.cache_steam_results {
                                        let warm: Vec<_> = observations
                                            .iter()
                                            .filter(|obs| is_public_active_for_app(obs, app_id))
                                            .cloned()
                                            .collect();
                                        self.spawn_cache_warming(warm, 0);
                                    }
                                    if let Some(obs) = observations
                                        .into_iter()
                                        .find(|o| o.outcome.workshop_id() == wid_typed)
                                    {
                                        if is_public_active_for_app(&obs, app_id) {
                                            if let WorkshopFetchOutcome::Found { item } =
                                                obs.outcome
                                            {
                                                if matches_filters(&item, &effective_query, true) {
                                                    exact_steam_item = Some(item);
                                                }
                                            }
                                        }
                                    }

                                    if is_pure_url {
                                        let (total, items) = match exact_steam_item {
                                            Some(item) => (
                                                1,
                                                if offset == 0 {
                                                    vec![with_catalog_source(item)]
                                                } else {
                                                    Vec::new()
                                                },
                                            ),
                                            None => (0, Vec::new()),
                                        };
                                        return Ok(HybridSearchOutput {
                                            response: SearchResponse {
                                                items,
                                                total,
                                                page,
                                                page_size,
                                                source: MetadataSource::CatalogServer,
                                            },
                                            mode: SearchExecutionMode::Hybrid,
                                        });
                                    }
                                }
                                Err(err) => {
                                    tracing::warn!(workshop_id = %wid_typed, error = %err, "精确 ID Steam CM 检索失败");
                                    if is_pure_url {
                                        return Ok(HybridSearchOutput {
                                            response: normalize_response(local_check),
                                            mode: SearchExecutionMode::LocalFallback,
                                        });
                                    }
                                }
                            }
                        } else if is_pure_url {
                            return Ok(HybridSearchOutput {
                                response: normalize_response(local_check),
                                mode: SearchExecutionMode::LocalOnly,
                            });
                        }
                    }
                }
            }
        }

        let trimmed_q = effective_query
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        let sort_plan = resolve_sort_plan(effective_query.sort.as_deref(), trimmed_q.is_some());
        let language = resolve_steam_language(
            effective_query.language.as_deref(),
            accept_language,
            &search_cfg.default_language,
        );
        let search_target = parsed_steam_target(effective_query.search_target.as_deref());
        let file_type = match effective_query.file_type {
            Some(WorkshopFileType::Collection) => MATCHING_FILE_TYPE_COLLECTIONS,
            _ => MATCHING_FILE_TYPE_ITEMS,
        };
        let required_tags = effective_query
            .tag
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|t| vec![t.to_string()])
            .unwrap_or_default();

        let base_request = SteamSearchRequest {
            query_type: sort_plan.query_type,
            file_type,
            page,
            num_per_page: page_size,
            search_text: trimmed_q.clone(),
            required_tags,
            language,
            days: sort_plan.days,
            search_text_target: trimmed_q.as_ref().and(search_target),
        };
        let fetch_ctx = SteamFetchCtx {
            app_id,
            cache_ttl,
            steam_timeout,
            deadline,
            cache_steam_results: search_cfg.cache_steam_results,
        };

        let local_limit = match sort_plan.merge {
            MergeStrategy::Deterministic(_) => (offset + u64::from(page_size))
                .clamp(u64::from(LOCAL_WINDOW_LIMIT), MAX_DETERMINISTIC_MERGE_ITEMS)
                as u32,
            _ => LOCAL_WINDOW_LIMIT,
        };
        let local_fut = self.fetch_local_range(&effective_query, 0, local_limit);
        let steam_fut = self.fetch_steam_page_cached(&base_request, fetch_ctx);
        let (local_res, steam_res) = tokio::join!(local_fut, steam_fut);
        let mut local_window = local_res?;
        let mut raw_local_loaded = local_window.items.len() as u64;

        let exact_id = exact_steam_item.as_ref().map(|item| item.workshop_id);
        if let Some(exact_item) = exact_steam_item {
            inject_exact_item_into_local(&mut local_window, exact_item);
        }

        let initial_steam_page = match steam_res {
            Ok(p) => p,
            Err(err) => {
                tracing::warn!(error = %err, "Steam CM 混合检索失败，回退至本地索引");
                return self
                    .fallback_to_local(
                        &effective_query,
                        &local_window,
                        raw_local_loaded,
                        exact_id,
                        page,
                        page_size,
                        offset,
                        SearchExecutionMode::LocalFallback,
                    )
                    .await;
            }
        };

        // If Steam returned no matches at all, fall back to exact local catalog pagination.
        if initial_steam_page.total == 0 && initial_steam_page.outcomes.is_empty() {
            return self
                .fallback_to_local(
                    &effective_query,
                    &local_window,
                    raw_local_loaded,
                    exact_id,
                    page,
                    page_size,
                    offset,
                    SearchExecutionMode::Hybrid,
                )
                .await;
        }

        let steam_total_raw = initial_steam_page
            .total
            .max(initial_steam_page.outcomes.len() as u64);
        let steam_pages_total = steam_total_raw.div_ceil(u64::from(page_size)).max(1) as u32;
        let can_full_dedup = local_window.total > 0
            && steam_total_raw <= MAX_FULL_STEAM_DEDUP_ITEMS
            && steam_pages_total <= MAX_FULL_STEAM_DEDUP_PAGES;

        let end = offset + u64::from(page_size);
        if can_full_dedup {
            let max_preload = match sort_plan.merge {
                MergeStrategy::Deterministic(_) => end
                    .max(u64::from(LOCAL_WINDOW_LIMIT))
                    .min(MAX_DETERMINISTIC_LOCAL_PRELOAD)
                    .min(local_window.total),
                _ => MAX_FULL_LOCAL_PRELOAD.min(local_window.total),
            };
            if raw_local_loaded < max_preload {
                let extra = self
                    .fetch_local_range(
                        &effective_query,
                        raw_local_loaded,
                        (max_preload - raw_local_loaded) as u32,
                    )
                    .await?;
                let fetched_len = extra.items.len() as u64;
                extend_local_window(&mut local_window, extra.items, exact_id);
                raw_local_loaded = raw_local_loaded.saturating_add(fetched_len);
            }
        }

        let outcome = if can_full_dedup {
            match self
                .build_full_steam_corpus(
                    &effective_query,
                    app_id,
                    page,
                    steam_pages_total,
                    &base_request,
                    &initial_steam_page,
                    fetch_ctx,
                )
                .await?
            {
                Ok(corpus) => match sort_plan.merge {
                    MergeStrategy::Relevance => {
                        self.merge_relevance_full(
                            &effective_query,
                            parsed.as_ref(),
                            search_target,
                            page,
                            page_size,
                            offset,
                            &local_window,
                            raw_local_loaded,
                            exact_id,
                            &corpus,
                        )
                        .await?
                    }
                    MergeStrategy::SteamOrder => {
                        self.merge_steam_order_full(
                            &effective_query,
                            page,
                            page_size,
                            offset,
                            &local_window,
                            raw_local_loaded,
                            exact_id,
                            &corpus,
                        )
                        .await?
                    }
                    MergeStrategy::Deterministic(det_sort) => {
                        self.merge_deterministic_full(
                            &effective_query,
                            det_sort,
                            page,
                            page_size,
                            offset,
                            &local_window,
                            raw_local_loaded,
                            exact_id,
                            &corpus,
                        )
                        .await?
                    }
                },
                Err(err) => MergeOutcome::SteamUnavailable(err),
            }
        } else {
            match sort_plan.merge {
                MergeStrategy::Relevance => {
                    self.merge_relevance_windowed(
                        &effective_query,
                        parsed.as_ref(),
                        search_target,
                        page,
                        page_size,
                        offset,
                        &base_request,
                        initial_steam_page,
                        &local_window,
                        raw_local_loaded,
                        exact_id,
                        fetch_ctx,
                    )
                    .await?
                }
                MergeStrategy::SteamOrder => MergeOutcome::Ready(
                    self.merge_steam_order_windowed(
                        &effective_query,
                        app_id,
                        page,
                        page_size,
                        offset,
                        initial_steam_page,
                        &local_window,
                        raw_local_loaded,
                        exact_id,
                    )
                    .await?,
                ),
                MergeStrategy::Deterministic(det_sort) => {
                    self.merge_deterministic_windowed(
                        &effective_query,
                        det_sort,
                        page,
                        page_size,
                        offset,
                        &base_request,
                        initial_steam_page,
                        &local_window,
                        raw_local_loaded,
                        exact_id,
                        fetch_ctx,
                    )
                    .await?
                }
            }
        };

        match outcome {
            MergeOutcome::Ready(response) => Ok(HybridSearchOutput {
                response,
                mode: SearchExecutionMode::Hybrid,
            }),
            MergeOutcome::SteamUnavailable(err) => {
                tracing::warn!(error = %err, "Steam CM 多页检索失败，回退至本地索引");
                self.fallback_to_local(
                    &effective_query,
                    &local_window,
                    raw_local_loaded,
                    exact_id,
                    page,
                    page_size,
                    offset,
                    SearchExecutionMode::LocalFallback,
                )
                .await
            }
        }
    }

    async fn run_local_only(
        &self,
        query: &SearchQuery,
        mode: SearchExecutionMode,
    ) -> anyhow::Result<HybridSearchOutput> {
        let q = query.clone();
        let resp = self.db_call(move |r| r.search(&q, true)).await?;
        Ok(HybridSearchOutput {
            response: normalize_response(resp),
            mode,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn fallback_to_local(
        &self,
        query: &SearchQuery,
        local_window: &RankedLocalSearch,
        raw_local_loaded: u64,
        exact_id: Option<WorkshopId>,
        page: u32,
        page_size: u32,
        offset: u64,
        mode: SearchExecutionMode,
    ) -> anyhow::Result<HybridSearchOutput> {
        let end = offset + u64::from(page_size);
        let covers_page = (local_window.items.len() as u64) >= end
            || raw_local_loaded >= local_window.total
            || (local_window.items.len() as u64) >= local_window.total;
        if covers_page {
            return Ok(HybridSearchOutput {
                response: slice_local_window(local_window, page, page_size, offset),
                mode,
            });
        }
        if exact_id.is_none() {
            return self.run_local_only(query, mode).await;
        }
        let fetched = self
            .fetch_local_range(query, offset.saturating_sub(1), page_size)
            .await?;
        let items: Vec<WorkshopItemRecord> = fetched
            .items
            .into_iter()
            .filter(|item| exact_id != Some(item.record.workshop_id))
            .take(page_size as usize)
            .map(|item| with_catalog_source(item.record))
            .collect();
        Ok(HybridSearchOutput {
            response: SearchResponse {
                items,
                total: local_window.total,
                page,
                page_size,
                source: MetadataSource::CatalogServer,
            },
            mode,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_full_steam_corpus(
        self: &Arc<Self>,
        query: &SearchQuery,
        app_id: u32,
        current_page: u32,
        steam_pages_total: u32,
        base_request: &SteamSearchRequest,
        initial_steam_page: &WorkshopQueryPage,
        fetch_ctx: SteamFetchCtx,
    ) -> anyhow::Result<Result<FullSteamCorpus, anyhow::Error>> {
        let missing_pages: Vec<u32> = (1..=steam_pages_total)
            .filter(|&p| p != current_page)
            .collect();
        let mut pages = match self
            .fetch_steam_pages_concurrent(base_request, missing_pages, fetch_ctx)
            .await
        {
            Ok(map) => map,
            Err(err) => return Ok(Err(err)),
        };
        pages.insert(current_page, initial_steam_page.clone());

        let mut raw_steam_ids = HashSet::new();
        let mut contradicted = HashMap::new();
        let mut all_overlaid = Vec::new();
        let mut seen_overlaid = HashSet::new();
        for p in 1..=steam_pages_total {
            if let Some(raw_page) = pages.get(&p) {
                record_steam_page_metadata(raw_page, app_id, &mut raw_steam_ids, &mut contradicted);
                let overlaid = self
                    .apply_local_overlay(query, app_id, &raw_page.outcomes, true)
                    .await?;
                for item in overlaid {
                    if seen_overlaid.insert(item.workshop_id) {
                        all_overlaid.push(item);
                    }
                }
            }
        }
        let suppressed_steam_ids: HashSet<WorkshopId> =
            raw_steam_ids.difference(&seen_overlaid).copied().collect();
        let steam_p1 = pages
            .remove(&1)
            .unwrap_or_else(|| initial_steam_page.clone());
        Ok(Ok(FullSteamCorpus {
            steam_p1,
            all_overlaid,
            raw_steam_ids,
            suppressed_steam_ids,
            contradicted,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    async fn merge_relevance_full(
        &self,
        query: &SearchQuery,
        parsed: Option<&ParsedQuery>,
        search_target: Option<SteamSearchTarget>,
        page: u32,
        page_size: u32,
        offset: u64,
        local_window: &RankedLocalSearch,
        raw_local_loaded: u64,
        exact_id: Option<WorkshopId>,
        corpus: &FullSteamCorpus,
    ) -> anyhow::Result<MergeOutcome> {
        let steam_p1_items = self
            .apply_local_overlay(
                query,
                self.config_store.get().steam.app_id,
                &corpus.steam_p1.outcomes,
                true,
            )
            .await?;
        let steam_p1_ids: HashSet<WorkshopId> =
            steam_p1_items.iter().map(|i| i.workshop_id).collect();
        let steam_p1_best_tier = parsed.and_then(|pq| {
            steam_p1_items
                .iter()
                .take(5)
                .filter_map(|item| score_for_target(pq, search_target, item))
                .map(|rank| rank.tier)
                .min()
        });

        let (promoted_local, supplemental_local, promoted_ids, contradicted_dropped) =
            partition_local_items(
                &local_window.items,
                &corpus.contradicted,
                &corpus.suppressed_steam_ids,
                &steam_p1_ids,
                steam_p1_best_tier,
            );

        let deduped_steam: Vec<WorkshopItemRecord> = corpus
            .all_overlaid
            .iter()
            .filter(|item| !promoted_ids.contains(&item.workshop_id))
            .cloned()
            .collect();

        let k = promoted_local.len() as u64;
        let steam_len = deduped_steam.len() as u64;
        let end = offset + u64::from(page_size);

        let mut page_items = Vec::with_capacity(page_size as usize);
        let mut emitted_ids = HashSet::new();

        if offset < k {
            let start_idx = offset as usize;
            let end_idx = end.min(k) as usize;
            for item in &promoted_local[start_idx..end_idx] {
                if emitted_ids.insert(item.workshop_id) {
                    page_items.push(item.clone());
                }
            }
        }

        if end > k && page_items.len() < page_size as usize {
            let s_start = offset.saturating_sub(k).min(steam_len) as usize;
            let s_end = end.saturating_sub(k).min(steam_len) as usize;
            for item in &deduped_steam[s_start..s_end] {
                if emitted_ids.insert(item.workshop_id) {
                    page_items.push(item.clone());
                    if page_items.len() >= page_size as usize {
                        break;
                    }
                }
            }
        }

        let mut excluded_for_tail = corpus.raw_steam_ids.clone();
        excluded_for_tail.extend(&promoted_ids);
        excluded_for_tail.extend(&emitted_ids);
        if let Some(eid) = exact_id {
            excluded_for_tail.insert(eid);
        }

        if page_items.len() < page_size as usize {
            let supp_skip = offset.saturating_sub(k.saturating_add(steam_len)) as usize;
            self.append_local_tail(
                LocalTailCursor {
                    query,
                    in_memory: &supplemental_local,
                    raw_local_loaded,
                    local_total: local_window.total,
                    contradicted: &corpus.contradicted,
                    excluded_ids: &excluded_for_tail,
                    to_skip: supp_skip,
                    page_size: page_size as usize,
                },
                &mut emitted_ids,
                &mut page_items,
            )
            .await?;
        }

        let overlap_supp = supplemental_local
            .iter()
            .filter(|item| corpus.raw_steam_ids.contains(&item.workshop_id))
            .count() as u64;
        let effective_local_total = local_window.total.saturating_sub(contradicted_dropped);
        let total = effective_local_total
            .saturating_add(steam_len)
            .saturating_sub(overlap_supp);
        let total = if page_items.is_empty() {
            total
        } else {
            total.max(offset + page_items.len() as u64)
        };

        Ok(MergeOutcome::Ready(SearchResponse {
            items: page_items.into_iter().map(with_catalog_source).collect(),
            total,
            page,
            page_size,
            source: MetadataSource::CatalogServer,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    async fn merge_steam_order_full(
        &self,
        query: &SearchQuery,
        page: u32,
        page_size: u32,
        offset: u64,
        local_window: &RankedLocalSearch,
        raw_local_loaded: u64,
        exact_id: Option<WorkshopId>,
        corpus: &FullSteamCorpus,
    ) -> anyhow::Result<MergeOutcome> {
        let mut contradicted_dropped = 0u64;
        let mut visible_local = Vec::new();
        for local in &local_window.items {
            if is_local_contradicted(local, &corpus.contradicted)
                || corpus
                    .suppressed_steam_ids
                    .contains(&local.record.workshop_id)
            {
                contradicted_dropped += 1;
            } else {
                visible_local.push(local.record.clone());
            }
        }

        let steam_len = corpus.all_overlaid.len() as u64;
        let end = offset + u64::from(page_size);
        let mut page_items = Vec::with_capacity(page_size as usize);
        let mut emitted_ids = HashSet::new();

        if offset < steam_len {
            let s_start = offset as usize;
            let s_end = end.min(steam_len) as usize;
            for item in &corpus.all_overlaid[s_start..s_end] {
                if emitted_ids.insert(item.workshop_id) {
                    page_items.push(item.clone());
                }
            }
        }

        let mut excluded_for_tail = corpus.raw_steam_ids.clone();
        excluded_for_tail.extend(&emitted_ids);
        if let Some(eid) = exact_id {
            excluded_for_tail.insert(eid);
        }
        if page_items.len() < page_size as usize {
            let supp_skip = offset.saturating_sub(steam_len) as usize;
            self.append_local_tail(
                LocalTailCursor {
                    query,
                    in_memory: &visible_local,
                    raw_local_loaded,
                    local_total: local_window.total,
                    contradicted: &corpus.contradicted,
                    excluded_ids: &excluded_for_tail,
                    to_skip: supp_skip,
                    page_size: page_size as usize,
                },
                &mut emitted_ids,
                &mut page_items,
            )
            .await?;
        }

        let overlap = visible_local
            .iter()
            .filter(|item| corpus.raw_steam_ids.contains(&item.workshop_id))
            .count() as u64;
        let total = local_window
            .total
            .saturating_sub(contradicted_dropped)
            .saturating_add(steam_len)
            .saturating_sub(overlap);
        let total = if page_items.is_empty() {
            total
        } else {
            total.max(offset + page_items.len() as u64)
        };

        Ok(MergeOutcome::Ready(SearchResponse {
            items: page_items.into_iter().map(with_catalog_source).collect(),
            total,
            page,
            page_size,
            source: MetadataSource::CatalogServer,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    async fn merge_deterministic_full(
        &self,
        query: &SearchQuery,
        det_sort: DeterministicSort,
        page: u32,
        page_size: u32,
        offset: u64,
        local_window: &RankedLocalSearch,
        raw_local_loaded: u64,
        exact_id: Option<WorkshopId>,
        corpus: &FullSteamCorpus,
    ) -> anyhow::Result<MergeOutcome> {
        let mut merged: HashMap<WorkshopId, WorkshopItemRecord> = HashMap::new();
        let mut contradicted_dropped = 0u64;
        let mut overlap = 0u64;
        for local in &local_window.items {
            if is_local_contradicted(local, &corpus.contradicted)
                || corpus
                    .suppressed_steam_ids
                    .contains(&local.record.workshop_id)
            {
                contradicted_dropped += 1;
                continue;
            }
            if corpus.raw_steam_ids.contains(&local.record.workshop_id) {
                overlap += 1;
            }
            merged.insert(local.record.workshop_id, local.record.clone());
        }
        for item in &corpus.all_overlaid {
            merged.insert(item.workshop_id, item.clone());
        }

        let mut all_items: Vec<WorkshopItemRecord> = merged.into_values().collect();
        sort_deterministic(&mut all_items, det_sort);

        let start = (offset as usize).min(all_items.len());
        let end = (start + page_size as usize).min(all_items.len());
        let mut page_items = all_items[start..end].to_vec();
        let mut emitted_ids: HashSet<WorkshopId> =
            page_items.iter().map(|i| i.workshop_id).collect();

        if page_items.len() < page_size as usize && raw_local_loaded < local_window.total {
            let mut excluded = corpus.raw_steam_ids.clone();
            excluded.extend(&emitted_ids);
            for item in &all_items {
                excluded.insert(item.workshop_id);
            }
            if let Some(eid) = exact_id {
                excluded.insert(eid);
            }
            let supp_skip = offset.saturating_sub(all_items.len() as u64) as usize;
            self.append_local_tail(
                LocalTailCursor {
                    query,
                    in_memory: &[],
                    raw_local_loaded,
                    local_total: local_window.total,
                    contradicted: &corpus.contradicted,
                    excluded_ids: &excluded,
                    to_skip: supp_skip,
                    page_size: page_size as usize,
                },
                &mut emitted_ids,
                &mut page_items,
            )
            .await?;
        }

        let total = local_window
            .total
            .saturating_sub(contradicted_dropped)
            .saturating_add(corpus.all_overlaid.len() as u64)
            .saturating_sub(overlap);
        let total = if page_items.is_empty() {
            total
        } else {
            total.max(offset + page_items.len() as u64)
        };

        Ok(MergeOutcome::Ready(SearchResponse {
            items: page_items.into_iter().map(with_catalog_source).collect(),
            total,
            page,
            page_size,
            source: MetadataSource::CatalogServer,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    async fn merge_relevance_windowed(
        self: &Arc<Self>,
        query: &SearchQuery,
        parsed: Option<&ParsedQuery>,
        search_target: Option<SteamSearchTarget>,
        page: u32,
        page_size: u32,
        offset: u64,
        base_request: &SteamSearchRequest,
        initial_steam_page: WorkshopQueryPage,
        local_window: &RankedLocalSearch,
        raw_local_loaded: u64,
        exact_id: Option<WorkshopId>,
        fetch_ctx: SteamFetchCtx,
    ) -> anyhow::Result<MergeOutcome> {
        let mut steam_pages: HashMap<u32, WorkshopQueryPage> = HashMap::new();
        steam_pages.insert(page, initial_steam_page.clone());

        let steam_total = initial_steam_page
            .total
            .max(initial_steam_page.outcomes.len() as u64);

        // Ensure page 1 is present if local matches exist on page > 1.
        if page > 1 && !local_window.items.is_empty() {
            let mut p1_req = base_request.clone();
            p1_req.page = 1;
            match self.fetch_steam_page_cached(&p1_req, fetch_ctx).await {
                Ok(p1) => {
                    steam_pages.insert(1, p1);
                }
                Err(err) => return Ok(MergeOutcome::SteamUnavailable(err)),
            }
        }

        let steam_p1 = steam_pages
            .get(&1)
            .cloned()
            .unwrap_or_else(|| initial_steam_page.clone());

        let mut seen_steam_ids = HashSet::new();
        let mut contradicted = HashMap::new();
        record_steam_page_metadata(
            &initial_steam_page,
            fetch_ctx.app_id,
            &mut seen_steam_ids,
            &mut contradicted,
        );
        if page > 1 {
            record_steam_page_metadata(
                &steam_p1,
                fetch_ctx.app_id,
                &mut seen_steam_ids,
                &mut contradicted,
            );
        }

        let mut overlaid_pages: HashMap<u32, Vec<WorkshopItemRecord>> = HashMap::new();
        let steam_p1_items = self
            .apply_local_overlay(query, fetch_ctx.app_id, &steam_p1.outcomes, true)
            .await?;
        let mut suppressed_steam_ids = collect_suppressed_ids(&steam_p1.outcomes, &steam_p1_items);
        overlaid_pages.insert(1, steam_p1_items.clone());

        if page > 1 {
            let init_overlaid = self
                .apply_local_overlay(query, fetch_ctx.app_id, &initial_steam_page.outcomes, true)
                .await?;
            suppressed_steam_ids.extend(collect_suppressed_ids(
                &initial_steam_page.outcomes,
                &init_overlaid,
            ));
            overlaid_pages.insert(page, init_overlaid);
        }

        let steam_p1_ids: HashSet<WorkshopId> =
            steam_p1_items.iter().map(|i| i.workshop_id).collect();
        let steam_p1_best_tier = parsed.and_then(|pq| {
            steam_p1_items
                .iter()
                .take(5)
                .filter_map(|item| score_for_target(pq, search_target, item))
                .map(|rank| rank.tier)
                .min()
        });

        let (promoted_local, supplemental_local, promoted_ids, mut contradicted_dropped) =
            partition_local_items(
                &local_window.items,
                &contradicted,
                &suppressed_steam_ids,
                &steam_p1_ids,
                steam_p1_best_tier,
            );

        let k = promoted_local.len() as u64;
        let steam_total = steam_total.max(steam_p1.total);
        let end = offset + u64::from(page_size);

        let ps_u64 = u64::from(page_size);
        let steam_start = offset.saturating_sub(k);
        let steam_end = end.saturating_sub(k).min(steam_total);

        let prev_contra_len = contradicted.len();
        let prev_suppressed_len = suppressed_steam_ids.len();
        if end > k && steam_start < steam_end {
            let first_sp = (steam_start / ps_u64 + 1) as u32;
            let last_sp = ((steam_end - 1) / ps_u64 + 1) as u32;
            let missing_slice_pages: Vec<u32> = (first_sp..=last_sp)
                .filter(|sp| !steam_pages.contains_key(sp))
                .collect();
            if !missing_slice_pages.is_empty() {
                match self
                    .fetch_steam_pages_concurrent(base_request, missing_slice_pages, fetch_ctx)
                    .await
                {
                    Ok(fetched) => {
                        for (sp, raw_page) in fetched {
                            record_steam_page_metadata(
                                &raw_page,
                                fetch_ctx.app_id,
                                &mut seen_steam_ids,
                                &mut contradicted,
                            );
                            let overlaid = self
                                .apply_local_overlay(
                                    query,
                                    fetch_ctx.app_id,
                                    &raw_page.outcomes,
                                    true,
                                )
                                .await?;
                            suppressed_steam_ids
                                .extend(collect_suppressed_ids(&raw_page.outcomes, &overlaid));
                            overlaid_pages.insert(sp, overlaid);
                            steam_pages.insert(sp, raw_page);
                        }
                    }
                    Err(err) => return Ok(MergeOutcome::SteamUnavailable(err)),
                }
            }
        }

        // Recompute local partitions if later Steam slice pages introduced contradictions/suppressions.
        let (promoted_local, supplemental_local, promoted_ids) = if contradicted.len()
            > prev_contra_len
            || suppressed_steam_ids.len() > prev_suppressed_len
        {
            let (p, s, pids, dropped) = partition_local_items(
                &local_window.items,
                &contradicted,
                &suppressed_steam_ids,
                &steam_p1_ids,
                steam_p1_best_tier,
            );
            contradicted_dropped = dropped;
            (p, s, pids)
        } else {
            (promoted_local, supplemental_local, promoted_ids)
        };
        let k = promoted_local.len() as u64;

        let mut page_items = Vec::with_capacity(page_size as usize);
        let mut emitted_ids = HashSet::new();

        if offset < k {
            let start_idx = offset as usize;
            let end_idx = end.min(k) as usize;
            for item in &promoted_local[start_idx..end_idx] {
                if emitted_ids.insert(item.workshop_id) {
                    page_items.push(item.clone());
                }
            }
        }

        let steam_start = offset.saturating_sub(k);
        let steam_end = end.saturating_sub(k).min(steam_total);
        if end > k && steam_start < steam_end {
            let first_sp = (steam_start / ps_u64 + 1) as u32;
            let last_sp = ((steam_end - 1) / ps_u64 + 1) as u32;
            for sp in first_sp..=last_sp {
                let overlaid = if let Some(cached_overlaid) = overlaid_pages.get(&sp) {
                    cached_overlaid.as_slice()
                } else {
                    &[]
                };
                let page_base = u64::from(sp - 1) * ps_u64;
                let take_from = steam_start.saturating_sub(page_base) as usize;
                let take_to = (steam_end.saturating_sub(page_base) as usize).min(overlaid.len());
                if take_from < take_to {
                    for item in &overlaid[take_from..take_to] {
                        if !promoted_ids.contains(&item.workshop_id)
                            && emitted_ids.insert(item.workshop_id)
                        {
                            page_items.push(item.clone());
                            if page_items.len() >= page_size as usize {
                                break;
                            }
                        }
                    }
                }
                if page_items.len() >= page_size as usize {
                    break;
                }
            }
        }

        let mut excluded_for_tail = seen_steam_ids.clone();
        excluded_for_tail.extend(&promoted_ids);
        excluded_for_tail.extend(&emitted_ids);
        if let Some(eid) = exact_id {
            excluded_for_tail.insert(eid);
        }

        if page_items.len() < page_size as usize {
            let supp_skip = offset.saturating_sub(k.saturating_add(steam_total)) as usize;
            self.append_local_tail(
                LocalTailCursor {
                    query,
                    in_memory: &supplemental_local,
                    raw_local_loaded,
                    local_total: local_window.total,
                    contradicted: &contradicted,
                    excluded_ids: &excluded_for_tail,
                    to_skip: supp_skip,
                    page_size: page_size as usize,
                },
                &mut emitted_ids,
                &mut page_items,
            )
            .await?;
        }

        let overlap = local_window
            .items
            .iter()
            .filter(|l| {
                !is_local_contradicted(l, &contradicted)
                    && seen_steam_ids.contains(&l.record.workshop_id)
            })
            .count() as u64;
        let total = local_window
            .total
            .saturating_sub(contradicted_dropped)
            .saturating_add(steam_total)
            .saturating_sub(overlap);
        let total = if page_items.is_empty() {
            total
        } else {
            total.max(offset + page_items.len() as u64)
        };

        Ok(MergeOutcome::Ready(SearchResponse {
            items: page_items.into_iter().map(with_catalog_source).collect(),
            total,
            page,
            page_size,
            source: MetadataSource::CatalogServer,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    async fn merge_steam_order_windowed(
        &self,
        query: &SearchQuery,
        app_id: u32,
        page: u32,
        page_size: u32,
        offset: u64,
        initial_steam_page: WorkshopQueryPage,
        local_window: &RankedLocalSearch,
        raw_local_loaded: u64,
        exact_id: Option<WorkshopId>,
    ) -> anyhow::Result<SearchResponse> {
        let mut seen_steam_ids = HashSet::new();
        let mut contradicted = HashMap::new();
        record_steam_page_metadata(
            &initial_steam_page,
            app_id,
            &mut seen_steam_ids,
            &mut contradicted,
        );

        let steam_total = initial_steam_page
            .total
            .max(initial_steam_page.outcomes.len() as u64);
        let page_items = self
            .apply_local_overlay(query, app_id, &initial_steam_page.outcomes, true)
            .await?;
        let suppressed_steam_ids =
            collect_suppressed_ids(&initial_steam_page.outcomes, &page_items);
        let mut page_items = page_items;
        let mut emitted_ids: HashSet<WorkshopId> =
            page_items.iter().map(|i| i.workshop_id).collect();

        let mut visible_local = Vec::new();
        let mut contradicted_dropped = 0u64;
        for local in &local_window.items {
            if is_local_contradicted(local, &contradicted)
                || suppressed_steam_ids.contains(&local.record.workshop_id)
            {
                contradicted_dropped += 1;
            } else {
                visible_local.push(local.record.clone());
            }
        }

        let mut excluded_for_tail = seen_steam_ids.clone();
        excluded_for_tail.extend(&emitted_ids);
        if let Some(eid) = exact_id {
            excluded_for_tail.insert(eid);
        }

        if page_items.len() < page_size as usize {
            let supp_skip = offset.saturating_sub(steam_total) as usize;
            self.append_local_tail(
                LocalTailCursor {
                    query,
                    in_memory: &visible_local,
                    raw_local_loaded,
                    local_total: local_window.total,
                    contradicted: &contradicted,
                    excluded_ids: &excluded_for_tail,
                    to_skip: supp_skip,
                    page_size: page_size as usize,
                },
                &mut emitted_ids,
                &mut page_items,
            )
            .await?;
        }

        let overlap = visible_local
            .iter()
            .filter(|i| seen_steam_ids.contains(&i.workshop_id))
            .count() as u64;
        let total = local_window
            .total
            .saturating_sub(contradicted_dropped)
            .saturating_add(steam_total)
            .saturating_sub(overlap);
        let total = if page_items.is_empty() {
            total
        } else {
            total.max(offset + page_items.len() as u64)
        };

        Ok(SearchResponse {
            items: page_items.into_iter().map(with_catalog_source).collect(),
            total,
            page,
            page_size,
            source: MetadataSource::CatalogServer,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn merge_deterministic_windowed(
        self: &Arc<Self>,
        query: &SearchQuery,
        det_sort: DeterministicSort,
        page: u32,
        page_size: u32,
        offset: u64,
        base_request: &SteamSearchRequest,
        initial_steam_page: WorkshopQueryPage,
        local_window: &RankedLocalSearch,
        raw_local_loaded: u64,
        exact_id: Option<WorkshopId>,
        fetch_ctx: SteamFetchCtx,
    ) -> anyhow::Result<MergeOutcome> {
        let steam_total = initial_steam_page
            .total
            .max(initial_steam_page.outcomes.len() as u64);
        let end = offset + u64::from(page_size);

        let mut seen_steam_ids = HashSet::new();
        let mut contradicted = HashMap::new();

        let (page_items, contradicted_dropped, overlap) = if page <= MAX_DETERMINISTIC_MERGE_PAGES
            && end <= MAX_DETERMINISTIC_MERGE_ITEMS
        {
            let extra_pages: Vec<u32> = (1..page).collect();
            let mut steam_pages = if extra_pages.is_empty() {
                HashMap::new()
            } else {
                match self
                    .fetch_steam_pages_concurrent(base_request, extra_pages, fetch_ctx)
                    .await
                {
                    Ok(map) => map,
                    Err(err) => return Ok(MergeOutcome::SteamUnavailable(err)),
                }
            };
            steam_pages.insert(page, initial_steam_page);

            let mut overlaid_pages = Vec::new();
            let mut suppressed_steam_ids = HashSet::new();
            for sp in 1..=page {
                if let Some(raw) = steam_pages.get(&sp) {
                    record_steam_page_metadata(
                        raw,
                        fetch_ctx.app_id,
                        &mut seen_steam_ids,
                        &mut contradicted,
                    );
                    let overlaid = self
                        .apply_local_overlay(query, fetch_ctx.app_id, &raw.outcomes, true)
                        .await?;
                    suppressed_steam_ids.extend(collect_suppressed_ids(&raw.outcomes, &overlaid));
                    overlaid_pages.extend(overlaid);
                }
            }

            let mut merged: HashMap<WorkshopId, WorkshopItemRecord> = HashMap::new();
            let mut dropped = 0u64;
            let mut ov = 0u64;
            for local in &local_window.items {
                if is_local_contradicted(local, &contradicted)
                    || suppressed_steam_ids.contains(&local.record.workshop_id)
                {
                    dropped += 1;
                    continue;
                }
                if seen_steam_ids.contains(&local.record.workshop_id) {
                    ov += 1;
                }
                merged.insert(local.record.workshop_id, local.record.clone());
            }

            for item in overlaid_pages {
                merged.insert(item.workshop_id, item);
            }

            let mut all_items: Vec<WorkshopItemRecord> = merged.into_values().collect();
            sort_deterministic(&mut all_items, det_sort);
            let start = (offset as usize).min(all_items.len());
            let slice_end = (start + page_size as usize).min(all_items.len());
            (all_items[start..slice_end].to_vec(), dropped, ov)
        } else {
            record_steam_page_metadata(
                &initial_steam_page,
                fetch_ctx.app_id,
                &mut seen_steam_ids,
                &mut contradicted,
            );
            let mut items = self
                .apply_local_overlay(query, fetch_ctx.app_id, &initial_steam_page.outcomes, true)
                .await?;
            let suppressed_steam_ids = collect_suppressed_ids(&initial_steam_page.outcomes, &items);
            let mut emitted: HashSet<WorkshopId> = items.iter().map(|i| i.workshop_id).collect();
            let mut visible_local = Vec::new();
            let mut dropped = 0u64;
            let mut ov = 0u64;
            for local in &local_window.items {
                if is_local_contradicted(local, &contradicted)
                    || suppressed_steam_ids.contains(&local.record.workshop_id)
                {
                    dropped += 1;
                    continue;
                }
                if seen_steam_ids.contains(&local.record.workshop_id) {
                    ov += 1;
                }
                visible_local.push(local.record.clone());
            }
            let mut excluded = seen_steam_ids.clone();
            excluded.extend(&emitted);
            if let Some(eid) = exact_id {
                excluded.insert(eid);
            }
            if items.len() < page_size as usize {
                let supp_skip = offset.saturating_sub(steam_total) as usize;
                self.append_local_tail(
                    LocalTailCursor {
                        query,
                        in_memory: &visible_local,
                        raw_local_loaded,
                        local_total: local_window.total,
                        contradicted: &contradicted,
                        excluded_ids: &excluded,
                        to_skip: supp_skip,
                        page_size: page_size as usize,
                    },
                    &mut emitted,
                    &mut items,
                )
                .await?;
            }
            (items, dropped, ov)
        };

        let total = local_window
            .total
            .saturating_sub(contradicted_dropped)
            .saturating_add(steam_total)
            .saturating_sub(overlap);
        let total = if page_items.is_empty() {
            total
        } else {
            total.max(offset + page_items.len() as u64)
        };

        Ok(MergeOutcome::Ready(SearchResponse {
            items: page_items.into_iter().map(with_catalog_source).collect(),
            total,
            page,
            page_size,
            source: MetadataSource::CatalogServer,
        }))
    }

    async fn append_local_tail(
        &self,
        mut cursor: LocalTailCursor<'_>,
        emitted_ids: &mut HashSet<WorkshopId>,
        out: &mut Vec<WorkshopItemRecord>,
    ) -> anyhow::Result<()> {
        for item in cursor.in_memory {
            let wid = item.workshop_id;
            if cursor.excluded_ids.contains(&wid) || emitted_ids.contains(&wid) {
                continue;
            }
            if cursor.to_skip > 0 {
                cursor.to_skip -= 1;
                continue;
            }
            emitted_ids.insert(wid);
            out.push(item.clone());
            if out.len() >= cursor.page_size {
                return Ok(());
            }
        }

        let mut db_offset = cursor.raw_local_loaded;
        while out.len() < cursor.page_size && db_offset < cursor.local_total {
            let needed = cursor.page_size - out.len();
            let batch_limit = (cursor.to_skip + needed + 32).clamp(32, 200) as u32;
            let batch = self
                .fetch_local_range(cursor.query, db_offset, batch_limit)
                .await?;
            if batch.items.is_empty() {
                break;
            }
            db_offset = db_offset.saturating_add(batch.items.len() as u64);
            for local in batch.items {
                if is_local_contradicted(&local, cursor.contradicted) {
                    continue;
                }
                let wid = local.record.workshop_id;
                if cursor.excluded_ids.contains(&wid) || emitted_ids.contains(&wid) {
                    continue;
                }
                if cursor.to_skip > 0 {
                    cursor.to_skip -= 1;
                    continue;
                }
                emitted_ids.insert(wid);
                out.push(local.record);
                if out.len() >= cursor.page_size {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    async fn apply_local_overlay(
        &self,
        query: &SearchQuery,
        app_id: u32,
        observations: &[WorkshopObservation],
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

        let mut meta_map = HashMap::new();
        for chunk in ids.chunks(OVERLAY_BATCH_SIZE) {
            let chunk_ids = chunk.to_vec();
            let (stored, _) = self.db_call(move |r| r.get_batch(&chunk_ids)).await?;
            for meta in stored {
                meta_map.insert(meta.record.workshop_id, meta);
            }
        }

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

    async fn fetch_steam_pages_concurrent(
        self: &Arc<Self>,
        base_request: &SteamSearchRequest,
        pages: Vec<u32>,
        ctx: SteamFetchCtx,
    ) -> anyhow::Result<HashMap<u32, WorkshopQueryPage>> {
        let unique_pages: BTreeSet<u32> = pages.into_iter().collect();
        if unique_pages.is_empty() {
            return Ok(HashMap::new());
        }
        let mut join_set = JoinSet::new();
        for p in unique_pages {
            let svc = Arc::clone(self);
            let mut req = base_request.clone();
            req.page = p;
            join_set.spawn(async move {
                let res = svc.fetch_steam_page_cached(&req, ctx).await;
                (p, res)
            });
        }
        let mut out = HashMap::new();
        while let Some(joined) = join_set.join_next().await {
            match joined {
                Ok((p, Ok(page))) => {
                    out.insert(p, page);
                }
                Ok((_, Err(err))) => {
                    join_set.abort_all();
                    return Err(err);
                }
                Err(join_err) => {
                    join_set.abort_all();
                    return Err(anyhow::anyhow!("Steam CM 并发检索任务失败: {join_err}"));
                }
            }
        }
        Ok(out)
    }

    async fn fetch_steam_page_cached(
        self: &Arc<Self>,
        request: &SteamSearchRequest,
        ctx: SteamFetchCtx,
    ) -> anyhow::Result<WorkshopQueryPage> {
        if !ctx.cache_ttl.is_zero() {
            let mut cache = self.cache.lock().await;
            if let Some(entry) = cache.get(request) {
                if entry.cached_at.elapsed() < ctx.cache_ttl {
                    return Ok(entry.page.clone());
                }
                cache.remove(request);
            }
        }

        let wait_budget = remaining_timeout(ctx.deadline)?;

        let mut rx = {
            let mut map = self.inflight.lock().await;
            if let Some(existing) = map.get(request).filter(|rx| !rx.is_closed()) {
                existing.resubscribe()
            } else {
                let (tx, rx) = broadcast::channel(1);
                map.insert(request.clone(), rx.resubscribe());
                let svc = Arc::clone(self);
                let req = request.clone();
                tokio::spawn(async move {
                    let result =
                        tokio::time::timeout(ctx.steam_timeout, svc.upstream.search_files(&req))
                            .await
                            .map_err(|_| "Steam CM 实时搜索超时".to_string())
                            .and_then(|res| res.map_err(|err| err.to_string()));

                    if let Ok(ref page) = result {
                        if !ctx.cache_ttl.is_zero() {
                            svc.insert_cached_page(req.clone(), page.clone(), ctx.cache_ttl)
                                .await;
                        }
                        if ctx.cache_steam_results {
                            let warm: Vec<_> = page
                                .outcomes
                                .iter()
                                .filter(|obs| is_public_active_for_app(obs, ctx.app_id))
                                .cloned()
                                .collect();
                            svc.spawn_cache_warming(warm, req.language);
                        }
                    }

                    let mut map = svc.inflight.lock().await;
                    map.remove(&req);
                    let _ = tx.send(result);
                });
                rx
            }
        };

        let result = tokio::time::timeout(wait_budget, rx.recv())
            .await
            .map_err(|_| anyhow::anyhow!("Steam CM 实时搜索等待超时"))?
            .map_err(|err| anyhow::anyhow!("Steam CM 搜索并发通道异常: {err}"))?;
        result.map_err(anyhow::Error::msg)
    }

    async fn insert_cached_page(
        &self,
        request: SteamSearchRequest,
        page: WorkshopQueryPage,
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

    fn spawn_cache_warming(&self, observations: Vec<WorkshopObservation>, language: i32) {
        if observations.is_empty() {
            return;
        }
        let repo = Arc::clone(&self.repo);
        let policy = if language == 0 {
            CatalogIndexPolicy::CacheOnly
        } else {
            CatalogIndexPolicy::InsertOnlyCache
        };
        tokio::spawn(async move {
            let _ = tokio::task::spawn_blocking(move || {
                repo.apply_upstream_outcomes_with_policy(&observations, policy)
            })
            .await;
        });
    }
}

fn remaining_timeout(deadline: Instant) -> anyhow::Result<Duration> {
    let now = Instant::now();
    if now >= deadline {
        anyhow::bail!("Steam CM 检索预算耗尽");
    }
    Ok(deadline.saturating_duration_since(now))
}

fn record_steam_page_metadata(
    page: &WorkshopQueryPage,
    app_id: u32,
    raw_steam_ids: &mut HashSet<WorkshopId>,
    contradicted: &mut HashMap<WorkshopId, DateTime<Utc>>,
) {
    for obs in &page.outcomes {
        let wid = obs.outcome.workshop_id();
        raw_steam_ids.insert(wid);
        if !is_public_active_for_app(obs, app_id) {
            let obs_at = obs.observed_at();
            contradicted
                .entry(wid)
                .and_modify(|prev| {
                    if obs_at > *prev {
                        *prev = obs_at;
                    }
                })
                .or_insert(obs_at);
        }
    }
}

fn is_local_contradicted(
    local: &RankedLocalItem,
    contradicted: &HashMap<WorkshopId, DateTime<Utc>>,
) -> bool {
    !local.is_admin_override
        && contradicted
            .get(&local.record.workshop_id)
            .is_some_and(|&obs_at| obs_at >= local.last_verified_at)
}

fn collect_suppressed_ids(
    raw: &[WorkshopObservation],
    overlaid: &[WorkshopItemRecord],
) -> HashSet<WorkshopId> {
    let overlaid_ids: HashSet<WorkshopId> = overlaid.iter().map(|i| i.workshop_id).collect();
    raw.iter()
        .map(|obs| obs.outcome.workshop_id())
        .filter(|wid| !overlaid_ids.contains(wid))
        .collect()
}

fn partition_local_items(
    local_items: &[RankedLocalItem],
    contradicted: &HashMap<WorkshopId, DateTime<Utc>>,
    suppressed_steam_ids: &HashSet<WorkshopId>,
    steam_p1_ids: &HashSet<WorkshopId>,
    steam_p1_best_tier: Option<u8>,
) -> (
    Vec<WorkshopItemRecord>,
    Vec<WorkshopItemRecord>,
    HashSet<WorkshopId>,
    u64,
) {
    let mut promoted_local = Vec::new();
    let mut supplemental_local = Vec::new();
    let mut promoted_ids = HashSet::new();
    let mut contradicted_dropped = 0u64;

    for local_item in local_items {
        let wid = local_item.record.workshop_id;
        if is_local_contradicted(local_item, contradicted) || suppressed_steam_ids.contains(&wid) {
            contradicted_dropped += 1;
            continue;
        }
        if steam_p1_ids.contains(&wid) {
            continue;
        }
        let promote = local_item.record.status == WorkshopItemStatus::Active
            && match local_item.rank {
                Some(rank) if rank.tier == 0 => true,
                Some(rank) if local_item.is_admin_override && rank.tier <= 3 => true,
                Some(rank)
                    if rank.tier <= 5 && steam_p1_best_tier.is_none_or(|best| rank.tier < best) =>
                {
                    true
                }
                _ => false,
            };
        if promote {
            promoted_ids.insert(wid);
            promoted_local.push(local_item.record.clone());
        } else {
            supplemental_local.push(local_item.record.clone());
        }
    }

    (
        promoted_local,
        supplemental_local,
        promoted_ids,
        contradicted_dropped,
    )
}

fn extend_local_window(
    window: &mut RankedLocalSearch,
    extra: Vec<RankedLocalItem>,
    exact_id: Option<WorkshopId>,
) {
    for item in extra {
        if exact_id == Some(item.record.workshop_id) {
            continue;
        }
        window.items.push(item);
    }
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

fn inject_exact_item_into_local(local_window: &mut RankedLocalSearch, exact: WorkshopItemRecord) {
    let wid = exact.workshop_id;
    let last_verified_at = exact.fetched_at;
    let exact_entry = RankedLocalItem {
        record: exact,
        rank: Some(MatchRank {
            tier: 0,
            sub_rank: 0,
        }),
        is_admin_override: false,
        last_verified_at,
    };
    if let Some(pos) = local_window
        .items
        .iter()
        .position(|item| item.record.workshop_id == wid)
    {
        local_window.items.remove(pos);
        local_window.items.insert(0, exact_entry);
    } else {
        local_window.items.insert(0, exact_entry);
        local_window.total = local_window.total.saturating_add(1);
    }
}

fn slice_local_window(
    local_window: &RankedLocalSearch,
    page: u32,
    page_size: u32,
    offset: u64,
) -> SearchResponse {
    let start = (offset as usize).min(local_window.items.len());
    let end = (start + page_size as usize).min(local_window.items.len());
    let items = local_window.items[start..end]
        .iter()
        .map(|item| with_catalog_source(item.record.clone()))
        .collect();
    SearchResponse {
        items,
        total: local_window.total,
        page,
        page_size,
        source: MetadataSource::CatalogServer,
    }
}

fn sort_deterministic(items: &mut [WorkshopItemRecord], sort: DeterministicSort) {
    match sort {
        DeterministicSort::Subscriptions => {
            items.sort_by_key(|item| (Reverse(item.subscriptions.unwrap_or(0)), item.workshop_id))
        }
        DeterministicSort::Updated => items.sort_by_key(|item| {
            (
                Reverse(item.time_updated.unwrap_or(0)),
                Reverse(item.workshop_id),
            )
        }),
        DeterministicSort::Newest => items.sort_by_key(|item| {
            (
                Reverse(item.time_created.unwrap_or(0)),
                Reverse(item.workshop_id),
            )
        }),
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
