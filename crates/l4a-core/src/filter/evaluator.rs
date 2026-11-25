use crate::db::repo::{
    is_remote_image_url, is_valid_local_cover_key, normalize_valid_remote_cover_url,
};
use crate::engine::L4aEngine;
use crate::error::CoreError;
use crate::filter::llm::{is_loopback_url, judge_with_openai_llm, LoadedImage};
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;
use chrono::Utc;
use futures_util::stream::{self, StreamExt};
use l4a_mirror::MirrorRouter;
use l4a_protocol::{
    AppConfig, ClientFilterConfig, ClientFilterSourceMode, CoreEvent, FilterDecisionSource,
    FilterEvaluationTarget, FilterItemVerdict, FilterPredictRequest, FilterProbabilities,
};
use sha2::{Digest, Sha256};
use std::time::Duration;

const MAX_INLINE_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const EVAL_CONCURRENCY: usize = 4;

pub fn compute_policy_signature(app_cfg: &AppConfig, local_commit_sha: Option<&str>) -> String {
    let cfg = &app_cfg.filter;
    let mut hasher = Sha256::new();
    hasher.update(b"l4a-filter-policy-v1\0");

    let mode_json = serde_json::to_string(&cfg.mode).unwrap_or_default();
    hasher.update(mode_json.as_bytes());
    hasher.update(b"\0");

    match cfg.mode {
        ClientFilterSourceMode::Local => {
            let params_json =
                serde_json::to_string(&cfg.local_params.sanitized()).unwrap_or_default();
            hasher.update(params_json.as_bytes());
            hasher.update(b"\0");
            hasher.update(local_commit_sha.unwrap_or("").as_bytes());
            hasher.update(b"\0");
        }
        ClientFilterSourceMode::Catalog => {
            hasher.update(
                app_cfg
                    .catalog_server_url
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .as_bytes(),
            );
            hasher.update(b"\0");
        }
        ClientFilterSourceMode::CustomEndpoint => {
            hasher.update(
                cfg.custom_endpoint_url
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .as_bytes(),
            );
            hasher.update(b"\0");
        }
    }

    let score_target_json = serde_json::to_string(&cfg.score_target).unwrap_or_default();
    hasher.update(score_target_json.as_bytes());
    hasher.update(b"\0");
    hasher.update([u8::from(cfg.enable_direct_flag)]);
    hasher.update(cfg.flag_threshold.to_bits().to_le_bytes());
    hasher.update([u8::from(cfg.llm_enabled)]);
    if cfg.llm_enabled {
        hasher.update(cfg.llm_threshold.to_bits().to_le_bytes());
        hasher.update(cfg.llm_endpoint.trim().as_bytes());
        hasher.update(b"\0");
        hasher.update(cfg.llm_model.trim().as_bytes());
        hasher.update(b"\0");
        hasher.update([u8::from(cfg.llm_send_images)]);
        hasher.update(cfg.llm_system_prompt.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update([u8::from(cfg.subdivide_modalities)]);
    hasher.update(cfg.modality_threshold.to_bits().to_le_bytes());

    hex::encode(hasher.finalize())
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

pub async fn evaluate_targets(
    engine: &L4aEngine,
    targets: Vec<FilterEvaluationTarget>,
) -> Result<Vec<FilterItemVerdict>, CoreError> {
    if targets.is_empty() {
        return Ok(Vec::new());
    }

    let app_cfg = engine.config();
    let cfg = &app_cfg.filter;
    let max_images = effective_max_images(cfg);

    // Pre-start local service once before concurrent target evaluation if configured
    if cfg.mode == ClientFilterSourceMode::Local
        && engine.local_filter().active_endpoint().is_none()
        && cfg.auto_start_local_service
        && engine.local_filter().is_deployed()
    {
        engine
            .local_filter()
            .ensure_started_endpoint(&cfg.local_params.sanitized())
            .await?;
    }

    let local_sha = if cfg.mode == ClientFilterSourceMode::Local {
        engine.local_filter().active_commit_sha()
    } else {
        None
    };
    let policy_sig = compute_policy_signature(&app_cfg, local_sha.as_deref());

    let mut ordered_verdicts: Vec<Option<FilterItemVerdict>> = vec![None; targets.len()];
    let mut misses: Vec<(usize, FilterEvaluationTarget, String)> = Vec::new();

    for (idx, target) in targets.into_iter().enumerate() {
        if target.target_key.trim().is_empty() {
            return Err(CoreError::InvalidInput("targetKey 不能为空".into()));
        }
        let content_sig = compute_content_signature(&target, max_images);
        if !target.bypass_cache {
            if let Some(cached) = engine.db().get_cached_filter_verdict(
                &target.target_key,
                &content_sig,
                &policy_sig,
            )? {
                ordered_verdicts[idx] = Some(cached);
                continue;
            }
        }
        misses.push((idx, target, content_sig));
    }

    if !misses.is_empty() {
        let computed: Vec<
            Result<(usize, String, FilterEvaluationTarget, FilterItemVerdict), CoreError>,
        > = stream::iter(misses)
            .map(|(idx, target, content_sig)| {
                let app_cfg_ref = &app_cfg;
                async move {
                    let verdict =
                        evaluate_single_target(engine, app_cfg_ref, &target, max_images).await?;
                    Ok((idx, content_sig, target, verdict))
                }
            })
            .buffered(EVAL_CONCURRENCY)
            .collect()
            .await;

        for item in computed {
            let (idx, content_sig, target, verdict) = item?;
            engine.db().put_cached_filter_verdict(
                &target.target_key,
                &content_sig,
                &policy_sig,
                &verdict,
            )?;
            ordered_verdicts[idx] = Some(verdict);
        }
    }

    let final_verdicts: Vec<FilterItemVerdict> = ordered_verdicts.into_iter().flatten().collect();

    if !final_verdicts.is_empty() {
        engine.emit_event(CoreEvent::FilterVerdictsUpdated {
            verdicts: final_verdicts.clone(),
        });
    }

    Ok(final_verdicts)
}

fn effective_max_images(cfg: &ClientFilterConfig) -> usize {
    let sanitized = cfg.local_params.sanitized();
    if cfg.mode == ClientFilterSourceMode::Local
        && !sanitized.enable_image
        && !(cfg.llm_enabled && cfg.llm_send_images)
    {
        return 0;
    }
    sanitized.max_images.clamp(1, 8) as usize
}

async fn evaluate_single_target(
    engine: &L4aEngine,
    app_cfg: &AppConfig,
    target: &FilterEvaluationTarget,
    max_images: usize,
) -> Result<FilterItemVerdict, CoreError> {
    let cfg = &app_cfg.filter;
    let images = resolve_target_images(engine, target, max_images, app_cfg.https_only).await;
    let images_b64: Vec<String> = images.iter().map(|img| img.b64.clone()).collect();

    let (mut probs, custom_predict_url) =
        fetch_probabilities(engine, app_cfg, target, &images_b64).await?;

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
        let vault = engine.token_vault();
        let is_sensitive = judge_with_openai_llm(
            cfg,
            &vault,
            engine.mirror_router(),
            &target.title,
            &target.tags,
            target.description.as_deref(),
            &images,
        )
        .await?;
        if is_sensitive {
            (true, FilterDecisionSource::LlmFlaggedTrue)
        } else {
            (false, FilterDecisionSource::LlmFlaggedFalse)
        }
    } else if !cfg.enable_direct_flag && !cfg.llm_enabled && evaluated_score >= flag_threshold {
        (true, FilterDecisionSource::ModelDirectFlag)
    } else {
        (false, FilterDecisionSource::ModelSafe)
    };

    // If CustomEndpoint returned raw Left4Filters output without unimodal breakdown,
    // issue follow-up unimodal /predict calls when flagged & subdivide_modalities is true.
    if flagged && cfg.subdivide_modalities {
        if let Some(ref endpoint_url) = custom_predict_url {
            populate_missing_unimodal_scores(
                engine.mirror_router(),
                endpoint_url,
                target,
                &images_b64,
                &mut probs,
            )
            .await;
        }
    }

    let (
        text_sensitive,
        title_sensitive,
        description_sensitive,
        image_sensitive,
        sensitive_image_indices,
    ) = compute_modality_subdivision(cfg, target, images.len(), &probs, flagged);

    Ok(FilterItemVerdict {
        target_key: target.target_key.clone(),
        flagged,
        decision_source,
        evaluated_score,
        probabilities: probs,
        text_sensitive,
        title_sensitive,
        description_sensitive,
        image_sensitive,
        sensitive_image_indices,
        evaluated_at: Utc::now(),
    })
}

async fn resolve_target_images(
    engine: &L4aEngine,
    target: &FilterEvaluationTarget,
    max_images: usize,
    https_only: bool,
) -> Vec<LoadedImage> {
    if max_images == 0 || target.image_sources.is_empty() {
        return Vec::new();
    }

    let mut loaded = Vec::with_capacity(max_images.min(target.image_sources.len()));
    for raw_src in &target.image_sources {
        if loaded.len() >= max_images {
            break;
        }
        let trimmed = raw_src.trim();
        if trimmed.is_empty() {
            continue;
        }
        match load_single_image(engine, trimmed, https_only).await {
            Ok(img) => loaded.push(img),
            Err(err) => {
                tracing::debug!(source = %trimmed, error = %err, "跳过无法加载的过滤预览图");
            }
        }
    }
    loaded
}

async fn load_single_image(
    engine: &L4aEngine,
    src: &str,
    https_only: bool,
) -> Result<LoadedImage, CoreError> {
    if is_valid_local_cover_key(src) {
        let bytes = engine.read_cached_image(src)?;
        let mime = detect_image_mime(&bytes, Some(src))?;
        return Ok(LoadedImage {
            mime,
            b64: STANDARD.encode(&bytes),
        });
    }

    if src
        .get(..11)
        .is_some_and(|p| p.eq_ignore_ascii_case("data:image/"))
    {
        return decode_data_image_url(src);
    }

    if is_remote_image_url(src) {
        let normalized = normalize_valid_remote_cover_url(src)
            .ok_or_else(|| CoreError::InvalidInput("无效的远程预览图地址".into()))?;
        if https_only && is_loopback_url(&normalized) {
            let (mime, bytes) = fetch_loopback_image(&normalized).await?;
            return Ok(LoadedImage {
                mime,
                b64: STANDARD.encode(&bytes),
            });
        }
        let (mime, bytes) = engine.fetch_workshop_cover(&normalized).await?;
        return Ok(LoadedImage {
            mime,
            b64: STANDARD.encode(&bytes),
        });
    }

    Err(CoreError::InvalidInput("不受支持的图片来源格式".into()))
}

fn decode_data_image_url(src: &str) -> Result<LoadedImage, CoreError> {
    let rest = &src["data:".len()..];
    let Some((meta, b64_part)) = rest.split_once(",") else {
        return Err(CoreError::InvalidInput("无效的 data URL".into()));
    };
    let mut meta_parts = meta.split(';');
    let raw_mime = meta_parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let is_b64 = meta_parts.any(|p| p.trim().eq_ignore_ascii_case("base64"));
    if !is_b64 || !raw_mime.starts_with("image/") {
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
    let mime = image::guess_format(&bytes)
        .map(|f| f.to_mime_type().to_string())
        .unwrap_or(raw_mime);
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

async fn fetch_loopback_image(url: &str) -> Result<(String, Vec<u8>), CoreError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| CoreError::Network(e.to_string()))?;
    let mut resp = client
        .get(url)
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

async fn fetch_probabilities(
    engine: &L4aEngine,
    app_cfg: &AppConfig,
    target: &FilterEvaluationTarget,
    images_b64: &[String],
) -> Result<(FilterProbabilities, Option<String>), CoreError> {
    let cfg = &app_cfg.filter;
    match cfg.mode {
        ClientFilterSourceMode::Local => {
            if engine.local_filter().active_endpoint().is_none() {
                if cfg.auto_start_local_service && engine.local_filter().is_deployed() {
                    engine
                        .local_filter()
                        .ensure_started_endpoint(&cfg.local_params.sanitized())
                        .await?;
                } else {
                    return Err(CoreError::InvalidInput(
                        "本地过滤守护进程未运行，请先部署或启动服务".into(),
                    ));
                }
            }
            let probs = engine
                .local_filter()
                .predict_local(
                    &target.title,
                    target.description.as_deref(),
                    &target.tags,
                    images_b64,
                )
                .await?;
            Ok((probs, None))
        }
        ClientFilterSourceMode::Catalog => {
            let base = app_cfg
                .catalog_server_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    CoreError::Config("未配置目录服务器地址 (catalogServerUrl)".into())
                })?;
            let url = format!("{}/api/v1/filter/predict", base.trim_end_matches('/'));
            let req = FilterPredictRequest {
                workshop_id: target.workshop_id,
                title: Some(target.title.clone()),
                description: target.description.clone(),
                tags: target.tags.clone(),
                image_base64: None,
                images_base64: images_b64.to_vec(),
                image_urls: Vec::new(),
            };
            let probs = post_predict_request(engine.mirror_router(), &url, &req).await?;
            Ok((probs, None))
        }
        ClientFilterSourceMode::CustomEndpoint => {
            let base = cfg
                .custom_endpoint_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    CoreError::Config("未配置自定义过滤服务端点 (customEndpointUrl)".into())
                })?
                .trim_end_matches('/');

            let req = FilterPredictRequest {
                workshop_id: target.workshop_id,
                title: Some(target.title.clone()),
                description: target.description.clone(),
                tags: target.tags.clone(),
                image_base64: None,
                images_base64: images_b64.to_vec(),
                image_urls: Vec::new(),
            };

            if base.ends_with("/predict") {
                let probs = post_predict_request(engine.mirror_router(), base, &req).await?;
                return Ok((probs, Some(base.to_string())));
            }

            let catalog_style_url = format!("{base}/api/v1/filter/predict");
            match post_predict_with_status(engine.mirror_router(), &catalog_style_url, &req).await {
                Ok(probs) => Ok((probs, Some(catalog_style_url))),
                Err(PredictHttpOutcome::NotFound) => {
                    let raw_predict_url = format!("{base}/predict");
                    let probs =
                        post_predict_request(engine.mirror_router(), &raw_predict_url, &req)
                            .await?;
                    Ok((probs, Some(raw_predict_url)))
                }
                Err(PredictHttpOutcome::Other(err)) => Err(err),
            }
        }
    }
}

enum PredictHttpOutcome {
    NotFound,
    Other(CoreError),
}

async fn post_predict_with_status(
    mirror: &MirrorRouter,
    url: &str,
    req_body: &FilterPredictRequest,
) -> Result<FilterProbabilities, PredictHttpOutcome> {
    let timeout = Duration::from_secs(30);
    let builder = if is_loopback_url(url) {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| PredictHttpOutcome::Other(CoreError::Network(e.to_string())))?;
        client.post(url)
    } else {
        let (client, _) = mirror
            .build_client_for(url, Some(timeout))
            .map_err(|e| PredictHttpOutcome::Other(CoreError::Network(e.to_string())))?;
        let (req, _) = mirror.prepare_post(&client, url);
        req
    };

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

async fn post_predict_request(
    mirror: &MirrorRouter,
    url: &str,
    req_body: &FilterPredictRequest,
) -> Result<FilterProbabilities, CoreError> {
    match post_predict_with_status(mirror, url, req_body).await {
        Ok(probs) => Ok(probs),
        Err(PredictHttpOutcome::NotFound) => Err(CoreError::NotFound(format!(
            "过滤预测接口不存在 (404): {url}"
        ))),
        Err(PredictHttpOutcome::Other(err)) => Err(err),
    }
}

async fn populate_missing_unimodal_scores(
    mirror: &MirrorRouter,
    predict_url: &str,
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
        let txt_req = FilterPredictRequest {
            workshop_id: None,
            title: Some(target.title.clone()),
            description: target.description.clone(),
            tags: target.tags.clone(),
            image_base64: None,
            images_base64: Vec::new(),
            image_urls: Vec::new(),
        };
        if let Ok(tp) = post_predict_request(mirror, predict_url, &txt_req).await {
            probs.text_prob = Some(tp.prob);
            probs.text_gore_prob = Some(tp.gore_prob);
            probs.text_nsfw_prob = Some(tp.nsfw_prob);
        }

        if has_title && probs.title_prob.is_none() {
            if !has_desc {
                probs.title_prob = probs.text_prob;
            } else {
                let title_req = FilterPredictRequest {
                    title: Some(target.title.clone()),
                    tags: target.tags.clone(),
                    ..Default::default()
                };
                if let Ok(tr) = post_predict_request(mirror, predict_url, &title_req).await {
                    probs.title_prob = Some(tr.prob);
                }
            }
        }

        if has_desc && probs.description_prob.is_none() {
            if !has_title {
                probs.description_prob = probs.text_prob;
            } else {
                let desc_req = FilterPredictRequest {
                    description: target.description.clone(),
                    ..Default::default()
                };
                if let Ok(dr) = post_predict_request(mirror, predict_url, &desc_req).await {
                    probs.description_prob = Some(dr.prob);
                }
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
            let img_req = FilterPredictRequest {
                images_base64: vec![b64.clone()],
                ..Default::default()
            };
            match post_predict_request(mirror, predict_url, &img_req).await {
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
    loaded_images_count: usize,
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
    let n_imgs = loaded_images_count.max(probs.per_image_probs.len());

    if !cfg.subdivide_modalities {
        let text_sensitive = has_text || n_imgs == 0;
        let title_sensitive = has_title || (!has_desc && text_sensitive);
        let description_sensitive = has_desc;
        let image_sensitive = n_imgs > 0;
        let sensitive_image_indices = (0..n_imgs).collect();
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

    let mut sensitive_image_indices: Vec<usize> = probs
        .per_image_probs
        .iter()
        .enumerate()
        .filter_map(|(idx, &p)| (p >= m_t).then_some(idx))
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
                probs
                    .per_image_probs
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
        if !probs.per_image_probs.is_empty() {
            let over_floor: Vec<usize> = probs
                .per_image_probs
                .iter()
                .enumerate()
                .filter_map(|(i, &p)| (p >= 0.15).then_some(i))
                .collect();
            if !over_floor.is_empty() {
                sensitive_image_indices = over_floor;
            } else if let Some((best_idx, _)) = probs
                .per_image_probs
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
            {
                sensitive_image_indices.push(best_idx);
            }
        } else {
            sensitive_image_indices = (0..n_imgs).collect();
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
