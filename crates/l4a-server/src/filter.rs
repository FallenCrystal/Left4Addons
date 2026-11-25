use crate::config::{CatalogConfigStore, FilterSection};
use crate::credentials::write_private_file;
use crate::db::ServerRepo;
use base64::engine::general_purpose::{STANDARD as BASE64_STD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use l4a_protocol::{
    CatalogFilterBackendKind, CatalogFilterPublicInfo, CatalogFilterStatus, FilterModelParams,
    FilterPredictRequest, FilterProbabilities, WorkshopItemStatus, FILTER_RUNNER_PY,
};
use serde::Deserialize;
use serde_json::json;
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
use tokio::sync::{Mutex as AsyncMutex, Semaphore};
use url::{Host, Url};

const MAX_CACHE_ENTRIES: usize = 4_096;
const MAX_CONCURRENT_PREDICTS: usize = 4;
const MAX_TITLE_BYTES: usize = 4_096;
const MAX_DESCRIPTION_BYTES: usize = 65_536;
const MAX_TAGS_COUNT: usize = 64;
const MAX_TAG_BYTES: usize = 256;
const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_BASE64_ENCODED_LEN: usize = (MAX_IMAGE_BYTES * 4 / 3) + 256;
const MAX_IMAGE_DIMENSION: u32 = 4_096;
const MAX_IMAGE_PIXELS: u64 = 16_777_216;
const MAX_REQUEST_IMAGE_URLS: usize = 16;
const MAX_REDIRECT_HOPS: usize = 3;
const IMAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(8);
const RUNNER_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(10);
const PREDICT_TIMEOUT: Duration = Duration::from_secs(20);
const CRASH_RESTART_BACKOFF: Duration = Duration::from_secs(3);
const MAX_STDERR_BYTES: usize = 8 * 1024;
const MAX_READY_LINE_BYTES: usize = 64 * 1024;
const MAX_BACKEND_RESPONSE_BYTES: usize = 64 * 1024;

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
    last_crash_respawn_at: Option<Instant>,
    ready: bool,
    running_pid: Option<u32>,
    active_endpoint: Option<String>,
    model_name: Option<String>,
    model_version: Option<String>,
    last_error: Option<String>,
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
    model_name: Option<String>,
    model_version: Option<String>,
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

#[derive(Debug, Deserialize)]
struct RunnerHealthPayload {
    #[serde(alias = "modelName", alias = "model_name")]
    model: Option<String>,
    #[serde(alias = "modelVersion", alias = "model_version")]
    version: Option<String>,
    host: Option<String>,
    port: Option<u16>,
}

pub struct CatalogFilterService {
    config_store: Arc<CatalogConfigStore>,
    repo: Arc<ServerRepo>,
    lifecycle_lock: AsyncMutex<Option<ManagedPythonHandle>>,
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

    /// Gracefully terminates any managed Python filter runner child process on server shutdown.
    pub async fn shutdown(&self) {
        let mut guard = self.lifecycle_lock.lock().await;
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
        if !cfg.enabled {
            return CatalogFilterPublicInfo {
                enabled: false,
                ready: false,
                model_name: None,
                model_version: None,
                max_images: params.max_images,
            };
        }

        self.ensure_initialized().await;
        self.refresh_managed_liveness();
        let snap = self.runtime.read().unwrap().clone();
        CatalogFilterPublicInfo {
            enabled: true,
            ready: snap.ready,
            model_name: snap.model_name,
            model_version: snap.model_version,
            max_images: params.max_images,
        }
    }

    pub async fn status(&self) -> CatalogFilterStatus {
        let cfg = self.current_filter_section();
        if cfg.enabled {
            self.ensure_initialized().await;
            self.refresh_managed_liveness();
        }
        let cfg = self.current_filter_section();
        let snap = self.runtime.read().unwrap().clone();
        CatalogFilterStatus {
            config: cfg.to_dto(),
            ready: cfg.enabled && snap.ready,
            running_pid: if cfg.enabled { snap.running_pid } else { None },
            active_endpoint: if cfg.enabled {
                snap.active_endpoint
            } else {
                None
            },
            model_name: if cfg.enabled { snap.model_name } else { None },
            model_version: if cfg.enabled {
                snap.model_version
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
        if !self.is_enabled() {
            return Err(FilterServiceError::Disabled(
                "内容过滤服务未启用".to_string(),
            ));
        }

        self.total_requests.fetch_add(1, Ordering::Relaxed);

        // 1. Enforce strict input bounds on caller-supplied text and image list counts upfront.
        validate_predict_request_bounds(&req)?;

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

        let mut title = req.title.unwrap_or_default();
        let mut description = req.description.unwrap_or_default();
        let mut tags = req.tags;
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
        let (endpoint, backend_kind) = {
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
            let Some(ref ep) = snap.active_endpoint else {
                self.failed_requests.fetch_add(1, Ordering::Relaxed);
                return Err(FilterServiceError::Unavailable(
                    "内容过滤模型后端未绑定有效地址".to_string(),
                ));
            };
            (ep.clone(), cfg.backend)
        };

        // 6. Check in-memory TTL cache.
        let cache_ttl = Duration::from_secs(cfg.cache_ttl_secs);
        let cache_key = compute_cache_key(
            &cfg,
            &effective_title,
            &effective_desc,
            &effective_tags,
            &decoded_b64_images,
            &effective_req_urls,
            &effective_db_urls,
        );
        if !cache_ttl.is_zero() {
            if let Some(hit) = self.get_cached(&cache_key, cache_ttl) {
                self.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(hit);
            }
        }

        // 7. Fetch remote images (strictly SSRF-checked + header/dimension bomb validated).
        let mut final_images = decoded_b64_images;
        for url in &effective_req_urls {
            let bytes = self.fetch_allowed_image(url, allow_loopback).await?;
            final_images.push(bytes);
        }
        let mut db_image_fetch_failed = false;
        for url in &effective_db_urls {
            match self.fetch_allowed_image(url, allow_loopback).await {
                Ok(bytes) => final_images.push(bytes),
                Err(err) => {
                    db_image_fetch_failed = true;
                    tracing::debug!(url = %url, error = %err, "跳过无法拉取的工坊缓存预览图");
                }
            }
        }

        // 8. Dispatch inference to backend (`ManagedPython` or `UpstreamHttp`), gated by semaphore.
        let has_txt = !effective_title.trim().is_empty()
            || !effective_desc.trim().is_empty()
            || !effective_tags.is_empty();
        let probabilities = if !has_txt && final_images.is_empty() {
            FilterProbabilities::default()
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

            match backend_kind {
                // ManagedPython runner already applies `FilterModelParams` calibration & unimodal breakdown.
                CatalogFilterBackendKind::ManagedPython => raw_probs,
                CatalogFilterBackendKind::UpstreamHttp => {
                    self.backfill_upstream_unimodal(
                        &endpoint,
                        &effective_title,
                        &effective_desc,
                        &effective_tags,
                        &encoded_images,
                        &mut raw_probs,
                    )
                    .await;
                    params.adjust_probabilities(&raw_probs)
                }
            }
        };

        // Never cache degraded partial text-only results when a DB preview image failed transiently!
        if !cache_ttl.is_zero() && !db_image_fetch_failed {
            self.put_cached(cache_key, probabilities.clone(), cache_ttl);
        }

        Ok(probabilities)
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
                raw_probs.image_prob = Some(raw_probs.prob);
                raw_probs.image_gore_prob = Some(raw_probs.gore_prob);
                raw_probs.image_nsfw_prob = Some(raw_probs.nsfw_prob);
                if raw_probs.per_image_probs.is_empty() && encoded_images.len() == 1 {
                    raw_probs.per_image_probs = vec![raw_probs.prob];
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
            raw_probs.image_prob = Some(img_res.prob);
            raw_probs.image_gore_prob = Some(img_res.gore_prob);
            raw_probs.image_nsfw_prob = Some(img_res.nsfw_prob);
            if raw_probs.per_image_probs.is_empty() {
                if !img_res.per_image_probs.is_empty() {
                    raw_probs.per_image_probs = img_res.per_image_probs;
                } else if encoded_images.len() == 1 {
                    raw_probs.per_image_probs = vec![img_res.prob];
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
            if snap.initialized_config.as_ref() == Some(&fast_cfg) {
                return;
            }
            if fast_cfg.enabled
                && snap.backoff_config.as_ref() == Some(&fast_cfg)
                && snap.next_retry_after.is_some_and(|t| Instant::now() < t)
            {
                return;
            }
        }

        let mut guard = self.lifecycle_lock.lock().await;
        self.reap_exited_child_locked(&mut guard);
        let cfg = self.current_filter_section();
        {
            let snap = self.runtime.read().unwrap();
            if snap.initialized_config.as_ref() == Some(&cfg) {
                return;
            }
            if cfg.enabled
                && snap.backoff_config.as_ref() == Some(&cfg)
                && snap.next_retry_after.is_some_and(|t| Instant::now() < t)
            {
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
        let next_sig = scoring_signature(cfg);
        let should_clear_cache = force_restart
            || !cfg.enabled
            || cfg.cache_ttl_secs == 0
            || self
                .runtime
                .read()
                .unwrap()
                .last_scoring_signature
                .as_deref()
                != Some(next_sig.as_str());
        if should_clear_cache {
            self.clear_cache();
        }
        let retained_sig = if cfg.enabled && cfg.cache_ttl_secs > 0 {
            Some(next_sig)
        } else {
            None
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
                last_crash_respawn_at: None,
                ready: false,
                running_pid: None,
                active_endpoint: None,
                model_name: None,
                model_version: None,
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
                    let mut snap = self.runtime.write().unwrap();
                    *snap = RuntimeSnapshot {
                        initialized_config: None,
                        last_scoring_signature: retained_sig,
                        backoff_config: Some(cfg.clone()),
                        next_retry_after: Some(Instant::now() + CRASH_RESTART_BACKOFF),
                        last_crash_respawn_at: snap.last_crash_respawn_at,
                        ready: false,
                        running_pid: None,
                        active_endpoint: None,
                        model_name: None,
                        model_version: None,
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
                                    let mut snap = self.runtime.write().unwrap();
                                    snap.initialized_config = Some(cfg.clone());
                                    snap.last_scoring_signature = retained_sig;
                                    snap.backoff_config = None;
                                    snap.next_retry_after = None;
                                    snap.ready = true;
                                    snap.running_pid = existing.pid;
                                    snap.active_endpoint = Some(existing.endpoint.clone());
                                    snap.model_name = existing.model_name.clone();
                                    snap.model_version = existing.model_version.clone();
                                    snap.last_error = None;
                                    return;
                                }
                                Err(err) => {
                                    tracing::warn!(
                                        error = %err,
                                        "热更新 Python 过滤参数失败，准备重启子进程"
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
                        let prev_crash_ts = self.runtime.read().unwrap().last_crash_respawn_at;
                        let snap_next = RuntimeSnapshot {
                            initialized_config: Some(cfg.clone()),
                            last_scoring_signature: retained_sig,
                            backoff_config: None,
                            next_retry_after: None,
                            last_crash_respawn_at: prev_crash_ts,
                            ready: true,
                            running_pid: handle.pid,
                            active_endpoint: Some(handle.endpoint.clone()),
                            model_name: handle.model_name.clone(),
                            model_version: handle.model_version.clone(),
                            last_error: None,
                        };
                        *guard = Some(handle);
                        *self.runtime.write().unwrap() = snap_next;
                    }
                    Err(err) => {
                        let prev_crash_ts = self.runtime.read().unwrap().last_crash_respawn_at;
                        *self.runtime.write().unwrap() = RuntimeSnapshot {
                            initialized_config: None,
                            last_scoring_signature: retained_sig,
                            backoff_config: Some(cfg.clone()),
                            next_retry_after: Some(Instant::now() + CRASH_RESTART_BACKOFF),
                            last_crash_respawn_at: prev_crash_ts,
                            ready: false,
                            running_pid: None,
                            active_endpoint: None,
                            model_name: None,
                            model_version: None,
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
                    *self.runtime.write().unwrap() = RuntimeSnapshot {
                        initialized_config: None,
                        last_scoring_signature: retained_sig,
                        backoff_config: Some(cfg.clone()),
                        next_retry_after: Some(Instant::now() + CRASH_RESTART_BACKOFF),
                        last_crash_respawn_at: None,
                        ready: false,
                        running_pid: None,
                        active_endpoint: None,
                        model_name: None,
                        model_version: None,
                        last_error: Some(
                            "未配置外部过滤推理服务地址 (filter.upstream_url)".to_string(),
                        ),
                    };
                    return;
                };

                let base_url = match validate_upstream_base_url(raw_url) {
                    Ok(u) => u,
                    Err(err) => {
                        *self.runtime.write().unwrap() = RuntimeSnapshot {
                            initialized_config: None,
                            last_scoring_signature: retained_sig,
                            backoff_config: Some(cfg.clone()),
                            next_retry_after: Some(Instant::now() + CRASH_RESTART_BACKOFF),
                            last_crash_respawn_at: None,
                            ready: false,
                            running_pid: None,
                            active_endpoint: None,
                            model_name: None,
                            model_version: None,
                            last_error: Some(err),
                        };
                        return;
                    }
                };

                match self.probe_upstream_health(&base_url).await {
                    Ok((model_name, model_version)) => {
                        *self.runtime.write().unwrap() = RuntimeSnapshot {
                            initialized_config: Some(cfg.clone()),
                            last_scoring_signature: retained_sig,
                            backoff_config: None,
                            next_retry_after: None,
                            last_crash_respawn_at: None,
                            ready: true,
                            running_pid: None,
                            active_endpoint: Some(base_url),
                            model_name,
                            model_version,
                            last_error: None,
                        };
                    }
                    Err(err) => {
                        *self.runtime.write().unwrap() = RuntimeSnapshot {
                            initialized_config: None,
                            last_scoring_signature: retained_sig,
                            backoff_config: Some(cfg.clone()),
                            next_retry_after: Some(Instant::now() + CRASH_RESTART_BACKOFF),
                            last_crash_respawn_at: None,
                            ready: false,
                            running_pid: None,
                            active_endpoint: Some(base_url),
                            model_name: None,
                            model_version: None,
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
            .send()
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

    async fn probe_upstream_health(
        &self,
        base_url: &str,
    ) -> Result<(Option<String>, Option<String>), String> {
        let url = format!("{}/health", base_url.trim_end_matches('/'));
        let resp = self
            .backend_client
            .get(&url)
            .timeout(HEALTH_CHECK_TIMEOUT)
            .send()
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
        if let Ok(parsed) = serde_json::from_slice::<RunnerHealthPayload>(&body) {
            Ok((parsed.model, parsed.version))
        } else {
            Ok((None, None))
        }
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
        let _permit = self.predict_semaphore.acquire().await.map_err(|_| {
            FilterServiceError::Unavailable("内容过滤推理并发限流器已关闭".to_string())
        })?;

        let url = format!("{}/predict", endpoint.trim_end_matches('/'));
        let payload = json!({
            "title": title,
            "description": description,
            "tags": tags,
            "imagesBase64": images_base64,
        });

        let resp = match self.backend_client.post(&url).json(&payload).send().await {
            Ok(r) => r,
            Err(err) => {
                self.refresh_managed_liveness();
                let msg = format!("调用过滤推理后端失败: {err}");
                if record_failure {
                    self.record_backend_failure(msg.clone());
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
                    self.record_backend_failure(msg.clone());
                }
                return Err(FilterServiceError::Unavailable(msg));
            }
            if record_failure {
                self.failed_requests.fetch_add(1, Ordering::Relaxed);
            }
            return Err(FilterServiceError::BadRequest(msg));
        }

        let body = match read_bounded_response_bytes(resp, MAX_BACKEND_RESPONSE_BYTES).await {
            Ok(b) => b,
            Err(err) => {
                let msg = format!("读取过滤推理后端响应失败: {err}");
                if record_failure {
                    self.record_backend_failure(msg.clone());
                }
                return Err(FilterServiceError::Unavailable(msg));
            }
        };

        serde_json::from_slice::<FilterProbabilities>(&body).map_err(|err| {
            let msg = format!("解析过滤推理后端响应失败: {err}");
            if record_failure {
                self.record_backend_failure(msg.clone());
            }
            FilterServiceError::Unavailable(msg)
        })
    }

    fn record_backend_failure(&self, message: String) {
        self.failed_requests.fetch_add(1, Ordering::Relaxed);
        let mut snap = self.runtime.write().unwrap();
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

                let mut resp = client.get(current.clone()).send().await.map_err(|e| {
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

                validate_image_bytes(&buf)?;
                return Ok(buf);
            }
            Err(FilterServiceError::BadRequest(
                "预览图重定向次数超限".to_string(),
            ))
        };

        tokio::time::timeout(IMAGE_FETCH_TIMEOUT, fut)
            .await
            .map_err(|_| FilterServiceError::BadRequest("拉取预览图超时 (8s)".to_string()))?
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
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
}

fn is_disallowed_upstream_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_disallowed_upstream_ipv4(mapped);
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

/// Zero-dependency pure-Rust image magic-byte and header-dimension validator against decompression bombs.
pub fn validate_image_bytes(bytes: &[u8]) -> Result<(), FilterServiceError> {
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

    // 1. PNG: 8-byte signature `\x89PNG\r\n\x1a\n` followed by `IHDR` chunk (width & height big-endian u32).
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
            return Err(FilterServiceError::BadRequest(
                "损坏的 PNG 图片头 (缺少 IHDR)".to_string(),
            ));
        }
        let ihdr_len = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        if ihdr_len != 13 {
            return Err(FilterServiceError::BadRequest(
                "损坏的 PNG IHDR 块长度".to_string(),
            ));
        }
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return check_image_dimensions(w, h, "PNG");
    }

    // 2. GIF: `GIF87a` or `GIF89a` followed by logical screen width & height (little-endian u16).
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        if bytes.len() < 10 {
            return Err(FilterServiceError::BadRequest(
                "损坏的 GIF 图片头".to_string(),
            ));
        }
        let w = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
        let h = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
        return check_image_dimensions(w, h, "GIF");
    }

    // 3. JPEG: `\xFF\xD8` followed by segments until SOF0..SOF3 / SOF5..SOF7 / SOF9..SOF11 / SOF13..SOF15.
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut pos = 2usize;
        while pos < bytes.len() {
            if bytes[pos] != 0xFF {
                return Err(FilterServiceError::BadRequest(
                    "损坏的 JPEG 段标记结构".to_string(),
                ));
            }
            while pos < bytes.len() && bytes[pos] == 0xFF {
                pos += 1;
            }
            if pos >= bytes.len() {
                break;
            }
            let marker = bytes[pos];
            pos += 1;

            match marker {
                0x00 => {
                    return Err(FilterServiceError::BadRequest(
                        "损坏的 JPEG 头部填充字节".to_string(),
                    ));
                }
                // Standalone markers without length payload
                0x01 | 0xD0..=0xD8 => continue,
                // EOI or SOS reached before any SOF frame header
                0xD9 | 0xDA => break,
                _ => {}
            }

            if pos + 2 > bytes.len() {
                return Err(FilterServiceError::BadRequest(
                    "截断的 JPEG 段长度头".to_string(),
                ));
            }
            let seg_len = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
            if seg_len < 2 || pos + seg_len > bytes.len() {
                return Err(FilterServiceError::BadRequest(
                    "JPEG 段长度越界或损坏".to_string(),
                ));
            }

            if matches!(
                marker,
                0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
            ) {
                if seg_len < 8 {
                    return Err(FilterServiceError::BadRequest(
                        "损坏的 JPEG SOF 帧头".to_string(),
                    ));
                }
                let h = u16::from_be_bytes([bytes[pos + 3], bytes[pos + 4]]) as u32;
                let w = u16::from_be_bytes([bytes[pos + 5], bytes[pos + 6]]) as u32;
                return check_image_dimensions(w, h, "JPEG");
            }

            pos += seg_len;
        }
        return Err(FilterServiceError::BadRequest(
            "JPEG 图片缺少有效的 SOF 尺寸头".to_string(),
        ));
    }

    // 4. WebP: `RIFF....WEBP` followed by `VP8 `, `VP8L`, or `VP8X` chunk.
    if bytes.len() >= 20 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let chunk_type = &bytes[12..16];
        let chunk_size = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]) as usize;
        match chunk_type {
            b"VP8 " => {
                if chunk_size < 10 || bytes.len() < 30 {
                    return Err(FilterServiceError::BadRequest(
                        "截断的 WebP VP8 帧头".to_string(),
                    ));
                }
                if (bytes[20] & 0x01) != 0 || bytes[23..26] != [0x9D, 0x01, 0x2A] {
                    return Err(FilterServiceError::BadRequest(
                        "损坏的 WebP VP8 关键帧签名".to_string(),
                    ));
                }
                let w = (u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3FFF) as u32;
                let h = (u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3FFF) as u32;
                return check_image_dimensions(w, h, "WebP");
            }
            b"VP8L" => {
                if chunk_size < 5 || bytes.len() < 25 || bytes[20] != 0x2F {
                    return Err(FilterServiceError::BadRequest(
                        "损坏的 WebP VP8L 无损帧头".to_string(),
                    ));
                }
                let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
                let w = (bits & 0x3FFF) + 1;
                let h = ((bits >> 14) & 0x3FFF) + 1;
                return check_image_dimensions(w, h, "WebP");
            }
            b"VP8X" => {
                if chunk_size < 10 || bytes.len() < 30 {
                    return Err(FilterServiceError::BadRequest(
                        "截断的 WebP VP8X 扩展头".to_string(),
                    ));
                }
                let w =
                    (bytes[24] as u32 | ((bytes[25] as u32) << 8) | ((bytes[26] as u32) << 16)) + 1;
                let h =
                    (bytes[27] as u32 | ((bytes[28] as u32) << 8) | ((bytes[29] as u32) << 16)) + 1;
                return check_image_dimensions(w, h, "WebP");
            }
            _ => {
                return Err(FilterServiceError::BadRequest(
                    "不支持或损坏的 WebP 块类型".to_string(),
                ));
            }
        }
    }

    Err(FilterServiceError::BadRequest(
        "不支持的图片格式（仅支持 PNG、JPEG、GIF、WebP）".to_string(),
    ))
}

fn decode_base64_image(raw: &str) -> Result<Vec<u8>, FilterServiceError> {
    let trimmed = raw.trim();
    let payload = if trimmed.len() >= 5 && trimmed[..5].eq_ignore_ascii_case("data:") {
        let Some((header, rest)) = trimmed.split_once(',') else {
            return Err(FilterServiceError::BadRequest(
                "无效的 data URI 图片格式".to_string(),
            ));
        };
        let header_trimmed = header.trim();
        if header_trimmed.len() < 7
            || !header_trimmed[header_trimmed.len() - 7..].eq_ignore_ascii_case(";base64")
        {
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
        "{:?}|{}|t={}|i={}|m={}|{:?}|gw={:08x}|nw={:08x}|temp={:08x}|bias={:08x}",
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

fn compute_cache_key(
    cfg: &FilterSection,
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
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() || is_batch_shim_path(path) {
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
        if is_executable_file(&candidate) {
            return std::fs::canonicalize(&candidate).ok().or(Some(candidate));
        }
        #[cfg(windows)]
        if Path::new(name).extension().is_none() {
            let exe = dir.join(format!("{name}.exe"));
            if is_executable_file(&exe) {
                return std::fs::canonicalize(&exe).ok().or(Some(exe));
            }
        }
    }
    None
}

fn resolve_and_validate_python_executable(
    configured: Option<&str>,
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
            let canonical = std::fs::canonicalize(explicit_path)
                .map_err(|e| format!("指定的 Python 解释器不存在 ({explicit}): {e}"))?;
            if !is_executable_file(&canonical) {
                return Err(format!(
                    "指定的 Python 解释器不是有效可执行文件: {}",
                    canonical.display()
                ));
            }
            return Ok(canonical);
        }

        return find_executable_in_path(explicit)
            .ok_or_else(|| format!("在 PATH 中未找到可执行的 Python 解释器: {explicit}"));
    }

    let mut candidate_dirs = vec![canonical_deploy_dir.to_path_buf()];
    if let Some(parent) = canonical_deploy_dir.parent() {
        candidate_dirs.push(parent.to_path_buf());
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
            let p = dir.join(rel);
            if is_executable_file(&p) {
                return Ok(std::fs::canonicalize(&p).unwrap_or(p));
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
    let canonical_deploy_dir = validate_and_canonicalize_deploy_dir(deploy_dir)?;
    let python_exe =
        resolve_and_validate_python_executable(configured_python, &canonical_deploy_dir)?;

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

    let mut deploy_arg = OsString::from("--deploy-dir=");
    deploy_arg.push(&canonical_deploy_dir);

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

    let mut child = command
        .spawn()
        .map_err(|e| format!("启动 Python 解释器 `{}` 失败: {e}", python_exe.display()))?;
    let pid = child.id();

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
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    let lossy = String::from_utf8_lossy(&chunk[..n]);
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
        loop {
            line_buf.clear();
            let n = (&mut stdout_reader)
                .take((MAX_READY_LINE_BYTES + 1) as u64)
                .read_until(b'\n', &mut line_buf)
                .await
                .map_err(|e| format!("读取 Python 子进程输出失败: {e}"))?;
            if n == 0 {
                return Err("Python 过滤子进程在输出就绪信号前提前退出".to_string());
            }
            if line_buf.len() > MAX_READY_LINE_BYTES {
                return Err("Python 子进程启动输出单行超过 64 KiB 上限".to_string());
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
                return Ok((endpoint, parsed.model, parsed.version));
            }
        }
    })
    .await;

    match ready_result {
        Ok(Ok((endpoint, model_name, model_version))) => {
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
                model_name,
                model_version,
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
}
