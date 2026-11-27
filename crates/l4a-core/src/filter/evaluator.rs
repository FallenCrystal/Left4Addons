use crate::db::repo::{
    is_remote_image_url, is_valid_local_cover_key, normalize_valid_remote_cover_url,
    strip_vpk_file_stem,
};
use crate::engine::{FilterCacheState, L4aEngine};
use crate::error::CoreError;
use crate::filter::deploy::{
    merge_raw_max, split_images_by_raw_budget, MAX_RAW_PREDICT_IMAGE_BATCH_BYTES,
};
use crate::filter::image_prep::{
    self, PreparedImage, FILTER_MAX_IMAGE_BYTES as MAX_INLINE_IMAGE_BYTES,
};
use crate::filter::llm::{is_loopback_url, judge_with_openai_llm_using_token, LoadedImage};
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;
use chrono::{TimeDelta, Utc};
use futures_util::stream::{self, StreamExt};
use l4a_logging::http::AuditedSend;
use l4a_protocol::{
    AddonId, AddonSummary, AppConfig, CatalogFilterPublicInfo, ClientFilterConfig,
    ClientFilterSourceMode, CoreEvent, FilterDecisionSource, FilterEvaluationBatchResponse,
    FilterEvaluationItemResult, FilterEvaluationTarget, FilterItemVerdict, FilterPredictRequest,
    FilterProbabilities, FilterScoreTarget, FilterTargetOutcome, RawFilterPredictRequest,
    CATALOG_FILTER_IMAGE_FETCH_FAILED, CATALOG_FILTER_MAX_BODY_BYTES,
    FILTER_REMOTE_VERDICT_MAX_AGE_SECS,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::MutexGuard;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const MAX_CATALOG_IMAGES: usize = 8;
const EVAL_CONCURRENCY: usize = 4;
const CATALOG_SCORING_INFO_TTL: Duration = Duration::from_secs(30);

pub(crate) fn remote_verdict_cache_max_age(cfg: &ClientFilterConfig) -> Option<TimeDelta> {
    match cfg.mode {
        ClientFilterSourceMode::Local => None,
        ClientFilterSourceMode::Catalog | ClientFilterSourceMode::CustomEndpoint => {
            Some(TimeDelta::seconds(FILTER_REMOTE_VERDICT_MAX_AGE_SECS))
        }
    }
}

pub fn collect_addon_image_sources(addon: &AddonSummary) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    let mut push_unique = |val: Option<&str>| {
        if let Some(trimmed) = val.map(str::trim).filter(|s| !s.is_empty()) {
            if !urls.iter().any(|u| u == trimmed) {
                urls.push(trimmed.to_string());
            }
        }
    };

    let primary = addon
        .cover_image
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            addon
                .local_cover_image
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        });
    push_unique(primary);
    for map in &addon.maps {
        push_unique(map.image_path.as_deref());
    }
    push_unique(addon.local_cover_image.as_deref());
    urls
}

fn addon_filter_title(addon: &AddonSummary) -> String {
    let file_stem = addon.current_file_name.as_deref().map(strip_vpk_file_stem);
    let mut titles = Vec::new();
    for title in [
        addon.workshop_title.as_deref(),
        addon.addon_title.as_deref(),
        Some(addon.title.as_str()),
        file_stem.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|title| !title.is_empty())
    {
        if !titles.contains(&title) {
            titles.push(title);
        }
    }
    titles.join("\n")
}

pub fn build_addon_filter_target(addon: &AddonSummary) -> FilterEvaluationTarget {
    let image_sources = collect_addon_image_sources(addon);
    let title = addon_filter_title(addon);
    let description = addon
        .addon_info
        .get("addondescription")
        .or_else(|| addon.addon_info.get("description"))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let revision_hint = build_addon_revision_hint(addon);

    FilterEvaluationTarget {
        target_key: format!("addon:{}", addon.id.as_str()),
        addon_id: Some(addon.id.clone()),
        workshop_id: addon.workshop_id,
        title,
        description,
        tags: addon.categories.clone(),
        image_sources,
        revision_hint: Some(revision_hint),
        bypass_cache: false,
    }
}

pub fn build_addon_revision_hint(addon: &AddonSummary) -> String {
    let image_sources = collect_addon_image_sources(addon);
    let title = addon_filter_title(addon);
    let description = addon
        .addon_info
        .get("addondescription")
        .or_else(|| addon.addon_info.get("description"))
        .map(|s| s.trim())
        .unwrap_or("");
    format!(
        "v2:{}:{}:{}:{}:{}:{}:{}",
        addon.content_hash.as_deref().unwrap_or(""),
        addon.local_time_updated.unwrap_or(0),
        addon.remote_time_updated.unwrap_or(0),
        l4a_protocol::fnv1a_hex(title.as_bytes()),
        l4a_protocol::fnv1a_hex(description.as_bytes()),
        addon.categories.join(","),
        image_sources.join("|")
    )
}

pub fn compute_addon_state_sig(addon: &AddonSummary) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"l4a-addon-state-v1\0");
    hasher.update(addon.id.as_str().as_bytes());
    hasher.update(b"\0");
    if let Some(wid) = addon.workshop_id {
        hasher.update(wid.as_u64().to_le_bytes());
    }
    hasher.update(b"\0");
    hasher.update(
        serde_json::to_string(&addon.install_state)
            .unwrap_or_default()
            .as_bytes(),
    );
    hasher.update(b"\0");
    hasher.update(addon.content_hash.as_deref().unwrap_or("").as_bytes());
    hasher.update(b"\0");
    hasher.update(addon.file_size.to_le_bytes());
    hasher.update(addon.local_time_updated.unwrap_or(-1).to_le_bytes());
    hasher.update(addon.remote_time_updated.unwrap_or(-1).to_le_bytes());
    hasher.update(addon.title.as_bytes());
    hasher.update(b"\0");
    hasher.update(addon.current_file_name.as_deref().unwrap_or("").as_bytes());
    hasher.update(b"\0");
    hasher.update(addon.addon_title.as_deref().unwrap_or("").as_bytes());
    hasher.update(b"\0");
    hasher.update(addon.workshop_title.as_deref().unwrap_or("").as_bytes());
    hasher.update(b"\0");
    let desc = addon
        .addon_info
        .get("addondescription")
        .or_else(|| addon.addon_info.get("description"))
        .map(String::as_str)
        .unwrap_or("");
    hasher.update(desc.as_bytes());
    hasher.update(b"\0");
    for cat in &addon.categories {
        hasher.update(cat.as_bytes());
        hasher.update(b"\x1f");
    }
    hasher.update(b"\0");
    hasher.update(addon.cover_image.as_deref().unwrap_or("").as_bytes());
    hasher.update(b"\0");
    hasher.update(addon.local_cover_image.as_deref().unwrap_or("").as_bytes());
    hasher.update(b"\0");
    for map in &addon.maps {
        hasher.update(map.code.as_bytes());
        hasher.update(b"\x1e");
        hasher.update(map.name.as_bytes());
        hasher.update(b"\x1e");
        hasher.update(map.image_path.as_deref().unwrap_or("").as_bytes());
        hasher.update(b"\x1e");
        hasher.update(map.image_hint.as_deref().unwrap_or("").as_bytes());
        hasher.update(b"\x1f");
    }

    hex::encode(hasher.finalize())
}

fn target_is_stale_for_addon(target: &FilterEvaluationTarget, addon: &AddonSummary) -> bool {
    if let Some(ref hint) = target.revision_hint {
        if hint.starts_with("v2:") {
            return hint != &build_addon_revision_hint(addon);
        }
        if target.target_key.starts_with("addon:") {
            if let (Some((hint_hash, _)), Some(ref current_hash)) =
                (hint.split_once(':'), &addon.content_hash)
            {
                if !hint_hash.is_empty() && hint_hash != current_hash {
                    return true;
                }
            }
        }
    }
    false
}

pub fn compute_content_signature(target: &FilterEvaluationTarget, max_images: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"l4a-filter-content-v2\0");
    hasher.update(target.target_key.as_bytes());
    hasher.update(b"\0");
    if let Some(ref aid) = target.addon_id {
        hasher.update(aid.as_str().as_bytes());
    }
    hasher.update(b"\0");
    if let Some(wid) = target.workshop_id {
        hasher.update(wid.as_u64().to_le_bytes());
    }
    hasher.update(b"\0");
    hasher.update(target.title.as_bytes());
    hasher.update(b"\0");
    hasher.update(target.description.as_deref().unwrap_or("").as_bytes());
    hasher.update(b"\0");
    for tag in &target.tags {
        hasher.update(tag.as_bytes());
        hasher.update(b"\x1f");
    }
    hasher.update(b"\0");
    hasher.update((target.image_sources.len() as u64).to_le_bytes());
    for (slot, img) in indexed_image_sources(target, max_images) {
        hasher.update((slot as u64).to_le_bytes());
        hasher.update(img.as_bytes());
        hasher.update(b"\x1f");
    }
    hasher.update(b"\0");
    hasher.update(target.revision_hint.as_deref().unwrap_or("").as_bytes());

    hex::encode(hasher.finalize())
}

fn normalize_catalog_base_key(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

#[derive(Default, Clone)]
pub(crate) struct CatalogScoringState {
    pub url: String,
    pub checked_at: Option<Instant>,
    pub revision: Option<String>,
    max_images: Option<usize>,
}

fn catalog_scoring_state(engine: &L4aEngine, cfg: &AppConfig) -> CatalogScoringState {
    let url = normalize_catalog_base_key(cfg.catalog_server_url.as_deref().unwrap_or_default());
    engine
        .catalog_scoring_cache
        .lock()
        .unwrap()
        .as_ref()
        .filter(|entry| entry.url == url)
        .cloned()
        .unwrap_or(CatalogScoringState {
            url,
            ..Default::default()
        })
}

fn record_catalog_observation(
    engine: &L4aEngine,
    cfg: &AppConfig,
    generation: u64,
    expected: &CatalogScoringState,
    revision: Option<&str>,
    info_max_images: Option<usize>,
) {
    let cache = engine.filter_cache.lock().unwrap();
    let live = engine.config();
    if cache.generation != generation
        || live.filter.mode != ClientFilterSourceMode::Catalog
        || live.https_only != cfg.https_only
        || normalize_catalog_base_key(live.catalog_server_url.as_deref().unwrap_or_default())
            != expected.url
    {
        return;
    }

    let changed = {
        let mut guard = engine.catalog_scoring_cache.lock().unwrap();
        let mut state = guard
            .as_ref()
            .filter(|entry| entry.url == expected.url)
            .cloned()
            .unwrap_or_else(|| CatalogScoringState {
                url: expected.url.clone(),
                ..Default::default()
            });
        // A late response may carry an older model revision. Only let a request
        // advance the revision it started with, or confirm the one now active.
        if state.revision != expected.revision && state.revision.as_deref() != revision {
            return;
        }
        let changed = revision.is_some() && revision != state.revision.as_deref();
        if changed {
            state.revision = revision.map(str::to_owned);
            state.checked_at = None;
            state.max_images = None;
        }
        if let Some(cap) = info_max_images {
            state.max_images = Some(cap.min(MAX_CATALOG_IMAGES));
            state.checked_at = Some(Instant::now());
        }
        *guard = Some(state);
        changed
    };

    if changed {
        let active_sig = engine.current_filter_policy_signature(&live);
        if let Err(err) = engine.db().prune_filter_verdicts_except(&active_sig) {
            tracing::warn!(error = %err, "Failed to clear stale cache after catalog server score version changed");
        }
        engine.emit_event(CoreEvent::SnapshotUpdated {
            external_change: false,
        });
    }
}

fn resolve_catalog_info_url(raw_base: &str) -> Result<String, CoreError> {
    let mut parsed = url::Url::parse(raw_base.trim())
        .map_err(|_| CoreError::InvalidInput("目录服务器地址不是有效的 URL".into()))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(CoreError::InvalidInput(
            "目录服务器地址仅支持不带内联凭据的 HTTP/HTTPS 协议".into(),
        ));
    }
    let clean_path = parsed.path().trim_end_matches('/').to_string();
    if let Some(prefix) = clean_path.strip_suffix("/api/v1/filter/predict") {
        parsed.set_path(&format!("{prefix}/api/v1/filter/info"));
    } else if clean_path.ends_with("/api/v1/filter/info") {
        parsed.set_path(&clean_path);
    } else {
        parsed.set_path(&format!("{clean_path}/api/v1/filter/info"));
    }
    parsed.set_fragment(None);
    Ok(parsed.into())
}

async fn ensure_catalog_scoring_revision_fresh(
    engine: &L4aEngine,
    app_cfg: &AppConfig,
    generation: u64,
) {
    let _probe = engine.catalog_scoring_probe.lock().await;
    let expected = catalog_scoring_state(engine, app_cfg);
    if expected.url.is_empty()
        || expected
            .checked_at
            .is_some_and(|at| at.elapsed() < CATALOG_SCORING_INFO_TTL)
    {
        return;
    }
    let Ok(info_url) = resolve_catalog_info_url(&expected.url) else {
        return;
    };
    let Ok(req) = engine.prepare_routed_get(&info_url, Duration::from_secs(5)) else {
        return;
    };

    if let Ok(resp) = req.send_audited("filter.catalog.info").await {
        if resp.status().is_success() {
            if let Ok(info) = resp.json::<CatalogFilterPublicInfo>().await {
                record_catalog_observation(
                    engine,
                    app_cfg,
                    generation,
                    &expected,
                    info.scoring_revision.as_deref(),
                    Some(info.max_images as usize),
                );
                return;
            }
        }
    }

    // Rate-limit info probes if an older Catalog has no /info endpoint.
    let cache = engine.filter_cache.lock().unwrap();
    if cache.generation == generation {
        let mut guard = engine.catalog_scoring_cache.lock().unwrap();
        if guard
            .as_ref()
            .is_none_or(|state| state.url == expected.url && state.revision == expected.revision)
        {
            *guard = Some(CatalogScoringState {
                checked_at: Some(Instant::now()),
                ..expected
            });
        }
    }
}

struct DeduplicatedMiss {
    indices: Vec<usize>,
    target: FilterEvaluationTarget,
    content_sig: String,
    expected_addon_sig: Option<String>,
    has_local_addon: bool,
    persist_cache: bool,
}

const POLICY_CHANGED: &str = "过滤评估策略已更新，请重试";

struct EvaluationPolicy {
    signature: String,
    cache_generation: u64,
    catalog: Option<CatalogScoringState>,
    max_images: usize,
    automatic: bool,
}

fn is_automatic_target_enabled(cfg: &ClientFilterConfig, target: &FilterEvaluationTarget) -> bool {
    cfg.enabled
        && (!target.target_key.starts_with("addon:") || cfg.apply_to_library)
        && (!target.target_key.starts_with("ws:") || cfg.apply_to_workshop)
}

impl EvaluationPolicy {
    fn lock_current<'a>(
        &self,
        engine: &'a L4aEngine,
        refresh_env_token: bool,
    ) -> Result<(MutexGuard<'a, FilterCacheState>, AppConfig), CoreError> {
        let mut cache = engine.filter_cache.lock().unwrap();
        let live = engine.config();
        if cache.generation != self.cache_generation
            || self.signature != engine.current_filter_policy_signature(&live)
        {
            return Err(CoreError::InvalidInput(POLICY_CHANGED.into()));
        }
        if refresh_env_token && engine.refresh_filter_env_token(&mut cache, &live.filter)? {
            drop(cache);
            engine.emit_workspace_update();
            return Err(CoreError::InvalidInput(POLICY_CHANGED.into()));
        }
        Ok((cache, live))
    }

    fn check_target(
        &self,
        engine: &L4aEngine,
        target: &FilterEvaluationTarget,
    ) -> Result<(), CoreError> {
        let (_cache, live) = self.lock_current(engine, false)?;
        if self.automatic && !is_automatic_target_enabled(&live.filter, target) {
            return Err(CoreError::InvalidInput("该范围的自动内容过滤已停用".into()));
        }
        Ok(())
    }

    fn resolve_llm_token(&self, engine: &L4aEngine) -> Result<Zeroizing<String>, CoreError> {
        let (mut cache, live) = self.lock_current(engine, false)?;
        let token = engine.resolve_filter_llm_token(&mut cache, &live.filter);
        if cache.generation != self.cache_generation {
            drop(cache);
            engine.emit_workspace_update();
            return Err(CoreError::InvalidInput(POLICY_CHANGED.into()));
        }
        token
    }
}

pub async fn evaluate_targets(
    engine: &L4aEngine,
    targets: Vec<FilterEvaluationTarget>,
    automatic: bool,
) -> Result<FilterEvaluationBatchResponse, CoreError> {
    let (generation, app_cfg, credential_changed) = {
        let mut cache = engine.filter_cache.lock().unwrap();
        let cfg = engine.config();
        let changed = engine.refresh_filter_env_token(&mut cache, &cfg.filter)?;
        (cache.generation, cfg, changed)
    };
    if credential_changed {
        engine.emit_workspace_update();
    }
    let is_catalog = app_cfg.filter.mode == ClientFilterSourceMode::Catalog;

    for attempt in 0..2 {
        if is_catalog && !targets.is_empty() {
            ensure_catalog_scoring_revision_fresh(engine, &app_cfg, generation).await;
        }
        let catalog = is_catalog.then(|| catalog_scoring_state(engine, &app_cfg));
        let policy = EvaluationPolicy {
            signature: app_cfg.filter.canonical_policy_signature(
                app_cfg.catalog_server_url.as_deref(),
                app_cfg.https_only,
                catalog.as_ref().and_then(|state| state.revision.as_deref()),
            ),
            cache_generation: generation,
            max_images: catalog
                .as_ref()
                .and_then(|state| state.max_images)
                .filter(|&cap| {
                    cap > 0 || !app_cfg.filter.llm_enabled || !app_cfg.filter.llm_send_images
                })
                .unwrap_or_else(|| effective_max_images(&app_cfg.filter)),
            catalog,
            automatic,
        };
        let items = evaluate_on_policy(engine, &app_cfg, &targets, &policy).await?;
        let mut cache = engine.filter_cache.lock().unwrap();
        let live = engine.config();
        let credential_changed = engine.refresh_filter_env_token(&mut cache, &live.filter)?;
        let current_policy = engine.current_filter_policy_signature(&live);

        if cache.generation == generation && current_policy == policy.signature {
            return Ok(FilterEvaluationBatchResponse {
                policy_signature: policy.signature,
                cache_generation: generation,
                items,
            });
        }

        let latest_catalog_sig = engine.current_filter_policy_signature(&app_cfg);
        if is_catalog
            && cache.generation == generation
            && attempt == 0
            && current_policy == latest_catalog_sig
            && policy.signature != latest_catalog_sig
        {
            // Re-evaluate the entire batch against one revision, including prior cache hits.
            continue;
        }
        if credential_changed {
            drop(cache);
            engine.emit_workspace_update();
        }

        return Ok(FilterEvaluationBatchResponse {
            policy_signature: policy.signature,
            cache_generation: generation,
            items: targets
                .into_iter()
                .map(|target| FilterEvaluationItemResult {
                    target_key: target.target_key,
                    outcome: FilterTargetOutcome::Failed {
                        error: POLICY_CHANGED.into(),
                    },
                })
                .collect(),
        });
    }
    unreachable!("at most one policy retry")
}

async fn evaluate_on_policy(
    engine: &L4aEngine,
    app_cfg: &AppConfig,
    targets: &[FilterEvaluationTarget],
    policy: &EvaluationPolicy,
) -> Result<Vec<FilterEvaluationItemResult>, CoreError> {
    let cfg = &app_cfg.filter;
    let max_age = remote_verdict_cache_max_age(cfg);
    let batch_started = std::time::Instant::now();

    let mut addons = HashMap::new();
    for id in targets.iter().filter_map(target_addon_id) {
        if let std::collections::hash_map::Entry::Vacant(entry) = addons.entry(id) {
            let addon = engine.db().get_addon(entry.key())?;
            entry.insert(addon);
        }
    }

    let mut target_keys: Vec<String> = Vec::with_capacity(targets.len());
    let mut ordered_outcomes: Vec<Option<FilterTargetOutcome>> = vec![None; targets.len()];
    let mut cached_hits: Vec<CachedHit<'_>> = Vec::new();
    let mut miss_groups: Vec<DeduplicatedMiss> = Vec::new();
    let mut miss_index_by_key: HashMap<(String, String), usize> = HashMap::new();

    for (idx, target) in targets.iter().cloned().enumerate() {
        target_keys.push(target.target_key.clone());

        if target.target_key.trim().is_empty() {
            let err = CoreError::InvalidInput("targetKey 不能为空".into());
            tracing::warn!(index = idx, error = %err, "Filter target is missing a valid targetKey");
            ordered_outcomes[idx] = Some(FilterTargetOutcome::Failed {
                error: err.to_string(),
            });
            continue;
        }

        if let Err(err) = policy.check_target(engine, &target) {
            ordered_outcomes[idx] = Some(FilterTargetOutcome::Failed {
                error: err.to_string(),
            });
            continue;
        }
        let matched_addon = target_addon_id(&target).and_then(|id| addons.get(&id)?.as_ref());
        let has_local_addon = matched_addon.is_some();
        let expected_addon_sig = matched_addon.and_then(|addon| {
            (!target_is_stale_for_addon(&target, addon)).then(|| compute_addon_state_sig(addon))
        });
        let stale_addon_target = has_local_addon && expected_addon_sig.is_none();

        let content_sig = compute_content_signature(&target, policy.max_images);
        if !target.bypass_cache && !stale_addon_target {
            if let Some(mut cached) = engine.db().get_cached_filter_verdict(
                &target.target_key,
                &content_sig,
                &policy.signature,
                expected_addon_sig.as_deref(),
                max_age,
            )? {
                cached.complete = true;
                cached_hits.push(CachedHit {
                    target: &targets[idx],
                    has_local_addon,
                    expected_addon_sig,
                    verdict: cached.clone(),
                });
                ordered_outcomes[idx] = Some(FilterTargetOutcome::Completed {
                    verdict: cached,
                    complete: true,
                });
                continue;
            }
        }

        let persist_this = !target.bypass_cache && !stale_addon_target;
        let dedup_key = (target.target_key.clone(), content_sig.clone());
        if let Some(&existing_group_idx) = miss_index_by_key.get(&dedup_key) {
            miss_groups[existing_group_idx].indices.push(idx);
            miss_groups[existing_group_idx].persist_cache |= persist_this;
        } else {
            let group_idx = miss_groups.len();
            miss_index_by_key.insert(dedup_key, group_idx);
            miss_groups.push(DeduplicatedMiss {
                indices: vec![idx],
                target,
                content_sig,
                expected_addon_sig,
                has_local_addon,
                persist_cache: persist_this,
            });
        }
    }

    let cache_hit_count = cached_hits.len();
    let miss_count = miss_groups.len();
    for hit in &cached_hits {
        tracing::debug!(
            target_key = %hit.target.target_key,
            flagged = hit.verdict.flagged,
            source = ?hit.verdict.decision_source,
            score = hit.verdict.evaluated_score,
            "Filter verdict cache hit"
        );
    }

    // Emit cache hits immediately so already-known cards render without waiting for uncached misses.
    emit_cached_verdicts_if_still_valid(engine, policy, cached_hits);

    if !miss_groups.is_empty() {
        let prestart_err = if cfg.mode == ClientFilterSourceMode::Local
            && engine.local_filter().active_endpoint().is_none()
            && cfg.auto_start_local_service
            && engine.local_filter().is_deployed()
        {
            engine
                .local_filter()
                .ensure_started_endpoint(&cfg.local_params.sanitized())
                .await
                .err()
        } else {
            None
        };

        if let Some(err) = prestart_err {
            tracing::warn!(error = %err, "Failed to pre-start local filter service");
            let err_msg = err.to_string();
            for group in miss_groups {
                for idx in group.indices {
                    ordered_outcomes[idx] = Some(FilterTargetOutcome::Failed {
                        error: err_msg.clone(),
                    });
                }
            }
        } else {
            let computed = run_deduplicated_misses(engine, app_cfg, miss_groups, policy).await;
            for (indices, outcome) in computed {
                for idx in indices {
                    ordered_outcomes[idx] = Some(outcome.clone());
                }
            }
        }
    }

    let (mut flagged, mut safe, mut failed) = (0usize, 0usize, 0usize);
    for outcome in &ordered_outcomes {
        match outcome {
            Some(FilterTargetOutcome::Completed { verdict, .. }) if verdict.flagged => flagged += 1,
            Some(FilterTargetOutcome::Completed { .. }) => safe += 1,
            _ => failed += 1,
        }
    }
    tracing::info!(
        mode = ?cfg.mode,
        automatic = policy.automatic,
        targets = targets.len(),
        cache_hits = cache_hit_count,
        evaluated = miss_count,
        flagged,
        safe,
        failed,
        elapsed_ms = batch_started.elapsed().as_millis() as u64,
        "Content filter batch completed"
    );

    Ok(target_keys
        .into_iter()
        .zip(ordered_outcomes)
        .map(|(target_key, outcome)| FilterEvaluationItemResult {
            target_key,
            outcome: outcome.unwrap_or_else(|| FilterTargetOutcome::Failed {
                error: "过滤评估未返回结果".into(),
            }),
        })
        .collect())
}

fn target_addon_id(target: &FilterEvaluationTarget) -> Option<AddonId> {
    target
        .target_key
        .strip_prefix("addon:")
        .filter(|id| !id.is_empty())
        .map(|id| AddonId(id.to_string()))
}

async fn run_deduplicated_misses(
    engine: &L4aEngine,
    app_cfg: &AppConfig,
    miss_groups: Vec<DeduplicatedMiss>,
    policy: &EvaluationPolicy,
) -> Vec<(Vec<usize>, FilterTargetOutcome)> {
    let mut pending = stream::iter(miss_groups)
        .map(|group| async move {
            let result = {
                let permit_res = engine.filter_eval_semaphore.acquire().await;
                let Ok(_permit) = permit_res else {
                    return (
                        group.indices,
                        FilterTargetOutcome::Failed {
                            error: "过滤评估并发信号量已关闭".into(),
                        },
                    );
                };
                let started = std::time::Instant::now();
                let result = evaluate_single_target(engine, app_cfg, &group.target, policy).await;
                (result, started.elapsed().as_millis() as u64)
            };
            let (result, elapsed_ms) = result;

            match result {
                Ok(verdict) => {
                    tracing::info!(
                        target_key = %group.target.target_key,
                        title = %l4a_logging::redact::truncate(&group.target.title, 80),
                        mode = ?app_cfg.filter.mode,
                        flagged = verdict.flagged,
                        source = ?verdict.decision_source,
                        score = verdict.evaluated_score,
                        complete = verdict.complete,
                        evaluated_images = verdict.evaluated_image_indices.len(),
                        sensitive_images = verdict.sensitive_image_indices.len(),
                        elapsed_ms,
                        "Content filter evaluation completed"
                    );
                    if verdict.complete {
                        persist_and_emit_verdict_if_still_valid(engine, policy, &group, &verdict);
                    }

                    (
                        group.indices,
                        FilterTargetOutcome::Completed {
                            complete: verdict.complete,
                            verdict,
                        },
                    )
                }
                Err(err) => {
                    tracing::warn!(
                        target_key = %group.target.target_key,
                        elapsed_ms,
                        error = %err,
                        "Content filter evaluation failed for item"
                    );
                    (
                        group.indices,
                        FilterTargetOutcome::Failed {
                            error: err.to_string(),
                        },
                    )
                }
            }
        })
        .buffer_unordered(EVAL_CONCURRENCY);

    let mut results = Vec::new();
    while let Some(item) = pending.next().await {
        results.push(item);
    }
    results
}

struct CachedHit<'a> {
    target: &'a FilterEvaluationTarget,
    has_local_addon: bool,
    expected_addon_sig: Option<String>,
    verdict: FilterItemVerdict,
}

fn local_addon_still_matches(
    engine: &L4aEngine,
    target: &FilterEvaluationTarget,
    has_local_addon: bool,
    expected_addon_sig: Option<&str>,
) -> bool {
    if !has_local_addon {
        return true;
    }
    target_addon_id(target)
        .and_then(|id| engine.db().get_addon(&id).ok().flatten())
        .zip(expected_addon_sig)
        .is_some_and(|(addon, sig)| compute_addon_state_sig(&addon) == sig)
}

fn emit_cached_verdicts_if_still_valid(
    engine: &L4aEngine,
    policy: &EvaluationPolicy,
    cached_hits: Vec<CachedHit<'_>>,
) {
    if cached_hits.is_empty() {
        return;
    }
    let has_llm_verdicts = cached_hits.iter().any(|hit| is_llm_verdict(&hit.verdict));
    let Ok((_cache, live)) = policy.lock_current(engine, has_llm_verdicts) else {
        return;
    };
    let mut seen_keys = HashSet::with_capacity(cached_hits.len());
    let verdicts: Vec<FilterItemVerdict> = cached_hits
        .into_iter()
        .filter(|hit| {
            (!policy.automatic || is_automatic_target_enabled(&live.filter, hit.target))
                && local_addon_still_matches(
                    engine,
                    hit.target,
                    hit.has_local_addon,
                    hit.expected_addon_sig.as_deref(),
                )
                && seen_keys.insert(hit.target.target_key.clone())
        })
        .map(|hit| hit.verdict)
        .collect();
    if !verdicts.is_empty() {
        engine.emit_event(CoreEvent::FilterVerdictsUpdated {
            verdicts,
            policy_signature: Some(policy.signature.clone()),
            cache_generation: policy.cache_generation,
        });
    }
}

fn persist_and_emit_verdict_if_still_valid(
    engine: &L4aEngine,
    policy: &EvaluationPolicy,
    group: &DeduplicatedMiss,
    verdict: &FilterItemVerdict,
) {
    let Ok((_cache, live)) = policy.lock_current(engine, is_llm_verdict(verdict)) else {
        return;
    };
    if policy.automatic && !is_automatic_target_enabled(&live.filter, &group.target) {
        return;
    }
    if !local_addon_still_matches(
        engine,
        &group.target,
        group.has_local_addon,
        group.expected_addon_sig.as_deref(),
    ) {
        return;
    }

    if group.persist_cache {
        if let Err(db_err) = engine.db().put_cached_filter_verdict(
            &group.target.target_key,
            &group.content_sig,
            group.expected_addon_sig.as_deref(),
            &policy.signature,
            &policy.signature,
            verdict,
        ) {
            tracing::warn!(
                target_key = %group.target.target_key,
                error = %db_err,
                "Failed to write filter verdict cache"
            );
        }
    }

    engine.emit_event(CoreEvent::FilterVerdictsUpdated {
        verdicts: vec![verdict.clone()],
        policy_signature: Some(policy.signature.clone()),
        cache_generation: policy.cache_generation,
    });
}

fn is_llm_verdict(verdict: &FilterItemVerdict) -> bool {
    matches!(
        verdict.decision_source,
        FilterDecisionSource::LlmFlaggedTrue | FilterDecisionSource::LlmFlaggedFalse
    )
}

fn effective_max_images(cfg: &ClientFilterConfig) -> usize {
    if cfg.mode == ClientFilterSourceMode::Catalog {
        return MAX_CATALOG_IMAGES;
    }
    let sanitized = cfg.local_params.sanitized();
    if cfg.mode == ClientFilterSourceMode::Local
        && !sanitized.enable_image
        && !(cfg.llm_enabled && cfg.llm_send_images)
    {
        return 0;
    }
    sanitized.max_images.clamp(1, 8) as usize
}

#[derive(Debug, Clone)]
enum ValidatedRemoteImage {
    SteamCdn(String),
    LoopbackTest(String),
    Mirrored(String),
}

enum CatalogImagePlan {
    RemoteUrls {
        slot_indices: Vec<usize>,
        image_urls: Vec<String>,
        remotes: Vec<(usize, ValidatedRemoteImage)>,
        had_load_error: bool,
    },
    InlineBase64,
}

#[derive(Debug, Clone)]
struct CustomEndpointUsed {
    url: String,
    is_raw_predict: bool,
}

async fn evaluate_single_target(
    engine: &L4aEngine,
    app_cfg: &AppConfig,
    target: &FilterEvaluationTarget,
    policy: &EvaluationPolicy,
) -> Result<FilterItemVerdict, CoreError> {
    policy.check_target(engine, target)?;
    let cfg = &app_cfg.filter;
    let max_images = policy.max_images;

    let mut had_load_error;
    let mut loaded_images: Vec<LoadedImage> = Vec::new();
    let mut loaded_slot_indices: Vec<usize> = Vec::new();
    let mut deferred_remotes: Vec<(usize, ValidatedRemoteImage)> = Vec::new();
    let submitted_slot_indices: Vec<usize>;

    let (mut probs, custom_endpoint) = if cfg.mode == ClientFilterSourceMode::Catalog {
        let catalog = policy
            .catalog
            .as_ref()
            .expect("Catalog mode has a scoring snapshot");
        let model_max_images = catalog.max_images.unwrap_or(max_images);
        let base = app_cfg
            .catalog_server_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::Config("未配置目录服务器地址 (catalogServerUrl)".into()))?;
        let url = resolve_filter_endpoint_url(
            base,
            "/api/v1/filter/predict",
            &["/api/v1/filter/predict"],
            "目录服务器地址",
        )?;

        let mut req = FilterPredictRequest {
            workshop_id: target.workshop_id,
            title: Some(target.title.clone()),
            description: target.description.clone(),
            tags: target.tags.clone(),
            ..Default::default()
        };
        let plan = prepare_catalog_images(engine, target, model_max_images);
        let remote_result = if let CatalogImagePlan::RemoteUrls {
            slot_indices,
            image_urls,
            remotes,
            had_load_error: prep_err,
        } = plan
        {
            deferred_remotes = remotes;
            req.image_urls = image_urls;
            if model_max_images > 0 && req.image_urls.is_empty() && !target.image_sources.is_empty()
            {
                req.workshop_id = None;
            }
            policy.check_target(engine, target)?;
            match post_catalog_predict(engine, &url, &req).await {
                Ok(probs) => Some((probs, slot_indices, prep_err)),
                Err(PredictHttpOutcome::ImageFetch(_)) if !req.image_urls.is_empty() => None,
                Err(err) => return Err(err.into_core_error(&url)),
            }
        } else {
            None
        };

        let probs = match remote_result {
            Some((probs, slots, missing)) => {
                submitted_slot_indices = slots;
                had_load_error = missing;
                probs
            }
            None => {
                req.image_urls.clear();
                let (loaded, load_err) =
                    resolve_catalog_inline_images(engine, target, model_max_images, &req).await?;
                had_load_error = load_err;
                deferred_remotes.clear();
                submitted_slot_indices = loaded.iter().map(|(slot, _)| *slot).collect();
                loaded_slot_indices = submitted_slot_indices.clone();
                if loaded.is_empty() && !target.image_sources.is_empty() {
                    req.workshop_id = None;
                }
                req.images_base64 = loaded.iter().map(|(_, img)| img.b64.clone()).collect();
                loaded_images = loaded.into_iter().map(|(_, img)| img).collect();
                policy.check_target(engine, target)?;
                post_catalog_predict(engine, &url, &req)
                    .await
                    .map_err(|err| err.into_core_error(&url))?
            }
        };

        let returned_revision = probs.scoring_revision.as_deref();
        record_catalog_observation(
            engine,
            app_cfg,
            policy.cache_generation,
            catalog,
            returned_revision,
            None,
        );
        if returned_revision != catalog.revision.as_deref() {
            return Err(CoreError::Network(POLICY_CHANGED.into()));
        }

        (probs, None)
    } else {
        let (imgs, load_err) = resolve_target_images(engine, target, max_images).await;
        had_load_error = load_err;
        submitted_slot_indices = imgs.iter().map(|(idx, _)| *idx).collect();
        loaded_slot_indices = submitted_slot_indices.clone();
        let images_b64: Vec<String> = imgs.iter().map(|(_, img)| img.b64.clone()).collect();
        loaded_images = imgs.into_iter().map(|(_, img)| img).collect();
        policy.check_target(engine, target)?;
        let (probs, custom_ep) =
            fetch_non_catalog_probabilities(engine, app_cfg, target, &images_b64).await?;
        (probs, custom_ep)
    };

    let evaluated_score = probs.score_for(cfg.score_target);
    let direct_flag_score = match cfg.score_target {
        FilterScoreTarget::Overall => probs.fusion_score(),
        _ => evaluated_score,
    };
    let flag_threshold = if cfg.flag_threshold.is_finite() {
        cfg.flag_threshold.clamp(0.0, 1.0)
    } else {
        0.72
    };
    let llm_threshold = if cfg.llm_threshold.is_finite() {
        cfg.llm_threshold.clamp(0.0, 1.0)
    } else {
        0.20
    };

    let direct_flag = cfg.enable_direct_flag && direct_flag_score >= flag_threshold;
    let (flagged, decision_source) = if direct_flag {
        (true, FilterDecisionSource::ModelDirectFlag)
    } else if cfg.llm_enabled && evaluated_score >= llm_threshold {
        if let Some(catalog) = policy
            .catalog
            .as_ref()
            .filter(|_| cfg.llm_send_images && loaded_images.is_empty())
        {
            let (materialized, mat_err) = if catalog.max_images == Some(0) {
                resolve_target_images(engine, target, max_images).await
            } else {
                materialize_deferred_remotes_for_llm(engine, &deferred_remotes).await
            };
            had_load_error |= mat_err;
            loaded_slot_indices = materialized.iter().map(|(slot, _)| *slot).collect();
            loaded_images = materialized.into_iter().map(|(_, img)| img).collect();
        }
        policy.check_target(engine, target)?;
        let is_sensitive = judge_with_openai_llm_using_token(
            engine,
            cfg,
            || policy.resolve_llm_token(engine),
            &target.title,
            &target.tags,
            target.description.as_deref(),
            &loaded_images,
        )
        .await?;
        if is_sensitive {
            (true, FilterDecisionSource::LlmFlaggedTrue)
        } else {
            (false, FilterDecisionSource::LlmFlaggedFalse)
        }
    } else {
        (false, FilterDecisionSource::ModelSafe)
    };

    // If CustomEndpoint returned raw Left4Filters output without unimodal breakdown,
    // issue follow-up unimodal /predict calls when flagged & subdivide_modalities is true.
    if flagged && cfg.subdivide_modalities {
        if let Some(ref ep) = custom_endpoint {
            policy.check_target(engine, target)?;
            let images_b64: Vec<String> = loaded_images.iter().map(|img| img.b64.clone()).collect();
            populate_missing_unimodal_scores(engine, ep, target, &images_b64, &mut probs).await;
        }
    }

    let llm_evaluated_image_slots: &[usize] = if matches!(
        decision_source,
        FilterDecisionSource::LlmFlaggedTrue | FilterDecisionSource::LlmFlaggedFalse
    ) && cfg.llm_send_images
        && !loaded_slot_indices.is_empty()
    {
        &loaded_slot_indices
    } else {
        &[]
    };
    let llm_flagged_with_images = decision_source == FilterDecisionSource::LlmFlaggedTrue
        && !llm_evaluated_image_slots.is_empty();

    let evaluated_image_indices = compute_evaluated_image_indices(
        cfg,
        &submitted_slot_indices,
        &probs,
        llm_evaluated_image_slots,
    );

    let (
        text_sensitive,
        title_sensitive,
        description_sensitive,
        image_sensitive,
        sensitive_image_indices,
    ) = compute_modality_subdivision_inner(
        cfg,
        target,
        &evaluated_image_indices,
        &probs,
        flagged,
        llm_flagged_with_images,
    );

    Ok(FilterItemVerdict {
        target_key: target.target_key.clone(),
        revision_hint: target.revision_hint.clone(),
        flagged,
        decision_source,
        evaluated_score,
        probabilities: probs,
        text_sensitive,
        title_sensitive,
        description_sensitive,
        image_sensitive,
        evaluated_image_indices,
        sensitive_image_indices,
        complete: !had_load_error,
        evaluated_at: Utc::now(),
    })
}

fn compute_evaluated_image_indices(
    cfg: &ClientFilterConfig,
    submitted_slot_indices: &[usize],
    probs: &FilterProbabilities,
    llm_evaluated_image_slots: &[usize],
) -> Vec<usize> {
    let model_evaluated = if submitted_slot_indices.is_empty()
        || (cfg.mode == ClientFilterSourceMode::Local && !cfg.local_params.sanitized().enable_image)
    {
        Vec::new()
    } else if !probs.per_image_probs.is_empty() {
        let count = probs
            .per_image_probs
            .len()
            .min(submitted_slot_indices.len());
        submitted_slot_indices[..count].to_vec()
    } else if probs.text_prob.is_some() && probs.image_prob.is_none() {
        // If the backend returned unimodal text breakdown with `image_prob == None` and empty
        // `per_image_probs`, server-side image evaluation was disabled.
        Vec::new()
    } else {
        submitted_slot_indices.to_vec()
    };

    if !model_evaluated.is_empty() {
        model_evaluated
    } else {
        llm_evaluated_image_slots.to_vec()
    }
}

fn indexed_image_sources(
    target: &FilterEvaluationTarget,
    max_images: usize,
) -> impl Iterator<Item = (usize, &str)> {
    target
        .image_sources
        .iter()
        .enumerate()
        .map(|(slot, src)| (slot, src.trim()))
        .filter(|(_, src)| !src.is_empty())
        .take(max_images)
}

fn prepare_catalog_images(
    engine: &L4aEngine,
    target: &FilterEvaluationTarget,
    max_images: usize,
) -> CatalogImagePlan {
    let requires_inline_mode = indexed_image_sources(target, max_images).any(|(_, src)| {
        is_valid_local_cover_key(src)
            || has_ascii_prefix_ignore_case(src, b"data:")
            || matches!(
                validate_remote_filter_image_url(engine, src),
                Ok(ValidatedRemoteImage::Mirrored(_))
            )
    });
    if requires_inline_mode {
        return CatalogImagePlan::InlineBase64;
    }

    let mut slot_indices = Vec::new();
    let mut image_urls = Vec::new();
    let mut remotes = Vec::new();
    let mut had_load_error = false;

    for (slot_idx, trimmed) in indexed_image_sources(target, max_images) {
        if is_remote_image_url(trimmed) {
            match validate_remote_filter_image_url(engine, trimmed) {
                Ok(
                    remote @ (ValidatedRemoteImage::SteamCdn(_)
                    | ValidatedRemoteImage::LoopbackTest(_)),
                ) => {
                    let url_str = match &remote {
                        ValidatedRemoteImage::SteamCdn(u)
                        | ValidatedRemoteImage::LoopbackTest(u) => u.clone(),
                        ValidatedRemoteImage::Mirrored(_) => unreachable!(),
                    };
                    slot_indices.push(slot_idx);
                    image_urls.push(url_str);
                    remotes.push((slot_idx, remote));
                }
                Ok(ValidatedRemoteImage::Mirrored(_)) => return CatalogImagePlan::InlineBase64,
                Err(err) => {
                    had_load_error = true;
                    tracing::debug!(source = %trimmed, error = %err, "Rejected unsafe remote filter preview image URL");
                }
            }
        } else {
            had_load_error = true;
            tracing::debug!(source = %trimmed, "Skipping unsupported filter preview image source format");
        }
    }

    CatalogImagePlan::RemoteUrls {
        slot_indices,
        image_urls,
        remotes,
        had_load_error,
    }
}

async fn resolve_catalog_inline_images(
    engine: &L4aEngine,
    target: &FilterEvaluationTarget,
    max_images: usize,
    request_without_images: &FilterPredictRequest,
) -> Result<(Vec<(usize, LoadedImage)>, bool), CoreError> {
    // Base64 uses only JSON-safe ASCII. Account for the new field, quotes and commas.
    let base_json =
        serialize_catalog_request(request_without_images).map_err(|err| err.into_core_error(""))?;
    let mut remaining = CATALOG_FILTER_MAX_BODY_BYTES
        .saturating_sub(base_json.len() + b",\"imagesBase64\":[]".len());
    drop(base_json);
    let mut loaded = Vec::new();
    let mut missing = false;
    for (slot, src) in indexed_image_sources(target, max_images) {
        match load_single_image(engine, src).await {
            Ok(img) if img.b64.len() + 3 <= remaining => {
                remaining -= img.b64.len() + 3;
                loaded.push((slot, img));
            }
            Ok(_) => missing = true,
            Err(err) => {
                missing = true;
                tracing::debug!(slot, error = %err, "Failed to load catalog filter preview image");
            }
        }
    }
    Ok((loaded, missing))
}

async fn materialize_deferred_remotes_for_llm(
    engine: &L4aEngine,
    remotes: &[(usize, ValidatedRemoteImage)],
) -> (Vec<(usize, LoadedImage)>, bool) {
    let mut out = Vec::with_capacity(remotes.len());
    let mut had_error = false;
    for (slot, remote) in remotes {
        match fetch_validated_remote_image(engine, remote).await {
            Ok(img) => out.push((*slot, img)),
            Err(err) => {
                had_error = true;
                tracing::debug!(slot = *slot, error = %err, "Failed to download remote preview image during LLM review");
            }
        }
    }
    (out, had_error)
}

async fn resolve_target_images(
    engine: &L4aEngine,
    target: &FilterEvaluationTarget,
    max_images: usize,
) -> (Vec<(usize, LoadedImage)>, bool) {
    if max_images == 0 || target.image_sources.is_empty() {
        return (Vec::new(), false);
    }

    let mut loaded = Vec::with_capacity(max_images.min(target.image_sources.len()));
    let mut had_load_error = false;
    for (slot_idx, raw_src) in target.image_sources.iter().enumerate() {
        if loaded.len() >= max_images {
            break;
        }
        let trimmed = raw_src.trim();
        if trimmed.is_empty() {
            continue;
        }
        match load_single_image(engine, trimmed).await {
            Ok(img) => loaded.push((slot_idx, img)),
            Err(err) => {
                had_load_error = true;
                tracing::debug!(
                    slot = slot_idx,
                    source = %trimmed,
                    error = %err,
                    "Skipping filter preview image that failed to load"
                );
            }
        }
    }
    (loaded, had_load_error)
}

fn has_ascii_prefix_ignore_case(s: &str, prefix: &[u8]) -> bool {
    s.as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn strip_ascii_prefix_ignore_case<'a>(s: &'a str, prefix: &[u8]) -> Option<&'a str> {
    if has_ascii_prefix_ignore_case(s, prefix) {
        s.get(prefix.len()..)
    } else {
        None
    }
}

pub(crate) fn is_allowed_steam_cdn_image_url(parsed: &url::Url) -> bool {
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
    {
        return false;
    }
    let Some(raw_host) = parsed.host_str() else {
        return false;
    };
    let host = raw_host.trim_end_matches('.').to_ascii_lowercase();
    if host == "media.steampowered.com"
        || host
            .strip_suffix(".steamusercontent.com")
            .is_some_and(|sub| !sub.is_empty() && !sub.starts_with('.') && !sub.ends_with('.'))
        || host
            .strip_suffix(".steamstatic.com")
            .is_some_and(|sub| !sub.is_empty() && !sub.starts_with('.') && !sub.ends_with('.'))
    {
        return true;
    }
    if let Some(sub) = host.strip_suffix(".steampowered.com") {
        let is_cloud_host = sub == "cloud"
            || sub.starts_with("cloud-")
            || sub.strip_prefix("cloud").is_some_and(|digits| {
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            });
        if !sub.contains('.') && is_cloud_host && parsed.path().starts_with("/ugc/") {
            return true;
        }
    }
    if let Some(prefix) = host.strip_suffix(".akamaihd.net") {
        if !prefix.contains('.')
            && (matches!(prefix, "steamuserimages" | "steamcdn")
                || prefix.starts_with("steamuserimages-")
                || prefix.starts_with("steamcdn-"))
        {
            return true;
        }
    }
    false
}

fn validate_remote_filter_image_url(
    engine: &L4aEngine,
    src: &str,
) -> Result<ValidatedRemoteImage, CoreError> {
    let normalized = normalize_valid_remote_cover_url(src)
        .ok_or_else(|| CoreError::InvalidInput("无效的远程预览图地址".into()))?;
    let mut parsed = url::Url::parse(&normalized)
        .map_err(|_| CoreError::InvalidInput("无效的远程预览图 URL".into()))?;

    if is_allowed_steam_cdn_image_url(&parsed) {
        if parsed.scheme() == "http" {
            let _ = parsed.set_scheme("https");
        }
        return Ok(ValidatedRemoteImage::SteamCdn(parsed.into()));
    }

    if engine.allow_loopback_image_urls_for_testing() && is_loopback_url(&normalized) {
        return Ok(ValidatedRemoteImage::LoopbackTest(normalized));
    }

    // Allow non-Steam URLs only if explicitly rewritten by a configured MirrorRouter rule
    let https_only = engine.config().https_only;
    let resolved = engine
        .mirror_router()
        .resolve_with_https_only(&normalized, https_only);
    if resolved.matched_rule {
        engine
            .mirror_router()
            .validate_url_with_https_only(&normalized, https_only)
            .map_err(|e| CoreError::InvalidInput(e.to_string()))?;
        return Ok(ValidatedRemoteImage::Mirrored(normalized));
    }

    Err(CoreError::InvalidInput(
        "远程预览图地址不在受信任的 Steam 创意工坊 CDN 白名单内".into(),
    ))
}

pub(crate) fn validate_filter_image_redirect_hop(
    hop: &l4a_mirror::RedirectHop<'_>,
) -> Result<(), CoreError> {
    if !hop.url.username().is_empty() || hop.url.password().is_some() {
        return Err(CoreError::InvalidInput(
            "预览图重定向地址不得包含内联凭据".into(),
        ));
    }
    // Explicitly configured mirror origin (same-origin hop on that mirror, or cross-origin
    // hop matching another explicit mirror rule): MirrorRouter has already enforced https_only.
    if hop.matched_mirror_rule {
        return Ok(());
    }
    // Unmirrored hops (same-origin or cross-origin) must stay strictly inside the trusted
    // public Steam CDN allowlist and use HTTPS (blocking private/loopback SSRF and HTTP downgrades).
    if is_allowed_steam_cdn_image_url(hop.url) && hop.resolved.url.starts_with("https://") {
        return Ok(());
    }
    Err(CoreError::InvalidInput(
        "预览图重定向地址不在受信任的 Steam 创意工坊 CDN 白名单内".into(),
    ))
}

async fn fetch_validated_remote_image(
    engine: &L4aEngine,
    remote: &ValidatedRemoteImage,
) -> Result<LoadedImage, CoreError> {
    let (mime, bytes) = match remote {
        ValidatedRemoteImage::LoopbackTest(url) => fetch_loopback_image(engine, url).await?,
        ValidatedRemoteImage::SteamCdn(url) | ValidatedRemoteImage::Mirrored(url) => {
            engine.fetch_filter_preview(url).await?
        }
    };
    finish_filter_image(mime, bytes).await
}

pub(crate) async fn load_single_image(
    engine: &L4aEngine,
    src: &str,
) -> Result<LoadedImage, CoreError> {
    if is_valid_local_cover_key(src) {
        let bytes = engine.read_cached_image(src)?;
        let mime = detect_image_mime(&bytes, Some(src))?;
        return finish_filter_image(mime, bytes).await;
    }

    if has_ascii_prefix_ignore_case(src, b"data:") {
        let (mime, bytes) = decode_data_image_url(src)?;
        return finish_filter_image(mime, bytes).await;
    }

    if is_remote_image_url(src) {
        let remote = validate_remote_filter_image_url(engine, src)?;
        return fetch_validated_remote_image(engine, &remote).await;
    }

    Err(CoreError::InvalidInput("不受支持的图片来源格式".into()))
}

/// Downscales images beyond the filter runner's pixel limits (which it would otherwise
/// reject as decompression bombs) and base64-encodes the result.
async fn finish_filter_image(mime: String, bytes: Vec<u8>) -> Result<LoadedImage, CoreError> {
    let (mime, bytes) = match image_prep::prepare_for_filter(bytes)
        .await
        .map_err(|err| CoreError::InvalidInput(err.to_string()))?
    {
        PreparedImage::Original(bytes) => (mime, bytes),
        PreparedImage::Shrunk(shrunk) => {
            tracing::info!(
                source_width = shrunk.source_size.0,
                source_height = shrunk.source_size.1,
                width = shrunk.output_size.0,
                height = shrunk.output_size.1,
                "Downscaled oversized preview image before content filtering"
            );
            (image_prep::SHRUNK_IMAGE_MIME.to_string(), shrunk.bytes)
        }
    };
    Ok(LoadedImage {
        mime,
        b64: STANDARD.encode(&bytes),
    })
}

pub(crate) fn decode_data_image_url(src: &str) -> Result<(String, Vec<u8>), CoreError> {
    let Some(rest) = strip_ascii_prefix_ignore_case(src.trim(), b"data:") else {
        return Err(CoreError::InvalidInput("无效的 data URL".into()));
    };
    let Some((meta, b64_part)) = rest.split_once(',') else {
        return Err(CoreError::InvalidInput("无效的 data URL".into()));
    };
    let mut meta_parts = meta.split(';');
    let raw_mime = meta_parts.next().unwrap_or("").trim();
    let Some(subtype) = strip_ascii_prefix_ignore_case(raw_mime, b"image/") else {
        return Err(CoreError::InvalidInput(
            "data URL 必须是 base64 编码的图片".into(),
        ));
    };
    let valid_subtype = !subtype.is_empty()
        && subtype
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'));
    let is_b64 = meta_parts.any(|p| p.trim().eq_ignore_ascii_case("base64"));
    if !valid_subtype || !is_b64 {
        return Err(CoreError::InvalidInput(
            "data URL 必须是 base64 编码的图片".into(),
        ));
    }
    let clean_b64: String = b64_part.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = STANDARD
        .decode(&clean_b64)
        .or_else(|_| STANDARD_NO_PAD.decode(&clean_b64))
        .map_err(|_| CoreError::InvalidInput("解析 data URL base64 失败".into()))?;
    if bytes.is_empty() || bytes.len() > MAX_INLINE_IMAGE_BYTES {
        return Err(CoreError::InvalidInput("内联图片为空或超出大小限制".into()));
    }
    let mime = detect_image_mime(&bytes, None)?;
    Ok((mime, bytes))
}

fn detect_image_mime(bytes: &[u8], filename_hint: Option<&str>) -> Result<String, CoreError> {
    if let Ok(fmt) = image::guess_format(bytes) {
        return Ok(fmt.to_mime_type().to_string());
    }
    if let Some(name) = filename_hint {
        match name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
        {
            Some(ext) if ext == "jpg" || ext == "jpeg" => return Ok("image/jpeg".into()),
            Some(ext) if ext == "png" => return Ok("image/png".into()),
            Some(ext) if ext == "webp" => return Ok("image/webp".into()),
            _ => {}
        }
    }
    Err(CoreError::InvalidInput("无法识别图片文件格式".into()))
}

async fn fetch_loopback_image(
    engine: &L4aEngine,
    url: &str,
) -> Result<(String, Vec<u8>), CoreError> {
    let mut resp = engine
        .loopback_http_client()
        .get(url)
        .timeout(Duration::from_secs(15))
        .send_audited_quiet("filter.image.loopback")
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| CoreError::Network(e.to_string()))?;
    let mut bytes = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| CoreError::Network(e.to_string()))?
    {
        if bytes.len() + chunk.len() > MAX_INLINE_IMAGE_BYTES {
            return Err(CoreError::InvalidInput("预览图片体积过大".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Err(CoreError::InvalidInput("预览图片为空".into()));
    }
    let fmt = image::guess_format(&bytes)
        .map_err(|_| CoreError::InvalidInput("无法识别预览图格式".into()))?;
    Ok((fmt.to_mime_type().to_string(), bytes))
}

pub(crate) fn resolve_filter_endpoint_url(
    raw_base: &str,
    append_suffix: &str,
    preserve_if_ends_with: &[&str],
    label: &str,
) -> Result<String, CoreError> {
    let mut parsed = url::Url::parse(raw_base.trim())
        .map_err(|_| CoreError::InvalidInput(format!("{label}不是有效的 URL")))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(CoreError::InvalidInput(format!(
            "{label}仅支持不带内联凭据的 HTTP/HTTPS 协议"
        )));
    }
    let clean_path = parsed.path().trim_end_matches('/').to_string();
    if preserve_if_ends_with
        .iter()
        .any(|sfx| clean_path.ends_with(sfx))
    {
        parsed.set_path(&clean_path);
    } else {
        parsed.set_path(&format!("{clean_path}{append_suffix}"));
    }
    parsed.set_fragment(None);
    Ok(parsed.into())
}

async fn fetch_non_catalog_probabilities(
    engine: &L4aEngine,
    app_cfg: &AppConfig,
    target: &FilterEvaluationTarget,
    images_b64: &[String],
) -> Result<(FilterProbabilities, Option<CustomEndpointUsed>), CoreError> {
    let cfg = &app_cfg.filter;
    match cfg.mode {
        ClientFilterSourceMode::Local => {
            let sanitized_params = cfg.local_params.sanitized();
            let probs = engine
                .local_filter()
                .predict_local(
                    &target.title,
                    target.description.as_deref(),
                    &target.tags,
                    images_b64,
                    cfg.auto_start_local_service,
                    &sanitized_params,
                )
                .await?;
            Ok((probs, None))
        }
        ClientFilterSourceMode::Catalog => unreachable!("handled in evaluate_single_target"),
        ClientFilterSourceMode::CustomEndpoint => {
            let base = cfg
                .custom_endpoint_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    CoreError::Config("未配置自定义过滤服务端点 (customEndpointUrl)".into())
                })?;

            let parsed_base = url::Url::parse(base)
                .map_err(|_| CoreError::InvalidInput("自定义过滤服务端点不是有效的 URL".into()))?;
            let clean_path = parsed_base.path().trim_end_matches('/');

            if clean_path.ends_with("/api/v1/filter/predict") {
                let direct_catalog_url = resolve_filter_endpoint_url(
                    base,
                    "",
                    &["/api/v1/filter/predict"],
                    "自定义过滤服务端点",
                )?;
                let req = FilterPredictRequest {
                    workshop_id: target.workshop_id,
                    title: Some(target.title.clone()),
                    description: target.description.clone(),
                    tags: target.tags.clone(),
                    image_base64: None,
                    images_base64: images_b64.to_vec(),
                    image_urls: Vec::new(),
                };
                let probs = post_catalog_predict(engine, &direct_catalog_url, &req)
                    .await
                    .map_err(|err| err.into_core_error(&direct_catalog_url))?;
                return Ok((
                    probs,
                    Some(CustomEndpointUsed {
                        url: direct_catalog_url,
                        is_raw_predict: false,
                    }),
                ));
            }

            if clean_path.ends_with("/predict") {
                let direct_raw_url =
                    resolve_filter_endpoint_url(base, "", &["/predict"], "自定义过滤服务端点")?;
                let probs =
                    post_raw_predict_subbatched(engine, &direct_raw_url, target, images_b64)
                        .await?;
                return Ok((
                    probs,
                    Some(CustomEndpointUsed {
                        url: direct_raw_url,
                        is_raw_predict: true,
                    }),
                ));
            }

            let catalog_style_url = resolve_filter_endpoint_url(
                base,
                "/api/v1/filter/predict",
                &[],
                "自定义过滤服务端点",
            )?;
            let catalog_req = FilterPredictRequest {
                workshop_id: target.workshop_id,
                title: Some(target.title.clone()),
                description: target.description.clone(),
                tags: target.tags.clone(),
                image_base64: None,
                images_base64: images_b64.to_vec(),
                image_urls: Vec::new(),
            };
            match post_catalog_predict(engine, &catalog_style_url, &catalog_req).await {
                Ok(probs) => Ok((
                    probs,
                    Some(CustomEndpointUsed {
                        url: catalog_style_url,
                        is_raw_predict: false,
                    }),
                )),
                Err(PredictHttpOutcome::NotFound) => {
                    let raw_predict_url =
                        resolve_filter_endpoint_url(base, "/predict", &[], "自定义过滤服务端点")?;
                    let probs =
                        post_raw_predict_subbatched(engine, &raw_predict_url, target, images_b64)
                            .await?;
                    Ok((
                        probs,
                        Some(CustomEndpointUsed {
                            url: raw_predict_url,
                            is_raw_predict: true,
                        }),
                    ))
                }
                Err(PredictHttpOutcome::PayloadTooLarge(size_err)) => {
                    let raw_predict_url =
                        resolve_filter_endpoint_url(base, "/predict", &[], "自定义过滤服务端点")?;
                    match post_raw_predict_subbatched(engine, &raw_predict_url, target, images_b64)
                        .await
                    {
                        Ok(probs) => Ok((
                            probs,
                            Some(CustomEndpointUsed {
                                url: raw_predict_url,
                                is_raw_predict: true,
                            }),
                        )),
                        Err(_) => Err(size_err),
                    }
                }
                Err(err) => Err(err.into_core_error(&catalog_style_url)),
            }
        }
    }
}

async fn post_raw_predict_subbatched(
    engine: &L4aEngine,
    url: &str,
    target: &FilterEvaluationTarget,
    images_b64: &[String],
) -> Result<FilterProbabilities, CoreError> {
    let sub_batches = split_images_by_raw_budget(images_b64, MAX_RAW_PREDICT_IMAGE_BATCH_BYTES);
    let mut parts: Vec<(usize, FilterProbabilities)> = Vec::with_capacity(sub_batches.len());
    for sub_imgs in sub_batches {
        let count = sub_imgs.len();
        let req = RawFilterPredictRequest {
            title: Some(target.title.clone()),
            description: target.description.clone(),
            tags: target.tags.clone(),
            image_base64: None,
            images_base64: sub_imgs,
        };
        let mut probs = post_predict_request(engine, url, &req).await?;
        probs.infer_stock_corpse_review(count);
        parts.push((count, probs));
    }
    Ok(merge_raw_max(parts))
}

enum PredictHttpOutcome {
    NotFound,
    PayloadTooLarge(CoreError),
    ImageFetch(CoreError),
    Other(CoreError),
}

impl PredictHttpOutcome {
    fn into_core_error(self, url: &str) -> CoreError {
        match self {
            Self::NotFound => CoreError::NotFound(format!("过滤预测接口不存在 (404): {url}")),
            Self::PayloadTooLarge(err) | Self::ImageFetch(err) | Self::Other(err) => err,
        }
    }
}

fn serialize_catalog_request(req: &FilterPredictRequest) -> Result<Vec<u8>, PredictHttpOutcome> {
    let body = serde_json::to_vec(req)
        .map_err(|e| PredictHttpOutcome::Other(CoreError::InvalidInput(e.to_string())))?;
    if body.len() > CATALOG_FILTER_MAX_BODY_BYTES {
        return Err(PredictHttpOutcome::PayloadTooLarge(
            CoreError::InvalidInput("内容过滤请求超过 Catalog 的 48 MiB 体积限制".into()),
        ));
    }
    Ok(body)
}

async fn post_catalog_predict(
    engine: &L4aEngine,
    url: &str,
    request: &FilterPredictRequest,
) -> Result<FilterProbabilities, PredictHttpOutcome> {
    let body = serialize_catalog_request(request)?;
    let builder = engine
        .prepare_routed_post(url, Duration::from_secs(30))
        .map_err(PredictHttpOutcome::Other)?;
    send_predict_request(
        builder
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body),
    )
    .await
}

async fn post_predict_with_status<T: Serialize + ?Sized>(
    engine: &L4aEngine,
    url: &str,
    req_body: &T,
) -> Result<FilterProbabilities, PredictHttpOutcome> {
    let builder = engine
        .prepare_routed_post(url, Duration::from_secs(30))
        .map_err(PredictHttpOutcome::Other)?;
    send_predict_request(builder.json(req_body)).await
}

async fn send_predict_request(
    builder: reqwest::RequestBuilder,
) -> Result<FilterProbabilities, PredictHttpOutcome> {
    let mut resp = builder
        .send_audited("filter.predict")
        .await
        .map_err(|e| PredictHttpOutcome::Other(CoreError::Network(e.to_string())))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(PredictHttpOutcome::NotFound);
    }
    if !status.is_success() {
        let err = CoreError::Network(format!("过滤服务端点返回 HTTP {status}"));
        if status == reqwest::StatusCode::BAD_REQUEST {
            let mut body = Vec::new();
            while let Ok(Some(chunk)) = resp.chunk().await {
                if body.len() + chunk.len() > 4096 {
                    break;
                }
                body.extend_from_slice(&chunk);
            }
            if serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_owned))
                .as_deref()
                == Some(CATALOG_FILTER_IMAGE_FETCH_FAILED)
            {
                return Err(PredictHttpOutcome::ImageFetch(err));
            }
        }
        return Err(PredictHttpOutcome::Other(err));
    }
    resp.json::<FilterProbabilities>()
        .await
        .map_err(|e| PredictHttpOutcome::Other(CoreError::Network(e.to_string())))
}

async fn post_predict_request<T: Serialize + ?Sized>(
    engine: &L4aEngine,
    url: &str,
    req_body: &T,
) -> Result<FilterProbabilities, CoreError> {
    post_predict_with_status(engine, url, req_body)
        .await
        .map_err(|err| err.into_core_error(url))
}

async fn post_unimodal_predict(
    engine: &L4aEngine,
    ep: &CustomEndpointUsed,
    title: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
    images_base64: Vec<String>,
) -> Result<FilterProbabilities, CoreError> {
    if ep.is_raw_predict {
        let req = RawFilterPredictRequest {
            title,
            description,
            tags,
            image_base64: None,
            images_base64,
        };
        let mut probs = post_predict_request(engine, &ep.url, &req).await?;
        probs.infer_stock_corpse_review(req.images_base64.len());
        Ok(probs)
    } else {
        let req = FilterPredictRequest {
            workshop_id: None,
            title,
            description,
            tags,
            image_base64: None,
            images_base64,
            image_urls: Vec::new(),
        };
        post_predict_request(engine, &ep.url, &req).await
    }
}

async fn populate_missing_unimodal_scores(
    engine: &L4aEngine,
    ep: &CustomEndpointUsed,
    target: &FilterEvaluationTarget,
    images_b64: &[String],
    probs: &mut FilterProbabilities,
) {
    let has_title = !target.title.trim().is_empty() || !target.tags.is_empty();
    let has_desc = target
        .description
        .as_deref()
        .is_some_and(|d| !d.trim().is_empty());
    let has_text = has_title || has_desc;

    let server_omitted_text_scores = (probs.image_prob.is_some()
        || !probs.per_image_probs.is_empty())
        && probs.text_prob.is_none()
        && probs.title_prob.is_none()
        && probs.description_prob.is_none();
    let server_omitted_image_scores = (probs.text_prob.is_some()
        || probs.title_prob.is_some()
        || probs.description_prob.is_some())
        && probs.image_prob.is_none()
        && probs.per_image_probs.is_empty();
    let image_only_zero_response = !has_text
        && probs.prob == 0.0
        && probs.gore_prob == 0.0
        && probs.nsfw_prob == 0.0
        && probs.image_prob.is_none()
        && probs.per_image_probs.is_empty();

    if has_text && probs.text_prob.is_none() && !server_omitted_text_scores {
        if let Ok(tp) = post_unimodal_predict(
            engine,
            ep,
            Some(target.title.clone()),
            target.description.clone(),
            target.tags.clone(),
            Vec::new(),
        )
        .await
        {
            probs.text_prob = Some(tp.prob);
            probs.text_gore_prob = Some(tp.gore_prob);
            probs.text_nsfw_prob = Some(tp.nsfw_prob);
        }

        if has_title && probs.title_prob.is_none() {
            if !has_desc {
                probs.title_prob = probs.text_prob;
            } else if let Ok(tr) = post_unimodal_predict(
                engine,
                ep,
                Some(target.title.clone()),
                None,
                target.tags.clone(),
                Vec::new(),
            )
            .await
            {
                probs.title_prob = Some(tr.prob);
            }
        }

        if has_desc && probs.description_prob.is_none() {
            if !has_title {
                probs.description_prob = probs.text_prob;
            } else if let Ok(dr) = post_unimodal_predict(
                engine,
                ep,
                None,
                target.description.clone(),
                Vec::new(),
                Vec::new(),
            )
            .await
            {
                probs.description_prob = Some(dr.prob);
            }
        }
    }

    if !images_b64.is_empty()
        && (probs.image_prob.is_none() || probs.per_image_probs.is_empty())
        && !server_omitted_image_scores
        && !image_only_zero_response
    {
        let mut per_img = Vec::with_capacity(images_b64.len());
        let mut max_ov = 0.0_f32;
        let mut max_gore = 0.0_f32;
        let mut max_nsfw = 0.0_f32;
        let mut corpse_indices = Vec::new();
        let mut all_ok = true;
        for (index, b64) in images_b64.iter().enumerate() {
            match post_unimodal_predict(engine, ep, None, None, Vec::new(), vec![b64.clone()]).await
            {
                Ok(ir) => {
                    let score = ir.fusion_score();
                    per_img.push(score);
                    max_ov = max_ov.max(score);
                    max_gore = max_gore.max(ir.gore_prob);
                    max_nsfw = max_nsfw.max(ir.nsfw_prob);
                    if ir.corpse_review_floor.is_some() {
                        corpse_indices.push(index);
                    }
                }
                Err(_) => {
                    all_ok = false;
                    break;
                }
            }
        }
        if all_ok && !per_img.is_empty() {
            if probs.image_prob.is_none() {
                probs.image_prob = Some(max_ov);
                probs.image_gore_prob = Some(max_gore);
                probs.image_nsfw_prob = Some(max_nsfw);
            }
            if probs.per_image_probs.is_empty() {
                probs.per_image_probs = per_img;
            }
            if probs.corpse_review_floor.is_some() && probs.corpse_image_indices.is_empty() {
                probs.corpse_image_indices = corpse_indices;
            }
        }
    }
}

pub fn compute_modality_subdivision(
    cfg: &ClientFilterConfig,
    target: &FilterEvaluationTarget,
    evaluated_slot_indices: &[usize],
    probs: &FilterProbabilities,
    flagged: bool,
) -> (bool, bool, bool, bool, Vec<usize>) {
    compute_modality_subdivision_inner(cfg, target, evaluated_slot_indices, probs, flagged, false)
}

fn compute_modality_subdivision_inner(
    cfg: &ClientFilterConfig,
    target: &FilterEvaluationTarget,
    evaluated_slot_indices: &[usize],
    probs: &FilterProbabilities,
    flagged: bool,
    llm_flagged_with_images: bool,
) -> (bool, bool, bool, bool, Vec<usize>) {
    if !flagged {
        return (false, false, false, false, Vec::new());
    }

    let has_title =
        !target.title.trim().is_empty() || !target.tags.is_empty() || probs.title_prob.is_some();
    let has_desc = target
        .description
        .as_deref()
        .is_some_and(|d| !d.trim().is_empty())
        || probs.description_prob.is_some();
    let has_text = has_title || has_desc;
    let n_imgs = evaluated_slot_indices.len();
    let map_slot = |submitted_idx: usize| -> Option<usize> {
        evaluated_slot_indices.get(submitted_idx).copied()
    };

    if !cfg.subdivide_modalities {
        let text_sensitive = has_text || n_imgs == 0;
        let title_sensitive = has_title || (!has_desc && text_sensitive);
        let description_sensitive = has_desc;
        let image_sensitive = n_imgs > 0 || !target.image_sources.is_empty();
        let sensitive_image_indices = if !target.image_sources.is_empty() {
            (0..target.image_sources.len()).collect()
        } else {
            evaluated_slot_indices.to_vec()
        };
        return (
            text_sensitive,
            title_sensitive,
            description_sensitive,
            image_sensitive,
            sensitive_image_indices,
        );
    }

    let m_t = if cfg.modality_threshold.is_finite() {
        cfg.modality_threshold.clamp(0.05, 0.95)
    } else {
        0.30
    };

    let mut title_sensitive = has_title && probs.title_prob.is_some_and(|p| p >= m_t);
    let mut description_sensitive = has_desc && probs.description_prob.is_some_and(|p| p >= m_t);
    let mut text_sensitive = has_text
        && (probs.text_prob.is_some_and(|p| p >= m_t) || title_sensitive || description_sensitive);

    let scored_len = probs.per_image_probs.len().min(n_imgs);
    let mut sensitive_image_indices: Vec<usize> = probs.per_image_probs[..scored_len]
        .iter()
        .enumerate()
        .filter_map(|(idx, &p)| (p >= m_t).then(|| map_slot(idx)).flatten())
        .collect();
    if cfg.score_target == FilterScoreTarget::Overall {
        sensitive_image_indices.extend(
            probs
                .corpse_image_indices
                .iter()
                .filter_map(|&i| map_slot(i)),
        );
        sensitive_image_indices.sort_unstable();
        sensitive_image_indices.dedup();
    }

    let mut image_sensitive = n_imgs > 0
        && (probs.image_prob.is_some_and(|p| p >= m_t) || !sensitive_image_indices.is_empty());

    // When the model did not score images (e.g., local/upstream `enable_image = false`)
    // but the LLM reviewed the images and flagged the item, conservatively mark those
    // images as sensitive so they remain blurred rather than leaking unblurred.
    if llm_flagged_with_images
        && probs.image_prob.is_none()
        && probs.per_image_probs.is_empty()
        && n_imgs > 0
    {
        image_sensitive = true;
    }

    // Ensure a flagged item always identifies at least one sensitive modality
    if !text_sensitive && !image_sensitive {
        let has_any_unimodal = probs.text_prob.is_some()
            || probs.image_prob.is_some()
            || !probs.per_image_probs.is_empty();

        if !has_any_unimodal {
            text_sensitive = has_text || n_imgs == 0;
            image_sensitive = n_imgs > 0;
        } else if has_text && n_imgs == 0 {
            text_sensitive = true;
        } else if !has_text && n_imgs > 0 {
            image_sensitive = true;
        } else if has_text && n_imgs > 0 {
            let tp = probs
                .text_prob
                .or(probs.title_prob)
                .or(probs.description_prob)
                .unwrap_or(0.0);
            let ip = probs.image_prob.unwrap_or_else(|| {
                probs.per_image_probs[..scored_len]
                    .iter()
                    .copied()
                    .fold(0.0_f32, f32::max)
            });

            if tp >= 0.15 || ip >= 0.15 {
                if tp >= 0.15 {
                    text_sensitive = true;
                }
                if ip >= 0.15 {
                    image_sensitive = true;
                }
            } else if ip >= tp {
                image_sensitive = true;
            } else {
                text_sensitive = true;
            }
            // If neither modality crossed `m_t` on the local model and the multimodal LLM
            // reviewed the images before flagging, conservatively blur the reviewed images too.
            if llm_flagged_with_images {
                image_sensitive = true;
            }
        } else {
            text_sensitive = true;
        }
    }

    if text_sensitive && !title_sensitive && !description_sensitive {
        match (has_title, has_desc) {
            (true, false) => title_sensitive = true,
            (false, true) => description_sensitive = true,
            (true, true) => match (probs.title_prob, probs.description_prob) {
                (Some(tp), Some(dp)) => {
                    if tp >= 0.15 || dp >= 0.15 {
                        title_sensitive = tp >= 0.15;
                        description_sensitive = dp >= 0.15;
                    } else if tp >= dp {
                        title_sensitive = true;
                    } else {
                        description_sensitive = true;
                    }
                }
                (Some(_), None) => title_sensitive = true,
                (None, Some(_)) => description_sensitive = true,
                (None, None) => {
                    title_sensitive = true;
                    description_sensitive = true;
                }
            },
            (false, false) => title_sensitive = true,
        }
    }

    if image_sensitive && sensitive_image_indices.is_empty() && n_imgs > 0 {
        if scored_len > 0 {
            let over_floor: Vec<usize> = probs.per_image_probs[..scored_len]
                .iter()
                .enumerate()
                .filter_map(|(i, &p)| (p >= 0.15).then(|| map_slot(i)).flatten())
                .collect();
            if !over_floor.is_empty() {
                sensitive_image_indices = over_floor;
            } else if let Some((best_idx, _)) = probs.per_image_probs[..scored_len]
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
            {
                if let Some(orig) = map_slot(best_idx) {
                    sensitive_image_indices.push(orig);
                }
            }
        } else {
            sensitive_image_indices = evaluated_slot_indices.to_vec();
        }
    }

    (
        text_sensitive,
        title_sensitive,
        description_sensitive,
        image_sensitive,
        sensitive_image_indices,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_cdn_allowlist_accepts_trusted_hosts_and_rejects_ssrf_targets() {
        for valid in [
            "https://images.steamusercontent.com/ugc/123/abc/",
            "http://steamuserimages-a.akamaihd.net/ugc/123/abc.jpg",
            "https://steamcdn-a.akamaihd.net/steamcommunity/public/images/items/550/a.jpg",
            "https://cdn.cloudflare.steamstatic.com/steam/apps/550/header.jpg",
            "https://media.steampowered.com/steamcommunity/public/images/avatars/aa/bb.jpg",
            "http://cloud-3.steampowered.com/ugc/999/preview.jpg",
            "http://cloud.steampowered.com/ugc/999/preview.jpg",
        ] {
            let parsed = url::Url::parse(valid).unwrap();
            assert!(
                is_allowed_steam_cdn_image_url(&parsed),
                "expected {valid} to be allowed"
            );
        }

        for rejected in [
            "https://steamusercontent.com/ugc/123/a.jpg",
            "https://steamstatic.com/steam/apps/550/header.jpg",
            "https://evil-steamusercontent.com/ugc/123/a.jpg",
            "https://evil.example.com/a.jpg",
            "http://127.0.0.1:8080/a.jpg",
            "http://169.254.169.254/latest/meta-data/",
            "http://192.168.1.1/a.jpg",
            "https://10.0.0.1/a.jpg",
            "http://cloud.steampowered.com/non-ugc/preview.jpg",
            "https://images.steamusercontent.com:8443/ugc/123/a.jpg",
            "https://user:pass@images.steamusercontent.com/ugc/123/a.jpg",
        ] {
            let parsed = url::Url::parse(rejected).unwrap();
            assert!(
                !is_allowed_steam_cdn_image_url(&parsed),
                "expected {rejected} to be rejected"
            );
        }
    }

    #[test]
    fn endpoint_url_append_preserves_query_parameters() {
        assert_eq!(
            resolve_filter_endpoint_url(
                "https://filter.example.com/base?token=abc&v=2",
                "/api/v1/filter/predict",
                &["/api/v1/filter/predict"],
                "endpoint"
            )
            .unwrap(),
            "https://filter.example.com/base/api/v1/filter/predict?token=abc&v=2"
        );
        assert_eq!(
            resolve_filter_endpoint_url(
                "https://filter.example.com/predict/?token=abc",
                "",
                &["/predict"],
                "endpoint"
            )
            .unwrap(),
            "https://filter.example.com/predict?token=abc"
        );
    }

    #[test]
    fn malformed_multibyte_utf8_data_urls_rejected_without_panicking() {
        for malformed in [
            "",
            "中文",
            "中",
            "data:中文;base64,AAA",
            "data:中;base64,AAA",
            "data:a中文",
            "data:image/中文;base64,AAA",
            "DATA:中文;base64,AAA=",
            "data:image/png;中文,AAA=",
        ] {
            assert!(
                decode_data_image_url(malformed).is_err(),
                "expected {malformed:?} to return Err without panicking"
            );
        }
    }

    #[test]
    fn compute_content_signature_skips_empty_image_sources_before_max_images() {
        let base = FilterEvaluationTarget {
            target_key: "ws:1".into(),
            addon_id: None,
            workshop_id: None,
            title: "Sample".into(),
            description: None,
            tags: vec![],
            image_sources: vec!["".into(), "  https://example.com/a.jpg  ".into()],
            revision_hint: None,
            bypass_cache: false,
        };
        let mut changed = base.clone();
        changed.image_sources = vec!["".into(), "https://example.com/b.jpg".into()];

        assert_ne!(
            compute_content_signature(&base, 1),
            compute_content_signature(&changed, 1),
            "empty image_sources must be skipped before taking max_images"
        );

        let mut normalized = base.clone();
        normalized.image_sources[1] = "https://example.com/a.jpg".into();
        assert_eq!(
            compute_content_signature(&base, 1),
            compute_content_signature(&normalized, 1)
        );
        normalized.image_sources.swap(0, 1);
        assert_ne!(
            compute_content_signature(&base, 1),
            compute_content_signature(&normalized, 1),
            "cached verdicts use the original image slot indices"
        );
    }

    #[test]
    fn modality_subdivision_honors_catalog_backfilled_description_and_llm_image_fallback() {
        let cfg = ClientFilterConfig {
            subdivide_modalities: true,
            modality_threshold: 0.30,
            ..Default::default()
        };

        // 1. Catalog DB backfilled description when local target.description is None
        let target_no_local_desc = FilterEvaluationTarget {
            target_key: "addon:ws:100".into(),
            addon_id: Some(l4a_protocol::AddonId("ws:100".into())),
            workshop_id: Some(l4a_protocol::WorkshopId(100)),
            title: "Benign Local Title".into(),
            description: None,
            tags: vec![],
            image_sources: vec![],
            revision_hint: None,
            bypass_cache: false,
        };
        let probs_backfilled_desc = FilterProbabilities {
            prob: 0.91,
            gore_prob: 0.02,
            nsfw_prob: 0.91,
            text_prob: Some(0.91),
            title_prob: Some(0.03),
            description_prob: Some(0.91),
            ..Default::default()
        };
        let (text_s, title_s, desc_s, img_s, _) = compute_modality_subdivision_inner(
            &cfg,
            &target_no_local_desc,
            &[],
            &probs_backfilled_desc,
            true,
            false,
        );
        assert!(text_s);
        assert!(!title_s);
        assert!(
            desc_s,
            "Catalog-backfilled description_prob >= m_t must set description_sensitive"
        );
        assert!(!img_s);

        // 2. LLM flagged with images when neither text nor image crossed m_t on the small model
        let target_with_img = FilterEvaluationTarget {
            target_key: "ws:101".into(),
            addon_id: None,
            workshop_id: Some(l4a_protocol::WorkshopId(101)),
            title: "Borderline Title".into(),
            description: None,
            tags: vec![],
            image_sources: vec!["https://images.steamusercontent.com/ugc/1/a.jpg".into()],
            revision_hint: None,
            bypass_cache: false,
        };
        let probs_low_both = FilterProbabilities {
            prob: 0.25,
            gore_prob: 0.05,
            nsfw_prob: 0.25,
            text_prob: Some(0.22),
            title_prob: Some(0.22),
            image_prob: Some(0.10),
            per_image_probs: vec![0.10],
            ..Default::default()
        };
        let (text_s2, _, _, img_s2, _) = compute_modality_subdivision_inner(
            &cfg,
            &target_with_img,
            &[0],
            &probs_low_both,
            true,
            true,
        );
        assert!(text_s2);
        assert!(
            img_s2,
            "LLM flagging with images when neither modality crossed m_t must conservatively mark image_sensitive"
        );
    }
}
