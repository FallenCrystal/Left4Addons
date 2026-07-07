use std::path::PathBuf;
use tauri::Manager;
use tauri::async_runtime::Mutex;

pub mod vpk;
pub mod commands;

pub struct AppState {
    pub db_path: PathBuf,
    pub known_addons_path: PathBuf,
    pub cache_dir: PathBuf,
    pub db: Mutex<commands::Database>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let mut runtime_dir = std::env::current_exe()
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

            if cfg!(debug_assertions) && runtime_dir.ends_with("src-tauri") {
                if let Some(parent) = runtime_dir.parent() {
                    runtime_dir = parent.to_path_buf();
                }
            }


            if !runtime_dir.exists() {
                let _ = std::fs::create_dir_all(&runtime_dir);
            }

            if let Some(app_data_dir) = app.path().app_data_dir().ok() {
                migrate_data(&app_data_dir, &runtime_dir);
            }

            let db_path = runtime_dir.join("db.json");
            let known_addons_path = runtime_dir.join("known_addons.json");
            let cache_dir = runtime_dir.join("cache");
            if !cache_dir.exists() {
                let _ = std::fs::create_dir_all(&cache_dir);
            }

            let db = commands::load_db(&db_path, &known_addons_path, app.handle());
            
            app.manage(AppState {
                db_path,
                known_addons_path,
                cache_dir,
                db: Mutex::new(db),
            });

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
            commands::get_settings,
            commands::save_settings,
            commands::get_addons,
            commands::get_cache_image,
            commands::move_addons,
            commands::toggle_addons,
            commands::rename_addon,
            commands::rename_addons,
            commands::group_action,
            commands::open_workshop,
            commands::open_url,
            commands::steam_sync,
            commands::fetch_workshop_html,
            commands::delete_addons,
            commands::download_addon,
            commands::fetch_collection,
            commands::add_known_addon,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}



fn migrate_data(app_data_dir: &std::path::Path, runtime_dir: &std::path::Path) {
    if app_data_dir == runtime_dir {
        return;
    }

    let old_db = app_data_dir.join("db.json");
    let new_db = runtime_dir.join("db.json");

    if !new_db.exists() && old_db.exists() {
        if std::fs::copy(&old_db, &new_db).is_ok() {
            let _ = std::fs::remove_file(&old_db);
        }
    }

    let old_cache = app_data_dir.join("cache");
    let new_cache = runtime_dir.join("cache");
    if !new_cache.exists() && old_cache.exists() {
        let _ = move_or_copy_dir(&old_cache, &new_cache);
    }

    let old_loading = app_data_dir.join("addons-loading");
    let new_loading = runtime_dir.join("addons-loading");
    if !new_loading.exists() && old_loading.exists() {
        let _ = move_or_copy_dir(&old_loading, &new_loading);
    }
}

fn move_or_copy_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    if !src.exists() {
        return Ok(());
    }
    if dst.exists() {
        return Ok(());
    }
    if std::fs::rename(src, dst).is_ok() {
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
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    let _ = std::fs::remove_dir_all(src);
    Ok(())
}
