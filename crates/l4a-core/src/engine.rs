use crate::config::{load_config, save_config, AppPaths};
use crate::db::LocalDb;
use crate::download_gate::DownloadGate;
use crate::error::CoreError;
use crate::file_changes::{occupied, same_file, take_matching_dummy, FileChanges};
use crate::grouping::{
    apply_profile, create_manual_bundle, delete_bundle, delete_profile, delete_user_group,
    rebuild_auto_bundles, save_profile, save_user_group, set_bundle_locked,
    track_workshop_collection, untrack_workshop_collection,
};
use crate::lifecycle::{source_provides_dependencies, sync_lifecycle};
use crate::ops::{
    addon_path, append_os_suffix, apply_renames, build_target_vpk_name, delete_addons, move_addons,
    preview_batch_rename, search_by_map_code, take_addon_file, toggle_addons,
};
use crate::reconcile::{auto_resolve_by_policy, resolve_reconcile_issue};
use crate::scanner::{scan_and_commit, scan_workspace};
use crate::tasks::{create_task, update_task};
use crate::watcher::AddonWatcher;
use l4a_mirror::MirrorRouter;
use l4a_protocol::{
    AddonBatchReport, AddonDirType, AddonId, AddonSummary, AppConfig, BundleId, BundleRecord,
    CoreEvent, DashboardSnapshot, GroupId, InstallState, IssueId, LoadoutProfile,
    ProfileApplyReport, ProfileId, ReconcileAction, ReconcileBatchItem, ReconcileBatchReport,
    ReconcileItemResult, RenamePreviewItem, RenameRuleConfig, SearchQuery, SearchResponse,
    SyncReport, TaskKind, TaskStatus, TrackedCollectionRecord, UserGroup, WorkshopFetchOutcome,
    WorkshopFileType, WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use l4a_steam::{BridgeError, DynamicSteamBridge, SteamBridge, SteamCapabilities};
use l4a_vpk::{extract_addon_metadata, inspect_dummy_vpk, DummyInspection, DummySpec};
use l4a_workshop::{
    upgrade_steam_cdn_https, CatalogBackendProvider, DownloadProgress, DownloadRequest,
    DownloadResult, ProgressSink, ResilientDownloader, SourceRoutingPolicy, SteamBridgeProvider,
    SteamWebApiProvider, WebScraperPlaceholderProvider, WorkshopError, WorkshopProvider,
    WorkshopResolver,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};
use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod download_tests;

pub trait EventSink: Send + Sync {
    fn emit(&self, event: CoreEvent);
}

#[derive(Default)]
pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn emit(&self, _event: CoreEvent) {}
}

type RescanCallback = Arc<dyn Fn() + Send + Sync>;

/// 客户端核心门面：协调本地 SQLite 状态、VPK 扫描与 Dummy v2 对账、生命周期同步、
/// 断点续传下载与四层分组预设操作。
pub struct L4aEngine {
    paths: AppPaths,
    db: Arc<LocalDb>,
    config: RwLock<AppConfig>,
    mirror: Arc<MirrorRouter>,
    steam_bridge: Arc<dyn SteamBridge>,
    dynamic_bridge: Option<Arc<DynamicSteamBridge>>,
    resolver: RwLock<Arc<WorkshopResolver>>,
    downloader: ResilientDownloader,
    watcher: Mutex<AddonWatcher>,
    watcher_rescan_cb: Mutex<Option<RescanCallback>>,
    events: Arc<dyn EventSink>,
    active_cancellations: Mutex<HashMap<WorkshopId, CancellationToken>>,
    download_gate: DownloadGate,
    hydrating_workshop: AtomicBool,
    hydration_pending: AtomicBool,
    hydration_backoff: Mutex<HashMap<WorkshopId, (u32, Instant)>>,
}

impl L4aEngine {
    pub fn open_default(
        runtime_dir: impl AsRef<Path>,
        events: Arc<dyn EventSink>,
    ) -> Result<Self, CoreError> {
        let paths = AppPaths::from_runtime_dir(runtime_dir);
        paths.ensure_dirs()?;

        let mut config = load_config(&paths.settings_path)?;
        config.download_concurrency = config.download_concurrency.clamp(1, 8);
        if config.loading_dir.is_empty() {
            if let Some(detected) = l4a_steam::detect_l4d2_addons_path() {
                let loading = Path::new(&detected);
                config.workshop_dir = loading.join("workshop").to_string_lossy().into_owned();
                config.loading_dir = detected;
            } else {
                let default_loading = paths.runtime_dir.join("addons-loading");
                config.workshop_dir = default_loading
                    .join("workshop")
                    .to_string_lossy()
                    .into_owned();
                config.loading_dir = default_loading.to_string_lossy().into_owned();
            }
            save_config(&paths.settings_path, &config)?;
        }

        let mirror = Arc::new(
            MirrorRouter::from_file(&paths.mirrors_path).unwrap_or_else(|_| MirrorRouter::empty()),
        );
        mirror.set_https_only(config.https_only);
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| paths.runtime_dir.clone());
        let dynamic_bridge = Arc::new(DynamicSteamBridge::new(
            exe_dir,
            !config.disable_steamworks_sdk,
        ));
        let steam_bridge: Arc<dyn SteamBridge> = dynamic_bridge.clone();

        let db = Arc::new(LocalDb::open(&paths.db_path)?);
        let resolver = build_resolver_from_config(&config, mirror.clone(), steam_bridge.clone());

        Ok(Self {
            downloader: ResilientDownloader::new(&paths.download_cache_dir, mirror.clone()),
            download_gate: DownloadGate::new(config.download_concurrency),
            paths,
            db,
            config: RwLock::new(config),
            mirror,
            steam_bridge,
            dynamic_bridge: Some(dynamic_bridge),
            resolver: RwLock::new(resolver),
            watcher: Mutex::new(AddonWatcher::default()),
            watcher_rescan_cb: Mutex::new(None),
            events,
            active_cancellations: Mutex::new(HashMap::new()),
            hydrating_workshop: AtomicBool::new(false),
            hydration_pending: AtomicBool::new(false),
            hydration_backoff: Mutex::new(HashMap::new()),
        })
    }

    pub fn with_dependencies(
        paths: AppPaths,
        db: Arc<LocalDb>,
        mut config: AppConfig,
        mirror: Arc<MirrorRouter>,
        steam_bridge: Arc<dyn SteamBridge>,
        resolver: Arc<WorkshopResolver>,
        events: Arc<dyn EventSink>,
    ) -> Result<Self, CoreError> {
        paths.ensure_dirs()?;
        config.download_concurrency = config.download_concurrency.clamp(1, 8);
        mirror.set_https_only(config.https_only);
        Ok(Self {
            downloader: ResilientDownloader::new(&paths.download_cache_dir, mirror.clone()),
            download_gate: DownloadGate::new(config.download_concurrency),
            paths,
            db,
            config: RwLock::new(config),
            mirror,
            steam_bridge,
            dynamic_bridge: None,
            resolver: RwLock::new(resolver),
            watcher: Mutex::new(AddonWatcher::default()),
            watcher_rescan_cb: Mutex::new(None),
            events,
            active_cancellations: Mutex::new(HashMap::new()),
            hydrating_workshop: AtomicBool::new(false),
            hydration_pending: AtomicBool::new(false),
            hydration_backoff: Mutex::new(HashMap::new()),
        })
    }

    pub fn db(&self) -> &Arc<LocalDb> {
        &self.db
    }

    pub fn config(&self) -> AppConfig {
        self.config.read().unwrap().clone()
    }

    pub fn resolve_managed_path(&self, raw_path: &str) -> Result<PathBuf, CoreError> {
        if raw_path.trim().is_empty() || raw_path.contains('\0') {
            return Err(CoreError::InvalidInput("文件路径无效".into()));
        }

        let cfg = self.config();
        let mut roots = Vec::new();
        for dir in [&cfg.loading_dir, &cfg.workshop_dir] {
            if dir.trim().is_empty() {
                continue;
            }
            if let Ok(root) = fs::canonicalize(dir) {
                if root.is_dir() {
                    roots.push(root);
                }
            }
        }
        if roots.is_empty() {
            return Err(CoreError::NotFound("未找到受管理的目录".into()));
        }

        let target = fs::canonicalize(raw_path).map_err(|err| {
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) {
                CoreError::NotFound(raw_path.into())
            } else {
                CoreError::Io(err)
            }
        })?;
        if !roots.iter().any(|root| target.starts_with(root)) {
            return Err(CoreError::InvalidInput("路径位于受管理的目录之外".into()));
        }
        Ok(target)
    }

    fn lock_workspace(&self) -> Result<std::sync::MutexGuard<'_, ()>, CoreError> {
        self.db.lock_workspace()
    }

    pub fn save_config(&self, mut next: AppConfig) -> Result<AppConfig, CoreError> {
        let workspace = self.lock_workspace()?;
        if next.workshop_dir.is_empty() && !next.loading_dir.is_empty() {
            next.workshop_dir = Path::new(&next.loading_dir)
                .join("workshop")
                .to_string_lossy()
                .into_owned();
        }
        next.download_concurrency = next.download_concurrency.clamp(1, 8);
        save_config(&self.paths.settings_path, &next)?;
        let dirs_changed = {
            let mut guard = self.config.write().unwrap();
            self.mirror.set_https_only(next.https_only);
            let changed =
                guard.loading_dir != next.loading_dir || guard.workshop_dir != next.workshop_dir;
            *guard = next.clone();
            changed
        };
        if let Some(ref dyn_bridge) = self.dynamic_bridge {
            dyn_bridge.set_enabled(!next.disable_steamworks_sdk);
        }
        let new_resolver =
            build_resolver_from_config(&next, self.mirror.clone(), self.steam_bridge.clone());
        *self.resolver.write().unwrap() = new_resolver;
        self.download_gate.set_limit(next.download_concurrency);
        self.hydration_backoff.lock().unwrap().clear();

        let watch_error = if dirs_changed {
            let callback = self.watcher_rescan_cb.lock().unwrap().clone();
            callback.and_then(|cb| self.start_watcher(cb).err())
        } else {
            None
        };
        drop(workspace);
        if let Some(err) = watch_error {
            self.events.emit(CoreEvent::WatchError {
                message: format!("重新绑定目录监听失败: {err}"),
            });
        };
        Ok(next)
    }

    pub fn scan(&self) -> Result<DashboardSnapshot, CoreError> {
        self.scan_internal(false)
    }

    pub fn scan_external(&self) -> Result<DashboardSnapshot, CoreError> {
        self.scan_internal(true)
    }

    fn refresh_bundles_logged(&self) {
        if let Err(err) = rebuild_auto_bundles(&self.db) {
            tracing::warn!(error = %err, "自动重建战役整合包失败");
        }
    }

    fn scan_internal(&self, external_change: bool) -> Result<DashboardSnapshot, CoreError> {
        let workspace = self.lock_workspace()?;
        let cfg = self.config();
        scan_workspace(&self.db, &cfg, &self.paths.image_cache_dir)?;
        let issues = self.db.list_open_reconcile_issues()?;
        auto_resolve_by_policy(&self.db, &cfg, &self.paths.image_cache_dir, &issues)?;
        self.refresh_bundles_logged();
        let snapshot = self.snapshot_locked()?;
        drop(workspace);
        if !snapshot.open_issues.is_empty() {
            self.events.emit(CoreEvent::ReconcileIssuesDetected {
                count: snapshot.open_issues.len(),
            });
        }
        self.events
            .emit(CoreEvent::SnapshotUpdated { external_change });
        Ok(snapshot)
    }

    pub fn snapshot(&self) -> Result<DashboardSnapshot, CoreError> {
        let _workspace = self.lock_workspace()?;
        self.snapshot_locked()
    }

    pub fn read_cached_image(&self, cache_key: &str) -> Result<Vec<u8>, CoreError> {
        let key = Path::new(cache_key);
        let mut components = key.components();
        let invalid = || CoreError::InvalidInput("无效的缓存图片文件名".into());
        if cache_key.contains(['/', '\\', '\0'])
            || cache_key.contains("..")
            || (cfg!(windows) && cache_key.contains(':'))
            || !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
            || !matches!(
                key.extension().and_then(|ext| ext.to_str()),
                Some("jpg" | "jpeg" | "png" | "webp")
            )
        {
            return Err(invalid());
        }

        let path = self.paths.image_cache_dir.join(key);
        let read_error = |err: io::Error| match err.kind() {
            io::ErrorKind::NotFound => CoreError::NotFound(format!("缓存图片不存在: {cache_key}")),
            _ => err.into(),
        };
        let metadata = fs::symlink_metadata(&path).map_err(read_error)?;
        if !metadata.is_file() {
            return Err(invalid());
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let mut file = options.open(path).map_err(read_error)?;
        if !same_file(&metadata, &file.metadata()?) {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub async fn fetch_workshop_cover(&self, url: &str) -> Result<(String, Vec<u8>), CoreError> {
        const MAX_COVER_BYTES: usize = 16 * 1024 * 1024;
        const COVER_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
        const CACHED_COVER_FORMATS: &[(&str, &str)] = &[
            ("jpg", "image/jpeg"),
            ("png", "image/png"),
            ("webp", "image/webp"),
            ("gif", "image/gif"),
        ];

        let normalized = upgrade_steam_cdn_https(url);
        let url = url::Url::parse(&normalized)
            .ok()
            .filter(|url| {
                matches!(url.scheme(), "http" | "https")
                    && url.username().is_empty()
                    && url.password().is_none()
            })
            .ok_or_else(|| CoreError::InvalidInput("无效的封面图片地址".into()))?;
        self.mirror
            .validate_url(url.as_str())
            .map_err(|err| WorkshopError::Network(err.to_string()))?;

        let url_hash = hex::encode(&Sha256::digest(url.as_str().as_bytes())[..16]);
        let mut stale_cached: Option<(&'static str, Vec<u8>)> = None;
        for &(ext, cached_mime) in CACHED_COVER_FORMATS {
            let cache_path = self
                .paths
                .image_cache_dir
                .join(format!("ws_cover_{url_hash}.{ext}"));
            if let Ok(meta) = fs::symlink_metadata(&cache_path) {
                if meta.is_file() && meta.len() > 0 && meta.len() <= MAX_COVER_BYTES as u64 {
                    if let Ok(bytes) = fs::read(&cache_path) {
                        let is_fresh = meta
                            .modified()
                            .ok()
                            .and_then(|m| SystemTime::now().duration_since(m).ok())
                            .is_some_and(|age| age < COVER_CACHE_TTL);
                        if is_fresh {
                            return Ok((cached_mime.to_string(), bytes));
                        }
                        stale_cached = Some((cached_mime, bytes));
                        break;
                    }
                }
            }
        }

        let (client, _) = self
            .mirror
            .build_client_for(url.as_str(), Some(Duration::from_secs(15)))
            .map_err(|err| WorkshopError::Network(err.to_string()))?;
        let (request, _) = self.mirror.prepare_get(&client, url.as_str());
        let mut response = match request.send().await {
            Ok(resp) => match resp.error_for_status() {
                Ok(ok_resp) => ok_resp,
                Err(status_err) => {
                    if status_err
                        .status()
                        .is_some_and(|s| s.is_server_error() || s.as_u16() == 429)
                    {
                        if let Some((stale_mime, stale_bytes)) = stale_cached {
                            return Ok((stale_mime.to_string(), stale_bytes));
                        }
                    }
                    return Err(WorkshopError::Network(status_err.to_string()).into());
                }
            },
            Err(net_err) => {
                if let Some((stale_mime, stale_bytes)) = stale_cached {
                    return Ok((stale_mime.to_string(), stale_bytes));
                }
                return Err(WorkshopError::Network(net_err.to_string()).into());
            }
        };

        let header_mime = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(|mime| mime.trim().to_ascii_lowercase())
            .filter(|mime| mime.starts_with("image/"));

        let mut bytes = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if bytes.len() + chunk.len() > MAX_COVER_BYTES {
                        return Err(CoreError::InvalidInput("封面图片过大".into()));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(net_err) => {
                    if let Some((stale_mime, stale_bytes)) = stale_cached {
                        return Ok((stale_mime.to_string(), stale_bytes));
                    }
                    return Err(WorkshopError::Network(net_err.to_string()).into());
                }
            }
        }
        if bytes.is_empty() {
            if let Some((stale_mime, stale_bytes)) = stale_cached {
                return Ok((stale_mime.to_string(), stale_bytes));
            }
            return Err(CoreError::InvalidInput("封面图片为空".into()));
        }

        let detected = image::guess_format(&bytes).ok();
        if detected.is_none() {
            if let Some((stale_mime, stale_bytes)) = stale_cached {
                return Ok((stale_mime.to_string(), stale_bytes));
            }
        }
        let mime = detected
            .map(|format| format.to_mime_type().to_string())
            .or(header_mime)
            .ok_or_else(|| CoreError::InvalidInput("无法识别封面图片格式".into()))?;

        // 仅当字节头通过真实图片格式探测时才落盘缓存，避免网关 HTML 错误页伪造 Content-Type 污染磁盘缓存。
        let cache_ext = match detected {
            Some(image::ImageFormat::Jpeg) => Some("jpg"),
            Some(image::ImageFormat::Png) => Some("png"),
            Some(image::ImageFormat::WebP) => Some("webp"),
            Some(image::ImageFormat::Gif) => Some("gif"),
            _ => None,
        };
        if let Some(ext) = cache_ext {
            let dest = self
                .paths
                .image_cache_dir
                .join(format!("ws_cover_{url_hash}.{ext}"));
            if let Ok(mut tmp) = tempfile::Builder::new()
                .prefix(".ws_cover_")
                .tempfile_in(&self.paths.image_cache_dir)
            {
                if tmp.write_all(&bytes).is_ok() && tmp.persist(&dest).is_ok() {
                    for &(other_ext, _) in CACHED_COVER_FORMATS {
                        if other_ext != ext {
                            let stale_other = self
                                .paths
                                .image_cache_dir
                                .join(format!("ws_cover_{url_hash}.{other_ext}"));
                            let _ = fs::remove_file(stale_other);
                        }
                    }
                }
            }
        }

        Ok((mime, bytes))
    }

    #[doc(hidden)]
    pub fn expire_hydration_backoff_for_test(&self) {
        let now = Instant::now();
        let mut guard = self.hydration_backoff.lock().unwrap();
        for (_, retry_at) in guard.values_mut() {
            *retry_at = now;
        }
    }

    /// 自动为尚未缓存工坊快照的本地/工坊模组补齐 Steam 创意工坊元数据（标题、封面 `preview_url`、作者、标签与依赖关系），
    /// 与 v1 启动/扫描时自动查询工坊元数据的行为保持一致，且使用单飞保护与有界退避重试避免并发及离线循环请求。
    pub async fn hydrate_missing_workshop_metadata(&self) -> Result<usize, CoreError> {
        const HYDRATE_BATCH_SIZE: usize = 50;
        const HYDRATION_RETRY_BASE: Duration = Duration::from_secs(15);
        const HYDRATION_RETRY_MAX: Duration = Duration::from_secs(300);

        struct HydrationGuard<'a>(&'a AtomicBool);
        impl Drop for HydrationGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }

        let record_unresolved = |ids: &[WorkshopId]| {
            if ids.is_empty() {
                return;
            }
            let now = Instant::now();
            let mut guard = self.hydration_backoff.lock().unwrap();
            for &wid in ids {
                let attempts = guard.get(&wid).map_or(0, |(n, _)| n.saturating_add(1));
                let factor = 1u32.checked_shl(attempts.min(5)).unwrap_or(32);
                let delay = HYDRATION_RETRY_BASE
                    .saturating_mul(factor)
                    .min(HYDRATION_RETRY_MAX);
                guard.insert(wid, (attempts, now + delay));
            }
        };

        self.hydration_pending.store(true, Ordering::SeqCst);
        let mut total_hydrated = 0usize;

        loop {
            if self
                .hydrating_workshop
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return Ok(total_hydrated);
            }
            let guard = HydrationGuard(&self.hydrating_workshop);

            while self.hydration_pending.swap(false, Ordering::SeqCst) {
                let resolver = self.resolver.read().unwrap().clone();
                if resolver.providers().is_empty() {
                    break;
                }

                let snapshots = self.db.list_workshop_snapshots()?;
                let in_backoff: HashSet<WorkshopId> = {
                    let now = Instant::now();
                    let backoff = self.hydration_backoff.lock().unwrap();
                    backoff
                        .iter()
                        .filter_map(|(&wid, &(_, retry_at))| (now < retry_at).then_some(wid))
                        .collect()
                };
                let mut missing: BTreeSet<WorkshopId> = self
                    .db
                    .list_addons()?
                    .into_iter()
                    .filter(|a| a.remote_status == WorkshopItemStatus::Unknown)
                    .filter_map(|a| a.workshop_id)
                    .filter(|wid| !snapshots.contains_key(wid) && !in_backoff.contains(wid))
                    .collect();
                for col in self.db.list_tracked_collections()? {
                    for wid in col.member_workshop_ids {
                        if !snapshots.contains_key(&wid) && !in_backoff.contains(&wid) {
                            missing.insert(wid);
                        }
                    }
                }

                if missing.is_empty() {
                    break;
                }

                let missing_ids: Vec<WorkshopId> = missing.into_iter().collect();
                let mut pass_hydrated = 0usize;

                for chunk in missing_ids.chunks(HYDRATE_BATCH_SIZE) {
                    let Ok((outcomes, _warnings)) = resolver.fetch_items(chunk).await else {
                        record_unresolved(chunk);
                        continue;
                    };

                    let mut resolved_ids = HashSet::with_capacity(outcomes.len());
                    for outcome in &outcomes {
                        let wid = match outcome {
                            WorkshopFetchOutcome::Found { item } => item.workshop_id,
                            WorkshopFetchOutcome::Removed { workshop_id, .. }
                            | WorkshopFetchOutcome::PrivateOrUnavailable { workshop_id, .. } => {
                                *workshop_id
                            }
                        };
                        resolved_ids.insert(wid);
                    }
                    if !resolved_ids.is_empty() {
                        let mut backoff = self.hydration_backoff.lock().unwrap();
                        for wid in &resolved_ids {
                            backoff.remove(wid);
                        }
                    }
                    let unresolved: Vec<WorkshopId> = chunk
                        .iter()
                        .copied()
                        .filter(|wid| !resolved_ids.contains(wid))
                        .collect();
                    record_unresolved(&unresolved);

                    if outcomes.is_empty() {
                        continue;
                    }

                    let mut chunk_changed = false;
                    {
                        let _workspace = self.lock_workspace()?;
                        for outcome in outcomes {
                            match outcome {
                                WorkshopFetchOutcome::Found { item } => {
                                    if self.db.has_workshop_snapshot(item.workshop_id)? {
                                        continue;
                                    }
                                    if item.file_type == WorkshopFileType::Collection {
                                        self.db.mark_workshop_status(
                                            item.workshop_id,
                                            WorkshopItemStatus::PrivateOrUnavailable,
                                            item.source,
                                        )?;
                                    } else {
                                        let replace_deps =
                                            source_provides_dependencies(item.source)
                                                || !item.dependencies.is_empty();
                                        self.db.upsert_workshop_snapshot(&item, replace_deps)?;
                                        pass_hydrated += 1;
                                    }
                                    chunk_changed = true;
                                }
                                WorkshopFetchOutcome::Removed {
                                    workshop_id,
                                    source,
                                    ..
                                } => {
                                    if self.db.has_workshop_snapshot(workshop_id)? {
                                        continue;
                                    }
                                    self.db.mark_workshop_status(
                                        workshop_id,
                                        WorkshopItemStatus::Removed,
                                        source,
                                    )?;
                                    chunk_changed = true;
                                }
                                WorkshopFetchOutcome::PrivateOrUnavailable {
                                    workshop_id,
                                    source,
                                    ..
                                } => {
                                    if self.db.has_workshop_snapshot(workshop_id)? {
                                        continue;
                                    }
                                    self.db.mark_workshop_status(
                                        workshop_id,
                                        WorkshopItemStatus::PrivateOrUnavailable,
                                        source,
                                    )?;
                                    chunk_changed = true;
                                }
                            }
                        }
                        if chunk_changed {
                            self.refresh_bundles_logged();
                        }
                    }

                    if chunk_changed {
                        self.events.emit(CoreEvent::SnapshotUpdated {
                            external_change: false,
                        });
                    }
                }

                total_hydrated += pass_hydrated;
                if pass_hydrated == 0 {
                    break;
                }
            }

            drop(guard);
            if !self.hydration_pending.load(Ordering::SeqCst) {
                break;
            }
        }

        Ok(total_hydrated)
    }

    fn snapshot_locked(&self) -> Result<DashboardSnapshot, CoreError> {
        Ok(DashboardSnapshot {
            config: self.config(),
            addons: self.db.list_addons()?,
            bundles: self.db.list_bundles()?,
            collections: self.db.list_tracked_collections()?,
            user_groups: self.db.list_user_groups()?,
            profiles: self.db.list_profiles()?,
            open_issues: self.db.list_open_reconcile_issues()?,
            tasks: self.db.list_tasks()?,
        })
    }

    pub fn resolve_issue(
        &self,
        issue_id: &IssueId,
        action: ReconcileAction,
    ) -> Result<DashboardSnapshot, CoreError> {
        let workspace = self.lock_workspace()?;
        let cfg = self.config();
        self.suppress_watcher();
        let result = resolve_reconcile_issue(
            &self.db,
            &cfg,
            &self.paths.image_cache_dir,
            issue_id,
            action,
        );
        if result.is_err() {
            self.refresh_after_error(&cfg);
        } else {
            self.refresh_bundles_logged();
        }
        let snapshot = result.and_then(|()| self.snapshot_locked());
        drop(workspace);
        self.emit_workspace_update();
        snapshot
    }

    pub fn resolve_issues_batch(
        &self,
        items: &[ReconcileBatchItem],
    ) -> Result<ReconcileBatchReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let cfg = self.config();
        self.suppress_watcher();

        let mut results = Vec::with_capacity(items.len());
        let mut any_ok = false;
        for item in items {
            let error = resolve_reconcile_issue(
                &self.db,
                &cfg,
                &self.paths.image_cache_dir,
                &item.issue_id,
                item.action,
            )
            .err()
            .map(|err| {
                self.refresh_after_error(&cfg);
                err.into()
            });
            if error.is_none() {
                any_ok = true;
            }
            results.push(ReconcileItemResult {
                issue_id: item.issue_id.clone(),
                action: item.action,
                ok: error.is_none(),
                error,
            });
        }
        if any_ok {
            self.refresh_bundles_logged();
        }

        let snapshot = self.snapshot_locked();
        drop(workspace);
        self.emit_workspace_update();
        Ok(ReconcileBatchReport {
            items: results,
            snapshot: snapshot?,
        })
    }

    pub async fn sync_workshop(&self) -> Result<SyncReport, CoreError> {
        self.hydration_backoff.lock().unwrap().clear();
        let resolver = self.resolver.read().unwrap().clone();

        let mut task = create_task(&self.db, TaskKind::Sync, "workshop-sync", None)?;
        update_task(&self.db, &mut task, TaskStatus::Running, 10, None, None)?;

        match sync_lifecycle(&self.db, &resolver).await {
            Ok(report) => {
                if let Ok(_workspace) = self.lock_workspace() {
                    self.refresh_bundles_logged();
                }
                update_task(
                    &self.db,
                    &mut task,
                    TaskStatus::Completed,
                    100,
                    Some(format!("同步完成，检查 {} 个条目", report.checked_items)),
                    None,
                )?;
                self.events.emit(CoreEvent::SnapshotUpdated {
                    external_change: false,
                });
                Ok(report)
            }
            Err(err) => {
                update_task(
                    &self.db,
                    &mut task,
                    TaskStatus::Failed,
                    0,
                    None,
                    Some(err.to_string()),
                )?;
                Err(err)
            }
        }
    }

    pub async fn search_workshop(&self, query: &SearchQuery) -> Result<SearchResponse, CoreError> {
        let resolver = self.resolver.read().unwrap().clone();
        let (resp, _) = resolver.search(query).await?;
        Ok(resp)
    }

    pub async fn fetch_workshop_item(
        &self,
        workshop_id: WorkshopId,
    ) -> Result<WorkshopItemRecord, CoreError> {
        let resolver = self.resolver.read().unwrap().clone();
        fetch_workshop_item(&resolver, workshop_id).await
    }

    pub async fn subscribed_workshop_ids(&self) -> Result<Vec<WorkshopId>, CoreError> {
        let items = self.steam_bridge.subscribed_items().await.map_err(|err| {
            let message = err.to_string();
            match err {
                BridgeError::Unavailable(_) => WorkshopError::Unsupported(message),
                _ => WorkshopError::Provider(message),
            }
        })?;
        Ok(items.into_iter().map(|item| item.workshop_id).collect())
    }

    pub async fn download_workshop_item(
        &self,
        workshop_id: WorkshopId,
    ) -> Result<AddonSummary, CoreError> {
        self.download_workshop_item_with_title(workshop_id, None)
            .await
    }

    pub async fn download_workshop_item_with_title(
        &self,
        workshop_id: WorkshopId,
        title_hint: Option<&str>,
    ) -> Result<AddonSummary, CoreError> {
        self.transfer_workshop_item(workshop_id, false, title_hint)
            .await
    }

    pub async fn update_workshop_item(
        &self,
        workshop_id: WorkshopId,
    ) -> Result<AddonSummary, CoreError> {
        self.update_workshop_item_with_title(workshop_id, None)
            .await
    }

    pub async fn update_workshop_item_with_title(
        &self,
        workshop_id: WorkshopId,
        title_hint: Option<&str>,
    ) -> Result<AddonSummary, CoreError> {
        self.transfer_workshop_item(workshop_id, true, title_hint)
            .await
    }

    async fn transfer_workshop_item(
        &self,
        workshop_id: WorkshopId,
        update: bool,
        title_hint: Option<&str>,
    ) -> Result<AddonSummary, CoreError> {
        self.transfer_workshop_item_with(
            workshop_id,
            update,
            title_hint,
            |request, cancel, progress| async move {
                self.downloader
                    .download(&request, cancel, Some(progress))
                    .await
            },
        )
        .await
    }

    async fn transfer_workshop_item_with<F>(
        &self,
        workshop_id: WorkshopId,
        update: bool,
        title_hint: Option<&str>,
        download: impl FnOnce(DownloadRequest, CancellationToken, ProgressSink) -> F,
    ) -> Result<AddonSummary, CoreError>
    where
        F: Future<Output = Result<DownloadResult, WorkshopError>>,
    {
        struct ActiveDownloadGuard<'a> {
            map: &'a Mutex<HashMap<WorkshopId, CancellationToken>>,
            wid: WorkshopId,
        }
        impl Drop for ActiveDownloadGuard<'_> {
            fn drop(&mut self) {
                if let Ok(mut guard) = self.map.lock() {
                    guard.remove(&self.wid);
                }
            }
        }

        let cancel = CancellationToken::new();
        {
            let mut guard = self.active_cancellations.lock().unwrap();
            if guard.contains_key(&workshop_id) {
                return Err(CoreError::Conflict(format!(
                    "条目 {workshop_id} 正在下载中"
                )));
            }
            guard.insert(workshop_id, cancel.clone());
        }
        let _active_guard = ActiveDownloadGuard {
            map: &self.active_cancellations,
            wid: workshop_id,
        };

        let (cfg, resolver, installed) = {
            let _workspace = self.lock_workspace()?;
            let cfg = self.config();
            let installed = if update {
                Some(installed_workshop_addon(&self.db, &cfg, workshop_id)?)
            } else {
                if cfg.loading_dir.trim().is_empty() {
                    return Err(CoreError::InvalidInput("未配置 loading_dir".into()));
                }
                None
            };
            (cfg, self.resolver.read().unwrap().clone(), installed)
        };

        let nonempty_title = |title: &str| {
            let title = title.trim();
            (!title.is_empty()).then(|| title.to_owned())
        };
        let initial_title = installed
            .as_ref()
            .and_then(|addon| nonempty_title(&addon.title))
            .or_else(|| self.db.cached_workshop_title(workshop_id).ok().flatten())
            .or_else(|| title_hint.and_then(nonempty_title));

        let mut task = create_task(
            &self.db,
            TaskKind::Download,
            workshop_id.to_string(),
            initial_title,
        )?;
        self.emit_workspace_update();
        let result = async {
            let _slot = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(WorkshopError::Cancelled.into()),
                slot = self.download_gate.acquire() => slot,
            };
            if cancel.is_cancelled() {
                return Err(WorkshopError::Cancelled.into());
            }
            update_task(&self.db, &mut task, TaskStatus::Running, 0, None, None)?;
            self.emit_workspace_update();
            let item = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(WorkshopError::Cancelled.into()),
                result = fetch_workshop_item(&resolver, workshop_id) => result?,
            };
            let resolved_title = nonempty_title(&item.title).or_else(|| task.target_title.clone());
            if resolved_title != task.target_title {
                task.target_title = resolved_title.clone();
                update_task(&self.db, &mut task, TaskStatus::Running, 0, None, None)?;
            }
            let file_url = item.file_url.clone().ok_or_else(|| {
                CoreError::InvalidInput(format!("条目 {workshop_id} 无可用下载直链"))
            })?;
            let dest = if let Some(addon) = &installed {
                addon_path(&cfg, addon)?.to_owned()
            } else {
                let name =
                    build_target_vpk_name(Some(workshop_id), None, &item.title, &cfg.rename_rules);
                let path = Path::new(&cfg.loading_dir).join(name);
                check_download_target(&self.db, &cfg, workshop_id, &path)?;
                path
            };
            let publish_dummy = cfg.enable_dummy_bypass
                && installed
                    .as_ref()
                    .is_none_or(|addon| addon.primary_dir == Some(AddonDirType::Loading));
            if publish_dummy && cfg.workshop_dir.trim().is_empty() {
                return Err(CoreError::InvalidInput(
                    "已启用 Dummy Bypass，但未配置 workshop_dir".into(),
                ));
            }

            let staged = tempfile::Builder::new()
                .prefix(".l4a-download-")
                .tempdir_in(&self.paths.download_cache_dir)?;
            let staged_name = dest
                .file_name()
                .ok_or_else(|| CoreError::InvalidInput("无效的下载目标文件名".into()))?;
            let staged_path = staged.path().join(staged_name);
            self.events.emit(CoreEvent::DownloadProgress {
                task_id: task.id.clone(),
                workshop_id,
                title: resolved_title.clone(),
                downloaded: 0,
                total: item.file_size,
                percent: 0,
                speed_bps: Some(0),
            });
            let events = self.events.clone();
            let task_id = task.id.clone();
            let sink_title = resolved_title.clone();
            let sink = Arc::new(move |p: DownloadProgress| {
                events.emit(CoreEvent::DownloadProgress {
                    task_id: task_id.clone(),
                    workshop_id: p.workshop_id,
                    title: sink_title.clone(),
                    downloaded: p.downloaded,
                    total: p.total,
                    percent: p.percent.min(99),
                    speed_bps: p.speed_bps,
                });
            });
            let transfer = download(
                DownloadRequest {
                    workshop_id,
                    url: file_url,
                    dest_path: staged_path.clone(),
                    max_retries: cfg.max_download_retries,
                },
                cancel.clone(),
                sink,
            )
            .await?;

            if !matches!(
                inspect_dummy_vpk(&staged_path),
                DummyInspection::RealVpk { .. }
            ) {
                return Err(CoreError::Vpk("下载内容不是有效的真实 VPK".into()));
            }
            let mut seed = downloaded_summary(
                &cfg,
                &self.paths.image_cache_dir,
                &item,
                &staged_path,
                &dest,
            )?;
            if let Some(addon) = &installed {
                seed.id.clone_from(&addon.id);
                seed.current_file_name.clone_from(&addon.current_file_name);
                seed.primary_dir = addon.primary_dir;
                seed.enabled = addon.enabled;
            }
            let summary = {
                let _workspace = self.lock_workspace()?;
                let current = self.config();
                if cfg.loading_dir != current.loading_dir
                    || cfg.workshop_dir != current.workshop_dir
                    || (installed.is_none() && cfg.rename_rules != current.rename_rules)
                    || cfg.enable_dummy_bypass != current.enable_dummy_bypass
                {
                    return Err(CoreError::Conflict(
                        "下载期间工作区配置已变化，请重试".into(),
                    ));
                }
                if let Some(addon) = &installed {
                    check_update_target(&self.db, addon, &dest)?;
                } else {
                    check_download_target(&self.db, &cfg, workshop_id, &dest)?;
                }
                self.suppress_watcher();
                let mut completed = task.clone();
                completed.status = TaskStatus::Completed;
                completed.progress_percent = 100;
                completed.message = Some("下载完成".into());
                completed.updated_at = chrono::Utc::now();
                FileChanges::run(|changes| {
                    if cancel.is_cancelled() {
                        return Err(WorkshopError::Cancelled.into());
                    }
                    if let Some(addon) = &installed {
                        take_addon_file(changes, addon, &dest)?;
                    }
                    changes.publish_copy(&staged_path, &dest)?;
                    if publish_dummy {
                        let dummy = Path::new(&cfg.workshop_dir).join(format!("{workshop_id}.vpk"));
                        take_matching_dummy(changes, &dummy, workshop_id)?;
                        changes.publish_dummy(
                            &dest,
                            &dummy,
                            &DummySpec {
                                workshop_id,
                                original_hash: seed.content_hash.clone().unwrap_or_default(),
                                original_size: seed.file_size,
                                original_time_updated: item.time_updated,
                            },
                        )?;
                    }
                    let expected = seed.clone();
                    let (_, addon) = scan_and_commit(
                        &self.db,
                        &cfg,
                        &self.paths.image_cache_dir,
                        Some(seed),
                        |tx, addons| {
                            changes.verify()?;
                            if installed.is_none()
                                && occupied(&append_os_suffix(&dest, ".disabled"))?
                            {
                                return Err(CoreError::Conflict(
                                    "下载期间禁用文件路径已被占用".into(),
                                ));
                            }
                            if cancel.is_cancelled() {
                                return Err(WorkshopError::Cancelled.into());
                            }
                            let mut addon = addons
                                .get(&expected.id)
                                .filter(|addon| {
                                    addon.current_path == expected.current_path
                                        && addon.content_hash == expected.content_hash
                                        && addon.current_file_name == expected.current_file_name
                                        && addon.primary_dir == expected.primary_dir
                                        && addon.enabled == expected.enabled
                                })
                                .cloned()
                                .ok_or_else(|| {
                                    CoreError::Conflict("下载文件发布后已变化".into())
                                })?;
                            let has_dependencies = !item.dependencies.is_empty();
                            let workshop_cover = item
                                .preview_url
                                .as_deref()
                                .and_then(crate::db::repo::normalize_valid_remote_cover_url);
                            addon.cover_image =
                                workshop_cover.or_else(|| addon.local_cover_image.clone());
                            if update {
                                LocalDb::save_workshop_update_tx(
                                    tx,
                                    &addon,
                                    &item,
                                    has_dependencies || source_provides_dependencies(item.source),
                                )?;
                            } else {
                                LocalDb::upsert_workshop_snapshot_tx(tx, &item, has_dependencies)?;
                            }
                            LocalDb::save_task_tx(tx, &completed)?;
                            addon.categories = crate::db::repo::merge_categories(
                                &addon.categories,
                                &addon.addon_info,
                                &addon.maps,
                                &item.tags,
                            );
                            addon.remote_status = item.status;
                            addon.remote_time_updated = item.time_updated;
                            addon.update_available = false;
                            Ok(addon)
                        },
                    )?;
                    Ok(addon)
                })?
            };
            if let Ok(_workspace) = self.lock_workspace() {
                self.refresh_bundles_logged();
            }
            self.events.emit(CoreEvent::DownloadProgress {
                task_id: task.id.clone(),
                workshop_id,
                title: resolved_title.clone(),
                downloaded: transfer.bytes_written,
                total: Some(transfer.bytes_written),
                percent: 100,
                speed_bps: None,
            });
            Ok(summary)
        }
        .await;

        if let Err(err) = &result {
            let status = if matches!(err, CoreError::Workshop(WorkshopError::Cancelled)) {
                TaskStatus::Cancelled
            } else {
                TaskStatus::Failed
            };
            if let Err(update_error) =
                update_task(&self.db, &mut task, status, 0, None, Some(err.to_string()))
            {
                tracing::error!(error = %update_error, "保存下载任务终态失败");
            }
        }
        self.emit_workspace_update();
        result
    }

    pub fn cancel_download(&self, workshop_id: WorkshopId) -> bool {
        if let Some(token) = self.active_cancellations.lock().unwrap().get(&workshop_id) {
            token.cancel();
            true
        } else {
            false
        }
    }

    pub fn move_addons(
        &self,
        addon_ids: &[AddonId],
        target_dir_type: AddonDirType,
    ) -> Result<AddonBatchReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let cfg = self.config();
        self.suppress_watcher();
        let report = move_addons(
            &self.db,
            &cfg,
            &self.paths.image_cache_dir,
            addon_ids,
            target_dir_type,
        )?;
        if report.refresh_error.is_none() {
            self.refresh_bundles_logged();
        }
        drop(workspace);
        self.emit_workspace_update();
        Ok(report)
    }

    pub fn toggle_addons(
        &self,
        addon_ids: &[AddonId],
        enabled: bool,
        allow_workshop_dir: bool,
    ) -> Result<AddonBatchReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let cfg = self.config();
        self.suppress_watcher();
        let report = toggle_addons(
            &self.db,
            &cfg,
            &self.paths.image_cache_dir,
            addon_ids.iter().map(|id| (id, enabled)),
            allow_workshop_dir,
        )?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(report)
    }

    pub fn preview_rename(
        &self,
        rules: &RenameRuleConfig,
        addon_ids: &[AddonId],
    ) -> Result<Vec<RenamePreviewItem>, CoreError> {
        let _workspace = self.lock_workspace()?;
        preview_batch_rename(&self.db, &self.config(), rules, addon_ids)
    }

    pub fn apply_renames(
        &self,
        renames: &[(AddonId, String)],
    ) -> Result<AddonBatchReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let cfg = self.config();
        self.suppress_watcher();
        let report = apply_renames(&self.db, &cfg, &self.paths.image_cache_dir, renames)?;
        if report.refresh_error.is_none() {
            self.refresh_bundles_logged();
        }
        drop(workspace);
        self.emit_workspace_update();
        Ok(report)
    }

    pub fn delete_addons(
        &self,
        addon_ids: &[AddonId],
        delete_disk_file: bool,
        purge_known_record: bool,
    ) -> Result<AddonBatchReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let report = self.delete_addons_locked(addon_ids, delete_disk_file, purge_known_record)?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(report)
    }

    pub fn safe_delete_obsolete_dependency(
        &self,
        workshop_id: WorkshopId,
        delete_disk_file: bool,
        purge_known_record: bool,
    ) -> Result<AddonBatchReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let addons = self.db.list_addons()?;
        let installed: HashSet<_> = addons
            .iter()
            .filter(|addon| addon.install_state == InstallState::Installed)
            .filter_map(|addon| addon.workshop_id)
            .collect();
        let parents: BTreeSet<_> = self
            .db
            .list_all_active_dependency_edges()?
            .into_iter()
            .filter_map(|(parent, dep)| {
                (dep == workshop_id && installed.contains(&parent)).then_some(parent)
            })
            .collect();
        if !parents.is_empty() {
            let ids: Vec<_> = parents.into_iter().map(|id| id.to_string()).collect();
            return Err(CoreError::Conflict(format!(
                "已安装的工坊条目仍依赖 {workshop_id}: {}",
                ids.join(", ")
            )));
        }
        let addon = addons
            .iter()
            .find(|addon| addon.workshop_id == Some(workshop_id))
            .ok_or_else(|| CoreError::NotFound(format!("未找到模组: {workshop_id}")))?;
        let report = self.delete_addons_locked(
            std::slice::from_ref(&addon.id),
            delete_disk_file,
            purge_known_record,
        )?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(report)
    }

    fn delete_addons_locked(
        &self,
        addon_ids: &[AddonId],
        delete_disk_file: bool,
        purge_known_record: bool,
    ) -> Result<AddonBatchReport, CoreError> {
        self.suppress_watcher();
        let report = delete_addons(
            &self.db,
            &self.config(),
            &self.paths.image_cache_dir,
            addon_ids,
            delete_disk_file,
            purge_known_record,
        )?;
        if report.refresh_error.is_none() {
            self.refresh_bundles_logged();
        }
        Ok(report)
    }

    pub fn rebuild_bundles(&self) -> Result<Vec<BundleRecord>, CoreError> {
        let workspace = self.lock_workspace()?;
        let res = rebuild_auto_bundles(&self.db)?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(res)
    }

    pub fn create_manual_bundle(
        &self,
        name: String,
        member_addon_ids: Vec<AddonId>,
    ) -> Result<BundleRecord, CoreError> {
        create_manual_bundle(&self.db, name, member_addon_ids)
    }

    pub fn set_bundle_locked(&self, id: &BundleId, locked: bool) -> Result<(), CoreError> {
        set_bundle_locked(&self.db, id, locked)
    }

    pub fn delete_bundle(&self, id: &BundleId) -> Result<(), CoreError> {
        let workspace = self.lock_workspace()?;
        delete_bundle(&self.db, id)?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(())
    }

    pub async fn track_collection(
        &self,
        collection_id: WorkshopId,
        auto_sync: bool,
    ) -> Result<TrackedCollectionRecord, CoreError> {
        let resolver = self.resolver.read().unwrap().clone();
        track_workshop_collection(&self.db, &resolver, collection_id, auto_sync).await
    }

    pub fn untrack_collection(&self, collection_id: WorkshopId) -> Result<(), CoreError> {
        untrack_workshop_collection(&self.db, collection_id)
    }

    pub fn save_user_group(&self, group: UserGroup) -> Result<UserGroup, CoreError> {
        save_user_group(&self.db, group)
    }

    pub fn delete_user_group(&self, id: &GroupId) -> Result<(), CoreError> {
        delete_user_group(&self.db, id)
    }

    pub fn save_profile(&self, profile: LoadoutProfile) -> Result<LoadoutProfile, CoreError> {
        save_profile(&self.db, profile)
    }

    pub fn delete_profile(&self, id: &ProfileId) -> Result<(), CoreError> {
        delete_profile(&self.db, id)
    }

    pub fn apply_profile(&self, id: &ProfileId) -> Result<ProfileApplyReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let cfg = self.config();
        self.suppress_watcher();
        let report = apply_profile(&self.db, id, &self.paths.image_cache_dir, &cfg)?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(report)
    }

    pub fn search_by_map_code(&self, query: &str) -> Result<Vec<AddonSummary>, CoreError> {
        search_by_map_code(&self.db, query)
    }

    pub async fn steam_capabilities(&self) -> SteamCapabilities {
        self.steam_bridge.capabilities().await
    }

    pub async fn shutdown(&self) {
        self.steam_bridge.shutdown().await;
    }

    pub fn start_watcher(
        &self,
        on_external_rescan: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), CoreError> {
        *self.watcher_rescan_cb.lock().unwrap() = Some(on_external_rescan.clone());
        let cfg = self.config();
        let events_err = self.events.clone();
        let mut guard = self.watcher.lock().unwrap();
        guard
            .watch(
                Path::new(&cfg.loading_dir),
                Path::new(&cfg.workshop_dir),
                Arc::new(move |external| {
                    if external {
                        on_external_rescan();
                    }
                }),
                Arc::new(move |message| {
                    events_err.emit(CoreEvent::WatchError { message });
                }),
            )
            .map_err(CoreError::InvalidInput)
    }

    fn refresh_after_error(&self, cfg: &AppConfig) {
        if let Err(err) = scan_workspace(&self.db, cfg, &self.paths.image_cache_dir) {
            tracing::warn!(error = %err, "操作失败后重新扫描目录失败");
        }
    }

    fn emit_workspace_update(&self) {
        self.events.emit(CoreEvent::SnapshotUpdated {
            external_change: false,
        });
    }

    fn suppress_watcher(&self) {
        if let Ok(guard) = self.watcher.lock() {
            guard.suppress_internal();
        }
    }
}

async fn fetch_workshop_item(
    resolver: &WorkshopResolver,
    workshop_id: WorkshopId,
) -> Result<WorkshopItemRecord, CoreError> {
    let (outcomes, _) = resolver.fetch_items(&[workshop_id]).await?;
    outcomes
        .into_iter()
        .find_map(|outcome| match outcome {
            WorkshopFetchOutcome::Found { item }
                if item.workshop_id == workshop_id
                    && !matches!(
                        item.status,
                        WorkshopItemStatus::Removed | WorkshopItemStatus::PrivateOrUnavailable
                    ) =>
            {
                Some(item)
            }
            _ => None,
        })
        .ok_or_else(|| CoreError::NotFound(format!("工坊条目不可用或已下架: {workshop_id}")))
}

fn installed_workshop_addon(
    db: &LocalDb,
    config: &AppConfig,
    wid: WorkshopId,
) -> Result<AddonSummary, CoreError> {
    let not_found = || CoreError::NotFound(format!("未安装工坊条目: {wid}"));
    let addon = db
        .list_addons()?
        .into_iter()
        .find(|addon| {
            addon.workshop_id == Some(wid) && addon.install_state == InstallState::Installed
        })
        .ok_or_else(not_found)?;
    let path = addon_path(config, &addon).map_err(|_| not_found())?;
    match fs::symlink_metadata(path) {
        Ok(meta)
            if meta.is_file()
                && matches!(inspect_dummy_vpk(path), DummyInspection::RealVpk { .. }) =>
        {
            Ok(addon)
        }
        Ok(_) => Err(not_found()),
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Err(not_found())
        }
        Err(err) => Err(err.into()),
    }
}

fn check_update_target(
    db: &LocalDb,
    installed: &AddonSummary,
    dest: &Path,
) -> Result<(), CoreError> {
    let unchanged = db.get_addon(&installed.id)?.is_some_and(|addon| {
        addon.install_state == InstallState::Installed
            && addon.workshop_id == installed.workshop_id
            && addon.current_path == installed.current_path
            && addon.current_file_name == installed.current_file_name
            && addon.primary_dir == installed.primary_dir
            && addon.enabled == installed.enabled
            && addon.content_hash == installed.content_hash
            && addon.file_size == installed.file_size
    });
    if !unchanged || !occupied(dest)? {
        return Err(CoreError::Conflict(format!(
            "下载期间已安装文件发生变化: {}",
            dest.display()
        )));
    }
    Ok(())
}

fn check_download_target(
    db: &LocalDb,
    config: &AppConfig,
    wid: WorkshopId,
    dest: &Path,
) -> Result<(), CoreError> {
    if config.enable_dummy_bypass && config.workshop_dir.trim().is_empty() {
        return Err(CoreError::InvalidInput(
            "已启用 Dummy Bypass，但未配置 workshop_dir".into(),
        ));
    }
    let recorded_file = db
        .get_addon_by_workshop_id(wid)?
        .filter(|addon| addon.install_state == InstallState::Installed)
        .and_then(|addon| addon.current_path);
    let installed_here = if let Some(path) =
        recorded_file.as_deref().map(Path::new).filter(|path| {
            [&config.loading_dir, &config.workshop_dir]
                .into_iter()
                .any(|dir| !dir.is_empty() && path.parent() == Some(Path::new(dir)))
        }) {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                metadata.is_file()
                    && matches!(inspect_dummy_vpk(path), DummyInspection::RealVpk { .. })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => return Err(err.into()),
        }
    } else {
        false
    };
    if occupied(dest)? || occupied(&append_os_suffix(dest, ".disabled"))? || installed_here {
        return Err(CoreError::Conflict(format!(
            "模组或目标文件已存在: {}",
            dest.display()
        )));
    }
    if !config.workshop_dir.is_empty() {
        let workshop = Path::new(&config.workshop_dir).join(format!("{wid}.vpk"));
        let disabled = append_os_suffix(&workshop, ".disabled");
        let has_other_file = if occupied(&workshop)? {
            crate::file_changes::regular_file(&workshop)?;
            !matches!(inspect_dummy_vpk(&workshop), DummyInspection::Dummy(meta)
                if meta.workshop_id.is_none_or(|id| id == wid))
        } else {
            false
        };
        if occupied(&disabled)? || has_other_file {
            return Err(CoreError::Conflict(format!(
                "Workshop 目录已有其他文件: {}",
                workshop.display()
            )));
        }
    }
    Ok(())
}

fn downloaded_summary(
    config: &AppConfig,
    image_cache_dir: &Path,
    item: &WorkshopItemRecord,
    staged_path: &Path,
    dest_path: &Path,
) -> Result<AddonSummary, CoreError> {
    let wid = item.workshop_id;
    let meta = extract_addon_metadata(staged_path, image_cache_dir);
    if let Some(err) = meta.error {
        return Err(CoreError::Vpk(err));
    }
    let size = fs::metadata(staged_path)?.len();
    let aid = AddonId::from_workshop_id(wid);
    let addon_title =
        crate::db::repo::clean_addon_title(meta.addon_info.get("addontitle").map(String::as_str));
    let workshop_title = crate::db::repo::clean_workshop_title(Some(item.title.as_str()));
    let title = workshop_title
        .clone()
        .or_else(|| addon_title.clone())
        .unwrap_or_else(|| {
            dest_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Workshop Addon")
                .to_string()
        });
    let author = crate::db::repo::clean_author_name(item.creator_name.as_deref()).or_else(|| {
        crate::db::repo::clean_author_name(
            meta.addon_info
                .get("addonauthor")
                .or_else(|| meta.addon_info.get("author"))
                .map(String::as_str),
        )
    });
    let version = meta
        .addon_info
        .get("addonversion")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let categories =
        crate::db::repo::merge_categories(&[], &meta.addon_info, &meta.maps, &item.tags);
    let local_cover = meta.image_path;
    let workshop_cover = item
        .preview_url
        .as_deref()
        .and_then(crate::db::repo::normalize_valid_remote_cover_url);
    let summary = AddonSummary {
        id: aid.clone(),
        workshop_id: Some(wid),
        title,
        addon_title,
        workshop_title,
        author,
        version,
        install_state: InstallState::Installed,
        primary_dir: Some(AddonDirType::Loading),
        enabled: true,
        current_file_name: dest_path
            .file_name()
            .and_then(|s| s.to_str())
            .map(ToString::to_string),
        current_path: Some(dest_path.to_string_lossy().into_owned()),
        content_hash: Some(meta.content_hash),
        file_size: size,
        files_count: meta.files_count,
        cover_image: workshop_cover.or_else(|| local_cover.clone()),
        local_cover_image: local_cover,
        categories,
        maps: meta.maps,
        has_dummy_bypass: config.enable_dummy_bypass,
        remote_status: item.status,
        local_time_updated: item.time_updated,
        remote_time_updated: item.time_updated,
        update_available: false,
        addon_info: meta.addon_info,
    };
    Ok(summary)
}

fn build_resolver_from_config(
    config: &AppConfig,
    mirror: Arc<MirrorRouter>,
    steam_bridge: Arc<dyn SteamBridge>,
) -> Arc<WorkshopResolver> {
    let mut providers: Vec<Arc<dyn WorkshopProvider>> = Vec::new();

    let catalog = config
        .catalog_server_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|url| Arc::new(CatalogBackendProvider::new(url, mirror.clone())));

    for source in &config.source_order {
        match source.as_str() {
            "catalog-server" => {
                if let Some(ref c) = catalog {
                    providers.push(c.clone());
                }
            }
            "steamworks-sdk" => {
                if !config.disable_steamworks_sdk {
                    providers.push(Arc::new(SteamBridgeProvider::new(steam_bridge.clone())));
                }
            }
            "steam-web-api" => {
                providers.push(Arc::new(SteamWebApiProvider::new(mirror.clone())));
            }
            "web-scraper" => {
                providers.push(Arc::new(WebScraperPlaceholderProvider::placeholder()));
            }
            _ => {}
        }
    }

    Arc::new(WorkshopResolver::new(
        providers,
        SourceRoutingPolicy::default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolves_existing_paths_only_inside_managed_directories() {
        let dir = tempdir().unwrap();
        let loading = dir.path().join("addons");
        let workshop = dir.path().join("workshop");
        let outside = dir.path().join("addons-other");
        for path in [&loading, &workshop, &outside] {
            fs::create_dir(path).unwrap();
        }
        let loading_file = loading.join("local.vpk");
        let workshop_file = workshop.join("123456.vpk");
        let outside_file = outside.join("outside.vpk");
        for path in [&loading_file, &workshop_file, &outside_file] {
            fs::write(path, b"test").unwrap();
        }
        let paths = AppPaths::from_runtime_dir(dir.path().join("runtime"));
        paths.ensure_dirs().unwrap();
        save_config(
            &paths.settings_path,
            &AppConfig {
                loading_dir: loading.to_str().unwrap().into(),
                workshop_dir: workshop.to_str().unwrap().into(),
                ..Default::default()
            },
        )
        .unwrap();
        let engine = L4aEngine::open_default(&paths.runtime_dir, Arc::new(NullEventSink)).unwrap();
        for path in [&loading, &workshop, &loading_file, &workshop_file] {
            assert_eq!(
                engine.resolve_managed_path(path.to_str().unwrap()).unwrap(),
                path.canonicalize().unwrap()
            );
        }
        for bad in ["", " \t", "nul\0path"] {
            assert!(matches!(
                engine.resolve_managed_path(bad),
                Err(CoreError::InvalidInput(_))
            ));
        }
        assert!(matches!(
            engine.resolve_managed_path(loading.join("missing.vpk").to_str().unwrap()),
            Err(CoreError::NotFound(_))
        ));
        for path in [
            outside_file.clone(),
            loading.join("../addons-other/outside.vpk"),
        ] {
            assert!(matches!(
                engine.resolve_managed_path(path.to_str().unwrap()),
                Err(CoreError::InvalidInput(_))
            ));
        }
        #[cfg(unix)]
        {
            let link = loading.join("outside-link.vpk");
            std::os::unix::fs::symlink(&outside_file, &link).unwrap();
            assert!(matches!(
                engine.resolve_managed_path(link.to_str().unwrap()),
                Err(CoreError::InvalidInput(_))
            ));
        }

        fs::remove_dir_all(workshop).unwrap();
        assert!(engine
            .resolve_managed_path(loading.to_str().unwrap())
            .is_ok());
        fs::remove_dir_all(loading).unwrap();
        assert!(matches!(
            engine.resolve_managed_path(outside_file.to_str().unwrap()),
            Err(CoreError::NotFound(_))
        ));
    }
}
