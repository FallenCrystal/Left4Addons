use crate::db::repo::{
    is_remote_image_url, is_valid_local_cover_key, normalize_valid_remote_cover_url,
};
use crate::engine::L4aEngine;
use crate::error::CoreError;
use crate::filter::deploy::{
    merge_subbatch_probabilities, split_images_by_raw_budget, MAX_RAW_PREDICT_IMAGE_BATCH_BYTES,
};
use crate::filter::llm::{is_loopback_url, judge_with_openai_llm, LoadedImage};
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;
use chrono::{TimeDelta, Utc};
use futures_util::stream::{self, StreamExt};
use l4a_protocol::{
    AddonSummary, AppConfig, CatalogFilterPublicInfo, ClientFilterConfig, ClientFilterSourceMode,
    CoreEvent, FilterDecisionSource, FilterEvaluationBatchResponse, FilterEvaluationItemResult,
    FilterEvaluationTarget, FilterGalleryAggregation, FilterItemVerdict, FilterPredictRequest,
    FilterProbabilities, FilterTargetOutcome, RawFilterPredictRequest,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const MAX_INLINE_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CATALOG_IMAGES: usize = 8;
const EVAL_CONCURRENCY: usize = 4;
const CATALOG_SCORING_INFO_TTL: Duration = Duration::from_secs(30);

pub(crate) fn remote_verdict_cache_max_age(cfg: &ClientFilterConfig) -> Option<TimeDelta> {
    match cfg.mode {
        ClientFilterSourceMode::Local => None,
        ClientFilterSourceMode::Catalog | ClientFilterSourceMode::CustomEndpoint => {
            Some(TimeDelta::hours(1))
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
    urls
}

pub fn build_addon_filter_target(addon: &AddonSummary) -> FilterEvaluationTarget {
    let image_sources = collect_addon_image_sources(addon);
    let title = addon
        .workshop_title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            addon
                .addon_title
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(&addon.title)
        .to_string();
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
    let title = addon
        .workshop_title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            addon
                .addon_title
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(&addon.title);
    let description = addon
        .addon_info
        .get("addondescription")
        .or_else(|| addon.addon_info.get("description"))
        .map(|s| s.trim())
        .unwrap_or("");
    format!(
        "{}:{}:{}:{}:{}:{}:{}",
        addon.content_hash.as_deref().unwrap_or(""),
        addon.local_time_updated.unwrap_or(0),
        addon.remote_time_updated.unwrap_or(0),
        title,
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
        if hint.matches(':').count() >= 4 {
            return hint != &build_addon_revision_hint(addon);
        }
        if let (Some((hint_hash, _)), Some(ref current_hash)) =
            (hint.split_once(':'), &addon.content_hash)
        {
            if !hint_hash.is_empty() && hint_hash != current_hash {
                return true;
            }
        }
    }
    false
}

pub fn compute_content_signature(target: &FilterEvaluationTarget, max_images: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"l4a-filter-content-v1\0");
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
    for img in target.image_sources.iter().take(max_images) {
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

fn record_catalog_scoring_revision(
    engine: &L4aEngine,
    normalized_catalog_url: &str,
    new_rev: Option<String>,
) -> bool {
    if normalized_catalog_url.is_empty() {
        return false;
    }
    let changed = {
        let mut guard = engine.catalog_scoring_cache.lock().unwrap();
        let prev_rev = match &*guard {
            Some((cached_url, _, rev)) if cached_url == normalized_catalog_url => rev.clone(),
            _ => None,
        };
        let did_change = prev_rev != new_rev;
        *guard = Some((normalized_catalog_url.to_string(), Instant::now(), new_rev));
        did_change
    };

    if changed {
        let live_cfg = engine.config();
        let active_sig = engine.current_filter_policy_signature(&live_cfg);
        if let Err(err) = engine.db().prune_filter_verdicts_except(&active_sig) {
            tracing::warn!(error = %err, "更新目录服务器评分版本后清理旧缓存失败");
        }
        engine.emit_event(CoreEvent::SnapshotUpdated {
            external_change: false,
        });
    }
    changed
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

async fn ensure_catalog_scoring_revision_fresh(engine: &L4aEngine, app_cfg: &AppConfig) {
    if app_cfg.filter.mode != ClientFilterSourceMode::Catalog {
        return;
    }
    let Some(raw_base) = app_cfg
        .catalog_server_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    let normalized_url = normalize_catalog_base_key(raw_base);

    let prev_rev = {
        let guard = engine.catalog_scoring_cache.lock().unwrap();
        if let Some((ref cached_url, fetched_at, ref rev)) = *guard {
            if cached_url == &normalized_url && fetched_at.elapsed() < CATALOG_SCORING_INFO_TTL {
                return;
            }
            if cached_url == &normalized_url {
                rev.clone()
            } else {
                None
            }
        } else {
            None
        }
    };

    let Ok(info_url) = resolve_catalog_info_url(raw_base) else {
        return;
    };
    let Ok(req) = engine.prepare_routed_get(&info_url, Duration::from_secs(5)) else {
        return;
    };

    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(info) = resp.json::<CatalogFilterPublicInfo>().await {
                record_catalog_scoring_revision(engine, &normalized_url, info.scoring_revision);
                return;
            }
        }
        _ => {}
    }

    // Touch timestamp without altering the last known revision when /info is unreachable or 404.
    let mut guard = engine.catalog_scoring_cache.lock().unwrap();
    *guard = Some((normalized_url, Instant::now(), prev_rev));
}

struct DeduplicatedMiss {
    indices: Vec<usize>,
    target: FilterEvaluationTarget,
    content_sig: String,
    expected_addon_sig: Option<String>,
    has_local_addon: bool,
    persist_cache: bool,
}

struct RecordedCacheHit {
    idx: usize,
    target: FilterEvaluationTarget,
    content_sig: String,
    expected_addon_sig: Option<String>,
    has_local_addon: bool,
}

pub async fn evaluate_targets(
    engine: &L4aEngine,
    targets: Vec<FilterEvaluationTarget>,
) -> Result<FilterEvaluationBatchResponse, CoreError> {
    let app_cfg = engine.config();
    if app_cfg.filter.mode == ClientFilterSourceMode::Catalog {
        ensure_catalog_scoring_revision_fresh(engine, &app_cfg).await;
    }

    let initial_policy_sig = engine.current_filter_policy_signature(&app_cfg);
    if targets.is_empty() {
        return Ok(FilterEvaluationBatchResponse {
            policy_signature: initial_policy_sig,
            items: Vec::new(),
        });
    }

    let cfg = &app_cfg.filter;
    let max_images = effective_max_images(cfg);
    let max_age = remote_verdict_cache_max_age(cfg);

    let needs_addons = targets
        .iter()
        .any(|t| t.addon_id.is_some() || t.target_key.starts_with("addon:"));
    let addons_by_key: HashMap<String, AddonSummary> = if needs_addons {
        engine
            .db()
            .list_addons()?
            .into_iter()
            .map(|a| (format!("addon:{}", a.id.as_str()), a))
            .collect()
    } else {
        HashMap::new()
    };

    let mut target_keys: Vec<String> = Vec::with_capacity(targets.len());
    let mut ordered_outcomes: Vec<Option<FilterTargetOutcome>> = vec![None; targets.len()];
    let mut miss_groups: Vec<DeduplicatedMiss> = Vec::new();
    let mut miss_index_by_key: HashMap<(String, String), usize> = HashMap::new();
    let mut cache_hits: Vec<RecordedCacheHit> = Vec::new();

    for (idx, target) in targets.into_iter().enumerate() {
        target_keys.push(target.target_key.clone());

        if target.target_key.trim().is_empty() {
            let err = CoreError::InvalidInput("targetKey 不能为空".into());
            tracing::warn!(index = idx, error = %err, "过滤目标缺少有效的 targetKey");
            ordered_outcomes[idx] = Some(FilterTargetOutcome::Failed {
                error: err.to_string(),
            });
            continue;
        }

        let lookup_key = if target.target_key.starts_with("addon:") {
            Some(target.target_key.clone())
        } else {
            target
                .addon_id
                .as_ref()
                .map(|aid| format!("addon:{}", aid.as_str()))
        };
        let matched_addon = lookup_key.as_deref().and_then(|key| addons_by_key.get(key));
        let has_local_addon = matched_addon.is_some();
        let expected_addon_sig = matched_addon.and_then(|addon| {
            (!target_is_stale_for_addon(&target, addon)).then(|| compute_addon_state_sig(addon))
        });
        let stale_addon_target = has_local_addon && expected_addon_sig.is_none();

        let content_sig = compute_content_signature(&target, max_images);
        if !target.bypass_cache && !stale_addon_target {
            if let Some(mut cached) = engine.db().get_cached_filter_verdict(
                &target.target_key,
                &content_sig,
                &initial_policy_sig,
                expected_addon_sig.as_deref(),
                max_age,
            )? {
                cached.complete = true;
                ordered_outcomes[idx] = Some(FilterTargetOutcome::Completed {
                    verdict: cached,
                    complete: true,
                });
                cache_hits.push(RecordedCacheHit {
                    idx,
                    target,
                    content_sig,
                    expected_addon_sig,
                    has_local_addon,
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
            tracing::warn!(error = %err, "预启动本地过滤服务失败");
            let err_msg = err.to_string();
            for group in miss_groups {
                for idx in group.indices {
                    ordered_outcomes[idx] = Some(FilterTargetOutcome::Failed {
                        error: err_msg.clone(),
                    });
                }
            }
        } else {
            let computed = run_deduplicated_misses(engine, &app_cfg, miss_groups, max_images).await;
            for (indices, outcome) in computed {
                for idx in indices {
                    ordered_outcomes[idx] = Some(outcome.clone());
                }
            }
        }
    }

    let mut final_policy_sig = engine.current_filter_policy_signature(&app_cfg);

    // If Catalog `scoring_revision` rotated mid-flight during `/predict`, re-evaluate any hits
    // that had been served from the pre-rotation SQLite cache so the batch is 100% consistent.
    if final_policy_sig != initial_policy_sig && !cache_hits.is_empty() {
        let mut retry_groups: Vec<DeduplicatedMiss> = Vec::new();
        let mut retry_map: HashMap<(String, String), usize> = HashMap::new();
        for hit in cache_hits {
            if let Ok(Some(mut fresh_hit)) = engine.db().get_cached_filter_verdict(
                &hit.target.target_key,
                &hit.content_sig,
                &final_policy_sig,
                hit.expected_addon_sig.as_deref(),
                max_age,
            ) {
                fresh_hit.complete = true;
                ordered_outcomes[hit.idx] = Some(FilterTargetOutcome::Completed {
                    verdict: fresh_hit,
                    complete: true,
                });
                continue;
            }
            let key = (hit.target.target_key.clone(), hit.content_sig.clone());
            if let Some(&g_idx) = retry_map.get(&key) {
                retry_groups[g_idx].indices.push(hit.idx);
            } else {
                let g_idx = retry_groups.len();
                retry_map.insert(key, g_idx);
                retry_groups.push(DeduplicatedMiss {
                    indices: vec![hit.idx],
                    target: hit.target,
                    content_sig: hit.content_sig,
                    expected_addon_sig: hit.expected_addon_sig,
                    has_local_addon: hit.has_local_addon,
                    persist_cache: true,
                });
            }
        }
        if !retry_groups.is_empty() {
            let recomputed =
                run_deduplicated_misses(engine, &app_cfg, retry_groups, max_images).await;
            for (indices, outcome) in recomputed {
                for idx in indices {
                    ordered_outcomes[idx] = Some(outcome.clone());
                }
            }
            final_policy_sig = engine.current_filter_policy_signature(&app_cfg);
        }
    }

    let mut complete_verdicts: Vec<FilterItemVerdict> = Vec::new();
    let mut items: Vec<FilterEvaluationItemResult> = Vec::with_capacity(target_keys.len());

    for (target_key, outcome_opt) in target_keys.into_iter().zip(ordered_outcomes) {
        let outcome = outcome_opt.unwrap_or_else(|| FilterTargetOutcome::Failed {
            error: "过滤评估未返回结果".into(),
        });
        if let FilterTargetOutcome::Completed {
            ref verdict,
            complete: true,
        } = outcome
        {
            complete_verdicts.push(verdict.clone());
        }
        items.push(FilterEvaluationItemResult {
            target_key,
            outcome,
        });
    }

    let live_active_sig = engine.current_filter_policy_signature(&engine.config());
    if !complete_verdicts.is_empty() && final_policy_sig == live_active_sig {
        engine.emit_event(CoreEvent::FilterVerdictsUpdated {
            verdicts: complete_verdicts,
            policy_signature: Some(final_policy_sig.clone()),
        });
    }

    Ok(FilterEvaluationBatchResponse {
        policy_signature: final_policy_sig,
        items,
    })
}

async fn run_deduplicated_misses(
    engine: &L4aEngine,
    app_cfg: &AppConfig,
    miss_groups: Vec<DeduplicatedMiss>,
    max_images: usize,
) -> Vec<(Vec<usize>, FilterTargetOutcome)> {
    stream::iter(miss_groups)
        .map(|group| async move {
            let permit_res = engine.filter_eval_semaphore.acquire().await;
            let Ok(_permit) = permit_res else {
                return (
                    group.indices,
                    FilterTargetOutcome::Failed {
                        error: "过滤评估并发信号量已关闭".into(),
                    },
                );
            };

            match evaluate_single_target(engine, app_cfg, &group.target, max_images).await {
                Ok((mut verdict, had_load_error)) => {
                    let is_complete = verdict.flagged || !had_load_error;
                    verdict.complete = is_complete;
                    verdict.revision_hint = group.target.revision_hint.clone();

                    if is_complete && group.persist_cache {
                        persist_verdict_if_still_valid(engine, app_cfg, &group, &verdict);
                    }

                    (
                        group.indices,
                        FilterTargetOutcome::Completed {
                            verdict,
                            complete: is_complete,
                        },
                    )
                }
                Err(err) => {
                    tracing::warn!(
                        target_key = %group.target.target_key,
                        error = %err,
                        "单项内容过滤评估失败"
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
        .buffered(EVAL_CONCURRENCY)
        .collect()
        .await
}

fn persist_verdict_if_still_valid(
    engine: &L4aEngine,
    eval_app_cfg: &AppConfig,
    group: &DeduplicatedMiss,
    verdict: &FilterItemVerdict,
) {
    if group.has_local_addon {
        let Some(ref expected_sig) = group.expected_addon_sig else {
            return;
        };
        let lookup_key = if group.target.target_key.starts_with("addon:") {
            group.target.target_key.clone()
        } else if let Some(ref aid) = group.target.addon_id {
            format!("addon:{}", aid.as_str())
        } else {
            return;
        };
        let addon_still_matches = engine.db().list_addons().ok().is_some_and(|addons| {
            addons
                .iter()
                .find(|a| format!("addon:{}", a.id.as_str()) == lookup_key)
                .is_some_and(|a| &compute_addon_state_sig(a) == expected_sig)
        });
        if !addon_still_matches {
            return;
        }
    }

    let verdict_policy_sig = engine.current_filter_policy_signature(eval_app_cfg);
    let live_active_sig = engine.current_filter_policy_signature(&engine.config());

    if let Err(db_err) = engine.db().put_cached_filter_verdict(
        &group.target.target_key,
        &group.content_sig,
        group.expected_addon_sig.as_deref(),
        &verdict_policy_sig,
        &live_active_sig,
        verdict,
    ) {
        tracing::warn!(
            target_key = %group.target.target_key,
            error = %db_err,
            "写入过滤判定缓存失败"
        );
    }
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
    InlineBase64 {
        slot_indices: Vec<usize>,
        loaded: Vec<(usize, LoadedImage)>,
        had_load_error: bool,
    },
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
    max_images: usize,
) -> Result<(FilterItemVerdict, bool), CoreError> {
    let cfg = &app_cfg.filter;

    let mut had_load_error;
    let mut loaded_images: Vec<LoadedImage> = Vec::new();
    let mut deferred_remotes: Vec<(usize, ValidatedRemoteImage)> = Vec::new();
    let mut submitted_slot_indices: Vec<usize>;

    let (mut probs, custom_endpoint) = if cfg.mode == ClientFilterSourceMode::Catalog {
        let base = app_cfg
            .catalog_server_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::Config("未配置目录服务器地址 (catalogServerUrl)".into()))?;
        let normalized_catalog_url = normalize_catalog_base_key(base);
        let url = resolve_filter_endpoint_url(
            base,
            "/api/v1/filter/predict",
            &["/api/v1/filter/predict"],
            "目录服务器地址",
        )?;

        let probs = match prepare_catalog_images(engine, target, max_images).await {
            CatalogImagePlan::RemoteUrls {
                slot_indices,
                image_urls,
                remotes,
                had_load_error: prep_err,
            } => {
                had_load_error = prep_err;
                submitted_slot_indices = slot_indices;
                deferred_remotes = remotes;

                let req = FilterPredictRequest {
                    workshop_id: target.workshop_id,
                    title: Some(target.title.clone()),
                    description: target.description.clone(),
                    tags: target.tags.clone(),
                    image_base64: None,
                    images_base64: Vec::new(),
                    image_urls: image_urls.clone(),
                };
                match post_predict_request(engine, &url, &req).await {
                    Ok(p) => p,
                    Err(first_err)
                        if !image_urls.is_empty()
                            && !matches!(first_err, CoreError::NotFound(_)) =>
                    {
                        let (fallback_loaded, fallback_err) =
                            resolve_target_images(engine, target, max_images).await;
                        if !fallback_err {
                            return Err(first_err);
                        }
                        had_load_error = true;
                        deferred_remotes.clear();
                        submitted_slot_indices =
                            fallback_loaded.iter().map(|(idx, _)| *idx).collect();
                        let images_b64: Vec<String> = fallback_loaded
                            .iter()
                            .map(|(_, img)| img.b64.clone())
                            .collect();
                        loaded_images = fallback_loaded.into_iter().map(|(_, img)| img).collect();
                        let retry_req = FilterPredictRequest {
                            workshop_id: target.workshop_id,
                            title: Some(target.title.clone()),
                            description: target.description.clone(),
                            tags: target.tags.clone(),
                            image_base64: None,
                            images_base64: images_b64,
                            image_urls: Vec::new(),
                        };
                        post_predict_request(engine, &url, &retry_req).await?
                    }
                    Err(err) => return Err(err),
                }
            }
            CatalogImagePlan::InlineBase64 {
                slot_indices,
                loaded,
                had_load_error: prep_err,
            } => {
                had_load_error = prep_err;
                submitted_slot_indices = slot_indices;
                let images_b64: Vec<String> =
                    loaded.iter().map(|(_, img)| img.b64.clone()).collect();
                loaded_images = loaded.into_iter().map(|(_, img)| img).collect();

                let req = FilterPredictRequest {
                    workshop_id: target.workshop_id,
                    title: Some(target.title.clone()),
                    description: target.description.clone(),
                    tags: target.tags.clone(),
                    image_base64: None,
                    images_base64: images_b64,
                    image_urls: Vec::new(),
                };
                post_predict_request(engine, &url, &req).await?
            }
        };

        if probs.scoring_revision.is_some() {
            record_catalog_scoring_revision(
                engine,
                &normalized_catalog_url,
                probs.scoring_revision.clone(),
            );
        }

        (probs, None)
    } else {
        let (imgs, load_err) = resolve_target_images(engine, target, max_images).await;
        had_load_error = load_err;
        submitted_slot_indices = imgs.iter().map(|(idx, _)| *idx).collect();
        let images_b64: Vec<String> = imgs.iter().map(|(_, img)| img.b64.clone()).collect();
        loaded_images = imgs.into_iter().map(|(_, img)| img).collect();
        let (probs, custom_ep) =
            fetch_non_catalog_probabilities(engine, app_cfg, target, &images_b64).await?;
        (probs, custom_ep)
    };

    let evaluated_score = probs.score_for(cfg.score_target);
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

    let (flagged, decision_source) = if cfg.enable_direct_flag && evaluated_score >= flag_threshold
    {
        (true, FilterDecisionSource::ModelDirectFlag)
    } else if cfg.llm_enabled && evaluated_score >= llm_threshold {
        if cfg.mode == ClientFilterSourceMode::Catalog
            && cfg.llm_send_images
            && loaded_images.is_empty()
            && !deferred_remotes.is_empty()
        {
            let (materialized, mat_err) =
                materialize_deferred_remotes_for_llm(engine, &deferred_remotes).await;
            if mat_err {
                had_load_error = true;
            }
            loaded_images = materialized;
        }
        let vault = engine.token_vault();
        let is_sensitive = judge_with_openai_llm(
            engine,
            cfg,
            &vault,
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
            let images_b64: Vec<String> = loaded_images.iter().map(|img| img.b64.clone()).collect();
            populate_missing_unimodal_scores(engine, ep, target, &images_b64, &mut probs).await;
        }
    }

    let evaluated_image_indices =
        compute_evaluated_image_indices(cfg, &submitted_slot_indices, &probs);

    let (
        text_sensitive,
        title_sensitive,
        description_sensitive,
        image_sensitive,
        sensitive_image_indices,
    ) = compute_modality_subdivision(cfg, target, &evaluated_image_indices, &probs, flagged);

    let is_complete = flagged || !had_load_error;

    Ok((
        FilterItemVerdict {
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
            complete: is_complete,
            evaluated_at: Utc::now(),
        },
        had_load_error,
    ))
}

fn compute_evaluated_image_indices(
    cfg: &ClientFilterConfig,
    submitted_slot_indices: &[usize],
    probs: &FilterProbabilities,
) -> Vec<usize> {
    if submitted_slot_indices.is_empty() {
        return Vec::new();
    }
    if cfg.mode == ClientFilterSourceMode::Local && !cfg.local_params.sanitized().enable_image {
        return Vec::new();
    }
    if !probs.per_image_probs.is_empty() {
        let count = probs
            .per_image_probs
            .len()
            .min(submitted_slot_indices.len());
        return submitted_slot_indices[..count].to_vec();
    }
    // If the backend returned unimodal text breakdown with `image_prob == None` and empty
    // `per_image_probs`, server-side image evaluation was disabled.
    if probs.text_prob.is_some() && probs.image_prob.is_none() {
        return Vec::new();
    }
    submitted_slot_indices.to_vec()
}

async fn prepare_catalog_images(
    engine: &L4aEngine,
    target: &FilterEvaluationTarget,
    max_images: usize,
) -> CatalogImagePlan {
    if max_images == 0 || target.image_sources.is_empty() {
        return CatalogImagePlan::RemoteUrls {
            slot_indices: Vec::new(),
            image_urls: Vec::new(),
            remotes: Vec::new(),
            had_load_error: false,
        };
    }

    let mut requires_inline_mode = false;
    for raw_src in &target.image_sources {
        let trimmed = raw_src.trim();
        if trimmed.is_empty() {
            continue;
        }
        if is_valid_local_cover_key(trimmed) || has_ascii_prefix_ignore_case(trimmed, b"data:") {
            requires_inline_mode = true;
            break;
        }
        if is_remote_image_url(trimmed)
            && matches!(
                validate_remote_filter_image_url(engine, trimmed),
                Ok(ValidatedRemoteImage::Mirrored(_))
            )
        {
            requires_inline_mode = true;
            break;
        }
    }

    if requires_inline_mode {
        let (loaded, had_load_error) = resolve_target_images(engine, target, max_images).await;
        let slot_indices = loaded.iter().map(|(idx, _)| *idx).collect();
        return CatalogImagePlan::InlineBase64 {
            slot_indices,
            loaded,
            had_load_error,
        };
    }

    let mut slot_indices = Vec::new();
    let mut image_urls = Vec::new();
    let mut remotes = Vec::new();
    let mut had_load_error = false;

    for (slot_idx, raw_src) in target.image_sources.iter().enumerate() {
        if slot_indices.len() >= max_images {
            break;
        }
        let trimmed = raw_src.trim();
        if trimmed.is_empty() {
            continue;
        }
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
                Ok(ValidatedRemoteImage::Mirrored(_)) => unreachable!("handled above"),
                Err(err) => {
                    had_load_error = true;
                    tracing::debug!(source = %trimmed, error = %err, "拒绝不安全的远程过滤预览图地址");
                }
            }
        } else {
            had_load_error = true;
            tracing::debug!(source = %trimmed, "跳过不受支持的过滤预览图来源格式");
        }
    }

    CatalogImagePlan::RemoteUrls {
        slot_indices,
        image_urls,
        remotes,
        had_load_error,
    }
}

async fn materialize_deferred_remotes_for_llm(
    engine: &L4aEngine,
    remotes: &[(usize, ValidatedRemoteImage)],
) -> (Vec<LoadedImage>, bool) {
    let mut out = Vec::with_capacity(remotes.len());
    let mut had_error = false;
    for (_, remote) in remotes {
        match fetch_validated_remote_image(engine, remote).await {
            Ok(img) => out.push(img),
            Err(err) => {
                had_error = true;
                tracing::debug!(error = %err, "LLM 复核阶段下载远程预览图失败");
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
                    "跳过无法加载的过滤预览图"
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
    let host = raw_host.to_ascii_lowercase();
    if host == "steamusercontent.com"
        || host.ends_with(".steamusercontent.com")
        || host == "steamstatic.com"
        || host.ends_with(".steamstatic.com")
        || host == "media.steampowered.com"
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
    let resolved = engine.mirror_router().resolve(&normalized);
    let rewritten_by_mirror = resolved.url != normalized
        || resolved.original_host.is_some()
        || !resolved.headers.is_empty();
    if rewritten_by_mirror {
        engine
            .mirror_router()
            .validate_url(&normalized)
            .map_err(|e| CoreError::InvalidInput(e.to_string()))?;
        return Ok(ValidatedRemoteImage::Mirrored(normalized));
    }

    Err(CoreError::InvalidInput(
        "远程预览图地址不在受信任的 Steam 创意工坊 CDN 白名单内".into(),
    ))
}

async fn fetch_validated_remote_image(
    engine: &L4aEngine,
    remote: &ValidatedRemoteImage,
) -> Result<LoadedImage, CoreError> {
    match remote {
        ValidatedRemoteImage::LoopbackTest(url) => {
            let (mime, bytes) = fetch_loopback_image(engine, url).await?;
            Ok(LoadedImage {
                mime,
                b64: STANDARD.encode(&bytes),
            })
        }
        ValidatedRemoteImage::SteamCdn(url) | ValidatedRemoteImage::Mirrored(url) => {
            let (mime, bytes) = engine.fetch_workshop_cover(url).await?;
            Ok(LoadedImage {
                mime,
                b64: STANDARD.encode(&bytes),
            })
        }
    }
}

async fn load_single_image(engine: &L4aEngine, src: &str) -> Result<LoadedImage, CoreError> {
    if is_valid_local_cover_key(src) {
        let bytes = engine.read_cached_image(src)?;
        let mime = detect_image_mime(&bytes, Some(src))?;
        return Ok(LoadedImage {
            mime,
            b64: STANDARD.encode(&bytes),
        });
    }

    if has_ascii_prefix_ignore_case(src, b"data:") {
        return decode_data_image_url(src);
    }

    if is_remote_image_url(src) {
        let remote = validate_remote_filter_image_url(engine, src)?;
        return fetch_validated_remote_image(engine, &remote).await;
    }

    Err(CoreError::InvalidInput("不受支持的图片来源格式".into()))
}

pub(crate) fn decode_data_image_url(src: &str) -> Result<LoadedImage, CoreError> {
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
    Ok(LoadedImage {
        mime,
        b64: STANDARD.encode(&bytes),
    })
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
        .send()
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
                let probs = post_predict_request(engine, &direct_catalog_url, &req).await?;
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
            match post_predict_with_status(engine, &catalog_style_url, &catalog_req).await {
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
                Err(PredictHttpOutcome::Other(err)) => Err(err),
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
        let probs = post_predict_request(engine, url, &req).await?;
        parts.push((count, probs));
    }
    Ok(merge_subbatch_probabilities(
        parts,
        FilterGalleryAggregation::Max,
    ))
}

enum PredictHttpOutcome {
    NotFound,
    Other(CoreError),
}

async fn post_predict_with_status<T: Serialize + ?Sized>(
    engine: &L4aEngine,
    url: &str,
    req_body: &T,
) -> Result<FilterProbabilities, PredictHttpOutcome> {
    let builder = engine
        .prepare_routed_post(url, Duration::from_secs(30))
        .map_err(PredictHttpOutcome::Other)?;

    let resp = builder
        .json(req_body)
        .send()
        .await
        .map_err(|e| PredictHttpOutcome::Other(CoreError::Network(e.to_string())))?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(PredictHttpOutcome::NotFound);
    }
    if !resp.status().is_success() {
        let status = resp.status();
        return Err(PredictHttpOutcome::Other(CoreError::Network(format!(
            "过滤服务端点返回 HTTP {status}"
        ))));
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
    match post_predict_with_status(engine, url, req_body).await {
        Ok(probs) => Ok(probs),
        Err(PredictHttpOutcome::NotFound) => Err(CoreError::NotFound(format!(
            "过滤预测接口不存在 (404): {url}"
        ))),
        Err(PredictHttpOutcome::Other(err)) => Err(err),
    }
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
        post_predict_request(engine, &ep.url, &req).await
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

    if has_text && probs.text_prob.is_none() {
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

    if !images_b64.is_empty() && (probs.image_prob.is_none() || probs.per_image_probs.is_empty()) {
        let mut per_img = Vec::with_capacity(images_b64.len());
        let mut max_ov = 0.0_f32;
        let mut max_gore = 0.0_f32;
        let mut max_nsfw = 0.0_f32;
        let mut all_ok = true;
        for b64 in images_b64 {
            match post_unimodal_predict(engine, ep, None, None, Vec::new(), vec![b64.clone()]).await
            {
                Ok(ir) => {
                    per_img.push(ir.prob);
                    max_ov = max_ov.max(ir.prob);
                    max_gore = max_gore.max(ir.gore_prob);
                    max_nsfw = max_nsfw.max(ir.nsfw_prob);
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
    if !flagged {
        return (false, false, false, false, Vec::new());
    }

    let has_title = !target.title.trim().is_empty() || !target.tags.is_empty();
    let has_desc = target
        .description
        .as_deref()
        .is_some_and(|d| !d.trim().is_empty());
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

    let mut image_sensitive = n_imgs > 0
        && (probs.image_prob.is_some_and(|p| p >= m_t) || !sensitive_image_indices.is_empty());

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
}
