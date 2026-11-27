use crate::config::{CatalogConfigStore, FilterSection};
use crate::credentials::write_private_file;
use crate::db::ServerRepo;
use crate::image_prep::{
    self, FILTER_MAX_IMAGE_BYTES as MAX_IMAGE_BYTES,
    FILTER_MAX_IMAGE_DIMENSION as MAX_IMAGE_DIMENSION, FILTER_MAX_IMAGE_PIXELS as MAX_IMAGE_PIXELS,
};
use base64::engine::general_purpose::{STANDARD as BASE64_STD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use l4a_logging::http::AuditedSend;
use l4a_protocol::{
    aggregate_gallery_triplets, merge_managed_predictions, CatalogFilterBackendKind,
    CatalogFilterPublicInfo, CatalogFilterStatus, FilterModelParams, FilterPredictRequest,
    FilterProbabilities, ManagedFilterPrediction, RawFilterPredictRequest, WorkshopItemStatus,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex as AsyncMutex, OwnedSemaphorePermit, RwLock as AsyncRwLock, Semaphore};
use url::{Host, Url};

const FILTER_RUNNER_PY: &str = include_str!("../../filter-assets/filter_runner.py");

const MAX_CACHE_ENTRIES: usize = 4_096;
const MAX_CONCURRENT_PREDICTS: usize = 4;
pub const MAX_PUBLIC_PREDICT_ADMISSIONS: usize = 8;
const DEFAULT_PUBLIC_ADMISSION_TIMEOUT_MS: u64 = 1_500;
pub const MAX_RUNNER_BATCH_RAW_IMAGE_BYTES: usize = 30 * 1024 * 1024;
const MAX_TITLE_BYTES: usize = 4_096;
const MAX_DESCRIPTION_BYTES: usize = 65_536;
const MAX_TAGS_COUNT: usize = 64;
const MAX_TAG_BYTES: usize = 256;
const MAX_BASE64_ENCODED_LEN: usize = (MAX_IMAGE_BYTES * 4 / 3) + 256;
const MAX_REQUEST_IMAGE_URLS: usize = 16;
const MAX_REDIRECT_HOPS: usize = 3;
const IMAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(8);
const RUNNER_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(10);
const PREDICT_TIMEOUT: Duration = Duration::from_secs(20);
const CRASH_RESTART_BACKOFF: Duration = Duration::from_secs(3);
const UPSTREAM_HEALTH_TTL: Duration = Duration::from_secs(60);
const MAX_STDERR_BYTES: usize = 8 * 1024;
const MAX_READY_LINE_BYTES: usize = 64 * 1024;
const MAX_BACKEND_RESPONSE_BYTES: usize = 64 * 1024;
const STOCK_LEFT4FILTERS_MODEL_NAME: &str = "Left4FiltersModel";
const STOCK_CORPSE_SCORES_REVISION: &str = "left4filters-corpse-scores";

#[derive(Debug, thiserror::Error)]
pub enum FilterServiceError {
    #[error("{0}")]
    Disabled(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    BackendRejected(String),
    #[error("{0}")]
    ImageFetch(String),
    #[error("内容过滤评分配置已更新，请重试")]
    ScoringChanged,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

#[derive(Debug, Clone)]
struct CacheEntry {
    probabilities: FilterProbabilities,
    inserted_at: Instant,
}

#[derive(Debug, Clone, Default)]
struct RuntimeSnapshot {
    initialized_config: Option<FilterSection>,
    last_scoring_signature: Option<String>,
    backoff_config: Option<FilterSection>,
    next_retry_after: Option<Instant>,
    upstream_verified_at: Option<Instant>,
    last_crash_respawn_at: Option<Instant>,
    ready: bool,
    running_pid: Option<u32>,
    active_endpoint: Option<String>,
    model: ModelIdentity,
    last_error: Option<String>,
}

impl RuntimeSnapshot {
    fn initialization_is_current(&self, cfg: &FilterSection) -> bool {
        if self.initialized_config.as_ref() == Some(cfg) {
            return !cfg.enabled
                || cfg.backend != CatalogFilterBackendKind::UpstreamHttp
                || self
                    .upstream_verified_at
                    .is_some_and(|at| at.elapsed() < UPSTREAM_HEALTH_TTL);
        }
        cfg.enabled
            && self.backoff_config.as_ref() == Some(cfg)
            && self.next_retry_after.is_some_and(|at| Instant::now() < at)
    }
}

struct ManagedPythonHandle {
    child: Child,
    stdin: Option<ChildStdin>,
    _runner_dir: TempDir,
    runner_token: String,
    endpoint: String,
    pid: Option<u32>,
    deploy_dir: PathBuf,
    python_bin: Option<String>,
    model: ModelIdentity,
    stderr_buf: Arc<StdMutex<String>>,
}

impl ManagedPythonHandle {
    async fn terminate(&mut self) {
        self.stdin.take();
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await;
    }
}

impl Drop for ManagedPythonHandle {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.start_kill();
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
struct ModelIdentity {
    #[serde(rename = "model", alias = "modelName", alias = "model_name")]
    name: Option<String>,
    #[serde(alias = "modelVersion", alias = "model_version")]
    version: Option<String>,
    #[serde(rename = "scoringRevision", alias = "scoring_revision")]
    revision: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RunnerHealthPayload {
    #[serde(flatten)]
    model: ModelIdentity,
    host: Option<String>,
    port: Option<u16>,
}

pub struct CatalogFilterService {
    config_store: Arc<CatalogConfigStore>,
    repo: Arc<ServerRepo>,
    lifecycle_lock: AsyncMutex<Option<ManagedPythonHandle>>,
    backend_execution: AsyncRwLock<()>,
    backend_generation: AtomicU64,
    public_admission_semaphore: Arc<Semaphore>,
    public_admission_timeout_ms: AtomicU64,
    predict_semaphore: Semaphore,
    runtime: RwLock<RuntimeSnapshot>,
    cache: StdMutex<HashMap<[u8; 32], CacheEntry>>,
    total_requests: AtomicU64,
    cache_hits: AtomicU64,
    failed_requests: AtomicU64,
    allow_loopback_image_urls: AtomicBool,
    backend_client: reqwest::Client,
    image_client: reqwest::Client,
    loopback_image_client: reqwest::Client,
}

impl CatalogFilterService {
    pub fn new(config_store: Arc<CatalogConfigStore>, repo: Arc<ServerRepo>) -> Arc<Self> {
        let backend_client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(PREDICT_TIMEOUT)
            .build()
            .expect("failed to build backend reqwest client");
        let user_agent = concat!("Left4Addons-Catalog/", env!("CARGO_PKG_VERSION"));
        let image_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(IMAGE_FETCH_TIMEOUT)
            .user_agent(user_agent)
            .build()
            .expect("failed to build image reqwest client");
        let loopback_image_client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(IMAGE_FETCH_TIMEOUT)
            .user_agent(user_agent)
            .build()
            .expect("failed to build loopback image reqwest client");

        let svc = Arc::new(Self {
            config_store,
            repo,
            lifecycle_lock: AsyncMutex::new(None),
            backend_execution: AsyncRwLock::new(()),
            backend_generation: AtomicU64::new(0),
            public_admission_semaphore: Arc::new(Semaphore::new(MAX_PUBLIC_PREDICT_ADMISSIONS)),
            public_admission_timeout_ms: AtomicU64::new(DEFAULT_PUBLIC_ADMISSION_TIMEOUT_MS),
            predict_semaphore: Semaphore::new(MAX_CONCURRENT_PREDICTS),
            runtime: RwLock::new(RuntimeSnapshot::default()),
            cache: StdMutex::new(HashMap::new()),
            total_requests: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            allow_loopback_image_urls: AtomicBool::new(false),
            backend_client,
            image_client,
            loopback_image_client,
        });

        if svc.config_store.get().filter.enabled {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let bg = svc.clone();
                handle.spawn(async move {
                    bg.ensure_initialized().await;
                });
            }
        }

        svc
    }

    pub fn is_enabled(&self) -> bool {
        self.config_store.get().filter.enabled
    }

    /// Both prediction routes share admission slots, including while reading their bodies.
    pub async fn acquire_request_admission(&self) -> Option<OwnedSemaphorePermit> {
        let timeout_ms = self.public_admission_timeout_ms.load(Ordering::Acquire);
        let sem = self.public_admission_semaphore.clone();
        if timeout_ms == 0 {
            return sem.try_acquire_owned().ok();
        }
        tokio::time::timeout(Duration::from_millis(timeout_ms), sem.acquire_owned())
            .await
            .ok()
            .and_then(Result::ok)
    }

    /// Overrides the public admission wait timeout strictly for integration tests.
    pub fn set_public_admission_timeout_for_testing(&self, timeout: Duration) {
        let ms = timeout.as_millis().min(u64::MAX as u128) as u64;
        self.public_admission_timeout_ms
            .store(ms, Ordering::Release);
    }

    /// Gracefully terminates any managed Python filter runner child process on server shutdown.
    pub async fn shutdown(&self) {
        let mut guard = self.lifecycle_lock.lock().await;
        let _execution = self.backend_execution.write().await;
        self.backend_generation.fetch_add(1, Ordering::AcqRel);
        if let Some(mut handle) = guard.take() {
            handle.terminate().await;
        }
        let mut snap = self.runtime.write().unwrap();
        snap.initialized_config = None;
        snap.backoff_config = None;
        snap.next_retry_after = None;
        snap.last_crash_respawn_at = None;
        snap.ready = false;
        snap.running_pid = None;
        snap.active_endpoint = None;
    }

    /// Enables `http://127.0.0.1` and `http://localhost` image URLs strictly for integration tests.
    /// Never enabled in production runtime or exposed via configuration/HTTP.
    pub fn set_allow_loopback_image_urls_for_testing(&self, allow: bool) {
        self.allow_loopback_image_urls
            .store(allow, Ordering::Release);
    }

    pub async fn public_info(&self) -> CatalogFilterPublicInfo {
        let cfg = self.current_filter_section();
        let params = cfg.params();
        let advertised_max_images = if params.enable_image {
            params.max_images
        } else {
            0
        };
        if !cfg.enabled {
            return CatalogFilterPublicInfo {
                enabled: false,
                ready: false,
                model_name: None,
                model_version: None,
                max_images: advertised_max_images,
                scoring_revision: None,
            };
        }

        self.ensure_initialized().await;
        self.refresh_managed_liveness();
        let _execution = self.backend_execution.read().await;
        let cfg = self.current_filter_section();
        let params = cfg.params();
        let advertised_max_images = if params.enable_image {
            params.max_images
        } else {
            0
        };
        if !cfg.enabled {
            return CatalogFilterPublicInfo {
                enabled: false,
                ready: false,
                model_name: None,
                model_version: None,
                max_images: advertised_max_images,
                scoring_revision: None,
            };
        }
        let snap = self.runtime.read().unwrap().clone();
        let ready = snap.ready && snap.initialized_config.as_ref() == Some(&cfg);
        let scoring_revision = ready.then(|| compute_scoring_revision(&cfg, &snap.model));
        CatalogFilterPublicInfo {
            enabled: true,
            ready,
            model_name: ready.then_some(snap.model.name).flatten(),
            model_version: ready.then_some(snap.model.version).flatten(),
            max_images: advertised_max_images,
            scoring_revision,
        }
    }

    pub async fn status(&self) -> CatalogFilterStatus {
        let cfg = self.current_filter_section();
        if cfg.enabled {
            self.ensure_initialized().await;
            self.refresh_managed_liveness();
        }
        let _execution = self.backend_execution.read().await;
        let cfg = self.current_filter_section();
        let snap = self.runtime.read().unwrap().clone();
        CatalogFilterStatus {
            config: cfg.to_dto(),
            ready: cfg.enabled && snap.ready && snap.initialized_config.as_ref() == Some(&cfg),
            running_pid: if cfg.enabled { snap.running_pid } else { None },
            active_endpoint: if cfg.enabled {
                snap.active_endpoint
            } else {
                None
            },
            model_name: if cfg.enabled { snap.model.name } else { None },
            model_version: if cfg.enabled {
                snap.model.version
            } else {
                None
            },
            total_requests: self.total_requests.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            failed_requests: self.failed_requests.load(Ordering::Relaxed),
            last_error: snap.last_error,
        }
    }

    pub async fn reconfigure(&self) -> CatalogFilterStatus {
        {
            let mut guard = self.lifecycle_lock.lock().await;
            let cfg = self.current_filter_section();
            self.clear_backoff_state();
            self.apply_lifecycle_locked(&mut guard, &cfg, false).await;
        }
        self.status().await
    }

    pub async fn restart(&self) -> CatalogFilterStatus {
        {
            let mut guard = self.lifecycle_lock.lock().await;
            let cfg = self.current_filter_section();
            self.clear_backoff_state();
            self.apply_lifecycle_locked(&mut guard, &cfg, true).await;
        }
        self.status().await
    }

    pub async fn predict(
        &self,
        req: FilterPredictRequest,
    ) -> Result<FilterProbabilities, FilterServiceError> {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        let result = match validate_predict_request_bounds(&req) {
            Ok(()) => match self.predict_once(&req).await {
                Err(FilterServiceError::ScoringChanged) => self.predict_once(&req).await,
                result => result,
            },
            Err(err) => Err(err),
        };
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let image_count =
            req.image_base64.iter().count() + req.images_base64.len() + req.image_urls.len();
        match &result {
            Ok(probs) => tracing::info!(
                workshop_id = ?req.workshop_id,
                title = %l4a_logging::redact::truncate(req.title.as_deref().unwrap_or_default(), 80),
                images = image_count,
                fusion_score = probs.fusion_score(),
                scoring_revision = probs.scoring_revision.as_deref().unwrap_or("-"),
                elapsed_ms,
                "Content filter inference completed"
            ),
            Err(err) => tracing::warn!(
                workshop_id = ?req.workshop_id,
                images = image_count,
                elapsed_ms,
                error = %err,
                "Content filter inference failed"
            ),
        }
        result
    }

    async fn predict_once(
        &self,
        req: &FilterPredictRequest,
    ) -> Result<FilterProbabilities, FilterServiceError> {
        if !self.is_enabled() {
            return Err(FilterServiceError::Disabled(
                "内容过滤服务未启用".to_string(),
            ));
        }

        self.ensure_initialized().await;
        self.refresh_managed_liveness();

        let cfg = self.current_filter_section();
        if !cfg.enabled {
            return Err(FilterServiceError::Disabled(
                "内容过滤服务未启用".to_string(),
            ));
        }
        let params = cfg.params();
        let allow_loopback = self.allow_loopback_image_urls.load(Ordering::Acquire);

        let max_imgs = if params.enable_image {
            params.max_images as usize
        } else {
            0
        };

        // 2. Slice `images_base64` and `image_urls` to `max_imgs` (`0` when `!enable_image`)
        //    BEFORE decoding base64 or validating/fetching URLs!
        let raw_b64_list: Vec<&str> = req
            .image_base64
            .iter()
            .chain(req.images_base64.iter())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        let raw_url_list: Vec<&str> = req
            .image_urls
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        let has_req_images = !raw_b64_list.is_empty() || !raw_url_list.is_empty();

        let b64_slice = &raw_b64_list[..raw_b64_list.len().min(max_imgs)];
        let mut decoded_b64_images = Vec::with_capacity(b64_slice.len());
        for raw_b64 in b64_slice {
            decoded_b64_images.push(decode_base64_image(raw_b64)?);
        }

        let remaining_slots = max_imgs.saturating_sub(decoded_b64_images.len());
        let url_slice = &raw_url_list[..raw_url_list.len().min(remaining_slots)];
        let mut effective_req_urls = Vec::with_capacity(url_slice.len());
        for raw_url in url_slice {
            effective_req_urls.push(validate_allowed_image_url(raw_url, allow_loopback)?);
        }

        // 3. Resolve optional `workshop_id` from public/active `ServerRepo` cache when fields are absent.
        let has_req_title = req.title.as_deref().is_some_and(|s| !s.trim().is_empty());
        let has_req_desc = req
            .description
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());
        let has_req_tags = !req.tags.is_empty();
        let has_req_text = has_req_title || has_req_desc || has_req_tags;

        let mut title = req.title.clone().unwrap_or_default();
        let mut description = req.description.clone().unwrap_or_default();
        let mut tags = req.tags.clone();
        let mut db_preview_urls: Vec<Url> = Vec::new();

        if let Some(wid) = req.workshop_id {
            if !has_req_title || !has_req_desc || !has_req_images {
                let repo = self.repo.clone();
                let stored = tokio::task::spawn_blocking(move || repo.get_item(wid))
                    .await
                    .map_err(|e| FilterServiceError::Internal(e.into()))?
                    .map_err(FilterServiceError::Internal)?;

                // Strictly restrict to public, active items: never expose private_or_unavailable,
                // removed tombstones, or unlisted (is_listed == Some(false)) records.
                let public_record = stored.and_then(|meta| {
                    if meta.record.status == WorkshopItemStatus::Active
                        && meta.is_listed != Some(false)
                    {
                        Some(meta.record)
                    } else {
                        None
                    }
                });

                if let Some(record) = public_record {
                    if !has_req_images && max_imgs > 0 {
                        db_preview_urls =
                            collect_item_preview_urls(&record, remaining_slots, allow_loopback);
                    }
                    if !has_req_title {
                        title = truncate_utf8_bytes(&record.title, MAX_TITLE_BYTES).to_string();
                    }
                    if !has_req_desc {
                        description = record
                            .description
                            .as_deref()
                            .map(|d| truncate_utf8_bytes(d, MAX_DESCRIPTION_BYTES).to_string())
                            .unwrap_or_default();
                    }
                    if !has_req_text && tags.is_empty() {
                        tags = record
                            .tags
                            .into_iter()
                            .take(MAX_TAGS_COUNT)
                            .map(|t| truncate_utf8_bytes(&t, MAX_TAG_BYTES).to_string())
                            .collect();
                    }
                } else if !has_req_text && !has_req_images {
                    return Err(FilterServiceError::NotFound(format!(
                        "未找到公开可用的工坊条目 {wid}"
                    )));
                }
            }
        }

        // 4. Enforce server-side `FilterModelParams` modality switches.
        let effective_title = if params.enable_text {
            title
        } else {
            String::new()
        };
        let effective_desc = if params.enable_text {
            description
        } else {
            String::new()
        };
        let effective_tags = if params.enable_text { tags } else { Vec::new() };
        let effective_db_urls: Vec<Url> = if effective_req_urls.is_empty() {
            db_preview_urls.into_iter().take(remaining_slots).collect()
        } else {
            Vec::new()
        };

        // 5. Verify backend availability before serving or caching.
        let (endpoint, scoring_revision, generation, cache_key) = {
            let _execution = self.backend_execution.read().await;
            let snap = self.runtime.read().unwrap();
            if !snap.ready {
                let detail = snap
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "模型推理后端尚未就绪".to_string());
                self.failed_requests.fetch_add(1, Ordering::Relaxed);
                return Err(FilterServiceError::Unavailable(format!(
                    "内容过滤模型后端暂不可用: {detail}"
                )));
            }
            if snap.initialized_config.as_ref() != Some(&cfg) {
                return Err(FilterServiceError::ScoringChanged);
            }
            let Some(ref ep) = snap.active_endpoint else {
                self.failed_requests.fetch_add(1, Ordering::Relaxed);
                return Err(FilterServiceError::Unavailable(
                    "内容过滤模型后端未绑定有效地址".to_string(),
                ));
            };
            let rev = compute_scoring_revision(&cfg, &snap.model);
            // Check the cache while the backend and its revision are still verified.
            let cache_key = compute_cache_key(
                &cfg,
                &rev,
                &effective_title,
                &effective_desc,
                &effective_tags,
                &decoded_b64_images,
                &effective_req_urls,
                &effective_db_urls,
            );
            if cfg.cache_ttl_secs > 0 {
                if let Some(hit) =
                    self.get_cached(&cache_key, Duration::from_secs(cfg.cache_ttl_secs))
                {
                    self.cache_hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(hit);
                }
            }
            (
                ep.clone(),
                rev,
                self.backend_generation.load(Ordering::Acquire),
                cache_key,
            )
        };

        // 7. Fetch remote images (strictly SSRF-checked + header/dimension bomb validated).
        let mut final_images = decoded_b64_images;
        for url in &effective_req_urls {
            let bytes = self
                .fetch_allowed_image(url, allow_loopback)
                .await
                .map_err(|err| FilterServiceError::ImageFetch(err.to_string()))?;
            final_images.push(bytes);
        }
        let mut db_image_fetch_failed = false;
        for url in &effective_db_urls {
            match self.fetch_allowed_image(url, allow_loopback).await {
                Ok(bytes) => final_images.push(bytes),
                Err(err) => {
                    db_image_fetch_failed = true;
                    tracing::debug!(url = %url, error = %err, "Skipping Workshop cached preview image that could not be fetched");
                }
            }
        }

        // Hold the backend generation through every inference call, including gallery subbatches.
        let _execution = self.backend_execution.read().await;
        if self.backend_generation.load(Ordering::Acquire) != generation
            || self.current_filter_section() != cfg
        {
            return Err(FilterServiceError::ScoringChanged);
        }
        let has_txt = !effective_title.trim().is_empty()
            || !effective_desc.trim().is_empty()
            || !effective_tags.is_empty();
        if !has_txt && final_images.is_empty() && db_image_fetch_failed {
            return Err(FilterServiceError::ImageFetch(
                "无法拉取任何工坊预览图".to_string(),
            ));
        }
        let mut probabilities = if !has_txt && final_images.is_empty() {
            FilterProbabilities::default()
        } else {
            match cfg.backend {
                // ManagedPython runner already applies `FilterModelParams` calibration & unimodal breakdown.
                CatalogFilterBackendKind::ManagedPython => {
                    self.predict_batched_backend(
                        &endpoint,
                        &effective_title,
                        &effective_desc,
                        &effective_tags,
                        &final_images,
                        &params,
                    )
                    .await?
                }
                CatalogFilterBackendKind::UpstreamHttp => {
                    if final_images.len() > 1 {
                        self.predict_upstream_gallery(
                            &endpoint,
                            &effective_title,
                            &effective_desc,
                            &effective_tags,
                            &final_images,
                            &params,
                        )
                        .await?
                    } else {
                        let encoded_images: Vec<String> =
                            final_images.iter().map(|b| BASE64_STD.encode(b)).collect();
                        let mut raw_probs = self
                            .call_backend_predict(
                                &endpoint,
                                &effective_title,
                                &effective_desc,
                                &effective_tags,
                                &encoded_images,
                            )
                            .await?;
                        self.backfill_upstream_unimodal(
                            &endpoint,
                            &effective_title,
                            &effective_desc,
                            &effective_tags,
                            &encoded_images,
                            &mut raw_probs,
                        )
                        .await;
                        let mut adjusted = params.adjust_probabilities(&raw_probs);
                        if adjusted.per_image_probs.len() == 1 {
                            if let Some(ip) = adjusted.image_prob {
                                adjusted.per_image_probs[0] = ip;
                            }
                        }
                        adjusted
                    }
                }
            }
        };
        let snap = self.runtime.read().unwrap();
        if !snap.ready || self.backend_generation.load(Ordering::Acquire) != generation {
            return Err(FilterServiceError::ScoringChanged);
        }
        probabilities.scoring_revision = Some(scoring_revision);

        // Never cache degraded partial text-only results when a DB preview image failed transiently!
        let cache_ttl = Duration::from_secs(cfg.cache_ttl_secs);
        if !cache_ttl.is_zero() && !db_image_fetch_failed {
            self.put_cached(cache_key, probabilities.clone(), cache_ttl);
        }

        Ok(probabilities)
    }

    async fn predict_batched_backend(
        &self,
        endpoint: &str,
        title: &str,
        description: &str,
        tags: &[String],
        images: &[Vec<u8>],
        params: &FilterModelParams,
    ) -> Result<FilterProbabilities, FilterServiceError> {
        if images.is_empty() {
            return self
                .call_backend_predict(endpoint, title, description, tags, &[])
                .await;
        }

        let ranges = split_image_ranges_by_bytes(images, MAX_RUNNER_BATCH_RAW_IMAGE_BYTES);
        let mut parts = Vec::with_capacity(ranges.len());
        for range in ranges {
            let count = range.len();
            let encoded_batch: Vec<String> =
                images[range].iter().map(|b| BASE64_STD.encode(b)).collect();
            let part = self
                .call_backend_prediction(endpoint, title, description, tags, &encoded_batch, true)
                .await?;
            parts.push((count, part));
        }
        merge_managed_predictions(parts, params).ok_or_else(|| {
            FilterServiceError::Unavailable("托管过滤模型未返回完整的逐图评分".into())
        })
    }

    async fn predict_upstream_gallery(
        &self,
        endpoint: &str,
        title: &str,
        description: &str,
        tags: &[String],
        images: &[Vec<u8>],
        params: &FilterModelParams,
    ) -> Result<FilterProbabilities, FilterServiceError> {
        let has_txt =
            !title.trim().is_empty() || !description.trim().is_empty() || !tags.is_empty();
        let mut image_rows = Vec::with_capacity(images.len());
        let mut joint_rows = Vec::with_capacity(if has_txt { images.len() } else { 0 });
        let mut corpse_scores = FilterProbabilities::default();

        let mut text_prob = None;
        let mut text_gore_prob = None;
        let mut text_nsfw_prob = None;
        let mut title_prob = None;
        let mut description_prob = None;

        for (index, img_bytes) in images.iter().enumerate() {
            let b64 = BASE64_STD.encode(img_bytes);
            let single_slice = std::slice::from_ref(&b64);

            let img_res = self
                .call_backend_predict(endpoint, "", "", &[], single_slice)
                .await?;
            corpse_scores.merge_corpse_scores(&img_res, index, 1);
            image_rows.push([img_res.fusion_score(), img_res.gore_prob, img_res.nsfw_prob]);

            if has_txt {
                let joint_res = self
                    .call_backend_predict(endpoint, title, description, tags, single_slice)
                    .await?;
                if text_prob.is_none() && joint_res.text_prob.is_some() {
                    text_prob = joint_res.text_prob;
                    text_gore_prob = joint_res.text_gore_prob;
                    text_nsfw_prob = joint_res.text_nsfw_prob;
                    title_prob = joint_res.title_prob;
                    description_prob = joint_res.description_prob;
                }
                joint_rows.push([
                    joint_res.fusion_score(),
                    joint_res.gore_prob,
                    joint_res.nsfw_prob,
                ]);
            }
        }

        if has_txt && text_prob.is_none() {
            if let Ok(txt_res) = self
                .call_backend_predict_inner(endpoint, title, description, tags, &[], false)
                .await
            {
                text_prob = Some(txt_res.prob);
                text_gore_prob = Some(txt_res.gore_prob);
                text_nsfw_prob = Some(txt_res.nsfw_prob);
                if title_prob.is_none() {
                    title_prob = txt_res.title_prob;
                }
                if description_prob.is_none() {
                    description_prob = txt_res.description_prob;
                }
            }
        }

        let img_agg = aggregate_gallery_triplets(&image_rows, params.gallery_aggregation);
        let joint_agg = if has_txt {
            aggregate_gallery_triplets(&joint_rows, params.gallery_aggregation)
        } else {
            img_agg
        };

        let mut raw_probs = FilterProbabilities {
            gore_prob: round6_prob(joint_agg[1]),
            nsfw_prob: round6_prob(joint_agg[2]),
            text_prob,
            text_gore_prob,
            text_nsfw_prob,
            title_prob,
            description_prob,
            image_prob: Some(round6_prob(img_agg[0])),
            image_gore_prob: Some(round6_prob(img_agg[1])),
            image_nsfw_prob: Some(round6_prob(img_agg[2])),
            per_image_probs: image_rows.iter().map(|r| round6_prob(r[0])).collect(),
            ..corpse_scores
        };
        raw_probs.set_fusion_score(round6_prob(joint_agg[0]));

        let mut adjusted = params.adjust_probabilities(&raw_probs);
        if !params.is_identity_calibration() {
            adjusted.per_image_probs = image_rows
                .iter()
                .map(|r| params.adjust_triplet(r[0], r[1], r[2]).0)
                .collect();
        }
        Ok(adjusted)
    }

    async fn backfill_upstream_unimodal(
        &self,
        endpoint: &str,
        title: &str,
        description: &str,
        tags: &[String],
        encoded_images: &[String],
        raw_probs: &mut FilterProbabilities,
    ) {
        let has_txt =
            !title.trim().is_empty() || !description.trim().is_empty() || !tags.is_empty();
        if has_txt && encoded_images.is_empty() {
            if raw_probs.text_prob.is_none() {
                raw_probs.text_prob = Some(raw_probs.prob);
                raw_probs.text_gore_prob = Some(raw_probs.gore_prob);
                raw_probs.text_nsfw_prob = Some(raw_probs.nsfw_prob);
            }
            return;
        }
        if !has_txt && !encoded_images.is_empty() {
            if raw_probs.image_prob.is_none() {
                raw_probs.image_prob = Some(raw_probs.fusion_score());
                raw_probs.image_gore_prob = Some(raw_probs.gore_prob);
                raw_probs.image_nsfw_prob = Some(raw_probs.nsfw_prob);
                if raw_probs.per_image_probs.is_empty() && encoded_images.len() == 1 {
                    raw_probs.per_image_probs = vec![raw_probs.fusion_score()];
                }
            }
            return;
        }
        if !has_txt || encoded_images.is_empty() {
            return;
        }

        let need_text = raw_probs.text_prob.is_none();
        let need_image = raw_probs.image_prob.is_none();
        if !need_text && !need_image {
            return;
        }

        let text_fut = async {
            if need_text {
                self.call_backend_predict_inner(endpoint, title, description, tags, &[], false)
                    .await
                    .ok()
            } else {
                None
            }
        };
        let img_fut = async {
            if need_image {
                self.call_backend_predict_inner(endpoint, "", "", &[], encoded_images, false)
                    .await
                    .ok()
            } else {
                None
            }
        };

        let (txt_opt, img_opt) = tokio::join!(text_fut, img_fut);
        if let Some(txt_res) = txt_opt {
            raw_probs.text_prob = Some(txt_res.prob);
            raw_probs.text_gore_prob = Some(txt_res.gore_prob);
            raw_probs.text_nsfw_prob = Some(txt_res.nsfw_prob);
            if raw_probs.title_prob.is_none() {
                raw_probs.title_prob = txt_res.title_prob;
            }
            if raw_probs.description_prob.is_none() {
                raw_probs.description_prob = txt_res.description_prob;
            }
        }
        if let Some(img_res) = img_opt {
            let image_fusion = img_res.fusion_score();
            raw_probs.image_prob = Some(image_fusion);
            raw_probs.image_gore_prob = Some(img_res.gore_prob);
            raw_probs.image_nsfw_prob = Some(img_res.nsfw_prob);
            if raw_probs.per_image_probs.is_empty() {
                if !img_res.per_image_probs.is_empty() {
                    raw_probs.per_image_probs = img_res.per_image_probs;
                } else if encoded_images.len() == 1 {
                    raw_probs.per_image_probs = vec![image_fusion];
                }
            }
        }
    }

    fn current_filter_section(&self) -> FilterSection {
        let mut section = self.config_store.get().filter;
        section.normalize();
        section
    }

    fn clear_backoff_state(&self) {
        let mut snap = self.runtime.write().unwrap();
        snap.backoff_config = None;
        snap.next_retry_after = None;
        snap.last_crash_respawn_at = None;
    }

    async fn ensure_initialized(&self) {
        self.refresh_managed_liveness();
        {
            let fast_cfg = self.current_filter_section();
            let snap = self.runtime.read().unwrap();
            if snap.initialization_is_current(&fast_cfg) {
                return;
            }
        }

        let mut guard = self.lifecycle_lock.lock().await;
        self.reap_exited_child_locked(&mut guard);
        let cfg = self.current_filter_section();
        {
            let snap = self.runtime.read().unwrap();
            if snap.initialization_is_current(&cfg) {
                return;
            }
        }
        self.apply_lifecycle_locked(&mut guard, &cfg, false).await;
    }

    fn refresh_managed_liveness(&self) {
        let Ok(mut guard) = self.lifecycle_lock.try_lock() else {
            return;
        };
        self.reap_exited_child_locked(&mut guard);
    }

    fn reap_exited_child_locked(&self, guard: &mut Option<ManagedPythonHandle>) {
        let Some(ref mut handle) = *guard else {
            return;
        };
        if let Ok(Some(exit_status)) = handle.child.try_wait() {
            let stderr_tail = handle.stderr_buf.lock().unwrap().trim().to_string();
            let msg = if stderr_tail.is_empty() {
                format!("托管 Python 过滤进程已退出 ({exit_status})")
            } else {
                format!("托管 Python 过滤进程已退出 ({exit_status}): {stderr_tail}")
            };
            let current_cfg = self.current_filter_section();
            let now = Instant::now();
            let mut snap = self.runtime.write().unwrap();
            self.backend_generation.fetch_add(1, Ordering::AcqRel);
            snap.initialized_config = None;
            snap.ready = false;
            snap.running_pid = None;
            snap.active_endpoint = None;
            snap.last_error = Some(msg);
            if snap
                .last_crash_respawn_at
                .is_some_and(|prev| now.duration_since(prev) < CRASH_RESTART_BACKOFF)
            {
                snap.backoff_config = Some(current_cfg);
                snap.next_retry_after = Some(now + CRASH_RESTART_BACKOFF);
            } else {
                snap.last_crash_respawn_at = Some(now);
                snap.backoff_config = None;
                snap.next_retry_after = None;
            }
            *guard = None;
        }
    }

    async fn apply_lifecycle_locked(
        &self,
        guard: &mut Option<ManagedPythonHandle>,
        cfg: &FilterSection,
        force_restart: bool,
    ) {
        // Routine UpstreamHttp health TTL refresh: probe `/health` without stalling in-flight
        // predictions or bumping `backend_generation` unless upstream fails or model metadata changed.
        if !force_restart
            && cfg.enabled
            && cfg.backend == CatalogFilterBackendKind::UpstreamHttp
            && guard.is_none()
        {
            if let Some(ref raw_url) = cfg.upstream_url {
                if let Ok(base_url) = validate_upstream_base_url(raw_url) {
                    let routine_refresh = {
                        let snap = self.runtime.read().unwrap();
                        if snap.ready
                            && snap.initialized_config.as_ref() == Some(cfg)
                            && snap.active_endpoint.as_deref() == Some(base_url.as_str())
                        {
                            Some(snap.model.clone())
                        } else {
                            None
                        }
                    };
                    if let Some(prev_model) = routine_refresh {
                        match self.probe_upstream_health(&base_url).await {
                            Ok(model) => {
                                if model == prev_model {
                                    let mut snap = self.runtime.write().unwrap();
                                    if snap.ready
                                        && snap.initialized_config.as_ref() == Some(cfg)
                                        && snap.active_endpoint.as_deref()
                                            == Some(base_url.as_str())
                                    {
                                        snap.upstream_verified_at = Some(Instant::now());
                                        snap.last_error = None;
                                        return;
                                    }
                                }
                                let _execution = self.backend_execution.write().await;
                                self.backend_generation.fetch_add(1, Ordering::AcqRel);
                                let prev_sig =
                                    self.runtime.read().unwrap().last_scoring_signature.clone();
                                let next_sig = if cfg.cache_ttl_secs > 0 {
                                    let rev = compute_scoring_revision(cfg, &model);
                                    if prev_sig.as_deref() != Some(rev.as_str()) {
                                        self.clear_cache();
                                    }
                                    Some(rev)
                                } else {
                                    self.clear_cache();
                                    None
                                };
                                *self.runtime.write().unwrap() = RuntimeSnapshot {
                                    initialized_config: Some(cfg.clone()),
                                    last_scoring_signature: next_sig,
                                    backoff_config: None,
                                    next_retry_after: None,
                                    upstream_verified_at: Some(Instant::now()),
                                    last_crash_respawn_at: None,
                                    ready: true,
                                    running_pid: None,
                                    active_endpoint: Some(base_url),
                                    model,
                                    last_error: None,
                                };
                                return;
                            }
                            Err(err) => {
                                let _execution = self.backend_execution.write().await;
                                self.backend_generation.fetch_add(1, Ordering::AcqRel);
                                let prev_sig =
                                    self.runtime.read().unwrap().last_scoring_signature.clone();
                                let unverified_rev =
                                    compute_scoring_revision(cfg, &ModelIdentity::default());
                                let next_sig = if cfg.cache_ttl_secs > 0 {
                                    if prev_sig.as_deref() != Some(unverified_rev.as_str()) {
                                        self.clear_cache();
                                    }
                                    Some(unverified_rev)
                                } else {
                                    self.clear_cache();
                                    None
                                };
                                *self.runtime.write().unwrap() = RuntimeSnapshot {
                                    initialized_config: None,
                                    last_scoring_signature: next_sig,
                                    backoff_config: Some(cfg.clone()),
                                    next_retry_after: Some(Instant::now() + CRASH_RESTART_BACKOFF),
                                    upstream_verified_at: None,
                                    last_crash_respawn_at: None,
                                    ready: false,
                                    running_pid: None,
                                    active_endpoint: Some(base_url),
                                    model: ModelIdentity::default(),
                                    last_error: Some(err),
                                };
                                return;
                            }
                        }
                    }
                }
            }
        }

        let _execution = self.backend_execution.write().await;
        self.backend_generation.fetch_add(1, Ordering::AcqRel);
        let prev_sig = self.runtime.read().unwrap().last_scoring_signature.clone();
        let unverified_rev = compute_scoring_revision(cfg, &ModelIdentity::default());
        if force_restart || !cfg.enabled || cfg.cache_ttl_secs == 0 {
            self.clear_cache();
        }
        let retained_for = |model: &ModelIdentity| {
            if cfg.enabled && cfg.cache_ttl_secs > 0 {
                let rev = compute_scoring_revision(cfg, model);
                if prev_sig.as_deref() != Some(rev.as_str()) {
                    self.clear_cache();
                }
                Some(rev)
            } else {
                None
            }
        };
        let retained_unverified = || {
            if cfg.enabled && cfg.cache_ttl_secs > 0 {
                if prev_sig.as_deref() != Some(unverified_rev.as_str()) {
                    self.clear_cache();
                }
                Some(unverified_rev.clone())
            } else {
                None
            }
        };

        if !cfg.enabled {
            if let Some(mut old) = guard.take() {
                old.terminate().await;
            }
            let mut snap = self.runtime.write().unwrap();
            *snap = RuntimeSnapshot {
                initialized_config: Some(cfg.clone()),
                last_scoring_signature: None,
                backoff_config: None,
                next_retry_after: None,
                upstream_verified_at: None,
                last_crash_respawn_at: None,
                ready: false,
                running_pid: None,
                active_endpoint: None,
                model: ModelIdentity::default(),
                last_error: None,
            };
            return;
        }

        match cfg.backend {
            CatalogFilterBackendKind::ManagedPython => {
                let Some(ref deploy_dir) = cfg.deploy_dir else {
                    if let Some(mut old) = guard.take() {
                        old.terminate().await;
                    }
                    let next_sig = retained_unverified();
                    let mut snap = self.runtime.write().unwrap();
                    *snap = RuntimeSnapshot {
                        initialized_config: None,
                        last_scoring_signature: next_sig,
                        backoff_config: Some(cfg.clone()),
                        next_retry_after: Some(Instant::now() + CRASH_RESTART_BACKOFF),
                        upstream_verified_at: None,
                        last_crash_respawn_at: snap.last_crash_respawn_at,
                        ready: false,
                        running_pid: None,
                        active_endpoint: None,
                        model: ModelIdentity::default(),
                        last_error: Some(
                            "未配置 Left4Filters 部署路径 (filter.deploy_dir)".to_string(),
                        ),
                    };
                    return;
                };

                // Attempt live `POST /configure` if the same ManagedPython child is still alive.
                if !force_restart {
                    if let Some(ref mut existing) = *guard {
                        let same_target = existing.deploy_dir == *deploy_dir
                            && existing.python_bin == cfg.python_bin;
                        let alive = matches!(existing.child.try_wait(), Ok(None));
                        if same_target && alive {
                            match self
                                .push_live_params(
                                    &existing.endpoint,
                                    &existing.runner_token,
                                    &cfg.params(),
                                )
                                .await
                            {
                                Ok(()) => {
                                    let next_sig = retained_for(&existing.model);
                                    let mut snap = self.runtime.write().unwrap();
                                    snap.initialized_config = Some(cfg.clone());
                                    snap.last_scoring_signature = next_sig;
                                    snap.backoff_config = None;
                                    snap.next_retry_after = None;
                                    snap.ready = true;
                                    snap.running_pid = existing.pid;
                                    snap.active_endpoint = Some(existing.endpoint.clone());
                                    snap.model = existing.model.clone();
                                    snap.last_error = None;
                                    return;
                                }
                                Err(err) => {
                                    tracing::warn!(
                                        error = %err,
                                        "Failed to hot-reload Python filter parameters; restarting child process"
                                    );
                                }
                            }
                        }
                    }
                }

                if let Some(mut old) = guard.take() {
                    old.terminate().await;
                }

                match spawn_managed_python_runner(
                    deploy_dir,
                    cfg.python_bin.as_deref(),
                    &cfg.params(),
                )
                .await
                {
                    Ok(handle) => {
                        let next_sig = retained_for(&handle.model);
                        let next_crash_ts = if force_restart {
                            None
                        } else {
                            self.runtime
                                .read()
                                .unwrap()
                                .last_crash_respawn_at
                                .map(|_| Instant::now())
                        };
                        let snap_next = RuntimeSnapshot {
                            initialized_config: Some(cfg.clone()),
                            last_scoring_signature: next_sig,
                            backoff_config: None,
                            next_retry_after: None,
                            upstream_verified_at: None,
                            last_crash_respawn_at: next_crash_ts,
                            ready: true,
                            running_pid: handle.pid,
                            active_endpoint: Some(handle.endpoint.clone()),
                            model: handle.model.clone(),
                            last_error: None,
                        };
                        *guard = Some(handle);
                        *self.runtime.write().unwrap() = snap_next;
                    }
                    Err(err) => {
                        let next_sig = retained_unverified();
                        let prev_crash_ts = self.runtime.read().unwrap().last_crash_respawn_at;
                        *self.runtime.write().unwrap() = RuntimeSnapshot {
                            initialized_config: None,
                            last_scoring_signature: next_sig,
                            backoff_config: Some(cfg.clone()),
                            next_retry_after: Some(Instant::now() + CRASH_RESTART_BACKOFF),
                            upstream_verified_at: None,
                            last_crash_respawn_at: prev_crash_ts,
                            ready: false,
                            running_pid: None,
                            active_endpoint: None,
                            model: ModelIdentity::default(),
                            last_error: Some(err),
                        };
                    }
                }
            }
            CatalogFilterBackendKind::UpstreamHttp => {
                if let Some(mut old) = guard.take() {
                    old.terminate().await;
                }

                let Some(ref raw_url) = cfg.upstream_url else {
                    let next_sig = retained_unverified();
                    *self.runtime.write().unwrap() = RuntimeSnapshot {
                        initialized_config: None,
                        last_scoring_signature: next_sig,
                        backoff_config: Some(cfg.clone()),
                        next_retry_after: Some(Instant::now() + CRASH_RESTART_BACKOFF),
                        upstream_verified_at: None,
                        last_crash_respawn_at: None,
                        ready: false,
                        running_pid: None,
                        active_endpoint: None,
                        model: ModelIdentity::default(),
                        last_error: Some(
                            "未配置外部过滤推理服务地址 (filter.upstream_url)".to_string(),
                        ),
                    };
                    return;
                };

                let base_url = match validate_upstream_base_url(raw_url) {
                    Ok(u) => u,
                    Err(err) => {
                        let next_sig = retained_unverified();
                        *self.runtime.write().unwrap() = RuntimeSnapshot {
                            initialized_config: None,
                            last_scoring_signature: next_sig,
                            backoff_config: Some(cfg.clone()),
                            next_retry_after: Some(Instant::now() + CRASH_RESTART_BACKOFF),
                            upstream_verified_at: None,
                            last_crash_respawn_at: None,
                            ready: false,
                            running_pid: None,
                            active_endpoint: None,
                            model: ModelIdentity::default(),
                            last_error: Some(err),
                        };
                        return;
                    }
                };

                match self.probe_upstream_health(&base_url).await {
                    Ok(model) => {
                        let next_sig = retained_for(&model);
                        *self.runtime.write().unwrap() = RuntimeSnapshot {
                            initialized_config: Some(cfg.clone()),
                            last_scoring_signature: next_sig,
                            backoff_config: None,
                            next_retry_after: None,
                            upstream_verified_at: Some(Instant::now()),
                            last_crash_respawn_at: None,
                            ready: true,
                            running_pid: None,
                            active_endpoint: Some(base_url),
                            model,
                            last_error: None,
                        };
                    }
                    Err(err) => {
                        let next_sig = retained_unverified();
                        *self.runtime.write().unwrap() = RuntimeSnapshot {
                            initialized_config: None,
                            last_scoring_signature: next_sig,
                            backoff_config: Some(cfg.clone()),
                            next_retry_after: Some(Instant::now() + CRASH_RESTART_BACKOFF),
                            upstream_verified_at: None,
                            last_crash_respawn_at: None,
                            ready: false,
                            running_pid: None,
                            active_endpoint: Some(base_url),
                            model: ModelIdentity::default(),
                            last_error: Some(err),
                        };
                    }
                }
            }
        }
    }

    async fn push_live_params(
        &self,
        endpoint: &str,
        runner_token: &str,
        params: &FilterModelParams,
    ) -> Result<(), String> {
        let url = format!("{}/configure", endpoint.trim_end_matches('/'));
        let resp = self
            .backend_client
            .post(&url)
            .timeout(CONFIGURE_TIMEOUT)
            .header("Authorization", format!("Bearer {runner_token}"))
            .json(&params.sanitized())
            .send_audited("filter.backend.configure")
            .await
            .map_err(|e| format!("请求 /configure 失败: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = read_lossy_response_snippet(resp, MAX_BACKEND_RESPONSE_BYTES).await;
            return Err(format!("/configure 返回 {status}: {text}"));
        }
        let _ = read_bounded_response_bytes(resp, MAX_BACKEND_RESPONSE_BYTES).await?;
        Ok(())
    }

    async fn probe_upstream_health(&self, base_url: &str) -> Result<ModelIdentity, String> {
        let url = format!("{}/health", base_url.trim_end_matches('/'));
        let resp = self
            .backend_client
            .get(&url)
            .timeout(HEALTH_CHECK_TIMEOUT)
            .send_audited_quiet("filter.backend.health")
            .await
            .map_err(|e| format!("连接上游过滤服务 {url} 失败: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let detail = read_lossy_response_snippet(resp, MAX_BACKEND_RESPONSE_BYTES).await;
            if detail.is_empty() {
                return Err(format!("上游过滤服务健康检查返回 HTTP {status}"));
            }
            return Err(format!("上游过滤服务健康检查返回 HTTP {status}: {detail}"));
        }
        let body = read_bounded_response_bytes(resp, MAX_BACKEND_RESPONSE_BYTES)
            .await
            .map_err(|e| format!("读取上游健康检查响应失败: {e}"))?;
        let mut model = serde_json::from_slice::<ModelIdentity>(&body).unwrap_or_default();
        if model.name.as_deref() == Some(STOCK_LEFT4FILTERS_MODEL_NAME)
            && model.version.is_none()
            && model.revision.is_none()
        {
            model.revision = match self.upstream_has_corpse_scores(base_url).await {
                Some(true) => Some(STOCK_CORPSE_SCORES_REVISION.into()),
                Some(false) => None,
                // A transient probe failure must not change the scoring revision. Reuse the
                // revision verified for the same endpoint and model, if there is one.
                None => {
                    let snap = self.runtime.read().unwrap();
                    (snap.active_endpoint.as_deref() == Some(base_url)
                        && snap.model.name == model.name
                        && snap.model.version.is_none())
                    .then(|| snap.model.revision.clone())
                    .flatten()
                }
            };
        }
        Ok(model)
    }

    // Stock Left4Filters /health omits the version. An empty prediction checks
    // for the new output fields without running image or text inference.
    // Returns `None` when the probe fails for a reason that may be transient.
    async fn upstream_has_corpse_scores(&self, base_url: &str) -> Option<bool> {
        let response = self
            .backend_client
            .post(format!("{}/predict", base_url.trim_end_matches('/')))
            .timeout(HEALTH_CHECK_TIMEOUT)
            .json(&RawFilterPredictRequest::default())
            .send_audited_quiet("filter.backend.probe_corpse")
            .await
            .ok()?;
        let status = response.status();
        let transient_client_error = matches!(
            status,
            reqwest::StatusCode::REQUEST_TIMEOUT | reqwest::StatusCode::TOO_MANY_REQUESTS
        );
        if status.is_client_error() && !transient_client_error {
            // Older nodes may reject an empty prediction. That answer is deterministic.
            return Some(false);
        }
        if !status.is_success() {
            return None;
        }
        let body = read_bounded_response_bytes(response, MAX_BACKEND_RESPONSE_BYTES)
            .await
            .ok()?;
        let scores = serde_json::from_slice::<FilterProbabilities>(&body).ok()?;
        Some(scores.corpse_prob.is_some() && scores.corpse_pile_prob.is_some())
    }

    async fn call_backend_predict(
        &self,
        endpoint: &str,
        title: &str,
        description: &str,
        tags: &[String],
        images_base64: &[String],
    ) -> Result<FilterProbabilities, FilterServiceError> {
        self.call_backend_predict_inner(endpoint, title, description, tags, images_base64, true)
            .await
    }

    async fn call_backend_predict_inner(
        &self,
        endpoint: &str,
        title: &str,
        description: &str,
        tags: &[String],
        images_base64: &[String],
        record_failure: bool,
    ) -> Result<FilterProbabilities, FilterServiceError> {
        Ok(self
            .call_backend_prediction(
                endpoint,
                title,
                description,
                tags,
                images_base64,
                record_failure,
            )
            .await?
            .probabilities)
    }

    async fn call_backend_prediction(
        &self,
        endpoint: &str,
        title: &str,
        description: &str,
        tags: &[String],
        images_base64: &[String],
        record_failure: bool,
    ) -> Result<ManagedFilterPrediction, FilterServiceError> {
        let _permit = self.predict_semaphore.acquire().await.map_err(|_| {
            FilterServiceError::Unavailable("内容过滤推理并发限流器已关闭".to_string())
        })?;

        let url = format!("{}/predict", endpoint.trim_end_matches('/'));
        let payload = RawFilterPredictRequest {
            title: Some(title.to_string()),
            description: Some(description.to_string()),
            tags: tags.to_vec(),
            image_base64: None,
            images_base64: images_base64.to_vec(),
        };

        let resp = match self
            .backend_client
            .post(&url)
            .json(&payload)
            .send_audited_quiet("filter.backend.predict")
            .await
        {
            Ok(r) => r,
            Err(err) => {
                self.refresh_managed_liveness();
                let msg = format!("调用过滤推理后端失败: {err}");
                if record_failure {
                    self.record_backend_failure(endpoint, msg.clone());
                }
                return Err(FilterServiceError::Unavailable(msg));
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let detail = read_lossy_response_snippet(resp, MAX_BACKEND_RESPONSE_BYTES).await;
            let msg = format!("过滤推理后端返回 HTTP {status}: {detail}");
            if status.is_server_error() {
                if record_failure {
                    self.record_backend_failure(endpoint, msg.clone());
                }
                return Err(FilterServiceError::Unavailable(msg));
            }
            if record_failure {
                self.failed_requests.fetch_add(1, Ordering::Relaxed);
            }
            return Err(FilterServiceError::BackendRejected(msg));
        }

        let body = match read_bounded_response_bytes(resp, MAX_BACKEND_RESPONSE_BYTES).await {
            Ok(b) => b,
            Err(err) => {
                let msg = format!("读取过滤推理后端响应失败: {err}");
                if record_failure {
                    self.record_backend_failure(endpoint, msg.clone());
                }
                return Err(FilterServiceError::Unavailable(msg));
            }
        };

        let mut prediction =
            serde_json::from_slice::<ManagedFilterPrediction>(&body).map_err(|err| {
                let msg = format!("解析过滤推理后端响应失败: {err}");
                if record_failure {
                    self.record_backend_failure(endpoint, msg.clone());
                }
                FilterServiceError::Unavailable(msg)
            })?;
        prediction
            .probabilities
            .infer_stock_corpse_review(images_base64.len());
        Ok(prediction)
    }

    fn record_backend_failure(&self, endpoint: &str, message: String) {
        self.failed_requests.fetch_add(1, Ordering::Relaxed);
        let mut snap = self.runtime.write().unwrap();
        if snap.active_endpoint.as_deref() != Some(endpoint) {
            return;
        }
        if snap
            .initialized_config
            .as_ref()
            .is_some_and(|cfg| cfg.backend == CatalogFilterBackendKind::UpstreamHttp)
        {
            self.backend_generation.fetch_add(1, Ordering::AcqRel);
            snap.backoff_config = snap.initialized_config.take();
            snap.upstream_verified_at = None;
            snap.next_retry_after = Some(Instant::now() + CRASH_RESTART_BACKOFF);
            snap.ready = false;
            snap.model = ModelIdentity::default();
            self.clear_cache();
        }
        snap.last_error = Some(message);
    }

    async fn fetch_allowed_image(
        &self,
        initial_url: &Url,
        allow_loopback: bool,
    ) -> Result<Vec<u8>, FilterServiceError> {
        let fut = async {
            let mut current = initial_url.clone();
            for hop in 0..=MAX_REDIRECT_HOPS {
                let client = if is_loopback_url(&current) {
                    &self.loopback_image_client
                } else {
                    &self.image_client
                };

                let mut resp = client
                    .get(current.clone())
                    .send_audited_quiet("filter.image.fetch")
                    .await
                    .map_err(|e| {
                        FilterServiceError::BadRequest(format!("拉取预览图失败 ({current}): {e}"))
                    })?;

                if resp.status().is_redirection() {
                    if hop == MAX_REDIRECT_HOPS {
                        return Err(FilterServiceError::BadRequest(
                            "预览图重定向次数过多".to_string(),
                        ));
                    }
                    let loc = resp
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|v| v.to_str().ok())
                        .ok_or_else(|| {
                            FilterServiceError::BadRequest(
                                "预览图重定向缺少 Location 头".to_string(),
                            )
                        })?;
                    let next = current.join(loc).map_err(|_| {
                        FilterServiceError::BadRequest("预览图重定向 URL 非法".to_string())
                    })?;
                    current = validate_allowed_image_url(next.as_str(), allow_loopback)?;
                    continue;
                }

                if !resp.status().is_success() {
                    return Err(FilterServiceError::BadRequest(format!(
                        "拉取预览图返回 HTTP {}",
                        resp.status()
                    )));
                }

                if let Some(len) = resp.content_length() {
                    if len > MAX_IMAGE_BYTES as u64 {
                        return Err(FilterServiceError::BadRequest(
                            "单张预览图大小超过 16 MiB 限制".to_string(),
                        ));
                    }
                }

                let mut buf = Vec::new();
                while let Some(chunk) = resp.chunk().await.map_err(|e| {
                    FilterServiceError::BadRequest(format!("读取预览图响应体失败: {e}"))
                })? {
                    if buf.len().saturating_add(chunk.len()) > MAX_IMAGE_BYTES {
                        return Err(FilterServiceError::BadRequest(
                            "单张预览图大小超过 16 MiB 限制".to_string(),
                        ));
                    }
                    buf.extend_from_slice(&chunk);
                }

                if buf.is_empty() {
                    return Err(FilterServiceError::BadRequest(
                        "预览图响应体为空".to_string(),
                    ));
                }

                // Validated (and downscaled when needed) by `prepare_fetched_image` below.
                return Ok(buf);
            }
            Err(FilterServiceError::BadRequest(
                "预览图重定向次数超限".to_string(),
            ))
        };

        let buf = tokio::time::timeout(IMAGE_FETCH_TIMEOUT, fut)
            .await
            .map_err(|_| FilterServiceError::BadRequest("拉取预览图超时 (8s)".to_string()))??;
        prepare_fetched_image(buf).await
    }

    fn clear_cache(&self) {
        self.cache.lock().unwrap().clear();
    }

    fn get_cached(&self, key: &[u8; 32], ttl: Duration) -> Option<FilterProbabilities> {
        let mut cache = self.cache.lock().unwrap();
        let entry = cache.get(key)?;
        if entry.inserted_at.elapsed() < ttl {
            Some(entry.probabilities.clone())
        } else {
            cache.remove(key);
            None
        }
    }

    fn put_cached(&self, key: [u8; 32], probabilities: FilterProbabilities, ttl: Duration) {
        let mut cache = self.cache.lock().unwrap();
        if cache.len() >= MAX_CACHE_ENTRIES {
            cache.retain(|_, v| v.inserted_at.elapsed() < ttl);
        }
        if cache.len() >= MAX_CACHE_ENTRIES {
            if let Some(oldest_key) = cache
                .iter()
                .min_by_key(|(_, v)| v.inserted_at)
                .map(|(k, _)| *k)
            {
                cache.remove(&oldest_key);
            }
        }
        cache.insert(
            key,
            CacheEntry {
                probabilities,
                inserted_at: Instant::now(),
            },
        );
    }
}

fn validate_predict_request_bounds(req: &FilterPredictRequest) -> Result<(), FilterServiceError> {
    if let Some(ref title) = req.title {
        if title.len() > MAX_TITLE_BYTES {
            return Err(FilterServiceError::BadRequest(format!(
                "title 长度不能超过 {MAX_TITLE_BYTES} 字节"
            )));
        }
    }
    if let Some(ref desc) = req.description {
        if desc.len() > MAX_DESCRIPTION_BYTES {
            return Err(FilterServiceError::BadRequest(format!(
                "description 长度不能超过 {MAX_DESCRIPTION_BYTES} 字节"
            )));
        }
    }
    if req.tags.len() > MAX_TAGS_COUNT {
        return Err(FilterServiceError::BadRequest(format!(
            "tags 数量不能超过 {MAX_TAGS_COUNT} 个"
        )));
    }
    if req.tags.iter().any(|tag| tag.len() > MAX_TAG_BYTES) {
        return Err(FilterServiceError::BadRequest(format!(
            "单个 tag 长度不能超过 {MAX_TAG_BYTES} 字节"
        )));
    }
    if req.image_urls.len() > MAX_REQUEST_IMAGE_URLS {
        return Err(FilterServiceError::BadRequest(format!(
            "单次请求 imageUrls 数量不能超过 {MAX_REQUEST_IMAGE_URLS}"
        )));
    }
    if req.images_base64.len() > MAX_REQUEST_IMAGE_URLS {
        return Err(FilterServiceError::BadRequest(format!(
            "单次请求 imagesBase64 数量不能超过 {MAX_REQUEST_IMAGE_URLS}"
        )));
    }
    Ok(())
}

fn truncate_utf8_bytes(input: &str, max_bytes: usize) -> &str {
    if input.len() <= max_bytes {
        return input;
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
}

async fn read_bounded_response_bytes(
    mut resp: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            return Err(format!("响应体大小 ({len} B) 超过 {max_bytes} 字节上限"));
        }
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("读取响应体失败: {e}"))?
    {
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("响应体大小超过 {max_bytes} 字节上限"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

async fn read_lossy_response_snippet(mut resp: reqwest::Response, max_bytes: usize) -> String {
    let mut buf = Vec::new();
    while let Ok(Some(chunk)) = resp.chunk().await {
        let remaining = max_bytes.saturating_sub(buf.len());
        if remaining == 0 {
            break;
        }
        let take_n = chunk.len().min(remaining);
        buf.extend_from_slice(&chunk[..take_n]);
        if buf.len() >= max_bytes {
            break;
        }
    }
    String::from_utf8_lossy(&buf).trim().to_string()
}

fn is_disallowed_upstream_ipv4(ip: Ipv4Addr) -> bool {
    ip == Ipv4Addr::new(169, 254, 169, 254)
        || ip == Ipv4Addr::new(100, 100, 100, 200)
        || ip.octets()[0] == 0
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
}

fn is_disallowed_upstream_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_disallowed_upstream_ipv4(mapped);
    }
    if let Some(compat) = ip.to_ipv4() {
        return is_disallowed_upstream_ipv4(compat);
    }
    // Block AWS IPv6 IMDS (fd00:ec2::254), link-local (fe80::/10), unspecified (::), multicast.
    let segs = ip.segments();
    let is_aws_imds_v6 = segs == [0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254];
    is_aws_imds_v6 || ip.is_unicast_link_local() || ip.is_unspecified() || ip.is_multicast()
}

pub fn validate_upstream_base_url(raw: &str) -> Result<String, String> {
    let parsed = Url::parse(raw.trim()).map_err(|e| format!("无效的 upstream_url: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("upstream_url 仅支持 http:// 或 https:// 协议".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("upstream_url 不允许包含内嵌账号密码".to_string());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("upstream_url 不允许包含查询参数 (?) 或片段 (#)".to_string());
    }
    match parsed.host() {
        Some(Host::Ipv4(ip)) if is_disallowed_upstream_ipv4(ip) => {
            return Err(format!(
                "upstream_url 不允许指向云厂商元数据或保留地址: {ip}"
            ));
        }
        Some(Host::Ipv6(ip)) if is_disallowed_upstream_ipv6(ip) => {
            return Err(format!(
                "upstream_url 不允许指向云厂商元数据或保留地址: [{ip}]"
            ));
        }
        Some(Host::Domain(dom)) => {
            let lower = dom.trim_end_matches('.').to_ascii_lowercase();
            if lower.is_empty() {
                return Err("upstream_url 缺少有效主机名".to_string());
            }
            if matches!(
                lower.as_str(),
                "metadata.google.internal" | "metadata.goog" | "instance-data"
            ) {
                return Err(format!("upstream_url 不允许指向云厂商元数据域名: {lower}"));
            }
        }
        Some(_) => {}
        None => return Err("upstream_url 缺少有效主机名".to_string()),
    }
    Ok(parsed.as_str().trim_end_matches('/').to_string())
}

fn is_loopback_url(url: &Url) -> bool {
    url.scheme() == "http"
        && matches!(
            url.host_str().map(|h| h.to_ascii_lowercase()).as_deref(),
            Some("127.0.0.1" | "localhost")
        )
}

fn is_allowed_steam_image_host_and_path(host_lower: &str, path: &str) -> bool {
    if host_lower == "media.steampowered.com" {
        return true;
    }
    if let Some(sub) = host_lower.strip_suffix(".steamusercontent.com") {
        return !sub.is_empty() && !sub.starts_with('.') && !sub.ends_with('.');
    }
    if let Some(sub) = host_lower.strip_suffix(".steamstatic.com") {
        return !sub.is_empty() && !sub.starts_with('.') && !sub.ends_with('.');
    }
    if let Some(sub) = host_lower.strip_suffix(".akamaihd.net") {
        return !sub.is_empty()
            && !sub.contains('.')
            && (sub.starts_with("steamuserimages") || sub.starts_with("steamcdn"));
    }
    if let Some(sub) = host_lower.strip_suffix(".steampowered.com") {
        return !sub.is_empty()
            && !sub.contains('.')
            && sub.starts_with("cloud")
            && path.starts_with("/ugc/");
    }
    false
}

pub fn validate_allowed_image_url(
    raw: &str,
    allow_loopback: bool,
) -> Result<Url, FilterServiceError> {
    let mut parsed = Url::parse(raw.trim())
        .map_err(|_| FilterServiceError::BadRequest(format!("非法的图片 URL: {raw}")))?;

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(FilterServiceError::BadRequest(
            "图片 URL 不允许包含认证凭据".to_string(),
        ));
    }

    let Some(host) = parsed.host_str() else {
        return Err(FilterServiceError::BadRequest(
            "图片 URL 缺少主机名".to_string(),
        ));
    };
    let host_lower = host.trim_end_matches('.').to_ascii_lowercase();

    if allow_loopback
        && parsed.scheme() == "http"
        && matches!(host_lower.as_str(), "127.0.0.1" | "localhost")
    {
        return Ok(parsed);
    }

    if !matches!(parsed.scheme(), "https" | "http") {
        return Err(FilterServiceError::BadRequest(
            "仅允许通过 HTTP/HTTPS 拉取受信任的 Steam CDN 预览图".to_string(),
        ));
    }

    // `url::Url::port()` returns `None` for the scheme's standard port (80 for http, 443 for https).
    if parsed.port().is_some() {
        return Err(FilterServiceError::BadRequest(
            "Steam CDN 预览图 URL 不允许指定非标准端口".to_string(),
        ));
    }

    if !is_allowed_steam_image_host_and_path(&host_lower, parsed.path()) {
        return Err(FilterServiceError::BadRequest(format!(
            "图片链接不在受信任的 Steam CDN 白名单内: {host_lower}"
        )));
    }

    // Auto-upgrade legacy `http://` Steam CDN URLs (e.g. `http://cloud-3.steamusercontent.com/ugc/...`) to `https://`.
    if parsed.scheme() == "http" {
        parsed.set_scheme("https").map_err(|_| {
            FilterServiceError::BadRequest("无法将 Steam CDN 图片链接升级为 HTTPS".to_string())
        })?;
    }

    Ok(parsed)
}

fn check_image_dimensions(w: u32, h: u32, fmt_name: &str) -> Result<(), FilterServiceError> {
    let pixels = (w as u64) * (h as u64);
    if w == 0
        || h == 0
        || w > MAX_IMAGE_DIMENSION
        || h > MAX_IMAGE_DIMENSION
        || pixels > MAX_IMAGE_PIXELS
    {
        return Err(FilterServiceError::BadRequest(format!(
            "{fmt_name} 图片像素尺寸非法或超出安全上限 ({w}x{h}，最大允许 4096x4096)"
        )));
    }
    Ok(())
}

/// Validates the encoded length and header dimensions without initializing a decoder.
pub fn validate_image_bytes(bytes: &[u8]) -> Result<(), FilterServiceError> {
    let (w, h, fmt_name) = sniff_image_dimensions(bytes)?;
    check_image_dimensions(w, h, fmt_name)
}

/// Validates a preview image fetched from the trusted Steam CDN, downscaling it when it is
/// only rejected for exceeding the runner's pixel limits. Workshop screenshots larger than
/// 4096x4096 are legitimate and must not make the whole evaluation fail.
async fn prepare_fetched_image(bytes: Vec<u8>) -> Result<Vec<u8>, FilterServiceError> {
    let (w, h, fmt_name) = sniff_image_dimensions(&bytes)?;
    if image_prep::fit_within_filter_limits(w, h).is_none() {
        check_image_dimensions(w, h, fmt_name)?;
        return Ok(bytes);
    }
    match image_prep::prepare_for_filter(bytes).await {
        Ok(image_prep::PreparedImage::Shrunk(shrunk)) => {
            tracing::info!(
                source_width = shrunk.source_size.0,
                source_height = shrunk.source_size.1,
                width = shrunk.output_size.0,
                height = shrunk.output_size.1,
                "Downscaled oversized Workshop preview image before content filtering"
            );
            validate_image_bytes(&shrunk.bytes)?;
            Ok(shrunk.bytes)
        }
        Ok(image_prep::PreparedImage::Original(original)) => {
            validate_image_bytes(&original)?;
            Ok(original)
        }
        Err(err) => Err(FilterServiceError::BadRequest(format!(
            "{fmt_name} 预览图 ({w}x{h}) 缩放失败: {err}"
        ))),
    }
}

/// Checks the encoded size before probing dimensions.
fn sniff_image_dimensions(bytes: &[u8]) -> Result<(u32, u32, &'static str), FilterServiceError> {
    if bytes.is_empty() {
        return Err(FilterServiceError::BadRequest(
            "图片内容不能为空".to_string(),
        ));
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(FilterServiceError::BadRequest(
            "单张图片大小不能超过 16 MiB".to_string(),
        ));
    }

    let (w, h, format) = image_prep::sniff_image_dimensions(bytes)
        .map_err(|err| FilterServiceError::BadRequest(err.to_string()))?;
    let fmt_name = match format {
        image::ImageFormat::Png => "PNG",
        image::ImageFormat::Gif => "GIF",
        image::ImageFormat::Jpeg => "JPEG",
        image::ImageFormat::WebP => "WebP",
        _ => unreachable!("the preview header probe supports only PNG, GIF, JPEG, and WebP"),
    };
    Ok((w, h, fmt_name))
}

fn decode_base64_image(raw: &str) -> Result<Vec<u8>, FilterServiceError> {
    let trimmed = raw.trim();
    let payload = if trimmed
        .as_bytes()
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"data:"))
    {
        let Some((header, rest)) = trimmed.split_once(',') else {
            return Err(FilterServiceError::BadRequest(
                "无效的 data URI 图片格式".to_string(),
            ));
        };
        let header_bytes = header.trim().as_bytes();
        if !header_bytes.is_ascii() {
            return Err(FilterServiceError::BadRequest(
                "无效的 data URI 图片媒体类型头".to_string(),
            ));
        }
        let has_base64_suffix = header_bytes
            .len()
            .checked_sub(7)
            .and_then(|start| header_bytes.get(start..))
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(b";base64"));
        if !has_base64_suffix {
            return Err(FilterServiceError::BadRequest(
                "data URI 图片必须使用 ;base64, 编码".to_string(),
            ));
        }
        rest.trim()
    } else {
        trimmed
    };

    if payload.len() > MAX_BASE64_ENCODED_LEN {
        return Err(FilterServiceError::BadRequest(
            "单张 Base64 图片大小不能超过 16 MiB".to_string(),
        ));
    }

    let decode_attempt = |input: &str| {
        BASE64_STD
            .decode(input)
            .or_else(|_| URL_SAFE.decode(input))
            .or_else(|_| URL_SAFE_NO_PAD.decode(input))
    };

    let decoded = if payload.bytes().any(|b| b.is_ascii_whitespace()) {
        let compact: String = payload
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect();
        decode_attempt(&compact)
    } else {
        decode_attempt(payload)
    }
    .map_err(|_| FilterServiceError::BadRequest("无效的 Base64 图片编码".to_string()))?;

    if decoded.is_empty() {
        return Err(FilterServiceError::BadRequest(
            "Base64 图片内容不能为空".to_string(),
        ));
    }
    if decoded.len() > MAX_IMAGE_BYTES {
        return Err(FilterServiceError::BadRequest(
            "单张解码后的图片大小不能超过 16 MiB".to_string(),
        ));
    }
    validate_image_bytes(&decoded)?;
    Ok(decoded)
}

fn round6_prob(v: f32) -> f32 {
    if !v.is_finite() {
        return 0.0;
    }
    ((v.clamp(0.0, 1.0) as f64) * 1_000_000.0).round() as f32 / 1_000_000.0
}

fn split_image_ranges_by_bytes(
    images: &[Vec<u8>],
    max_batch_bytes: usize,
) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut current_bytes = 0usize;
    for (idx, img) in images.iter().enumerate() {
        let len = img.len();
        if idx > start && current_bytes.saturating_add(len) > max_batch_bytes {
            ranges.push(start..idx);
            start = idx;
            current_bytes = len;
        } else {
            current_bytes = current_bytes.saturating_add(len);
        }
    }
    if start < images.len() {
        ranges.push(start..images.len());
    }
    ranges
}

fn collect_item_preview_urls(
    record: &l4a_protocol::WorkshopItemRecord,
    max_images: usize,
    allow_loopback: bool,
) -> Vec<Url> {
    if max_images == 0 {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut urls = Vec::with_capacity(max_images);

    let candidates = record.preview_url.iter().map(String::as_str).chain(
        record
            .previews
            .iter()
            .filter(|p| p.preview_type == 0)
            .filter_map(|p| p.url.as_deref()),
    );

    for raw in candidates {
        if urls.len() >= max_images {
            break;
        }
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(valid) = validate_allowed_image_url(trimmed, allow_loopback) {
            if seen.insert(valid.as_str().to_string()) {
                urls.push(valid);
            }
        }
    }
    urls
}

fn scoring_signature(cfg: &FilterSection) -> String {
    let p = cfg.params();
    let target = match cfg.backend {
        CatalogFilterBackendKind::ManagedPython => cfg
            .deploy_dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_default(),
        CatalogFilterBackendKind::UpstreamHttp => cfg.upstream_url.clone().unwrap_or_default(),
    };
    format!(
        "v3|{:?}|{}|t={}|i={}|m={}|{:?}|gw={:08x}|nw={:08x}|temp={:08x}|bias={:08x}",
        cfg.backend,
        target,
        p.enable_text,
        p.enable_image,
        p.max_images,
        p.gallery_aggregation,
        p.gore_weight.to_bits(),
        p.nsfw_weight.to_bits(),
        p.temperature.to_bits(),
        p.bias.to_bits(),
    )
}

fn compute_scoring_revision(cfg: &FilterSection, model: &ModelIdentity) -> String {
    let mut hasher = Sha256::new();
    let sig = scoring_signature(cfg);
    hasher.update((sig.len() as u64).to_le_bytes());
    hasher.update(sig.as_bytes());
    for value in [&model.name, &model.version, &model.revision] {
        let value = value.as_deref().unwrap_or("");
        hasher.update((value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    let digest = hasher.finalize();
    hex::encode(&digest[..8])
}

#[allow(clippy::too_many_arguments)]
fn compute_cache_key(
    cfg: &FilterSection,
    scoring_revision: &str,
    title: &str,
    description: &str,
    tags: &[String],
    images_bytes: &[Vec<u8>],
    req_urls: &[Url],
    db_urls: &[Url],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    let sig = scoring_signature(cfg);
    hasher.update((sig.len() as u64).to_le_bytes());
    hasher.update(sig.as_bytes());

    hasher.update((scoring_revision.len() as u64).to_le_bytes());
    hasher.update(scoring_revision.as_bytes());

    hasher.update((title.len() as u64).to_le_bytes());
    hasher.update(title.as_bytes());

    hasher.update((description.len() as u64).to_le_bytes());
    hasher.update(description.as_bytes());

    hasher.update((tags.len() as u64).to_le_bytes());
    for tag in tags {
        hasher.update((tag.len() as u64).to_le_bytes());
        hasher.update(tag.as_bytes());
    }

    hasher.update((images_bytes.len() as u64).to_le_bytes());
    for img in images_bytes {
        let img_digest = Sha256::digest(img);
        hasher.update(img_digest);
    }

    hasher.update((req_urls.len() as u64).to_le_bytes());
    for u in req_urls {
        let s = u.as_str();
        hasher.update((s.len() as u64).to_le_bytes());
        hasher.update(s.as_bytes());
    }

    hasher.update((db_urls.len() as u64).to_le_bytes());
    for u in db_urls {
        let s = u.as_str();
        hasher.update((s.len() as u64).to_le_bytes());
        hasher.update(s.as_bytes());
    }

    hasher.finalize().into()
}

fn is_batch_shim_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("bat")
                || ext.eq_ignore_ascii_case("cmd")
                || ext.eq_ignore_ascii_case("ps1")
        })
}

fn is_executable_file(path: &Path) -> bool {
    if is_batch_shim_path(path) {
        return false;
    }
    if std::fs::canonicalize(path)
        .ok()
        .is_some_and(|real| is_batch_shim_path(&real))
    {
        return false;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn find_executable_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if let Ok(abs_path) = std::path::absolute(&candidate) {
            if is_executable_file(&abs_path) {
                return Some(abs_path);
            }
        }
        #[cfg(windows)]
        if Path::new(name).extension().is_none() {
            let exe = dir.join(format!("{name}.exe"));
            if let Ok(abs_exe) = std::path::absolute(&exe) {
                if is_executable_file(&abs_exe) {
                    return Some(abs_exe);
                }
            }
        }
    }
    None
}

fn resolve_and_validate_python_executable(
    configured: Option<&str>,
    abs_deploy_dir: &Path,
    canonical_deploy_dir: &Path,
) -> Result<PathBuf, String> {
    if let Some(explicit) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        let explicit_path = Path::new(explicit);
        if is_batch_shim_path(explicit_path) {
            return Err(format!(
                "不支持使用脚本包装器 (.bat/.cmd/.ps1) 作为 Python 解释器: {explicit}"
            ));
        }

        let has_sep = explicit.contains('/') || explicit.contains('\\');
        if has_sep || explicit_path.is_absolute() {
            let abs_path = std::path::absolute(explicit_path)
                .map_err(|e| format!("无法解析 Python 解释器路径 ({explicit}): {e}"))?;
            let meta = std::fs::metadata(&abs_path)
                .map_err(|e| format!("指定的 Python 解释器不存在 ({explicit}): {e}"))?;
            if !meta.is_file() || !is_executable_file(&abs_path) {
                return Err(format!(
                    "指定的 Python 解释器不是有效可执行文件: {}",
                    abs_path.display()
                ));
            }
            return Ok(abs_path);
        }

        return find_executable_in_path(explicit)
            .ok_or_else(|| format!("在 PATH 中未找到可执行的 Python 解释器: {explicit}"));
    }

    let mut candidate_dirs = Vec::with_capacity(4);
    let mut push_unique = |p: PathBuf| {
        if !candidate_dirs.contains(&p) {
            candidate_dirs.push(p);
        }
    };
    push_unique(abs_deploy_dir.to_path_buf());
    if let Some(parent) = abs_deploy_dir.parent() {
        push_unique(parent.to_path_buf());
    }
    push_unique(canonical_deploy_dir.to_path_buf());
    if let Some(parent) = canonical_deploy_dir.parent() {
        push_unique(parent.to_path_buf());
    }

    for dir in candidate_dirs {
        #[cfg(windows)]
        let rels = [".venv\\Scripts\\python.exe", "venv\\Scripts\\python.exe"];
        #[cfg(not(windows))]
        let rels = [
            ".venv/bin/python3",
            ".venv/bin/python",
            "venv/bin/python3",
            "venv/bin/python",
        ];
        for rel in rels {
            let candidate = dir.join(rel);
            if let Ok(abs_path) = std::path::absolute(&candidate) {
                if is_executable_file(&abs_path) {
                    return Ok(abs_path);
                }
            }
        }
    }

    #[cfg(windows)]
    let fallbacks = ["python", "python3"];
    #[cfg(not(windows))]
    let fallbacks = ["python3", "python"];

    for bin in fallbacks {
        if let Some(found) = find_executable_in_path(bin) {
            return Ok(found);
        }
    }

    Err("未检测到可用的 Python 3 可执行解释器".to_string())
}

fn validate_and_canonicalize_deploy_dir(deploy_dir: &Path) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(deploy_dir).map_err(|e| {
        format!(
            "Left4Filters 部署目录不存在或无法访问 ({}): {e}",
            deploy_dir.display()
        )
    })?;
    if !canonical.is_dir() {
        return Err(format!(
            "Left4Filters 部署路径不是有效目录: {}",
            canonical.display()
        ));
    }
    let has_direct = canonical.join("l4filter.py").is_file();
    let has_nested = canonical.join("deploy").join("l4filter.py").is_file();
    if !has_direct && !has_nested {
        return Err(format!(
            "Left4Filters 部署目录缺少 l4filter.py 或 deploy/l4filter.py: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

async fn spawn_managed_python_runner(
    deploy_dir: &Path,
    configured_python: Option<&str>,
    params: &FilterModelParams,
) -> Result<ManagedPythonHandle, String> {
    let abs_deploy_dir = std::path::absolute(deploy_dir).map_err(|e| {
        format!(
            "无法解析 Left4Filters 部署目录路径 ({}): {e}",
            deploy_dir.display()
        )
    })?;
    let canonical_deploy_dir = validate_and_canonicalize_deploy_dir(&abs_deploy_dir)?;
    let python_exe = resolve_and_validate_python_executable(
        configured_python,
        &abs_deploy_dir,
        &canonical_deploy_dir,
    )?;

    let runner_dir = tempfile::Builder::new()
        .prefix("l4a-catalog-filter-")
        .tempdir()
        .map_err(|e| format!("无法创建临时运行目录: {e}"))?;
    let script_path = runner_dir.path().join("filter_runner.py");
    write_private_file(&script_path, FILTER_RUNNER_PY.as_bytes())
        .map_err(|e| format!("无法写入托管 Python 运行脚本: {e}"))?;

    let runner_token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let params_json = serde_json::to_string(&params.sanitized())
        .map_err(|e| format!("序列化模型参数失败: {e}"))?;

    let clean_deploy_dir = {
        let raw = canonical_deploy_dir.to_string_lossy();
        #[cfg(windows)]
        {
            raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
        }
        #[cfg(not(windows))]
        {
            raw.into_owned()
        }
    };
    let mut deploy_arg = OsString::from("--deploy-dir=");
    deploy_arg.push(&clean_deploy_dir);

    let mut command = Command::new(&python_exe);
    command
        .arg(&script_path)
        .arg("--host=127.0.0.1")
        .arg("--port=0")
        .arg(deploy_arg)
        .arg(format!("--params-json={params_json}"))
        .arg(format!("--max-concurrency={MAX_CONCURRENT_PREDICTS}"))
        .arg("--watch-stdin")
        .env("L4A_RUNNER_TOKEN", &runner_token)
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(windows)]
    command.creation_flags(0x0800_0000);

    let audit =
        l4a_logging::process::CommandAudit::start("filter.managed_runner", command.as_std());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            audit.finish_error(&e);
            return Err(format!(
                "启动 Python 解释器 `{}` 失败: {e}",
                python_exe.display()
            ));
        }
    };
    let pid = child.id();
    audit.finish_spawned(pid);

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "无法捕获 Python 子进程 stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法捕获 Python 子进程 stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法捕获 Python 子进程 stderr".to_string())?;

    // Drain stderr via raw byte chunks (never `BufReader::lines()`, which drops the pipe on non-UTF-8 bytes!).
    let stderr_buf = Arc::new(StdMutex::new(String::new()));
    let stderr_writer = stderr_buf.clone();
    tokio::spawn(async move {
        let mut stderr = stderr;
        let mut chunk = [0u8; 4096];
        let mut pending_line = String::new();
        let forward_line = |line: &str| {
            let line = line.trim();
            if !line.is_empty() {
                tracing::info!(
                    target: "l4a::filter_runner",
                    pid,
                    "{}",
                    l4a_logging::redact::truncate(line, 1000)
                );
            }
        };
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) => {
                    forward_line(&pending_line);
                    break;
                }
                Ok(n) => {
                    let lossy = String::from_utf8_lossy(&chunk[..n]);
                    pending_line.push_str(&lossy);
                    while let Some(pos) = pending_line.find('\n') {
                        forward_line(&pending_line[..pos]);
                        pending_line.drain(..=pos);
                    }
                    if pending_line.len() > MAX_STDERR_BYTES {
                        forward_line(&pending_line);
                        pending_line.clear();
                    }
                    let mut guard = stderr_writer.lock().unwrap();
                    guard.push_str(&lossy);
                    if guard.len() > MAX_STDERR_BYTES {
                        let drain_to = guard.len() - MAX_STDERR_BYTES;
                        let boundary = guard
                            .char_indices()
                            .find(|(idx, _)| *idx >= drain_to)
                            .map(|(idx, _)| idx)
                            .unwrap_or(guard.len());
                        guard.drain(..boundary);
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });

    let mut stdout_reader = BufReader::new(stdout);
    let ready_result = tokio::time::timeout(RUNNER_STARTUP_TIMEOUT, async {
        let mut line_buf = Vec::with_capacity(512);
        let mut total_scanned: usize = 0;
        loop {
            line_buf.clear();
            let remaining = MAX_READY_LINE_BYTES.saturating_sub(total_scanned);
            let n = (&mut stdout_reader)
                .take((remaining + 1) as u64)
                .read_until(b'\n', &mut line_buf)
                .await
                .map_err(|e| format!("读取 Python 子进程输出失败: {e}"))?;
            if n == 0 {
                return Err("Python 过滤子进程在输出就绪信号前提前退出".to_string());
            }
            total_scanned = total_scanned.saturating_add(line_buf.len());
            if total_scanned > MAX_READY_LINE_BYTES {
                return Err("Python 子进程启动输出超过 64 KiB 上限".to_string());
            }
            let line = String::from_utf8_lossy(&line_buf);
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if let Some(rest) = trimmed.strip_prefix("L4A_FILTER_READY ") {
                let parsed: RunnerHealthPayload = serde_json::from_str(rest)
                    .map_err(|e| format!("解析 L4A_FILTER_READY 横幅失败: {e}"))?;
                let port = parsed
                    .port
                    .filter(|p| *p > 0)
                    .ok_or_else(|| "L4A_FILTER_READY 缺少有效监听端口".to_string())?;
                let host = parsed.host.unwrap_or_else(|| "127.0.0.1".to_string());
                let endpoint = if host.contains(':') {
                    format!("http://[{host}]:{port}")
                } else {
                    format!("http://{host}:{port}")
                };
                return Ok((endpoint, parsed.model));
            }
        }
    })
    .await;

    match ready_result {
        Ok(Ok((endpoint, model))) => {
            // Keep draining stdout using raw byte chunks until EOF so non-UTF-8 bytes never trigger SIGPIPE.
            tokio::spawn(async move {
                let mut sink = [0u8; 4096];
                loop {
                    match stdout_reader.read(&mut sink).await {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            });

            Ok(ManagedPythonHandle {
                child,
                stdin: Some(stdin),
                _runner_dir: runner_dir,
                runner_token,
                endpoint,
                pid,
                deploy_dir: deploy_dir.to_path_buf(),
                python_bin: configured_python.map(str::to_owned),
                model,
                stderr_buf,
            })
        }
        Ok(Err(reason)) => {
            drop(stdin);
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            let err_tail = stderr_buf.lock().unwrap().trim().to_string();
            if err_tail.is_empty() {
                Err(reason)
            } else {
                Err(format!("{reason}: {err_tail}"))
            }
        }
        Err(_) => {
            drop(stdin);
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            let err_tail = stderr_buf.lock().unwrap().trim().to_string();
            if err_tail.is_empty() {
                Err("启动托管 Python 过滤服务超时 (15s)".to_string())
            } else {
                Err(format!("启动托管 Python 过滤服务超时 (15s): {err_tail}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_png(w: u32, h: u32) -> Vec<u8> {
        let mut buf = Vec::with_capacity(33);
        buf.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        buf.extend_from_slice(&13u32.to_be_bytes());
        buf.extend_from_slice(b"IHDR");
        buf.extend_from_slice(&w.to_be_bytes());
        buf.extend_from_slice(&h.to_be_bytes());
        buf.extend_from_slice(&[8, 2, 0, 0, 0]);
        buf.extend_from_slice(&[0, 0, 0, 0]); // CRC placeholder
        buf
    }

    #[test]
    fn ssrf_validator_upgrades_legacy_steam_cdn_http_and_rejects_untrusted_hosts() {
        assert!(validate_allowed_image_url(
            "https://steamuserimages-a.akamaihd.net/ugc/123/preview.jpg",
            false
        )
        .is_ok());
        assert!(validate_allowed_image_url(
            "https://images.steamusercontent.com/ugc/456/shot.png",
            false
        )
        .is_ok());
        let upgraded = validate_allowed_image_url(
            "http://cloud-3.steamusercontent.com/ugc/456/shot.png",
            false,
        )
        .unwrap();
        assert_eq!(
            upgraded.as_str(),
            "https://cloud-3.steamusercontent.com/ugc/456/shot.png"
        );
        let upgraded_cloud =
            validate_allowed_image_url("http://cloud-2.steampowered.com/ugc/789/legacy.jpg", false)
                .unwrap();
        assert_eq!(
            upgraded_cloud.as_str(),
            "https://cloud-2.steampowered.com/ugc/789/legacy.jpg"
        );
        assert!(validate_allowed_image_url(
            "https://cloud-2.steampowered.com/non-ugc/secret.jpg",
            false
        )
        .is_err());
        assert!(validate_allowed_image_url(
            "https://images.steamusercontent.com:8443/ugc/456/shot.png",
            false
        )
        .is_err());
        assert!(validate_allowed_image_url(
            "http://images.steamusercontent.com:8080/ugc/456/shot.png",
            false
        )
        .is_err());
        assert!(validate_allowed_image_url(
            "https://user:pass@images.steamusercontent.com/ugc/456/shot.png",
            false
        )
        .is_err());
        assert!(validate_allowed_image_url("https://evil.example.com/a.png", false).is_err());
        assert!(validate_allowed_image_url("http://evil.example.com/a.png", false).is_err());
        assert!(
            validate_allowed_image_url("http://169.254.169.254/latest/meta-data/", false).is_err()
        );
        assert!(validate_allowed_image_url("http://127.0.0.1:9000/test.png", false).is_err());
        assert!(validate_allowed_image_url("http://127.0.0.1:9000/test.png", true).is_ok());
    }

    #[test]
    fn upstream_url_validator_rejects_cloud_metadata_ips() {
        assert!(validate_upstream_base_url("http://127.0.0.1:8090").is_ok());
        assert!(validate_upstream_base_url("http://169.254.169.254").is_err());
        assert!(validate_upstream_base_url("http://[::ffff:169.254.169.254]:80").is_err());
        assert!(validate_upstream_base_url("http://[fd00:ec2::254]").is_err());
        assert!(validate_upstream_base_url("http://metadata.google.internal").is_err());
    }

    #[test]
    fn image_header_validator_checks_png_gif_jpeg_webp_and_blocks_decompression_bombs() {
        // PNG valid vs bomb vs zero
        assert!(validate_image_bytes(&minimal_png(512, 512)).is_ok());
        assert!(validate_image_bytes(&minimal_png(4096, 4096)).is_ok());
        assert!(validate_image_bytes(&minimal_png(4097, 1)).is_err());
        assert!(validate_image_bytes(&minimal_png(1, 4097)).is_err());
        assert!(validate_image_bytes(&minimal_png(0, 100)).is_err());

        // GIF valid vs bomb
        let mut gif_ok = b"GIF89a".to_vec();
        gif_ok.extend_from_slice(&320u16.to_le_bytes());
        gif_ok.extend_from_slice(&240u16.to_le_bytes());
        assert!(validate_image_bytes(&gif_ok).is_ok());

        let mut gif_bomb = b"GIF89a".to_vec();
        gif_bomb.extend_from_slice(&5000u16.to_le_bytes());
        gif_bomb.extend_from_slice(&5000u16.to_le_bytes());
        assert!(validate_image_bytes(&gif_bomb).is_err());

        // JPEG SOF0 valid vs bomb
        let jpeg_with_dims = |w: u16, h: u16| -> Vec<u8> {
            let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0];
            v.extend_from_slice(&8u16.to_be_bytes());
            v.push(8); // precision
            v.extend_from_slice(&h.to_be_bytes());
            v.extend_from_slice(&w.to_be_bytes());
            v.push(3); // components
            v
        };
        assert!(validate_image_bytes(&jpeg_with_dims(1920, 1080)).is_ok());
        assert!(validate_image_bytes(&jpeg_with_dims(8192, 100)).is_err());

        // WebP VP8, VP8L, VP8X valid vs bomb
        let mut vp8 = Vec::new();
        vp8.extend_from_slice(b"RIFF\x18\x00\x00\x00WEBP");
        vp8.extend_from_slice(b"VP8 ");
        vp8.extend_from_slice(&10u32.to_le_bytes());
        vp8.extend_from_slice(&[0x00, 0x00, 0x00, 0x9D, 0x01, 0x2A]);
        vp8.extend_from_slice(&640u16.to_le_bytes());
        vp8.extend_from_slice(&480u16.to_le_bytes());
        assert!(validate_image_bytes(&vp8).is_ok());

        let mut vp8l_bomb = Vec::new();
        vp8l_bomb.extend_from_slice(b"RIFF\x0e\x00\x00\x00WEBP");
        vp8l_bomb.extend_from_slice(b"VP8L");
        vp8l_bomb.extend_from_slice(&5u32.to_le_bytes());
        vp8l_bomb.push(0x2F);
        let packed = 4999u32 | (4999u32 << 14);
        vp8l_bomb.extend_from_slice(&packed.to_le_bytes());
        assert!(validate_image_bytes(&vp8l_bomb).is_err());

        let mut vp8x_ok = Vec::new();
        vp8x_ok.extend_from_slice(b"RIFF\x16\x00\x00\x00WEBP");
        vp8x_ok.extend_from_slice(b"VP8X");
        vp8x_ok.extend_from_slice(&10u32.to_le_bytes());
        vp8x_ok.extend_from_slice(&[0, 0, 0, 0]);
        vp8x_ok.extend_from_slice(&[0xFF, 0x03, 0x00]); // w = 1024
        vp8x_ok.extend_from_slice(&[0xFF, 0x02, 0x00]); // h = 768
        assert!(validate_image_bytes(&vp8x_ok).is_ok());

        assert!(validate_image_bytes(b"not-an-image-header").is_err());
    }

    #[tokio::test]
    async fn fetched_oversized_images_are_downscaled_but_bombs_are_still_rejected() {
        let mut big = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(4500, 300, image::Rgb([5, 120, 200]))
            .write_to(&mut big, image::ImageFormat::Png)
            .unwrap();
        let big = big.into_inner();
        assert!(validate_image_bytes(&big).is_err());

        let shrunk = prepare_fetched_image(big).await.unwrap();
        validate_image_bytes(&shrunk).unwrap();
        let decoded = image::load_from_memory(&shrunk).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (4096, 273));

        let small = minimal_png(512, 512);
        assert_eq!(prepare_fetched_image(small.clone()).await.unwrap(), small);

        // Headers beyond the decode budget are rejected without decoding anything.
        assert!(prepare_fetched_image(minimal_png(20_000, 20_000))
            .await
            .is_err());
        assert!(prepare_fetched_image(b"not-an-image-header".to_vec())
            .await
            .is_err());
    }

    #[test]
    fn base64_decoder_accepts_data_uri_and_rejects_empty_corrupt_or_invalid_headers() {
        let raw = minimal_png(16, 16);
        let b64 = BASE64_STD.encode(&raw);
        assert_eq!(decode_base64_image(&b64).unwrap(), raw);
        let data_uri = format!("data:image/png;base64,{b64}");
        assert_eq!(decode_base64_image(&data_uri).unwrap(), raw);
        assert!(decode_base64_image("").is_err());
        assert!(decode_base64_image("%%%not-base64%%%").is_err());

        let bomb_b64 = BASE64_STD.encode(minimal_png(8192, 8192));
        assert!(decode_base64_image(&bomb_b64).is_err());
    }

    #[test]
    fn base64_decoder_rejects_malformed_unicode_inputs_without_panicking() {
        let valid_b64 = BASE64_STD.encode(minimal_png(8, 8));
        for malformed in [
            "data:中文;base64,AAAA",
            &format!("data:中文;base64,{valid_b64}"),
            "中文中文",
            "中",
            "data:中文,AAAA",
            "data:x中文,AAAA",
            "DATA:🔥;base64,AAAA",
        ] {
            let err = decode_base64_image(malformed).expect_err(malformed);
            assert!(
                matches!(err, FilterServiceError::BadRequest(_)),
                "expected BadRequest for {malformed:?}, got {err:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn resolve_python_executable_preserves_virtualenv_symlinks_and_rejects_shims() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let repo_root = dir.path().join("Left4Filters");
        let deploy_dir = repo_root.join("deploy");
        std::fs::create_dir_all(&deploy_dir).unwrap();
        std::fs::write(deploy_dir.join("l4filter.py"), "# mock\n").unwrap();

        let sys_python = find_executable_in_path("python3").expect("python3 must exist on PATH");
        let canonical_sys_python = std::fs::canonicalize(&sys_python).unwrap();

        // Create `<repo_root>/venv/bin/python` as a symlink to the system interpreter.
        let venv_bin = repo_root.join("venv").join("bin");
        std::fs::create_dir_all(&venv_bin).unwrap();
        let venv_python = venv_bin.join("python");
        symlink(&canonical_sys_python, &venv_python).unwrap();

        let abs_deploy = std::path::absolute(&deploy_dir).unwrap();
        let canonical_deploy = validate_and_canonicalize_deploy_dir(&abs_deploy).unwrap();

        // 1. Auto-discovery from `<deploy_dir>/../venv/bin/python` preserves symlink path!
        let auto_resolved =
            resolve_and_validate_python_executable(None, &abs_deploy, &canonical_deploy).unwrap();
        assert_eq!(auto_resolved, std::path::absolute(&venv_python).unwrap());
        assert_ne!(
            auto_resolved, canonical_sys_python,
            "must NOT canonicalize symlinked venv/bin/python to system interpreter"
        );

        // 2. Explicit `python_bin` pointing to symlinked venv also preserves symlink path!
        let explicit_resolved = resolve_and_validate_python_executable(
            Some(venv_python.to_str().unwrap()),
            &abs_deploy,
            &canonical_deploy,
        )
        .unwrap();
        assert_eq!(
            explicit_resolved,
            std::path::absolute(&venv_python).unwrap()
        );

        // 3. Rejects directories and `.bat`/`.cmd`/`.ps1` shims (including symlinks to shims).
        assert!(resolve_and_validate_python_executable(
            Some(venv_bin.to_str().unwrap()),
            &abs_deploy,
            &canonical_deploy,
        )
        .is_err());

        let bat_shim = dir.path().join("shim.bat");
        std::fs::write(&bat_shim, "@echo off\n").unwrap();
        std::fs::set_permissions(&bat_shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let symlink_to_bat = dir.path().join("python_bat_link");
        symlink(&bat_shim, &symlink_to_bat).unwrap();
        assert!(resolve_and_validate_python_executable(
            Some(bat_shim.to_str().unwrap()),
            &abs_deploy,
            &canonical_deploy,
        )
        .is_err());
        assert!(resolve_and_validate_python_executable(
            Some(symlink_to_bat.to_str().unwrap()),
            &abs_deploy,
            &canonical_deploy,
        )
        .is_err());
    }

    #[test]
    fn batch_splitter_and_gallery_aggregator_respect_limits_and_modes() {
        let mb12 = vec![0u8; 12 * 1024 * 1024];
        let images = vec![mb12.clone(), mb12.clone(), mb12];
        let ranges = split_image_ranges_by_bytes(&images, MAX_RUNNER_BATCH_RAW_IMAGE_BYTES);
        assert_eq!(ranges, vec![0..2, 2..3]);

        let rows = vec![[0.90, 0.80, 0.20], [0.30, 0.20, 0.10], [0.60, 0.50, 0.40]];
        let max_agg =
            aggregate_gallery_triplets(&rows, l4a_protocol::FilterGalleryAggregation::Max);
        assert!((max_agg[0] - 0.90).abs() < 1e-6);
        assert!((max_agg[1] - 0.80).abs() < 1e-6);
        assert!((max_agg[2] - 0.40).abs() < 1e-6);

        let mean_agg =
            aggregate_gallery_triplets(&rows, l4a_protocol::FilterGalleryAggregation::Mean);
        assert!((mean_agg[0] - 0.60).abs() < 1e-6);
        assert!((mean_agg[1] - 0.50).abs() < 1e-6);
        assert!((mean_agg[2] - 0.70 / 3.0).abs() < 1e-6);

        let top2_agg =
            aggregate_gallery_triplets(&rows, l4a_protocol::FilterGalleryAggregation::Top2Mean);
        assert!((top2_agg[0] - 0.75).abs() < 1e-6);
        assert!((top2_agg[1] - 0.65).abs() < 1e-6);
        assert!((top2_agg[2] - 0.30).abs() < 1e-6);
    }

    #[test]
    fn scoring_revision_is_16_hex_chars_and_tracks_params_and_model_metadata() {
        let mut cfg = FilterSection {
            enabled: true,
            ..Default::default()
        };
        let mut model = ModelIdentity {
            name: Some("Left4Filters".into()),
            version: Some("1.0.0".into()),
            ..ModelIdentity::default()
        };
        let r1 = compute_scoring_revision(&cfg, &model);
        assert_eq!(r1.len(), 16);
        assert!(r1.chars().all(|c| c.is_ascii_hexdigit()));

        model.version = Some("1.1.0".into());
        let r2 = compute_scoring_revision(&cfg, &model);
        assert_ne!(r1, r2);

        cfg.bias = 0.25;
        model.version = Some("1.0.0".into());
        let r3 = compute_scoring_revision(&cfg, &model);
        assert_ne!(r1, r3);
    }

    #[tokio::test]
    async fn stock_upstream_upgrade_changes_revision_and_retains_corpse_review_in_gallery() {
        use axum::{
            routing::{get, post},
            Json, Router,
        };
        use serde_json::{json, Value};

        let corpse_supported = Arc::new(AtomicBool::new(false));
        let model_state = corpse_supported.clone();
        let hit_image = BASE64_STD.encode(b"corpse fixture");
        let app = Router::new()
            .route("/health", get(|| async { Json(json!({"model": "Left4FiltersModel"})) }))
            .route("/predict", post(move |Json(req): Json<Value>| {
                let has_corpse_scores = model_state.load(Ordering::Acquire);
                let hit = has_corpse_scores && req["images_base64"][0].as_str() == Some(&hit_image);
                async move {
                    let mut scores = json!({"prob": if hit { 0.20 } else { 0.04 }, "gore_prob": 0.03, "nsfw_prob": 0.01});
                    if has_corpse_scores {
                        scores["corpse_prob"] = json!(if hit { 0.50 } else { 0.05 });
                        scores["corpse_pile_prob"] = json!(0.01);
                    }
                    Json(scores)
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = Arc::new(CatalogConfigStore::new(None, Default::default()));
        store
            .update(|cfg| {
                cfg.filter.enabled = true;
                cfg.filter.backend = CatalogFilterBackendKind::UpstreamHttp;
                cfg.filter.upstream_url = Some(endpoint.clone());
                cfg.filter.gallery_aggregation = l4a_protocol::FilterGalleryAggregation::Mean;
                cfg.filter.bias = -1.0;
            })
            .unwrap();
        let service =
            CatalogFilterService::new(store, Arc::new(ServerRepo::open_in_memory().unwrap()));
        let before = service.public_info().await;
        assert!(before.ready);

        corpse_supported.store(true, Ordering::Release);
        service.runtime.write().unwrap().upstream_verified_at = None;
        let after = service.public_info().await;
        assert!(after.ready);
        assert_eq!(after.model_version, None);
        assert_ne!(before.scoring_revision, after.scoring_revision);

        let prediction = service
            .predict_upstream_gallery(
                &endpoint,
                "Workshop item",
                "",
                &[],
                &[b"ordinary fixture".to_vec(), b"corpse fixture".to_vec()],
                &service.current_filter_section().params(),
            )
            .await
            .unwrap();
        assert_eq!(prediction.prob, 0.20);
        assert!(prediction.fusion_score() < 0.04);
        assert_eq!(prediction.corpse_prob, Some(0.50));
        assert_eq!(prediction.corpse_image_indices, vec![1]);
        server.abort();
    }

    #[tokio::test]
    async fn stock_upstream_corpse_probe_failure_keeps_verified_revision() {
        use axum::{
            http::StatusCode,
            routing::{get, post},
            Json, Router,
        };
        use serde_json::json;
        use std::sync::atomic::AtomicU16;

        let probe_status = Arc::new(AtomicU16::new(200));
        let state = probe_status.clone();
        let app = Router::new()
            .route(
                "/health",
                get(|| async { Json(json!({"model": "Left4FiltersModel"})) }),
            )
            .route(
                "/predict",
                post(move || {
                    let status = state.load(Ordering::Acquire);
                    async move {
                        let body = if status == 200 {
                            json!({
                                "prob": 0.0, "gore_prob": 0.0, "nsfw_prob": 0.0,
                                "corpse_prob": 0.0, "corpse_pile_prob": 0.0
                            })
                            .to_string()
                        } else {
                            "unexpected response".to_string()
                        };
                        (StatusCode::from_u16(status).unwrap(), body)
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = Arc::new(CatalogConfigStore::new(None, Default::default()));
        store
            .update(|cfg| {
                cfg.filter.enabled = true;
                cfg.filter.backend = CatalogFilterBackendKind::UpstreamHttp;
                cfg.filter.upstream_url = Some(endpoint.clone());
            })
            .unwrap();
        let service =
            CatalogFilterService::new(store, Arc::new(ServerRepo::open_in_memory().unwrap()));
        let verified = service.public_info().await;
        assert!(verified.ready);
        assert_eq!(
            service.runtime.read().unwrap().model.revision.as_deref(),
            Some(STOCK_CORPSE_SCORES_REVISION)
        );

        for transient in [503, 429, 201] {
            probe_status.store(transient, Ordering::Release);
            service.runtime.write().unwrap().upstream_verified_at = None;
            let refreshed = service.public_info().await;
            assert!(refreshed.ready);
            assert_eq!(refreshed.scoring_revision, verified.scoring_revision);
        }

        probe_status.store(400, Ordering::Release);
        service.runtime.write().unwrap().upstream_verified_at = None;
        let rejected = service.public_info().await;
        assert!(rejected.ready);
        assert_ne!(rejected.scoring_revision, verified.scoring_revision);
        server.abort();
    }

    #[tokio::test]
    async fn upstream_ttl_refresh_restores_ready_state_if_backend_failed_during_health_probe() {
        use axum::{routing::get, Json, Router};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::Notify;

        let store = Arc::new(crate::config::CatalogConfigStore::new(
            None,
            crate::config::CatalogConfig::default(),
        ));
        let repo = Arc::new(crate::db::ServerRepo::open_in_memory().unwrap());

        let probe_count = Arc::new(AtomicUsize::new(0));
        let second_entered = Arc::new(Notify::new());
        let second_release = Arc::new(Notify::new());

        let app = {
            let probe_count = probe_count.clone();
            let second_entered = second_entered.clone();
            let second_release = second_release.clone();
            Router::new().route(
                "/health",
                get(move || {
                    let probe_count = probe_count.clone();
                    let second_entered = second_entered.clone();
                    let second_release = second_release.clone();
                    async move {
                        let n = probe_count.fetch_add(1, Ordering::SeqCst) + 1;
                        if n == 2 {
                            second_entered.notify_one();
                            second_release.notified().await;
                        }
                        Json(serde_json::json!({
                            "status": "ok",
                            "model_name": "Left4Filters",
                            "model_version": "1.0.0"
                        }))
                    }
                }),
            )
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let base_url = format!("http://{addr}");
        store
            .update(|cfg| {
                cfg.filter.enabled = true;
                cfg.filter.backend = CatalogFilterBackendKind::UpstreamHttp;
                cfg.filter.upstream_url = Some(base_url.clone());
            })
            .unwrap();

        let service = CatalogFilterService::new(store, repo);
        let _ = service.reconfigure().await;
        assert!(service.runtime.read().unwrap().ready);

        // Force TTL expiration so the next reconfigure() takes the routine refresh path.
        service.runtime.write().unwrap().upstream_verified_at =
            Some(Instant::now() - UPSTREAM_HEALTH_TTL - Duration::from_secs(5));
        let gen_before = service.backend_generation.load(Ordering::Acquire);

        let svc_clone = service.clone();
        let refresh_task = tokio::spawn(async move {
            let _ = svc_clone.reconfigure().await;
        });

        second_entered.notified().await;
        service.record_backend_failure(&base_url, "simulated upstream disconnect".into());
        assert!(!service.runtime.read().unwrap().ready);
        second_release.notify_one();

        refresh_task.await.unwrap();

        let snap = service.runtime.read().unwrap().clone();
        assert!(
            snap.ready,
            "successful /health refresh must restore ready=true after concurrent failure"
        );
        assert!(snap.initialized_config.is_some());
        assert!(snap.backoff_config.is_none());
        assert!(snap.next_retry_after.is_none());
        assert!(snap.last_error.is_none());
        assert!(service.backend_generation.load(Ordering::Acquire) > gen_before);

        server_task.abort();
    }

    #[test]
    fn validate_upstream_base_url_rejects_query_fragment_and_reserved_metadata_ips() {
        assert!(validate_upstream_base_url("http://127.0.0.1:8788").is_ok());
        assert!(validate_upstream_base_url("https://filter.internal:8443/v1").is_ok());

        for bad in [
            "http://127.0.0.1:8788?token=abc",
            "http://127.0.0.1:8788#frag",
            "http://0.1.2.3:8788",
            "http://100.100.100.200/latest/meta-data/",
            "http://[::169.254.169.254]/",
            "http://[::ffff:100.100.100.200]/",
        ] {
            assert!(
                validate_upstream_base_url(bad).is_err(),
                "expected validate_upstream_base_url to reject {bad}"
            );
        }
    }

    #[tokio::test]
    async fn predict_once_fails_closed_when_db_only_images_fail_and_no_text() {
        use axum::{routing::get, Json, Router};

        let store = Arc::new(crate::config::CatalogConfigStore::new(
            None,
            crate::config::CatalogConfig::default(),
        ));
        let repo = Arc::new(crate::db::ServerRepo::open_in_memory().unwrap());

        let app = Router::new()
            .route(
                "/health",
                get(|| async {
                    Json(serde_json::json!({
                        "status": "ok",
                        "model_name": "Left4Filters",
                        "model_version": "1.0.0"
                    }))
                }),
            )
            .route(
                "/broken-preview.jpg",
                get(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let base_url = format!("http://{addr}");
        let item = l4a_protocol::WorkshopItemRecord {
            workshop_id: l4a_protocol::WorkshopId(777001),
            file_type: l4a_protocol::WorkshopFileType::Item,
            status: l4a_protocol::WorkshopItemStatus::Active,
            title: "DB Title Ignored When Text Disabled".into(),
            description: Some("DB Description".into()),
            creator_steam_id: None,
            creator_name: None,
            preview_url: Some(format!("{base_url}/broken-preview.jpg")),
            previews: vec![],
            file_url: None,
            file_size: None,
            time_created: None,
            time_updated: None,
            subscriptions: None,
            favorites: None,
            tags: vec![],
            dependencies: vec![],
            collection_children: vec![],
            source: l4a_protocol::MetadataSource::CatalogServer,
            fetched_at: chrono::Utc::now(),
        };
        repo.upsert_item(&item, false, false).unwrap();

        store
            .update(|cfg| {
                cfg.filter.enabled = true;
                cfg.filter.backend = CatalogFilterBackendKind::UpstreamHttp;
                cfg.filter.upstream_url = Some(base_url.clone());
                cfg.filter.enable_text = false;
                cfg.filter.enable_image = true;
            })
            .unwrap();

        let service = CatalogFilterService::new(store, repo);
        service.set_allow_loopback_image_urls_for_testing(true);
        let _ = service.reconfigure().await;

        let err = service
            .predict(l4a_protocol::FilterPredictRequest {
                workshop_id: Some(l4a_protocol::WorkshopId(777001)),
                title: None,
                description: None,
                tags: vec![],
                image_base64: None,
                images_base64: vec![],
                image_urls: vec![],
            })
            .await
            .expect_err(
                "expected ImageFetch error when all DB preview images fail and text is disabled",
            );
        assert!(
            matches!(err, FilterServiceError::ImageFetch(_)),
            "expected ImageFetch error, got: {err:?}"
        );

        server_task.abort();
    }
}
