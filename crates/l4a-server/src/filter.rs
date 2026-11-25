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
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex as AsyncMutex;
use url::Url;

const MAX_CACHE_ENTRIES: usize = 4_096;
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_BASE64_ENCODED_LEN: usize = (MAX_IMAGE_BYTES * 4 / 3) + 256;
const MAX_REQUEST_IMAGE_URLS: usize = 16;
const MAX_REDIRECT_HOPS: usize = 3;
const IMAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(8);
const RUNNER_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(10);
const PREDICT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_STDERR_BYTES: usize = 8 * 1024;

const ALLOWED_STEAM_IMAGE_HOSTS: &[&str] = &[
    "steamuserimages-a.akamaihd.net",
    "images.steamusercontent.com",
    "steamcdn-a.akamaihd.net",
    "cdn.cloudflare.steamstatic.com",
    "shared.cloudflare.steamstatic.com",
    "shared.fastly.steamstatic.com",
];

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
    ready: bool,
    running_pid: Option<u32>,
    active_endpoint: Option<String>,
    model_name: Option<String>,
    model_version: Option<String>,
    last_error: Option<String>,
}

struct ManagedPythonHandle {
    child: Child,
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
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await;
    }
}

impl Drop for ManagedPythonHandle {
    fn drop(&mut self) {
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
        let cfg = self.current_filter_section();
        {
            let mut guard = self.lifecycle_lock.lock().await;
            let prev_cfg = self.runtime.read().unwrap().initialized_config.clone();
            let should_clear_cache = prev_cfg.as_ref().is_none_or(|prev| {
                !cfg.enabled
                    || cfg.cache_ttl_secs == 0
                    || scoring_signature(prev) != scoring_signature(&cfg)
            });
            if should_clear_cache {
                self.clear_cache();
            }
            self.apply_lifecycle_locked(&mut guard, &cfg, false).await;
        }
        self.status().await
    }

    pub async fn restart(&self) -> CatalogFilterStatus {
        let cfg = self.current_filter_section();
        {
            let mut guard = self.lifecycle_lock.lock().await;
            self.clear_cache();
            self.apply_lifecycle_locked(&mut guard, &cfg, true).await;
        }
        self.status().await
    }

    pub async fn predict(
        &self,
        req: FilterPredictRequest,
    ) -> Result<FilterProbabilities, FilterServiceError> {
        let cfg = self.current_filter_section();
        if !cfg.enabled {
            return Err(FilterServiceError::Disabled(
                "内容过滤概率服务当前未启用".to_string(),
            ));
        }

        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.ensure_initialized().await;
        self.refresh_managed_liveness();

        let params = cfg.params();
        let allow_loopback = self.allow_loopback_image_urls.load(Ordering::Acquire);

        // 1. Validate caller-provided `image_urls` upfront (SSRF protection).
        if req.image_urls.len() > MAX_REQUEST_IMAGE_URLS {
            return Err(FilterServiceError::BadRequest(format!(
                "单次请求 imageUrls 数量不能超过 {MAX_REQUEST_IMAGE_URLS}"
            )));
        }
        let mut validated_req_urls = Vec::with_capacity(req.image_urls.len());
        for raw_url in &req.image_urls {
            let trimmed = raw_url.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parsed = validate_allowed_image_url(trimmed, allow_loopback)?;
            validated_req_urls.push(parsed);
        }

        // 2. Decode and validate caller-provided base64 images upfront (size cap).
        let mut decoded_b64_images: Vec<Vec<u8>> = Vec::new();
        if let Some(ref single) = req.image_base64 {
            if !single.trim().is_empty() {
                decoded_b64_images.push(decode_base64_image(single)?);
            }
        }
        for b64 in &req.images_base64 {
            if !b64.trim().is_empty() {
                decoded_b64_images.push(decode_base64_image(b64)?);
            }
        }

        // 3. Resolve optional `workshop_id` from public/active `ServerRepo` cache when fields are absent.
        let has_req_title = req.title.as_deref().is_some_and(|s| !s.trim().is_empty());
        let has_req_desc = req
            .description
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());
        let has_req_tags = !req.tags.is_empty();
        let has_req_text = has_req_title || has_req_desc || has_req_tags;
        let has_req_images = !decoded_b64_images.is_empty() || !validated_req_urls.is_empty();

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
                    if !has_req_images && params.enable_image {
                        db_preview_urls = collect_item_preview_urls(
                            &record,
                            params.max_images as usize,
                            allow_loopback,
                        );
                    }
                    if !has_req_title {
                        title = record.title;
                    }
                    if !has_req_desc {
                        description = record.description.unwrap_or_default();
                    }
                    if !has_req_text && tags.is_empty() {
                        tags = record.tags;
                    }
                } else if !has_req_text && !has_req_images {
                    return Err(FilterServiceError::NotFound(format!(
                        "未找到公开可用的工坊条目 {wid}"
                    )));
                }
            }
        }

        // 4. Enforce server-side `FilterModelParams` modality switches and image count limits.
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

        let max_imgs = if params.enable_image {
            params.max_images as usize
        } else {
            0
        };
        if decoded_b64_images.len() > max_imgs {
            decoded_b64_images.truncate(max_imgs);
        }
        let remaining_slots = max_imgs.saturating_sub(decoded_b64_images.len());
        let effective_req_urls: Vec<Url> = validated_req_urls
            .into_iter()
            .take(remaining_slots)
            .collect();
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

        // 7. Fetch remote images (strictly SSRF-checked).
        let mut final_images = decoded_b64_images;
        for url in &effective_req_urls {
            let bytes = self.fetch_allowed_image(url, allow_loopback).await?;
            final_images.push(bytes);
        }
        for url in &effective_db_urls {
            match self.fetch_allowed_image(url, allow_loopback).await {
                Ok(bytes) => final_images.push(bytes),
                Err(err) => {
                    tracing::debug!(url = %url, error = %err, "跳过无法拉取的工坊缓存预览图");
                }
            }
        }

        // 8. Dispatch inference to backend (`ManagedPython` or `UpstreamHttp`).
        let has_txt = !effective_title.trim().is_empty()
            || !effective_desc.trim().is_empty()
            || !effective_tags.is_empty();
        let probabilities = if !has_txt && final_images.is_empty() {
            FilterProbabilities::default()
        } else {
            let encoded_images: Vec<String> =
                final_images.iter().map(|b| BASE64_STD.encode(b)).collect();
            let raw_probs = self
                .call_backend_predict(
                    &endpoint,
                    &effective_title,
                    &effective_desc,
                    &effective_tags,
                    &encoded_images,
                )
                .await?;
            match backend_kind {
                // ManagedPython runner already applies `FilterModelParams` calibration & aggregation.
                CatalogFilterBackendKind::ManagedPython => raw_probs,
                // UpstreamHttp may have identity calibration; apply server-configured calibration here.
                CatalogFilterBackendKind::UpstreamHttp => params.adjust_probabilities(&raw_probs),
            }
        };

        if !cache_ttl.is_zero() {
            self.put_cached(cache_key, probabilities.clone(), cache_ttl);
        }

        Ok(probabilities)
    }

    fn current_filter_section(&self) -> FilterSection {
        let mut section = self.config_store.get().filter;
        section.normalize();
        section
    }

    async fn ensure_initialized(&self) {
        let cfg = self.current_filter_section();
        {
            let snap = self.runtime.read().unwrap();
            if snap.initialized_config.as_ref() == Some(&cfg) {
                return;
            }
        }
        let mut guard = self.lifecycle_lock.lock().await;
        {
            let snap = self.runtime.read().unwrap();
            if snap.initialized_config.as_ref() == Some(&cfg) {
                return;
            }
        }
        self.apply_lifecycle_locked(&mut guard, &cfg, false).await;
    }

    fn refresh_managed_liveness(&self) {
        let Ok(mut guard) = self.lifecycle_lock.try_lock() else {
            return;
        };
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
            let mut snap = self.runtime.write().unwrap();
            snap.ready = false;
            snap.running_pid = None;
            snap.active_endpoint = None;
            snap.last_error = Some(msg);
            *guard = None;
        }
    }

    async fn apply_lifecycle_locked(
        &self,
        guard: &mut Option<ManagedPythonHandle>,
        cfg: &FilterSection,
        force_restart: bool,
    ) {
        if !cfg.enabled {
            if let Some(mut old) = guard.take() {
                old.terminate().await;
            }
            let mut snap = self.runtime.write().unwrap();
            *snap = RuntimeSnapshot {
                initialized_config: Some(cfg.clone()),
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
                        initialized_config: Some(cfg.clone()),
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
                        let snap_next = RuntimeSnapshot {
                            initialized_config: Some(cfg.clone()),
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
                        *self.runtime.write().unwrap() = RuntimeSnapshot {
                            initialized_config: Some(cfg.clone()),
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
                        initialized_config: Some(cfg.clone()),
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

                let base_url = match normalize_upstream_base_url(raw_url) {
                    Ok(u) => u,
                    Err(err) => {
                        *self.runtime.write().unwrap() = RuntimeSnapshot {
                            initialized_config: Some(cfg.clone()),
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
                            initialized_config: Some(cfg.clone()),
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
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("/configure 返回 {status}: {text}"));
        }
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
            return Err(format!("上游过滤服务健康检查返回 HTTP {}", resp.status()));
        }
        let body = resp
            .bytes()
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
                let msg = format!("调用过滤推理后端失败: {err}");
                self.record_backend_failure(msg.clone());
                return Err(FilterServiceError::Unavailable(msg));
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            let msg = format!("过滤推理后端返回 HTTP {status}: {detail}");
            if status.is_server_error() {
                self.record_backend_failure(msg.clone());
                return Err(FilterServiceError::Unavailable(msg));
            }
            self.failed_requests.fetch_add(1, Ordering::Relaxed);
            return Err(FilterServiceError::BadRequest(msg));
        }

        resp.json::<FilterProbabilities>().await.map_err(|err| {
            let msg = format!("解析过滤推理后端响应失败: {err}");
            self.record_backend_failure(msg.clone());
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
                            "单张预览图大小超过 10 MiB 限制".to_string(),
                        ));
                    }
                }

                let mut buf = Vec::new();
                while let Some(chunk) = resp.chunk().await.map_err(|e| {
                    FilterServiceError::BadRequest(format!("读取预览图响应体失败: {e}"))
                })? {
                    if buf.len().saturating_add(chunk.len()) > MAX_IMAGE_BYTES {
                        return Err(FilterServiceError::BadRequest(
                            "单张预览图大小超过 10 MiB 限制".to_string(),
                        ));
                    }
                    buf.extend_from_slice(&chunk);
                }

                if buf.is_empty() {
                    return Err(FilterServiceError::BadRequest(
                        "预览图响应体为空".to_string(),
                    ));
                }

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

fn normalize_upstream_base_url(raw: &str) -> Result<String, String> {
    let parsed = Url::parse(raw.trim()).map_err(|e| format!("无效的 upstream_url: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("upstream_url 仅支持 http:// 或 https:// 协议".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("upstream_url 缺少有效主机名".to_string());
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

pub fn validate_allowed_image_url(
    raw: &str,
    allow_loopback: bool,
) -> Result<Url, FilterServiceError> {
    let parsed = Url::parse(raw.trim())
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
    let host_lower = host.to_ascii_lowercase();

    if allow_loopback
        && parsed.scheme() == "http"
        && matches!(host_lower.as_str(), "127.0.0.1" | "localhost")
    {
        return Ok(parsed);
    }

    if parsed.scheme() != "https" {
        return Err(FilterServiceError::BadRequest(
            "仅允许通过 HTTPS 拉取受信任的 Steam CDN 预览图".to_string(),
        ));
    }

    if let Some(port) = parsed.port() {
        if port != 443 {
            return Err(FilterServiceError::BadRequest(
                "Steam CDN 预览图 URL 不允许指定非标准端口".to_string(),
            ));
        }
    }

    if !ALLOWED_STEAM_IMAGE_HOSTS.contains(&host_lower.as_str()) {
        return Err(FilterServiceError::BadRequest(format!(
            "图片域名不在受信任的 Steam CDN 白名单内: {host_lower}"
        )));
    }

    Ok(parsed)
}

fn decode_base64_image(raw: &str) -> Result<Vec<u8>, FilterServiceError> {
    let trimmed = raw.trim();
    let payload = if let Some((_, rest)) = trimmed.split_once(";base64,") {
        rest.trim()
    } else {
        trimmed
    };

    if payload.len() > MAX_BASE64_ENCODED_LEN {
        return Err(FilterServiceError::BadRequest(
            "单张 Base64 图片大小不能超过 10 MiB".to_string(),
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
            "单张解码后的图片大小不能超过 10 MiB".to_string(),
        ));
    }
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
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        if let Ok(valid) = validate_allowed_image_url(trimmed, allow_loopback) {
            urls.push(valid);
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

fn resolve_python_executable(configured: Option<&str>, deploy_dir: &Path) -> String {
    if let Some(explicit) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        return explicit.to_string();
    }

    let mut candidate_dirs = vec![deploy_dir.to_path_buf()];
    if let Some(parent) = deploy_dir.parent() {
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
            if p.is_file() {
                return p.display().to_string();
            }
        }
    }

    #[cfg(windows)]
    let fallbacks = ["python", "python3", "py"];
    #[cfg(not(windows))]
    let fallbacks = ["python3", "python"];

    for bin in fallbacks {
        if std::process::Command::new(bin)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return bin.to_string();
        }
    }

    fallbacks[0].to_string()
}

async fn spawn_managed_python_runner(
    deploy_dir: &Path,
    configured_python: Option<&str>,
    params: &FilterModelParams,
) -> Result<ManagedPythonHandle, String> {
    if !deploy_dir.exists() {
        return Err(format!(
            "Left4Filters 部署目录不存在: {}",
            deploy_dir.display()
        ));
    }

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
    let python_exe = resolve_python_executable(configured_python, deploy_dir);

    let mut command = Command::new(&python_exe);
    command
        .arg(&script_path)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg("0")
        .arg("--deploy-dir")
        .arg(deploy_dir)
        .arg("--params-json")
        .arg(&params_json)
        .env("L4A_RUNNER_TOKEN", &runner_token)
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(windows)]
    command.creation_flags(0x0800_0000);

    let mut child = command
        .spawn()
        .map_err(|e| format!("启动 Python 解释器 `{python_exe}` 失败: {e}"))?;
    let pid = child.id();

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法捕获 Python 子进程 stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法捕获 Python 子进程 stderr".to_string())?;

    let stderr_buf = Arc::new(StdMutex::new(String::new()));
    let stderr_writer = stderr_buf.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let mut guard = stderr_writer.lock().unwrap();
            if !guard.is_empty() {
                guard.push('\n');
            }
            guard.push_str(&line);
            if guard.len() > MAX_STDERR_BYTES {
                let drain_to = guard.len() - MAX_STDERR_BYTES;
                let boundary = guard
                    .char_indices()
                    .find(|(idx, _)| *idx >= drain_to)
                    .map(|(idx, _)| idx)
                    .unwrap_or(0);
                guard.drain(..boundary);
            }
        }
    });

    let mut stdout_lines = BufReader::new(stdout).lines();
    let ready_result = tokio::time::timeout(RUNNER_STARTUP_TIMEOUT, async {
        while let Some(line) = stdout_lines
            .next_line()
            .await
            .map_err(|e| format!("读取 Python 子进程输出失败: {e}"))?
        {
            if let Some(rest) = line.strip_prefix("L4A_FILTER_READY ") {
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
        Err("Python 过滤子进程在输出就绪信号前提前退出".to_string())
    })
    .await;

    match ready_result {
        Ok(Ok((endpoint, model_name, model_version))) => {
            // Keep draining stdout so the child pipe never blocks.
            tokio::spawn(async move { while let Ok(Some(_)) = stdout_lines.next_line().await {} });

            Ok(ManagedPythonHandle {
                child,
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

    #[test]
    fn ssrf_validator_enforces_https_and_steam_cdn_allowlist() {
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
        assert!(validate_allowed_image_url(
            "http://images.steamusercontent.com/ugc/456/shot.png",
            false
        )
        .is_err());
        assert!(validate_allowed_image_url(
            "https://images.steamusercontent.com:8443/ugc/456/shot.png",
            false
        )
        .is_err());
        assert!(validate_allowed_image_url(
            "https://user:pass@images.steamusercontent.com/ugc/456/shot.png",
            false
        )
        .is_err());
        assert!(validate_allowed_image_url("https://evil.example.com/a.png", false).is_err());
        assert!(
            validate_allowed_image_url("http://169.254.169.254/latest/meta-data/", false).is_err()
        );
        assert!(validate_allowed_image_url("http://127.0.0.1:9000/test.png", false).is_err());
        assert!(validate_allowed_image_url("http://127.0.0.1:9000/test.png", true).is_ok());
    }

    #[test]
    fn base64_decoder_accepts_data_uri_and_rejects_empty_or_corrupt() {
        let raw = b"\x89PNG\r\n\x1a\n";
        let b64 = BASE64_STD.encode(raw);
        assert_eq!(decode_base64_image(&b64).unwrap(), raw);
        let data_uri = format!("data:image/png;base64,{b64}");
        assert_eq!(decode_base64_image(&data_uri).unwrap(), raw);
        assert!(decode_base64_image("").is_err());
        assert!(decode_base64_image("%%%not-base64%%%").is_err());
    }
}
