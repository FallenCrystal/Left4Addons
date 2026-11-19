mod commands;

use l4a_core::{EventSink, L4aEngine};
use l4a_protocol::CoreEvent;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

pub struct TauriState {
    pub engine: Arc<L4aEngine>,
}

struct TauriEventSink {
    app_handle: AppHandle,
}

impl EventSink for TauriEventSink {
    fn emit(&self, event: CoreEvent) {
        let _ = self.app_handle.emit("l4a-core-event", event);
    }
}

fn dir_is_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".l4a_write_probe_{}", std::process::id()));
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn resolve_writable_runtime_dir(app: &tauri::App, preferred: PathBuf) -> PathBuf {
    if dir_is_writable(&preferred) {
        return preferred;
    }
    if let Ok(app_data) = app.path().app_data_dir() {
        let fallback = app_data.join("l4a");
        if dir_is_writable(&fallback) {
            return fallback;
        }
    }
    preferred
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let mut host_runtime_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(PathBuf::from))
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

            if cfg!(debug_assertions) && host_runtime_dir.iter().any(|c| c == "target") {
                host_runtime_dir = std::env::current_dir().unwrap_or(host_runtime_dir);
                if host_runtime_dir.ends_with("src-tauri") {
                    if let Some(parent) = host_runtime_dir.parent() {
                        host_runtime_dir = parent.to_path_buf();
                    }
                }
            }

            let preferred_dir = if host_runtime_dir.ends_with("l4a") {
                host_runtime_dir
            } else {
                host_runtime_dir.join("l4a")
            };
            let runtime_dir = resolve_writable_runtime_dir(app, preferred_dir);

            let sink = Arc::new(TauriEventSink {
                app_handle: app.handle().clone(),
            });
            let engine = Arc::new(
                L4aEngine::open_default(&runtime_dir, sink.clone())
                    .map_err(|e| format!("初始化 L4A v2 引擎失败: {e}"))?,
            );

            let watcher_engine = engine.clone();
            if let Err(err) = engine.start_watcher(Arc::new(move || {
                let _ = watcher_engine.scan_external();
            })) {
                let message = format!("启动模组目录监听失败: {err}");
                eprintln!("{message}");
                sink.emit(CoreEvent::WatchError { message });
            }

            app.manage(TauriState { engine });

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(tauri_plugin_log::log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_dashboard_snapshot,
            commands::scan_workspace,
            commands::save_config,
            commands::detect_game_addons_path,
            commands::get_steam_capabilities,
            commands::sync_workshop,
            commands::search_workshop,
            commands::download_workshop_item,
            commands::cancel_download,
            commands::resolve_reconcile_issue,
            commands::move_addons,
            commands::toggle_addons,
            commands::preview_rename,
            commands::apply_renames,
            commands::delete_addons,
            commands::rebuild_bundles,
            commands::create_manual_bundle,
            commands::set_bundle_locked,
            commands::track_collection,
            commands::untrack_collection,
            commands::save_user_group,
            commands::delete_user_group,
            commands::save_profile,
            commands::delete_profile,
            commands::apply_profile,
            commands::search_by_map_code,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            let engine = app_handle.state::<TauriState>().engine.clone();
            tauri::async_runtime::block_on(engine.shutdown());
        }
    });
}
