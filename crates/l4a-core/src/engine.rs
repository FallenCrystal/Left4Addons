use crate::config::{load_config, save_config, AppPaths};
use crate::db::LocalDb;
use crate::error::CoreError;
use crate::grouping::{
    apply_profile, create_manual_bundle, delete_profile, delete_user_group, rebuild_auto_bundles,
    save_profile, save_user_group, set_bundle_locked, track_workshop_collection,
    untrack_workshop_collection,
};
use crate::lifecycle::sync_lifecycle;
use crate::ops::{
    append_os_suffix, apply_renames, build_target_vpk_name, delete_addons, move_addons,
    preview_batch_rename, search_by_map_code, toggle_addons, write_dummy_atomically,
};
use crate::reconcile::{auto_resolve_by_policy, detect_reconcile_issues, resolve_reconcile_issue};
use crate::scanner::scan_workspace;
use crate::tasks::{create_task, update_task};
use crate::watcher::AddonWatcher;
use l4a_mirror::MirrorRouter;
use l4a_protocol::{
    AddonDirType, AddonId, AddonSummary, AppConfig, BundleId, BundleRecord, CoreEvent,
    DashboardSnapshot, GroupId, InstallState, IssueId, LoadoutProfile, ProfileApplyReport,
    ProfileId, ReconcileAction, RenamePreviewItem, RenameRuleConfig, SearchQuery, SearchResponse,
    SyncReport, TaskKind, TaskStatus, TrackedCollectionRecord, UserGroup, WorkshopFetchOutcome,
    WorkshopId,
};
use l4a_steam::{DynamicSteamBridge, SteamBridge, SteamCapabilities};
use l4a_vpk::{extract_addon_metadata, DummySpec};
use l4a_workshop::{
    CatalogBackendProvider, DownloadProgress, DownloadRequest, ResilientDownloader,
    SourceRoutingPolicy, SteamBridgeProvider, SteamWebApiProvider, WebScraperPlaceholderProvider,
    WorkshopProvider, WorkshopResolver,
};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use tokio_util::sync::CancellationToken;

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
    catalog_reporter: RwLock<Option<Arc<CatalogBackendProvider>>>,
    downloader: ResilientDownloader,
    watcher: Mutex<AddonWatcher>,
    watcher_rescan_cb: Mutex<Option<RescanCallback>>,
    events: Arc<dyn EventSink>,
    active_cancellations: Mutex<HashMap<WorkshopId, CancellationToken>>,
}

impl L4aEngine {
    pub fn open_default(runtime_dir: impl AsRef<Path>, events: Arc<dyn EventSink>) -> Result<Self, CoreError> {
        let paths = AppPaths::from_runtime_dir(runtime_dir);
        paths.ensure_dirs()?;

        let mut config = load_config(&paths.settings_path)?;
        if config.loading_dir.is_empty() {
            if let Some(detected) = l4a_steam::detect_l4d2_addons_path() {
                let loading = Path::new(&detected);
                config.workshop_dir = loading.join("workshop").to_string_lossy().into_owned();
                config.loading_dir = detected;
            } else {
                let default_loading = paths.runtime_dir.join("addons-loading");
                config.workshop_dir = default_loading.join("workshop").to_string_lossy().into_owned();
                config.loading_dir = default_loading.to_string_lossy().into_owned();
            }
            save_config(&paths.settings_path, &config)?;
        }

        let mirror = Arc::new(
            MirrorRouter::from_file(&paths.mirrors_path)
                .unwrap_or_else(|_| MirrorRouter::empty()),
        );
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
        let (resolver, reporter) = build_resolver_from_config(&config, mirror.clone(), steam_bridge.clone());

        Ok(Self {
            downloader: ResilientDownloader::new(&paths.download_cache_dir, mirror.clone()),
            paths,
            db,
            config: RwLock::new(config),
            mirror,
            steam_bridge,
            dynamic_bridge: Some(dynamic_bridge),
            resolver: RwLock::new(resolver),
            catalog_reporter: RwLock::new(reporter),
            watcher: Mutex::new(AddonWatcher::default()),
            watcher_rescan_cb: Mutex::new(None),
            events,
            active_cancellations: Mutex::new(HashMap::new()),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_dependencies(
        paths: AppPaths,
        db: Arc<LocalDb>,
        config: AppConfig,
        mirror: Arc<MirrorRouter>,
        steam_bridge: Arc<dyn SteamBridge>,
        resolver: Arc<WorkshopResolver>,
        catalog_reporter: Option<Arc<CatalogBackendProvider>>,
        events: Arc<dyn EventSink>,
    ) -> Result<Self, CoreError> {
        paths.ensure_dirs()?;
        Ok(Self {
            downloader: ResilientDownloader::new(&paths.download_cache_dir, mirror.clone()),
            paths,
            db,
            config: RwLock::new(config),
            mirror,
            steam_bridge,
            dynamic_bridge: None,
            resolver: RwLock::new(resolver),
            catalog_reporter: RwLock::new(catalog_reporter),
            watcher: Mutex::new(AddonWatcher::default()),
            watcher_rescan_cb: Mutex::new(None),
            events,
            active_cancellations: Mutex::new(HashMap::new()),
        })
    }

    pub fn db(&self) -> &Arc<LocalDb> {
        &self.db
    }

    pub fn config(&self) -> AppConfig {
        self.config.read().unwrap().clone()
    }

    pub fn save_config(&self, mut next: AppConfig) -> Result<AppConfig, CoreError> {
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
            let changed =
                guard.loading_dir != next.loading_dir || guard.workshop_dir != next.workshop_dir;
            *guard = next.clone();
            changed
        };
        if let Some(ref dyn_bridge) = self.dynamic_bridge {
            dyn_bridge.set_enabled(!next.disable_steamworks_sdk);
        }
        let (new_resolver, new_reporter) =
            build_resolver_from_config(&next, self.mirror.clone(), self.steam_bridge.clone());
        *self.resolver.write().unwrap() = new_resolver;
        *self.catalog_reporter.write().unwrap() = new_reporter;

        if dirs_changed {
            let cb_opt = self.watcher_rescan_cb.lock().unwrap().clone();
            if let Some(cb) = cb_opt {
                if let Err(err) = self.start_watcher(cb) {
                    self.events.emit(CoreEvent::WatchError {
                        message: format!("重新绑定目录监听失败: {err}"),
                    });
                }
            }
        }
        Ok(next)
    }

    pub fn scan(&self) -> Result<DashboardSnapshot, CoreError> {
        self.scan_internal(false)
    }

    pub fn scan_external(&self) -> Result<DashboardSnapshot, CoreError> {
        self.scan_internal(true)
    }

    fn scan_internal(&self, external_change: bool) -> Result<DashboardSnapshot, CoreError> {
        let cfg = self.config();
        scan_workspace(&self.db, &cfg, &self.paths.image_cache_dir)?;
        let issues = detect_reconcile_issues(&self.db, &cfg)?;
        let resolved = auto_resolve_by_policy(&self.db, &cfg, &self.paths.image_cache_dir, &issues)?;
        if resolved > 0 {
            scan_workspace(&self.db, &cfg, &self.paths.image_cache_dir)?;
            detect_reconcile_issues(&self.db, &cfg)?;
        }
        let open_issues = self.db.list_open_reconcile_issues()?;
        if !open_issues.is_empty() {
            self.events.emit(CoreEvent::ReconcileIssuesDetected {
                count: open_issues.len(),
            });
        }
        self.events.emit(CoreEvent::SnapshotUpdated { external_change });
        self.snapshot()
    }

    pub fn snapshot(&self) -> Result<DashboardSnapshot, CoreError> {
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

    pub fn resolve_issue(&self, issue_id: &IssueId, action: ReconcileAction) -> Result<DashboardSnapshot, CoreError> {
        let cfg = self.config();
        let target_wid = self
            .db
            .list_open_reconcile_issues()?
            .into_iter()
            .find(|i| &i.id == issue_id)
            .and_then(|i| i.workshop_id);
        self.suppress_watcher();
        resolve_reconcile_issue(&self.db, &cfg, &self.paths.image_cache_dir, issue_id, action)?;
        scan_workspace(&self.db, &cfg, &self.paths.image_cache_dir)?;
        detect_reconcile_issues(&self.db, &cfg)?;
        if action == ReconcileAction::KeepWorkshopDeleteLoading {
            if let Some(wid) = target_wid {
                let _ = self.db.mark_issue_resolved(&IssueId(format!("new_ws_{wid}")));
            }
        }
        self.events.emit(CoreEvent::SnapshotUpdated {
            external_change: false,
        });
        self.snapshot()
    }

    pub async fn sync_workshop(&self) -> Result<SyncReport, CoreError> {
        let cfg = self.config();
        let resolver = self.resolver.read().unwrap().clone();
        let reporter = self.catalog_reporter.read().unwrap().clone();

        let mut task = create_task(&self.db, TaskKind::Sync, "workshop-sync")?;
        update_task(&self.db, &mut task, TaskStatus::Running, 10, None, None)?;

        match sync_lifecycle(&self.db, &cfg, &resolver, reporter).await {
            Ok(report) => {
                let _ = rebuild_auto_bundles(&self.db);
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

    pub async fn download_workshop_item(&self, workshop_id: WorkshopId) -> Result<AddonSummary, CoreError> {
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
                return Err(CoreError::Conflict(format!("条目 {workshop_id} 正在下载中")));
            }
            guard.insert(workshop_id, cancel.clone());
        }
        let _active_guard = ActiveDownloadGuard {
            map: &self.active_cancellations,
            wid: workshop_id,
        };

        let cfg = self.config();
        if cfg.loading_dir.trim().is_empty() {
            return Err(CoreError::InvalidInput("未配置 loading_dir".into()));
        }

        let resolver = self.resolver.read().unwrap().clone();
        let (outcomes, _) = resolver.fetch_items(&[workshop_id]).await?;
        let item = outcomes
            .into_iter()
            .find_map(|o| match o {
                WorkshopFetchOutcome::Found { item } => Some(item),
                _ => None,
            })
            .ok_or_else(|| CoreError::NotFound(format!("工坊条目不可用或已下架: {workshop_id}")))?;

        let file_url = item
            .file_url
            .clone()
            .ok_or_else(|| CoreError::InvalidInput(format!("条目 {workshop_id} 无可用下载直链")))?;

        let dest_name = build_target_vpk_name(Some(workshop_id), None, &item.title, &cfg.rename_rules);
        let dest_path = Path::new(&cfg.loading_dir).join(&dest_name);
        let disabled_dest = append_os_suffix(&dest_path, ".disabled");
        if dest_path.exists() || disabled_dest.exists() {
            return Err(CoreError::Conflict(format!("文件已存在: {}", dest_path.display())));
        }

        let mut task = create_task(&self.db, TaskKind::Download, workshop_id.to_string())?;
        update_task(&self.db, &mut task, TaskStatus::Running, 0, Some(item.title.clone()), None)?;

        let events = self.events.clone();
        let task_id = task.id.clone();
        let sink = Arc::new(move |p: DownloadProgress| {
            events.emit(CoreEvent::DownloadProgress {
                task_id: task_id.clone(),
                workshop_id: p.workshop_id,
                downloaded: p.downloaded,
                total: p.total,
                percent: p.percent,
            });
        });

        self.suppress_watcher();
        let dl_res = self
            .downloader
            .download(
                &DownloadRequest {
                    workshop_id,
                    url: file_url,
                    dest_path: dest_path.clone(),
                    max_retries: cfg.max_download_retries,
                },
                cancel,
                Some(sink),
            )
            .await;

        match dl_res {
            Ok(_) => {
                self.suppress_watcher();
                db_save_downloaded_item(&self.db, &cfg, &self.paths.image_cache_dir, &item, &dest_path)?;
                scan_workspace(&self.db, &cfg, &self.paths.image_cache_dir)?;
                detect_reconcile_issues(&self.db, &cfg)?;
                update_task(&self.db, &mut task, TaskStatus::Completed, 100, Some("下载完成".into()), None)?;
                self.events.emit(CoreEvent::SnapshotUpdated {
                    external_change: false,
                });
                self.db
                    .get_addon_by_workshop_id(workshop_id)?
                    .ok_or_else(|| CoreError::NotFound("下载入库后未找到记录".into()))
            }
            Err(err) => {
                let status = if matches!(err, l4a_workshop::WorkshopError::Cancelled) {
                    TaskStatus::Cancelled
                } else {
                    TaskStatus::Failed
                };
                update_task(&self.db, &mut task, status, 0, None, Some(err.to_string()))?;
                Err(CoreError::Workshop(err))
            }
        }
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
    ) -> Result<DashboardSnapshot, CoreError> {
        let cfg = self.config();
        let moved_wids: Vec<WorkshopId> = if target_dir_type == AddonDirType::Workshop {
            addon_ids
                .iter()
                .filter_map(|aid| {
                    self.db
                        .get_addon(aid)
                        .ok()
                        .flatten()
                        .and_then(|a| a.workshop_id)
                })
                .collect()
        } else {
            Vec::new()
        };
        self.suppress_watcher();
        move_addons(&self.db, &cfg, &self.paths.image_cache_dir, addon_ids, target_dir_type)?;
        scan_workspace(&self.db, &cfg, &self.paths.image_cache_dir)?;
        detect_reconcile_issues(&self.db, &cfg)?;
        if target_dir_type == AddonDirType::Workshop {
            for wid in moved_wids {
                let _ = self.db.mark_issue_resolved(&IssueId(format!("new_ws_{wid}")));
            }
        }
        self.events.emit(CoreEvent::SnapshotUpdated {
            external_change: false,
        });
        self.snapshot()
    }

    pub fn toggle_addons(
        &self,
        addon_ids: &[AddonId],
        enabled: bool,
        allow_workshop_dir: bool,
    ) -> Result<DashboardSnapshot, CoreError> {
        let cfg = self.config();
        self.suppress_watcher();
        toggle_addons(
            &self.db,
            &cfg,
            &self.paths.image_cache_dir,
            addon_ids,
            enabled,
            allow_workshop_dir,
        )?;
        scan_workspace(&self.db, &cfg, &self.paths.image_cache_dir)?;
        self.events.emit(CoreEvent::SnapshotUpdated {
            external_change: false,
        });
        self.snapshot()
    }

    pub fn preview_rename(
        &self,
        rules: &RenameRuleConfig,
        addon_ids: &[AddonId],
    ) -> Result<Vec<RenamePreviewItem>, CoreError> {
        preview_batch_rename(&self.db, rules, addon_ids)
    }

    pub fn apply_renames(&self, renames: &[(AddonId, String)]) -> Result<DashboardSnapshot, CoreError> {
        let cfg = self.config();
        self.suppress_watcher();
        apply_renames(&self.db, renames, &cfg.rename_rules)?;
        scan_workspace(&self.db, &cfg, &self.paths.image_cache_dir)?;
        self.events.emit(CoreEvent::SnapshotUpdated {
            external_change: false,
        });
        self.snapshot()
    }

    pub fn delete_addons(
        &self,
        addon_ids: &[AddonId],
        delete_disk_file: bool,
        purge_known_record: bool,
    ) -> Result<DashboardSnapshot, CoreError> {
        let cfg = self.config();
        self.suppress_watcher();
        delete_addons(&self.db, &cfg, addon_ids, delete_disk_file, purge_known_record)?;
        scan_workspace(&self.db, &cfg, &self.paths.image_cache_dir)?;
        detect_reconcile_issues(&self.db, &cfg)?;
        self.events.emit(CoreEvent::SnapshotUpdated {
            external_change: false,
        });
        self.snapshot()
    }

    pub fn rebuild_bundles(&self) -> Result<Vec<BundleRecord>, CoreError> {
        let res = rebuild_auto_bundles(&self.db)?;
        self.events.emit(CoreEvent::SnapshotUpdated {
            external_change: false,
        });
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
        let cfg = self.config();
        self.suppress_watcher();
        let report = apply_profile(&self.db, id, &self.paths.image_cache_dir, &cfg)?;
        scan_workspace(&self.db, &cfg, &self.paths.image_cache_dir)?;
        self.events.emit(CoreEvent::SnapshotUpdated {
            external_change: false,
        });
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

    pub fn start_watcher(&self, on_external_rescan: Arc<dyn Fn() + Send + Sync>) -> Result<(), CoreError> {
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

    fn suppress_watcher(&self) {
        if let Ok(guard) = self.watcher.lock() {
            guard.suppress_internal();
        }
    }
}

fn db_save_downloaded_item(
    db: &LocalDb,
    config: &AppConfig,
    image_cache_dir: &Path,
    item: &l4a_protocol::WorkshopItemRecord,
    dest_path: &Path,
) -> Result<(), CoreError> {
    let wid = item.workshop_id;
    let meta = extract_addon_metadata(dest_path, image_cache_dir);
    if let Some(err) = meta.error {
        return Err(CoreError::Vpk(err));
    }
    db.upsert_workshop_snapshot(item, !item.dependencies.is_empty())?;
    let size = fs::metadata(dest_path)?.len();
    let aid = AddonId::from_workshop_id(wid);

    if config.enable_dummy_bypass {
        if config.workshop_dir.trim().is_empty() {
            return Err(CoreError::InvalidInput("已启用 Dummy Bypass，但未配置 workshop_dir".into()));
        }
        let dummy_path = Path::new(&config.workshop_dir).join(format!("{wid}.vpk"));
        write_dummy_atomically(
            dest_path,
            &dummy_path,
            &DummySpec {
                workshop_id: wid,
                original_hash: meta.content_hash.clone(),
                original_size: size,
                original_time_updated: item.time_updated,
            },
        )?;
    }

    let summary = AddonSummary {
        id: aid.clone(),
        workshop_id: Some(wid),
        title: if item.title.trim().is_empty() {
            dest_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Workshop Addon")
                .to_string()
        } else {
            item.title.clone()
        },
        author: item.creator_name.clone(),
        version: None,
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
        cover_image: meta.image_path.or_else(|| item.preview_url.clone()),
        categories: crate::db::repo::derive_categories_from_info(&meta.addon_info),
        maps: meta.maps,
        has_dummy_bypass: config.enable_dummy_bypass,
        remote_status: item.status,
        local_time_updated: item.time_updated,
        remote_time_updated: item.time_updated,
        update_available: false,
        addon_info: meta.addon_info,
    };
    db.upsert_addon(&summary)
}

fn build_resolver_from_config(
    config: &AppConfig,
    mirror: Arc<MirrorRouter>,
    steam_bridge: Arc<dyn SteamBridge>,
) -> (Arc<WorkshopResolver>, Option<Arc<CatalogBackendProvider>>) {
    let mut providers: Vec<Arc<dyn WorkshopProvider>> = Vec::new();
    let mut reporter = None;

    let catalog = config
        .catalog_server_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|url| Arc::new(CatalogBackendProvider::new(url, mirror.clone())));

    if let Some(ref c) = catalog {
        reporter = Some(c.clone());
    }

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

    (
        Arc::new(WorkshopResolver::new(providers, SourceRoutingPolicy::default())),
        reporter,
    )
}
