use std::path::PathBuf;
use tauri::Manager;
use tauri::async_runtime::Mutex;

pub mod vpk;
pub mod commands;

pub struct AppState {
    pub db_path: PathBuf,
    pub cache_dir: PathBuf,
    pub db: Mutex<commands::Database>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let runtime_dir = std::env::current_exe()
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

            if !runtime_dir.exists() {
                let _ = std::fs::create_dir_all(&runtime_dir);
            }

            if let Some(app_data_dir) = app.path().app_data_dir().ok() {
                migrate_data(&app_data_dir, &runtime_dir);
            }

            let db_path = runtime_dir.join("db.json");
            let cache_dir = runtime_dir.join("cache");
            if !cache_dir.exists() {
                let _ = std::fs::create_dir_all(&cache_dir);
            }

            let db = commands::load_db(&db_path, app.handle());
            
            app.manage(AppState {
                db_path,
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
        .register_uri_scheme_protocol("cache", |ctx, request| {
            let uri = request.uri();
            let path = uri.path();
            let filename = path.trim_start_matches('/');
            let decoded_filename = url_decode(filename);
            
            let state = ctx.app_handle().state::<AppState>();

            let file_path = state.cache_dir.join(decoded_filename);
            
            let response = if file_path.exists() {
                if let Ok(bytes) = std::fs::read(&file_path) {
                    tauri::http::Response::builder()
                        .header("content-type", "image/jpeg")
                        .header("access-control-allow-origin", "*")
                        .body(bytes)
                } else {
                    tauri::http::Response::builder()
                        .status(500)
                        .body(Vec::new())
                }
            } else {
                tauri::http::Response::builder()
                    .status(404)
                    .body(Vec::new())
            };
            
            response.unwrap()

        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::get_addons,
            commands::move_addons,
            commands::toggle_addons,
            commands::rename_addon,
            commands::rename_addons,
            commands::group_action,
            commands::open_workshop,
            commands::open_url,
            commands::steam_sync,
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

fn url_decode(input: &str) -> String {
    let mut bytes = Vec::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let mut hex = String::new();
            if let Some(&h1) = chars.peek() {
                hex.push(h1);
                chars.next();
            }
            if let Some(&h2) = chars.peek() {
                hex.push(h2);
                chars.next();
            }
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    bytes.push(byte);
                    continue;
                }
            }
            bytes.push(b'%');
            bytes.extend_from_slice(hex.as_bytes());
        } else {
            let mut buf = [0; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}
