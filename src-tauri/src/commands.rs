use crate::TauriState;
use l4a_protocol::{
    AddonBatchReport, AddonDirType, AddonId, AddonSummary, AppConfig, BundleId, BundleRecord,
    DashboardSnapshot, DesktopError, GroupId, IssueId, LoadoutProfile, ProfileApplyReport,
    ProfileId, ReconcileAction, RenamePreviewItem, RenameRuleConfig, SearchQuery, SearchResponse,
    SyncReport, TrackedCollectionRecord, UserGroup, WorkshopId,
};
use l4a_steam::SteamCapabilities;
use tauri::State;

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
    state.engine.scan().map_err(Into::into)
}

#[tauri::command]
pub async fn save_config(
    config: AppConfig,
    state: State<'_, TauriState>,
) -> Result<AppConfig, DesktopError> {
    state.engine.save_config(config).map_err(Into::into)
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
pub async fn download_workshop_item(
    workshop_id: WorkshopId,
    state: State<'_, TauriState>,
) -> Result<AddonSummary, DesktopError> {
    state
        .engine
        .download_workshop_item(workshop_id)
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
pub async fn track_collection(
    collection_id: WorkshopId,
    auto_sync: bool,
    state: State<'_, TauriState>,
) -> Result<TrackedCollectionRecord, DesktopError> {
    state
        .engine
        .track_collection(collection_id, auto_sync)
        .await
        .map_err(Into::into)
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
