mod commands;

use l4a_core::{EventSink, L4aEngine};
use l4a_protocol::CoreEvent;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::http::{header, Method, Request, Response, StatusCode};
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

pub(crate) fn spawn_workshop_hydration(engine: &Arc<L4aEngine>) {
    let engine = engine.clone();
    tauri::async_runtime::spawn(async move {
        let _ = engine.hydrate_missing_workshop_metadata().await;
    });
}

async fn workshop_cover_response(
    engine: &L4aEngine,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let error_response = || Response::builder().header(header::CACHE_CONTROL, "no-store");
    if request.method() != Method::GET || request.uri().path() != "/image" {
        return error_response()
            .status(StatusCode::BAD_REQUEST)
            .body(Vec::new())
            .unwrap();
    }
    let url = tauri::Url::parse(&request.uri().to_string())
        .ok()
        .and_then(|request| {
            request
                .query_pairs()
                .find(|(key, _)| key == "url")
                .map(|(_, url)| url.into_owned())
        });
    if let Some(url) = url {
        if let Ok((mime, bytes)) = engine.fetch_workshop_cover(&url).await {
            return Response::builder()
                .header(header::CACHE_CONTROL, "public, max-age=86400")
                .header(header::CONTENT_TYPE, mime)
                .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                .header(
                    header::CONTENT_SECURITY_POLICY,
                    "default-src 'none'; sandbox",
                )
                .body(bytes)
                .unwrap();
        }
    }
    error_response()
        .status(StatusCode::NOT_FOUND)
        .body(Vec::new())
        .unwrap()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("l4a-cover", |context, request, responder| {
            let engine = context.app_handle().state::<TauriState>().engine.clone();
            tauri::async_runtime::spawn(async move {
                responder.respond(workshop_cover_response(&engine, request).await);
            });
        })
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

            spawn_workshop_hydration(&engine);

            let watcher_engine = engine.clone();
            if let Err(err) = engine.start_watcher(Arc::new(move || {
                if watcher_engine.scan_external().is_ok() {
                    spawn_workshop_hydration(&watcher_engine);
                }
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
            commands::read_cached_image,
            commands::open_external_url,
            commands::open_in_file_manager,
            commands::save_config,
            commands::detect_game_addons_path,
            commands::get_steam_capabilities,
            commands::get_subscribed_workshop_ids,
            commands::sync_workshop,
            commands::search_workshop,
            commands::fetch_workshop_item,
            commands::download_workshop_item,
            commands::update_workshop_item,
            commands::cancel_download,
            commands::resolve_reconcile_issue,
            commands::resolve_reconcile_issues_batch,
            commands::move_addons,
            commands::toggle_addons,
            commands::preview_rename,
            commands::apply_renames,
            commands::delete_addons,
            commands::safe_delete_obsolete_dependency,
            commands::rebuild_bundles,
            commands::create_manual_bundle,
            commands::set_bundle_locked,
            commands::delete_bundle,
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
