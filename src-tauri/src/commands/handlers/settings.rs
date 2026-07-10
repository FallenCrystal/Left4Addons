use super::*;
use serde::Deserialize;
use tauri::{AppHandle, State};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsPayload {
    loading_dir: String,
    download_concurrency: u32,
    enable_dummy_bypass: bool,
    suppress_sdk_unavailable_warning: bool,
    disable_steamworks_sdk: bool,
    force_steamworks_sdk_download: bool,
    workshop_source_settings: Option<WorkshopSourceSettings>,
}

#[tauri::command]
pub async fn get_settings(state: State<'_, crate::AppState>) -> Result<Settings, String> {
    let db = state.db.lock().await;
    Ok(db.settings.clone())
}

#[tauri::command]
pub async fn save_settings(
    payload: SaveSettingsPayload,
    state: State<'_, crate::AppState>,
    app_handle: AppHandle,
) -> Result<Database, String> {
    let SaveSettingsPayload {
        loading_dir,
        download_concurrency,
        enable_dummy_bypass,
        suppress_sdk_unavailable_warning,
        disable_steamworks_sdk,
        force_steamworks_sdk_download,
        workshop_source_settings,
    } = payload;

    let mut db = state.db.lock().await;
    let loading_path = PathBuf::from(&loading_dir);
    let workshop_dir = loading_path.join("workshop").to_string_lossy().to_string();

    db.settings.workshop_dir = workshop_dir;
    db.settings.loading_dir = loading_dir;
    db.settings.download_concurrency = download_concurrency.clamp(1, 8);
    db.settings.enable_dummy_bypass = enable_dummy_bypass;
    db.settings.suppress_sdk_unavailable_warning = suppress_sdk_unavailable_warning;
    db.settings.disable_steamworks_sdk = disable_steamworks_sdk;
    db.settings.force_steamworks_sdk_download = force_steamworks_sdk_download;
    if let Some(workshop_source_settings) = workshop_source_settings {
        db.settings.workshop_source_settings = workshop_source_settings;
    }
    if disable_steamworks_sdk {
        state.workshop_service.shutdown();
    }
    save_db_internal(
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &db,
    );

    scan_addons_internal(
        &mut db,
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &state.cache_dir,
        &state.workshop_service,
    )
    .await?;
    if let Err(err) = crate::watcher::rebind_addon_watcher(&app_handle, &loading_path) {
        crate::watcher::emit_watch_error(&app_handle, &err);
    }
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
        &state.known_addons_path,
    ))
}
