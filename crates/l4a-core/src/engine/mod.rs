mod covers;
mod filter;
mod hydration;
mod operations;
mod transfers;
mod workshop;

#[cfg(test)]
mod download_tests;

use crate::config::{load_config, save_config, AppPaths};
use crate::db::LocalDb;
use crate::download_gate::DownloadGate;
use crate::error::CoreError;
use crate::filter::evaluator::CatalogScoringState;
use crate::filter::{LocalFilterManager, TokenVault};
use crate::grouping::rebuild_auto_bundles;
use crate::reconcile::auto_resolve_by_policy;
use crate::scanner::scan_workspace;
use crate::watcher::AddonWatcher;
pub(crate) use filter::FilterCacheState;
use filter::{sanitize_filter_config, sanitize_filter_config_for_startup};
use l4a_mirror::MirrorRouter;
use l4a_protocol::{AppConfig, ClientFilterSourceMode, CoreEvent, DashboardSnapshot, WorkshopId};
use l4a_steam::{DynamicSteamBridge, SteamBridge, SteamCapabilities};
use l4a_workshop::{ResilientDownloader, WorkshopResolver};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use workshop::build_resolver_from_config;

pub trait EventSink: Send + Sync {
    fn emit(&self, event: CoreEvent);
}

#[derive(Default)]
pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn emit(&self, _event: CoreEvent) {}
}

type RescanCallback = Arc<dyn Fn() + Send + Sync>;

fn build_loopback_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

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
    local_filter: Arc<LocalFilterManager>,
    token_vault: RwLock<TokenVault>,
    pub(crate) filter_eval_semaphore: tokio::sync::Semaphore,
    // Serialize cache invalidation with writes and verdict events from active evaluations.
    pub(crate) filter_cache: Mutex<FilterCacheState>,
    pub(crate) catalog_scoring_cache: Mutex<Option<CatalogScoringState>>,
    pub(crate) catalog_scoring_probe: tokio::sync::Mutex<()>,
    allow_loopback_image_urls_for_testing: AtomicBool,
    loopback_http_client: reqwest::Client,
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
        sanitize_filter_config_for_startup(&mut config.filter, config.https_only);
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
        let local_filter = Arc::new(LocalFilterManager::new(paths.clone(), events.clone()));
        local_filter.set_desired_params(&config.filter.local_params);
        let token_vault = TokenVault::default_for_paths(&paths);
        let filter_cache = FilterCacheState::new(&db, &config.filter, &token_vault)?;
        let loopback_http_client = build_loopback_http_client();

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
            local_filter,
            token_vault: RwLock::new(token_vault),
            filter_eval_semaphore: tokio::sync::Semaphore::new(4),
            filter_cache: Mutex::new(filter_cache),
            catalog_scoring_cache: Mutex::new(None),
            catalog_scoring_probe: tokio::sync::Mutex::new(()),
            allow_loopback_image_urls_for_testing: AtomicBool::new(false),
            loopback_http_client,
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
        sanitize_filter_config_for_startup(&mut config.filter, config.https_only);
        mirror.set_https_only(config.https_only);
        let local_filter = Arc::new(LocalFilterManager::new(paths.clone(), events.clone()));
        local_filter.set_desired_params(&config.filter.local_params);
        let token_vault = TokenVault::default_for_paths(&paths);
        let filter_cache = FilterCacheState::new(&db, &config.filter, &token_vault)?;
        let loopback_http_client = build_loopback_http_client();
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
            local_filter,
            token_vault: RwLock::new(token_vault),
            filter_eval_semaphore: tokio::sync::Semaphore::new(4),
            filter_cache: Mutex::new(filter_cache),
            catalog_scoring_cache: Mutex::new(None),
            catalog_scoring_probe: tokio::sync::Mutex::new(()),
            allow_loopback_image_urls_for_testing: AtomicBool::new(false),
            loopback_http_client,
        })
    }

    /// Enables loopback (`http://127.0.0.1` / `http://localhost`) image URLs strictly for integration tests.
    pub fn set_allow_loopback_image_urls_for_testing(&self, allow: bool) {
        self.allow_loopback_image_urls_for_testing
            .store(allow, Ordering::Release);
    }

    /// Expires the 30-second Catalog `/api/v1/filter/info` cache timestamp strictly for integration tests.
    pub fn expire_catalog_scoring_cache_for_testing(&self) {
        if let Some(entry) = self.catalog_scoring_cache.lock().unwrap().as_mut() {
            entry.checked_at = None;
        }
    }

    pub(crate) fn allow_loopback_image_urls_for_testing(&self) -> bool {
        self.allow_loopback_image_urls_for_testing
            .load(Ordering::Acquire)
    }

    pub(crate) fn loopback_http_client(&self) -> &reqwest::Client {
        &self.loopback_http_client
    }

    pub(crate) fn prepare_routed_get(
        &self,
        url: &str,
        timeout: Duration,
    ) -> Result<reqwest::RequestBuilder, CoreError> {
        if crate::filter::llm::is_loopback_url(url) {
            return Ok(self.loopback_http_client.get(url).timeout(timeout));
        }
        let https_only = self.config.read().unwrap().https_only;
        let (req, _) = self
            .mirror
            .get_no_redirect_with_policy(url, Some(timeout), https_only)
            .map_err(|e| CoreError::Network(e.to_string()))?;
        Ok(req)
    }

    pub(crate) fn prepare_routed_post(
        &self,
        url: &str,
        timeout: Duration,
    ) -> Result<reqwest::RequestBuilder, CoreError> {
        if crate::filter::llm::is_loopback_url(url) {
            return Ok(self.loopback_http_client.post(url).timeout(timeout));
        }
        let https_only = self.config.read().unwrap().https_only;
        let (req, _) = self
            .mirror
            .post_no_redirect_with_policy(url, Some(timeout), https_only)
            .map_err(|e| CoreError::Network(e.to_string()))?;
        Ok(req)
    }

    pub(crate) fn cached_catalog_scoring_revision(
        &self,
        catalog_server_url: Option<&str>,
    ) -> Option<String> {
        let normalized = catalog_server_url
            .unwrap_or("")
            .trim()
            .trim_end_matches('/');
        if normalized.is_empty() {
            return None;
        }
        let guard = self.catalog_scoring_cache.lock().unwrap();
        match &*guard {
            Some(entry) if entry.url == normalized => entry.revision.clone(),
            _ => None,
        }
    }

    pub(crate) fn current_filter_policy_signature(&self, cfg: &AppConfig) -> String {
        let catalog_rev = if cfg.filter.mode == ClientFilterSourceMode::Catalog {
            self.cached_catalog_scoring_revision(cfg.catalog_server_url.as_deref())
        } else {
            None
        };
        cfg.filter.canonical_policy_signature(
            cfg.catalog_server_url.as_deref(),
            cfg.https_only,
            catalog_rev.as_deref(),
        )
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    pub fn db(&self) -> &Arc<LocalDb> {
        &self.db
    }

    pub(crate) fn mirror_router(&self) -> &MirrorRouter {
        &self.mirror
    }

    pub(crate) fn local_filter(&self) -> &LocalFilterManager {
        &self.local_filter
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

    fn apply_saved_config(&self, mut next: AppConfig) -> Result<(AppConfig, bool), CoreError> {
        let workspace = self.lock_workspace()?;
        if next.workshop_dir.is_empty() && !next.loading_dir.is_empty() {
            next.workshop_dir = Path::new(&next.loading_dir)
                .join("workshop")
                .to_string_lossy()
                .into_owned();
        }
        next.download_concurrency = next.download_concurrency.clamp(1, 8);
        sanitize_filter_config(&mut next.filter, next.https_only)?;
        save_config(&self.paths.settings_path, &next)?;
        let filter_cache = self.filter_cache.lock().unwrap();
        let (dirs_changed, filter_params_changed) = {
            let mut guard = self.config.write().unwrap();
            self.mirror.set_https_only(next.https_only);
            let changed =
                guard.loading_dir != next.loading_dir || guard.workshop_dir != next.workshop_dir;
            let params_changed = guard.filter.local_params != next.filter.local_params;
            *guard = next.clone();
            (changed, params_changed)
        };
        self.local_filter
            .set_desired_params(&next.filter.local_params);

        let active_policy_sig = self.current_filter_policy_signature(&next);
        if let Err(err) = self.db.prune_filter_verdicts_except(&active_policy_sig) {
            tracing::warn!(error = %err, "Failed to clear stale filter verdict cache while saving config");
        }
        drop(filter_cache);

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
        }
        Ok((next, filter_params_changed))
    }

    pub fn save_config(&self, next: AppConfig) -> Result<AppConfig, CoreError> {
        let (saved, filter_params_changed) = self.apply_saved_config(next)?;
        if filter_params_changed {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let lf = self.local_filter.clone();
                handle.spawn(async move {
                    if let Err(err) = lf.reconfigure_to_desired().await {
                        tracing::warn!(error = %err, "Failed to hot-reload local filter runtime parameters");
                    }
                });
            }
        }
        Ok(saved)
    }

    pub async fn save_config_async(&self, next: AppConfig) -> Result<AppConfig, CoreError> {
        let (saved, filter_params_changed) = self.apply_saved_config(next)?;
        if filter_params_changed {
            if let Err(err) = self.local_filter.reconfigure_to_desired().await {
                tracing::warn!(error = %err, "Failed to hot-reload local filter runtime parameters");
            }
        }
        Ok(saved)
    }

    pub fn scan(&self) -> Result<DashboardSnapshot, CoreError> {
        self.scan_internal(false)
    }

    pub fn scan_external(&self) -> Result<DashboardSnapshot, CoreError> {
        self.scan_internal(true)
    }

    fn refresh_bundles_logged(&self) {
        if let Err(err) = rebuild_auto_bundles(&self.db) {
            tracing::warn!(error = %err, "Failed to rebuild campaign bundles automatically");
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

    fn snapshot_locked(&self) -> Result<DashboardSnapshot, CoreError> {
        let mut filter_cache = self.filter_cache.lock().unwrap();
        let config = self.config();
        self.refresh_filter_env_token(&mut filter_cache, &config.filter)?;
        let addons = self.db.list_addons()?;
        let filter_policy_signature = self.current_filter_policy_signature(&config);
        let max_age = crate::filter::evaluator::remote_verdict_cache_max_age(&config.filter);
        let raw_verdict_rows = self
            .db
            .list_filter_verdicts_for_policy(&filter_policy_signature, max_age)?;

        let addon_sigs_by_key: HashMap<String, String> = addons
            .iter()
            .map(|addon| {
                (
                    format!("addon:{}", addon.id.as_str()),
                    crate::filter::compute_addon_state_sig(addon),
                )
            })
            .collect();

        let cached_filter_verdicts = raw_verdict_rows
            .into_iter()
            .filter_map(|row| {
                if let Some(current_sig) = addon_sigs_by_key.get(&row.verdict.target_key) {
                    if row.addon_state_sig.as_deref() != Some(current_sig.as_str()) {
                        return None;
                    }
                } else if row.addon_state_sig.is_some() {
                    return None;
                }
                Some(row.verdict)
            })
            .collect();

        Ok(DashboardSnapshot {
            config,
            addons,
            bundles: self.db.list_bundles()?,
            collections: self.db.list_tracked_collections()?,
            user_groups: self.db.list_user_groups()?,
            profiles: self.db.list_profiles()?,
            open_issues: self.db.list_open_reconcile_issues()?,
            tasks: self.db.list_tasks()?,
            filter_exclusions: self.db.list_filter_exclusions()?,
            cached_filter_verdicts,
            filter_policy_signature,
            filter_cache_generation: filter_cache.generation,
        })
    }

    pub async fn steam_capabilities(&self) -> SteamCapabilities {
        self.steam_bridge.capabilities().await
    }

    pub async fn shutdown(&self) {
        self.local_filter.shutdown().await;
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
            tracing::warn!(error = %err, "Failed to rescan addons directory after operation failure");
        }
    }

    pub(crate) fn emit_workspace_update(&self) {
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

    #[tokio::test]
    async fn covers_and_routed_requests_reuse_tcp_keep_alive_connections() {
        use l4a_logging::http::AuditedSend;
        use l4a_steam::MockSteamBridge;
        use l4a_workshop::{SourceRoutingPolicy, WorkshopResolver};
        use std::collections::HashSet;
        use std::sync::atomic::AtomicUsize;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let next_conn_id = Arc::new(AtomicUsize::new(1));
        let served_conns = Arc::new(Mutex::new(Vec::<(String, usize)>::new()));
        let served_conns_bg = Arc::clone(&served_conns);

        tokio::spawn(async move {
            // Minimal valid JPEG header bytes (`image::guess_format` checks magic bytes).
            let jpeg_bytes: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F'];
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let conn_id = next_conn_id.fetch_add(1, Ordering::SeqCst);
                let served_conn = Arc::clone(&served_conns_bg);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let mut pending = Vec::new();
                    loop {
                        let header_end = loop {
                            if let Some(pos) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
                                break Some(pos + 4);
                            }
                            match socket.read(&mut buf).await {
                                Ok(0) | Err(_) => break None,
                                Ok(n) => pending.extend_from_slice(&buf[..n]),
                            }
                        };
                        let Some(end) = header_end else {
                            break;
                        };
                        let header_text = String::from_utf8_lossy(&pending[..end]).into_owned();
                        let path = header_text
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or("/")
                            .to_string();
                        let content_length = header_text
                            .lines()
                            .find_map(|line| {
                                let (k, v) = line.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        pending.drain(..end);
                        while pending.len() < content_length {
                            match socket.read(&mut buf).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => pending.extend_from_slice(&buf[..n]),
                            }
                        }
                        pending.drain(..content_length);

                        served_conn.lock().unwrap().push((path.clone(), conn_id));
                        if path.starts_with("/redirect") {
                            let head = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/blocked\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n";
                            if socket.write_all(head.as_bytes()).await.is_err() {
                                break;
                            }
                        } else {
                            let head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                                jpeg_bytes.len()
                            );
                            if socket.write_all(head.as_bytes()).await.is_err()
                                || socket.write_all(jpeg_bytes).await.is_err()
                            {
                                break;
                            }
                        }
                    }
                });
            }
        });

        let dir = tempdir().unwrap();
        let loading = dir.path().join("addons");
        fs::create_dir_all(&loading).unwrap();
        let mirror = Arc::new(MirrorRouter::from_rules(vec![
            serde_json::from_value(serde_json::json!({
                "rules": "images.steamusercontent.com",
                "target": addr.to_string(),
            }))
            .unwrap(),
            serde_json::from_value(serde_json::json!({
                "rules": "filter.insecure.test",
                "target": addr.to_string(),
                "insecure-tls": true,
            }))
            .unwrap(),
        ]));
        mirror.set_no_proxy_for_testing(true);
        let engine = L4aEngine::with_dependencies(
            AppPaths::from_runtime_dir(dir.path().join("runtime")),
            Arc::new(LocalDb::open_in_memory().unwrap()),
            AppConfig {
                loading_dir: loading.to_string_lossy().into_owned(),
                https_only: false,
                ..Default::default()
            },
            mirror,
            Arc::new(MockSteamBridge::new(false)),
            Arc::new(WorkshopResolver::new(
                Vec::new(),
                SourceRoutingPolicy::default(),
            )),
            Arc::new(NullEventSink),
        )
        .unwrap();

        // 1. Multiple cover fetches share a single keep-alive TCP connection.
        for idx in 1..=3 {
            let (mime, bytes) = engine
                .fetch_workshop_cover(&format!(
                    "http://images.steamusercontent.com/ugc/{idx}/cover.jpg"
                ))
                .await
                .unwrap();
            assert_eq!(mime, "image/jpeg");
            assert!(!bytes.is_empty());
            tokio::task::yield_now().await;
        }
        {
            let recorded = served_conns.lock().unwrap();
            assert_eq!(recorded.len(), 3);
            let cover_conns: HashSet<usize> = recorded.iter().map(|(_, id)| *id).collect();
            assert_eq!(
                cover_conns.len(),
                1,
                "fetch_workshop_cover should reuse one serving TCP connection across distinct cover URLs"
            );
        }

        // 2. Routed GET and POST with insecure-tls mirror rule do not follow redirects and reuse one connection.
        let resp_get = engine
            .prepare_routed_get(
                "http://filter.insecure.test/redirect/get",
                Duration::from_secs(5),
            )
            .unwrap()
            .send_audited_quiet("test.routed.no_redirect.get")
            .await
            .unwrap();
        assert_eq!(resp_get.status(), reqwest::StatusCode::FOUND);
        let _ = resp_get.bytes().await.unwrap();
        tokio::task::yield_now().await;

        let resp_post = engine
            .prepare_routed_post(
                "http://filter.insecure.test/redirect/post",
                Duration::from_secs(5),
            )
            .unwrap()
            .body("{}")
            .send_audited_quiet("test.routed.no_redirect.post")
            .await
            .unwrap();
        assert_eq!(resp_post.status(), reqwest::StatusCode::FOUND);
        let _ = resp_post.bytes().await.unwrap();

        let recorded = served_conns.lock().unwrap();
        assert_eq!(recorded.len(), 5);
        let routed_conns: HashSet<usize> = recorded[3..].iter().map(|(_, id)| *id).collect();
        assert_eq!(
            routed_conns.len(),
            1,
            "insecure-tls no-redirect GET and POST should reuse one dedicated pooled connection"
        );
        assert_ne!(
            recorded[0].1, recorded[3].1,
            "verified cover pool and insecure-tls routed pool must remain separate connections"
        );
    }

    #[tokio::test]
    async fn shared_mirror_router_does_not_override_engine_https_only_policy() {
        use l4a_steam::MockSteamBridge;
        use l4a_workshop::{SourceRoutingPolicy, WorkshopResolver};

        let dir = tempdir().unwrap();
        let loading = dir.path().join("addons");
        fs::create_dir_all(&loading).unwrap();

        let shared_mirror = Arc::new(MirrorRouter::empty());
        let strict_engine = L4aEngine::with_dependencies(
            AppPaths::from_runtime_dir(dir.path().join("runtime-strict")),
            Arc::new(LocalDb::open_in_memory().unwrap()),
            AppConfig {
                loading_dir: loading.to_string_lossy().into_owned(),
                https_only: true,
                ..Default::default()
            },
            Arc::clone(&shared_mirror),
            Arc::new(MockSteamBridge::new(false)),
            Arc::new(WorkshopResolver::new(
                Vec::new(),
                SourceRoutingPolicy::default(),
            )),
            Arc::new(NullEventSink),
        )
        .unwrap();

        // Second engine sharing the same MirrorRouter flips shared_mirror to https_only = false.
        let permissive_engine = L4aEngine::with_dependencies(
            AppPaths::from_runtime_dir(dir.path().join("runtime-permissive")),
            Arc::new(LocalDb::open_in_memory().unwrap()),
            AppConfig {
                loading_dir: loading.to_string_lossy().into_owned(),
                https_only: false,
                ..Default::default()
            },
            Arc::clone(&shared_mirror),
            Arc::new(MockSteamBridge::new(false)),
            Arc::new(WorkshopResolver::new(
                Vec::new(),
                SourceRoutingPolicy::default(),
            )),
            Arc::new(NullEventSink),
        )
        .unwrap();

        // Strict engine MUST still reject plain HTTP even though shared_mirror was mutated.
        assert!(strict_engine
            .prepare_routed_get("http://filter.example.com/info", Duration::from_secs(5))
            .is_err());
        assert!(strict_engine
            .prepare_routed_post("http://filter.example.com/predict", Duration::from_secs(5))
            .is_err());
        assert!(strict_engine
            .fetch_workshop_cover("http://other.example.com/cover.jpg")
            .await
            .is_err());

        // Permissive engine continues to accept HTTP according to its own config.
        assert!(permissive_engine
            .prepare_routed_get("http://filter.example.com/info", Duration::from_secs(5))
            .is_ok());
    }

    #[tokio::test]
    async fn filter_preview_image_blocks_cross_origin_ssrf_redirects_and_path_escapes() {
        use l4a_steam::MockSteamBridge;
        use l4a_workshop::{SourceRoutingPolicy, WorkshopResolver};
        use std::sync::atomic::AtomicUsize;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        // Private internal service that must NEVER be contacted via redirect.
        let private_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let private_addr = private_listener.local_addr().unwrap();
        let private_hits = Arc::new(AtomicUsize::new(0));
        let private_hits_bg = Arc::clone(&private_hits);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = private_listener.accept().await else {
                    break;
                };
                private_hits_bg.fetch_add(1, Ordering::SeqCst);
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nsecret")
                    .await;
            }
        });

        // Mirrored Steam CDN fixture: `/ugc/ssrf` redirects to private_addr; `/ugc/same` redirects same-origin.
        let mirror_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mirror_addr = mirror_listener.local_addr().unwrap();
        let mut jpeg_bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg_bytes)
            .encode(&[255, 0, 0], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        let jpeg_bytes = Arc::new(jpeg_bytes);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = mirror_listener.accept().await else {
                    break;
                };
                let jpeg_bytes = Arc::clone(&jpeg_bytes);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    let text = String::from_utf8_lossy(&buf[..n]);
                    let path = text
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/");
                    if path == "/ugc/ssrf/cover.jpg" {
                        let resp = format!(
                            "HTTP/1.1 302 Found\r\nLocation: http://{private_addr}/internal.jpg\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        let _ = socket.write_all(resp.as_bytes()).await;
                    } else if path == "/ugc/same/cover.jpg" {
                        let resp = "HTTP/1.1 302 Found\r\nLocation: /ugc/same-final/cover.jpg\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = socket.write_all(resp.as_bytes()).await;
                    } else {
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            jpeg_bytes.len()
                        );
                        let _ = socket.write_all(head.as_bytes()).await;
                        let _ = socket.write_all(&jpeg_bytes).await;
                    }
                });
            }
        });

        let dir = tempdir().unwrap();
        let loading = dir.path().join("addons");
        fs::create_dir_all(&loading).unwrap();
        let mirror = Arc::new(MirrorRouter::from_rules(vec![serde_json::from_value(
            serde_json::json!({
                "rules": "mirror.cdn.test",
                "target": mirror_addr.to_string(),
            }),
        )
        .unwrap()]));
        mirror.set_no_proxy_for_testing(true);
        let engine = L4aEngine::with_dependencies(
            AppPaths::from_runtime_dir(dir.path().join("runtime")),
            Arc::new(LocalDb::open_in_memory().unwrap()),
            AppConfig {
                loading_dir: loading.to_string_lossy().into_owned(),
                https_only: false,
                ..Default::default()
            },
            mirror,
            Arc::new(MockSteamBridge::new(false)),
            Arc::new(WorkshopResolver::new(
                Vec::new(),
                SourceRoutingPolicy::default(),
            )),
            Arc::new(NullEventSink),
        )
        .unwrap();

        // 1. Cross-origin redirect to an unmirrored private address MUST be rejected before connecting.
        let ssrf_err = crate::filter::evaluator::load_single_image(
            &engine,
            "http://mirror.cdn.test/ugc/ssrf/cover.jpg",
        )
        .await;
        assert!(
            matches!(ssrf_err, Err(CoreError::InvalidInput(_))),
            "cross-origin SSRF redirect must be rejected, got {ssrf_err:?}"
        );
        assert_eq!(
            private_hits.load(Ordering::SeqCst),
            0,
            "private service must never be contacted via filter image redirect"
        );

        // 2. Same-origin redirect on the explicitly configured mirror succeeds.
        let same_ok = crate::filter::evaluator::load_single_image(
            &engine,
            "http://mirror.cdn.test/ugc/same/cover.jpg",
        )
        .await
        .expect("same-origin redirect on configured mirror should succeed");
        assert_eq!(same_ok.mime, "image/jpeg");

        // 3. Direct Steam CDN hop escaping cloud.steampowered.com `/ugc/` path or downgrading to HTTP is rejected.
        let bad_path = url::Url::parse("https://cloud.steampowered.com/other/secret.jpg").unwrap();
        let bad_resolved = l4a_mirror::ResolvedRequest {
            url: bad_path.to_string(),
            original_host: None,
            headers: HashMap::new(),
            insecure_tls: false,
            matched_rule: false,
        };
        assert!(
            crate::filter::evaluator::validate_filter_image_redirect_hop(
                &l4a_mirror::RedirectHop {
                    hop: 1,
                    url: &bad_path,
                    same_origin: true,
                    matched_mirror_rule: false,
                    resolved: &bad_resolved,
                }
            )
            .is_err()
        );
    }
}
