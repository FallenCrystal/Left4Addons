use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use tauri::async_runtime::Mutex;
use tauri::Manager;

pub mod commands;
pub mod steam;
pub mod vpk;
pub mod watcher;
pub mod mirrors;

pub struct AppState {
    pub settings_path: PathBuf,
    pub groups_path: PathBuf,
    pub known_addons_path: PathBuf,
    pub workshop_cache_path: PathBuf,
    pub workshop_crawl_log_path: PathBuf,
    pub background_tasks_path: PathBuf,
    pub cache_dir: PathBuf,
    pub download_cache_dir: PathBuf,
    pub workshop_service: steam::WorkshopService,
    pub db: Mutex<commands::Database>,
    pub workshop_cache_write_lock: StdMutex<()>,
    pub addon_watcher: StdMutex<watcher::AddonWatcherController>,
    pub cancelled_downloads: StdMutex<HashSet<String>>,
    pub runtime_dir: PathBuf,
}

impl Drop for AppState {
    fn drop(&mut self) {
        self.workshop_service.shutdown();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let mut host_runtime_dir = std::env::current_exe()
                .ok()
                .and_then(|p| {
                    p.parent().map(|d| {
                        let is_target = d.iter().any(|c| c == "target");
                        if cfg!(debug_assertions) && is_target {
                            std::env::current_dir().unwrap_or_else(|_| d.to_path_buf())
                        } else {
                            d.to_path_buf()
                        }
                    })
                })
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

            if cfg!(debug_assertions) && host_runtime_dir.ends_with("src-tauri") {
                if let Some(parent) = host_runtime_dir.parent() {
                    host_runtime_dir = parent.to_path_buf();
                }
            }

            let runtime_dir = if host_runtime_dir.ends_with("l4a") {
                host_runtime_dir.clone()
            } else {
                host_runtime_dir.join("l4a")
            };
            crate::mirrors::RUNTIME_DIR.set(runtime_dir.clone()).ok();
            let config_dir = runtime_dir.join("config");
            let cache_root_dir = runtime_dir.join("cache");
            let cache_dir = cache_root_dir.join("images");
            let download_cache_dir = cache_root_dir.join("downloading");

            let _ = std::fs::create_dir_all(&config_dir);
            let _ = std::fs::create_dir_all(&cache_root_dir);
            let _ = std::fs::create_dir_all(&cache_dir);
            let _ = std::fs::create_dir_all(&download_cache_dir);

            migrate_data(
                app.path().app_data_dir().ok().as_deref(),
                &host_runtime_dir,
                &runtime_dir,
            );

            let settings_path = config_dir.join("settings.json");
            let groups_path = config_dir.join("groups.json");
            let known_addons_path = config_dir.join("known_addons.json");
            let workshop_cache_path = cache_root_dir.join("workshop_cache.json");
            let workshop_crawl_log_path = cache_root_dir.join("workshop_crawl_log.jsonl");
            let background_tasks_path = cache_root_dir.join("background_tasks.json");
            let bridge_base_dir = std::env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| host_runtime_dir.clone());
            let db = commands::load_db(
                &settings_path,
                &groups_path,
                &known_addons_path,
                &runtime_dir,
                app.handle(),
            );
            let initial_loading_dir = db.settings.loading_dir.clone();

            app.manage(AppState {
                settings_path,
                groups_path,
                known_addons_path,
                workshop_cache_path,
                workshop_crawl_log_path,
                background_tasks_path,
                cache_dir,
                download_cache_dir,
                workshop_service: steam::WorkshopService::new(&bridge_base_dir),
                db: Mutex::new(db),
                workshop_cache_write_lock: StdMutex::new(()),
                addon_watcher: StdMutex::new(watcher::AddonWatcherController::default()),
                cancelled_downloads: StdMutex::new(HashSet::new()),
                runtime_dir: runtime_dir.clone(),
            });

            if let Err(err) =
                watcher::rebind_addon_watcher(app.handle(), Path::new(&initial_loading_dir))
            {
                watcher::emit_watch_error(app.handle(), &err);
            }

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::handlers::settings::get_settings,
            commands::handlers::settings::save_settings,
            commands::handlers::db::get_addons,
            commands::handlers::cache::get_cache_image,
            commands::handlers::cache::cache_remote_image,
            commands::handlers::addons::move_addons,
            commands::handlers::addons::toggle_addons,
            commands::handlers::addons::rename_addon,
            commands::handlers::addons::rename_addons,
            commands::handlers::addons::group_action,
            commands::handlers::workshop::open_workshop,
            commands::handlers::workshop::open_url,
            commands::handlers::workshop::steam_sync,
            commands::handlers::workshop::get_workshop_capabilities,
            commands::handlers::workshop::query_workshop_home,
            commands::handlers::workshop::query_workshop_items,
            commands::handlers::workshop::query_workshop_item,
            commands::handlers::workshop::query_workshop_details,
            commands::handlers::workshop::query_workshop_collection,
            commands::handlers::workshop::fetch_workshop_html,
            commands::handlers::addons::delete_addons,
            commands::handlers::addons::download_addon,
            commands::handlers::workshop::fetch_collection,
            commands::handlers::workshop::add_known_addon,
            commands::handlers::workshop::get_workshop_cache,
            commands::handlers::workshop::record_workshop_items_seen,
            commands::handlers::workshop::persist_workshop_page_details,
            commands::handlers::tasks::get_background_tasks,
            commands::handlers::tasks::save_background_task_snapshot,
            commands::handlers::tasks::cancel_download,
            commands::handlers::tasks::append_workshop_crawl_log,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if matches!(
            event,
            tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
        ) {
            app_handle.state::<AppState>().workshop_service.shutdown();
        }
    });
}

fn migrate_data(app_data_dir: Option<&Path>, legacy_runtime_dir: &Path, runtime_dir: &Path) {
    if let Some(app_data_dir) = app_data_dir {
        if app_data_dir != runtime_dir {
            migrate_from_root(app_data_dir, runtime_dir);
        }
    }

    if legacy_runtime_dir != runtime_dir {
        migrate_from_root(legacy_runtime_dir, runtime_dir);
    }
}

fn migrate_from_root(src_root: &Path, runtime_dir: &Path) {
    if src_root == runtime_dir {
        return;
    }

    let target_config_dir = runtime_dir.join("config");
    let target_cache_dir = runtime_dir.join("cache");
    let target_image_cache_dir = target_cache_dir.join("images");
    let target_loading_dir = runtime_dir.join("addons-loading");

    let _ = std::fs::create_dir_all(&target_config_dir);
    let _ = std::fs::create_dir_all(&target_cache_dir);

    migrate_legacy_db_file(
        &src_root.join("db.json"),
        &target_config_dir.join("settings.json"),
        &target_config_dir.join("groups.json"),
        &target_config_dir.join("known_addons.json"),
    );
    move_file_if_missing(
        &src_root.join("settings.json"),
        &target_config_dir.join("settings.json"),
    );
    move_file_if_missing(
        &src_root.join("groups.json"),
        &target_config_dir.join("groups.json"),
    );
    move_file_if_missing(
        &src_root.join("known_addons.json"),
        &target_config_dir.join("known_addons.json"),
    );
    move_file_if_missing(
        &src_root.join("workshop_cache.json"),
        &target_cache_dir.join("workshop_cache.json"),
    );
    move_file_if_missing(
        &src_root.join("background_tasks.json"),
        &target_cache_dir.join("background_tasks.json"),
    );
    move_file_if_missing(
        &src_root.join("workshop_crawl_log.jsonl"),
        &target_cache_dir.join("workshop_crawl_log.jsonl"),
    );

    let old_cache = src_root.join("cache");
    if old_cache.exists() {
        let _ = move_or_copy_dir(&old_cache, &target_image_cache_dir);
    }

    let old_loading = src_root.join("addons-loading");
    if old_loading.exists() {
        let _ = move_or_copy_dir(&old_loading, &target_loading_dir);
    }
}

fn move_file_if_missing(src: &Path, dst: &Path) {
    if !src.exists() || dst.exists() {
        return;
    }

    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if std::fs::rename(src, dst).is_ok() {
        return;
    }

    if std::fs::copy(src, dst).is_ok() {
        let _ = std::fs::remove_file(src);
    }
}

fn migrate_legacy_db_file(
    legacy_db_path: &Path,
    settings_path: &Path,
    groups_path: &Path,
    known_addons_path: &Path,
) {
    if !legacy_db_path.exists() {
        return;
    }

    let Ok(content) = std::fs::read_to_string(legacy_db_path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };

    if !settings_path.exists() {
        let settings = commands::SettingsStore {
            settings: serde_json::from_value(value.get("settings").cloned().unwrap_or_default())
                .unwrap_or_default(),
            master_collections: serde_json::from_value(
                value.get("masterCollections").cloned().unwrap_or_default(),
            )
            .unwrap_or_default(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&settings) {
            let _ = std::fs::write(settings_path, json);
        }
    }

    if !groups_path.exists() {
        let groups: Vec<commands::Group> =
            serde_json::from_value(value.get("groups").cloned().unwrap_or_default())
                .unwrap_or_default();
        if let Ok(json) = serde_json::to_string_pretty(&groups) {
            let _ = std::fs::write(groups_path, json);
        }
    }

    if !known_addons_path.exists() {
        let mut known_addons = std::collections::HashMap::new();
        if let Some(addons) = value.get("addons").and_then(|v| v.as_object()) {
            for item in addons.values() {
                if let Ok(mut addon) = serde_json::from_value::<commands::Addon>(item.clone()) {
                    if addon.id.is_empty() {
                        addon.id = addon
                            .workshop_id
                            .clone()
                            .unwrap_or_else(|| addon.vpk_name.clone());
                    }
                    if addon.is_dummy || commands::is_dummy_addon_info(&addon.addon_info) {
                        continue;
                    }
                    let id = addon.id.clone();
                    known_addons.insert(
                        id.clone(),
                        commands::KnownAddonEntry {
                            id,
                            vpk_name: addon.vpk_name.clone(),
                            workshop_id: addon.workshop_id.clone(),
                            filename_workshop_id_candidate: addon.filename_workshop_id_candidate,
                            filename_workshop_id_validation_status: addon.filename_workshop_id_validation_status,
                            filename_workshop_id_last_attempt_at: addon.filename_workshop_id_last_attempt_at,
                            addon_info: addon.addon_info,
                            has_image: addon.has_image,
                            image_path: addon.image_path,
                            steam_details: addon.steam_details,
                        },
                    );
                }
            }
        }
        if let Ok(json) = serde_json::to_string_pretty(&known_addons) {
            let _ = std::fs::write(known_addons_path, json);
        }
    }

    if settings_path.exists() && groups_path.exists() && known_addons_path.exists() {
        let _ = std::fs::remove_file(legacy_db_path);
    }
}

fn move_or_copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !src.exists() {
        return Ok(());
    }

    if !dst.exists() && std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }

    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            move_or_copy_dir(&src_path, &dst_path)?;
        } else if !dst_path.exists() {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    let _ = std::fs::remove_dir_all(src);
    Ok(())
}
