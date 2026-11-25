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
use crate::filter::{LocalFilterManager, TokenVault};
use crate::grouping::rebuild_auto_bundles;
use crate::reconcile::auto_resolve_by_policy;
use crate::scanner::scan_workspace;
use crate::watcher::AddonWatcher;
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

fn build_reusable_http_clients() -> (reqwest::Client, reqwest::Client, reqwest::Client) {
    let loopback = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let https_only = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let standard = reqwest::Client::builder()
        .https_only(false)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    (loopback, https_only, standard)
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
    allow_loopback_image_urls_for_testing: AtomicBool,
    loopback_http_client: reqwest::Client,
    https_only_http_client: reqwest::Client,
    standard_http_client: reqwest::Client,
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
        let token_vault = RwLock::new(TokenVault::default_for_paths(&paths));
        let (loopback_http_client, https_only_http_client, standard_http_client) =
            build_reusable_http_clients();

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
            token_vault,
            allow_loopback_image_urls_for_testing: AtomicBool::new(false),
            loopback_http_client,
            https_only_http_client,
            standard_http_client,
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
        let token_vault = RwLock::new(TokenVault::default_for_paths(&paths));
        let (loopback_http_client, https_only_http_client, standard_http_client) =
            build_reusable_http_clients();
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
            token_vault,
            allow_loopback_image_urls_for_testing: AtomicBool::new(false),
            loopback_http_client,
            https_only_http_client,
            standard_http_client,
        })
    }

    /// Enables loopback (`http://127.0.0.1` / `http://localhost`) image URLs strictly for integration tests.
    pub fn set_allow_loopback_image_urls_for_testing(&self, allow: bool) {
        self.allow_loopback_image_urls_for_testing
            .store(allow, Ordering::Release);
    }

    pub(crate) fn allow_loopback_image_urls_for_testing(&self) -> bool {
        self.allow_loopback_image_urls_for_testing
            .load(Ordering::Acquire)
    }

    pub(crate) fn loopback_http_client(&self) -> &reqwest::Client {
        &self.loopback_http_client
    }

    pub(crate) fn prepare_routed_post(
        &self,
        url: &str,
        timeout: Duration,
    ) -> Result<reqwest::RequestBuilder, CoreError> {
        if crate::filter::llm::is_loopback_url(url) {
            return Ok(self.loopback_http_client.post(url).timeout(timeout));
        }
        let resolved = self
            .mirror
            .validate_url(url)
            .map_err(|e| CoreError::Network(e.to_string()))?;
        let https_only = self.config.read().unwrap().https_only;
        if resolved.insecure_tls && !https_only {
            let (client, _) = self
                .mirror
                .build_client_for(url, Some(timeout))
                .map_err(|e| CoreError::Network(e.to_string()))?;
            let (req, _) = self.mirror.prepare_post(&client, url);
            return Ok(req.timeout(timeout));
        }
        let client = if https_only {
            &self.https_only_http_client
        } else {
            &self.standard_http_client
        };
        let (req, _) = self.mirror.prepare_post(client, url);
        Ok(req.timeout(timeout))
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

    pub(crate) fn token_vault(&self) -> TokenVault {
        self.token_vault.read().unwrap().clone()
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
                let params = saved.filter.local_params.clone();
                handle.spawn(async move {
                    if let Err(err) = lf.reconfigure_params(&params).await {
                        tracing::warn!(error = %err, "热更新本地过滤运行参数失败");
                    }
                });
            }
        }
        Ok(saved)
    }

    pub async fn save_config_async(&self, next: AppConfig) -> Result<AppConfig, CoreError> {
        let (saved, filter_params_changed) = self.apply_saved_config(next)?;
        if filter_params_changed {
            if let Err(err) = self
                .local_filter
                .reconfigure_params(&saved.filter.local_params)
                .await
            {
                tracing::warn!(error = %err, "热更新本地过滤运行参数失败");
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

    fn snapshot_locked(&self) -> Result<DashboardSnapshot, CoreError> {
        let config = self.config();
        let local_sha = if config.filter.mode == ClientFilterSourceMode::Local {
            self.local_filter.active_commit_sha()
        } else {
            None
        };
        let policy_sig =
            crate::filter::evaluator::compute_policy_signature(&config, local_sha.as_deref());
        let cached_filter_verdicts = self.db.list_filter_verdicts_for_policy(&policy_sig)?;

        Ok(DashboardSnapshot {
            config,
            addons: self.db.list_addons()?,
            bundles: self.db.list_bundles()?,
            collections: self.db.list_tracked_collections()?,
            user_groups: self.db.list_user_groups()?,
            profiles: self.db.list_profiles()?,
            open_issues: self.db.list_open_reconcile_issues()?,
            tasks: self.db.list_tasks()?,
            filter_exclusions: self.db.list_filter_exclusions()?,
            cached_filter_verdicts,
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
