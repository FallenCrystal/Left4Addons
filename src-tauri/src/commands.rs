use crate::TauriState;
use l4a_core::{validate_external_url, CoreError};
use l4a_protocol::{
    AddonBatchReport, AddonDirType, AddonId, AddonSummary, AppConfig, BundleId, BundleRecord,
    DashboardSnapshot, DeployLocalFilterRequest, DesktopError, DesktopErrorCode,
    FilterEvaluationBatchResponse, FilterEvaluationTarget, FilterTokenStatus, GroupId, IssueId,
    LoadoutProfile, LocalFilterDeploymentStatus, ProfileApplyReport, ProfileId, ReconcileAction,
    ReconcileBatchItem, ReconcileBatchReport, RenamePreviewItem, RenameRuleConfig, SearchQuery,
    SearchResponse, SyncReport, TrackedCollectionRecord, UpsertFilterExclusionRequest, UserGroup,
    WorkshopId, WorkshopItemRecord,
};
use l4a_steam::SteamCapabilities;
use std::io;
use std::path::{Path, PathBuf};
use tauri::State;
use tokio::process::Command;

#[tauri::command]
pub async fn get_dashboard_snapshot(
    state: State<'_, TauriState>,
) -> Result<DashboardSnapshot, DesktopError> {
    state.engine.snapshot().map_err(Into::into)
}

#[tauri::command]
pub async fn scan_workspace(
    state: State<'_, TauriState>,
) -> Result<DashboardSnapshot, DesktopError> {
    let snapshot = state.engine.scan()?;
    crate::spawn_workshop_hydration(&state.engine);
    Ok(snapshot)
}

#[tauri::command]
pub async fn read_cached_image(
    cache_key: String,
    state: State<'_, TauriState>,
) -> Result<Vec<u8>, DesktopError> {
    state
        .engine
        .read_cached_image(&cache_key)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn open_external_url(url: String) -> Result<(), DesktopError> {
    let validated = validate_external_url(&url)?;
    let program = match std::env::consts::OS {
        "linux" => "xdg-open",
        "macos" => "open",
        "windows" => "rundll32.exe",
        _ => {
            return Err(DesktopError {
                code: DesktopErrorCode::Unsupported,
                message: "当前平台不支持打开外部链接".into(),
            });
        }
    };
    let mut command = Command::new(program);
    if cfg!(windows) {
        command.arg("url.dll,FileProtocolHandler");
    }
    command.arg(&validated).spawn().map_err(CoreError::from)?;
    Ok(())
}

#[tauri::command]
pub async fn open_in_file_manager(
    path: String,
    state: State<'_, TauriState>,
) -> Result<(), DesktopError> {
    let target = state.engine.resolve_managed_path(&path)?;
    tokio::task::spawn_blocking(move || show_in_folder(&target))
        .await
        .map_err(|err| CoreError::Io(io::Error::other(err)))??;
    Ok(())
}

#[cfg(target_os = "windows")]
fn clean_canonical_path(path: &Path) -> PathBuf {
    let path = path.to_string_lossy();
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else {
        PathBuf::from(path.strip_prefix(r"\\?\").unwrap_or(&path))
    }
}

#[cfg(not(target_os = "windows"))]
fn clean_canonical_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

fn show_in_folder(path: &Path) -> Result<(), CoreError> {
    let is_file = path.is_file();
    let canonical = clean_canonical_path(path);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        let path = canonical.to_string_lossy().replace('"', "");
        let argument = if is_file {
            format!("/select,\"{path}\"")
        } else {
            format!("\"{path}\"")
        };
        std::process::Command::new("explorer")
            .raw_arg(argument)
            .spawn()?;
    }

    #[cfg(target_os = "macos")]
    {
        let mut open = Command::new("open");
        if is_file {
            open.arg("-R");
        }
        open.arg(&canonical).spawn()?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let mut directory = canonical.as_path();
        if is_file {
            if reveal_with_dbus(&canonical) {
                return Ok(());
            }
            for manager in [
                "nautilus",
                "dolphin",
                "thunar",
                "nemo",
                "pcmanfm-qt",
                "dde-file-manager",
            ] {
                if Command::new(manager)
                    .arg("--select")
                    .arg(&canonical)
                    .spawn()
                    .is_ok()
                {
                    return Ok(());
                }
            }
            if let Some(parent) = canonical.parent() {
                directory = parent;
            }
        }
        Command::new("xdg-open").arg(directory).spawn()?;
    }

    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn path_to_file_url(path: &Path) -> String {
    use std::fmt::Write;

    let mut url = String::from("file://");
    for &byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            url.push(char::from(byte));
        } else {
            write!(url, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    url
}

#[cfg(all(test, not(any(target_os = "windows", target_os = "macos"))))]
#[test]
fn encodes_file_manager_url() {
    assert_eq!(
        path_to_file_url(Path::new("/addons/Café 100% #1_~.vpk")),
        "file:///addons/Caf%C3%A9%20100%25%20%231_~.vpk"
    );
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn reveal_with_dbus(path: &Path) -> bool {
    use std::process::{Command, Stdio};

    let url = path_to_file_url(path);
    let success = |command: &mut Command| {
        command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    success(Command::new("dbus-send").args([
        "--session",
        "--dest=org.freedesktop.FileManager1",
        "--type=method_call",
        "--print-reply",
        "--reply-timeout=3000",
        "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1.ShowItems",
        &format!("array:string:{url}"),
        "string:",
    ])) || success(Command::new("busctl").args([
        "--user",
        "--timeout=3s",
        "call",
        "org.freedesktop.FileManager1",
        "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1",
        "ShowItems",
        "ass",
        "1",
        &url,
        "",
    ]))
}

#[tauri::command]
pub async fn save_config(
    config: AppConfig,
    state: State<'_, TauriState>,
) -> Result<AppConfig, DesktopError> {
    let engine = state.engine.clone();
    let saved = engine.save_config_async(config).await?;
    crate::spawn_workshop_hydration(&state.engine);
    Ok(saved)
}

#[tauri::command]
pub async fn detect_game_addons_path() -> Result<Option<String>, DesktopError> {
    Ok(l4a_steam::detect_l4d2_addons_path())
}

#[tauri::command]
pub async fn get_steam_capabilities(
    state: State<'_, TauriState>,
) -> Result<SteamCapabilities, DesktopError> {
    Ok(state.engine.steam_capabilities().await)
}

#[tauri::command]
pub async fn get_subscribed_workshop_ids(
    state: State<'_, TauriState>,
) -> Result<Vec<WorkshopId>, DesktopError> {
    state
        .engine
        .subscribed_workshop_ids()
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn sync_workshop(state: State<'_, TauriState>) -> Result<SyncReport, DesktopError> {
    state.engine.sync_workshop().await.map_err(Into::into)
}

#[tauri::command]
pub async fn search_workshop(
    query: SearchQuery,
    state: State<'_, TauriState>,
) -> Result<SearchResponse, DesktopError> {
    state
        .engine
        .search_workshop(&query)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn fetch_workshop_item(
    workshop_id: WorkshopId,
    state: State<'_, TauriState>,
) -> Result<WorkshopItemRecord, DesktopError> {
    state
        .engine
        .fetch_workshop_item(workshop_id)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn download_workshop_item(
    workshop_id: WorkshopId,
    title_hint: Option<String>,
    state: State<'_, TauriState>,
) -> Result<AddonSummary, DesktopError> {
    state
        .engine
        .download_workshop_item_with_title(workshop_id, title_hint.as_deref())
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn update_workshop_item(
    workshop_id: WorkshopId,
    title_hint: Option<String>,
    state: State<'_, TauriState>,
) -> Result<AddonSummary, DesktopError> {
    state
        .engine
        .update_workshop_item_with_title(workshop_id, title_hint.as_deref())
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn cancel_download(
    workshop_id: WorkshopId,
    state: State<'_, TauriState>,
) -> Result<bool, DesktopError> {
    Ok(state.engine.cancel_download(workshop_id))
}

#[tauri::command]
pub async fn resolve_reconcile_issue(
    issue_id: IssueId,
    action: ReconcileAction,
    state: State<'_, TauriState>,
) -> Result<DashboardSnapshot, DesktopError> {
    state
        .engine
        .resolve_issue(&issue_id, action)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn resolve_reconcile_issues_batch(
    items: Vec<ReconcileBatchItem>,
    state: State<'_, TauriState>,
) -> Result<ReconcileBatchReport, DesktopError> {
    state
        .engine
        .resolve_issues_batch(&items)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn move_addons(
    addon_ids: Vec<AddonId>,
    target_dir_type: AddonDirType,
    state: State<'_, TauriState>,
) -> Result<AddonBatchReport, DesktopError> {
    state
        .engine
        .move_addons(&addon_ids, target_dir_type)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn toggle_addons(
    addon_ids: Vec<AddonId>,
    enabled: bool,
    allow_workshop_dir: Option<bool>,
    state: State<'_, TauriState>,
) -> Result<AddonBatchReport, DesktopError> {
    state
        .engine
        .toggle_addons(&addon_ids, enabled, allow_workshop_dir.unwrap_or(false))
        .map_err(Into::into)
}

#[tauri::command]
pub async fn preview_rename(
    rules: RenameRuleConfig,
    addon_ids: Vec<AddonId>,
    state: State<'_, TauriState>,
) -> Result<Vec<RenamePreviewItem>, DesktopError> {
    state
        .engine
        .preview_rename(&rules, &addon_ids)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn apply_renames(
    renames: Vec<(AddonId, String)>,
    state: State<'_, TauriState>,
) -> Result<AddonBatchReport, DesktopError> {
    state.engine.apply_renames(&renames).map_err(Into::into)
}

#[tauri::command]
pub async fn delete_addons(
    addon_ids: Vec<AddonId>,
    delete_disk_file: bool,
    purge_known_record: bool,
    state: State<'_, TauriState>,
) -> Result<AddonBatchReport, DesktopError> {
    state
        .engine
        .delete_addons(&addon_ids, delete_disk_file, purge_known_record)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn safe_delete_obsolete_dependency(
    workshop_id: WorkshopId,
    delete_disk_file: bool,
    purge_known_record: bool,
    state: State<'_, TauriState>,
) -> Result<AddonBatchReport, DesktopError> {
    state
        .engine
        .safe_delete_obsolete_dependency(workshop_id, delete_disk_file, purge_known_record)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn rebuild_bundles(
    state: State<'_, TauriState>,
) -> Result<Vec<BundleRecord>, DesktopError> {
    state.engine.rebuild_bundles().map_err(Into::into)
}

#[tauri::command]
pub async fn create_manual_bundle(
    name: String,
    member_addon_ids: Vec<AddonId>,
    state: State<'_, TauriState>,
) -> Result<BundleRecord, DesktopError> {
    state
        .engine
        .create_manual_bundle(name, member_addon_ids)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn set_bundle_locked(
    bundle_id: BundleId,
    locked: bool,
    state: State<'_, TauriState>,
) -> Result<(), DesktopError> {
    state
        .engine
        .set_bundle_locked(&bundle_id, locked)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn delete_bundle(
    bundle_id: BundleId,
    state: State<'_, TauriState>,
) -> Result<(), DesktopError> {
    state.engine.delete_bundle(&bundle_id).map_err(Into::into)
}

#[tauri::command]
pub async fn track_collection(
    collection_id: WorkshopId,
    auto_sync: bool,
    state: State<'_, TauriState>,
) -> Result<TrackedCollectionRecord, DesktopError> {
    let record = state
        .engine
        .track_collection(collection_id, auto_sync)
        .await?;
    crate::spawn_workshop_hydration(&state.engine);
    Ok(record)
}

#[tauri::command]
pub async fn untrack_collection(
    collection_id: WorkshopId,
    state: State<'_, TauriState>,
) -> Result<(), DesktopError> {
    state
        .engine
        .untrack_collection(collection_id)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn save_user_group(
    group: UserGroup,
    state: State<'_, TauriState>,
) -> Result<UserGroup, DesktopError> {
    state.engine.save_user_group(group).map_err(Into::into)
}

#[tauri::command]
pub async fn delete_user_group(
    group_id: GroupId,
    state: State<'_, TauriState>,
) -> Result<(), DesktopError> {
    state
        .engine
        .delete_user_group(&group_id)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn save_profile(
    profile: LoadoutProfile,
    state: State<'_, TauriState>,
) -> Result<LoadoutProfile, DesktopError> {
    state.engine.save_profile(profile).map_err(Into::into)
}

#[tauri::command]
pub async fn delete_profile(
    profile_id: ProfileId,
    state: State<'_, TauriState>,
) -> Result<(), DesktopError> {
    state.engine.delete_profile(&profile_id).map_err(Into::into)
}

#[tauri::command]
pub async fn apply_profile(
    profile_id: ProfileId,
    state: State<'_, TauriState>,
) -> Result<ProfileApplyReport, DesktopError> {
    state.engine.apply_profile(&profile_id).map_err(Into::into)
}

#[tauri::command]
pub async fn search_by_map_code(
    query: String,
    state: State<'_, TauriState>,
) -> Result<Vec<AddonSummary>, DesktopError> {
    state.engine.search_by_map_code(&query).map_err(Into::into)
}

#[tauri::command]
pub async fn get_local_filter_status(
    state: State<'_, TauriState>,
) -> Result<LocalFilterDeploymentStatus, DesktopError> {
    state
        .engine
        .get_local_filter_status()
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn check_local_filter_updates(
    state: State<'_, TauriState>,
) -> Result<LocalFilterDeploymentStatus, DesktopError> {
    state
        .engine
        .check_local_filter_updates()
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn deploy_local_filter(
    request: DeployLocalFilterRequest,
    state: State<'_, TauriState>,
) -> Result<LocalFilterDeploymentStatus, DesktopError> {
    state
        .engine
        .deploy_local_filter(request)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn start_local_filter_service(
    state: State<'_, TauriState>,
) -> Result<LocalFilterDeploymentStatus, DesktopError> {
    state
        .engine
        .start_local_filter_service()
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn stop_local_filter_service(
    state: State<'_, TauriState>,
) -> Result<LocalFilterDeploymentStatus, DesktopError> {
    state
        .engine
        .stop_local_filter_service()
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn get_filter_token_status(
    state: State<'_, TauriState>,
) -> Result<FilterTokenStatus, DesktopError> {
    state.engine.get_filter_token_status().map_err(Into::into)
}

#[tauri::command]
pub async fn set_filter_llm_token(
    token: Option<String>,
    state: State<'_, TauriState>,
) -> Result<FilterTokenStatus, DesktopError> {
    state.engine.set_filter_llm_token(token).map_err(Into::into)
}

#[tauri::command]
pub async fn evaluate_filter_items(
    targets: Vec<FilterEvaluationTarget>,
    state: State<'_, TauriState>,
) -> Result<FilterEvaluationBatchResponse, DesktopError> {
    state
        .engine
        .evaluate_filter_items(targets)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn clear_filter_cache(state: State<'_, TauriState>) -> Result<u64, DesktopError> {
    state.engine.clear_filter_cache().map_err(Into::into)
}

#[tauri::command]
pub async fn upsert_filter_exclusion(
    request: UpsertFilterExclusionRequest,
    state: State<'_, TauriState>,
) -> Result<DashboardSnapshot, DesktopError> {
    state
        .engine
        .upsert_filter_exclusion(request)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn delete_filter_exclusion(
    addon_id: AddonId,
    state: State<'_, TauriState>,
) -> Result<DashboardSnapshot, DesktopError> {
    state
        .engine
        .delete_filter_exclusion(&addon_id)
        .map_err(Into::into)
}
