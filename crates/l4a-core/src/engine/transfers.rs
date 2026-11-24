use super::workshop::fetch_workshop_item;
use super::L4aEngine;
use crate::db::LocalDb;
use crate::error::CoreError;
use crate::file_changes::{occupied, take_matching_dummy, FileChanges};
use crate::lifecycle::source_provides_dependencies;
use crate::ops::{addon_path, append_os_suffix, build_target_vpk_name, take_addon_file};
use crate::scanner::scan_and_commit;
use crate::tasks::{create_task, update_task};
use l4a_protocol::{
    AddonDirType, AddonId, AddonSummary, AppConfig, CoreEvent, InstallState, TaskKind, TaskStatus,
    WorkshopId, WorkshopItemRecord,
};
use l4a_vpk::{extract_addon_metadata, inspect_dummy_vpk, DummyInspection, DummySpec};
use l4a_workshop::{
    DownloadProgress, DownloadRequest, DownloadResult, ProgressSink, WorkshopError,
};
use std::collections::HashMap;
use std::fs;
use std::future::Future;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

impl L4aEngine {
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

    pub(super) async fn transfer_workshop_item_with<F>(
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
