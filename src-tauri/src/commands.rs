use crate::audit::{
    audited, batch_summary, filter_batch_summary, filter_status_summary, no_summary,
    profile_summary, reconcile_summary, search_summary, sync_summary, OP_TARGET,
};
use crate::TauriState;
use l4a_core::{validate_external_url, CoreError};
use l4a_logging::redact::redact_url_str;
use l4a_protocol::{
    AddonBatchReport, AddonDirType, AddonId, AddonSummary, AppConfig, BundleId, BundleRecord,
    DashboardSnapshot, DeployLocalFilterRequest, DesktopError, DesktopErrorCode,
    FilterCacheClearResult, FilterEvaluationBatchResponse, FilterEvaluationTarget,
    FilterTokenStatus, GroupId, IssueId, LoadoutProfile, LocalFilterDeploymentStatus,
    ProfileApplyReport, ProfileId, ReconcileAction, ReconcileBatchItem, ReconcileBatchReport,
    RenamePreviewItem, RenameRuleConfig, SearchQuery, SearchResponse, SyncReport,
    TrackedCollectionRecord, UpsertFilterExclusionRequest, UserGroup, WorkshopId,
    WorkshopItemRecord,
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
    let snapshot = audited(
        "scan_workspace",
        String::new(),
        |snapshot: &DashboardSnapshot| format!("addons={}", snapshot.addons.len()),
        async { state.engine.scan() },
    )
    .await?;
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
    let validated = validate_external_url(&url).inspect_err(|err| {
        tracing::warn!(target: OP_TARGET, op = "open_external_url", url = %redact_url_str(&url), error = %err, "Refused to open external URL");
    })?;
    tracing::info!(target: OP_TARGET, op = "open_external_url", url = %redact_url_str(&validated), "Opening external URL");
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
    tracing::info!(target: OP_TARGET, op = "open_in_file_manager", path = %target.display(), "Revealing in file manager");
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
    let saved = audited(
        "save_config",
        format!(
            "loading_dir={:?} workshop_dir={:?} catalog_server={:?} https_only={} steamworks_disabled={} filter_mode={:?}",
            config.loading_dir,
            config.workshop_dir,
            config.catalog_server_url.as_deref().map(redact_url_str),
            config.https_only,
            config.disable_steamworks_sdk,
            config.filter.mode
        ),
        no_summary,
        engine.save_config_async(config),
    )
    .await?;
    crate::spawn_workshop_hydration(&state.engine);
    Ok(saved)
}

#[tauri::command]
pub async fn detect_game_addons_path() -> Result<Option<String>, DesktopError> {
    let detected = l4a_steam::detect_l4d2_addons_path();
    tracing::info!(target: OP_TARGET, op = "detect_game_addons_path", result = ?detected, "Detected game addons directory");
    Ok(detected)
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
    audited(
        "get_subscribed_workshop_ids",
        String::new(),
        |ids: &Vec<WorkshopId>| format!("count={}", ids.len()),
        state.engine.subscribed_workshop_ids(),
    )
    .await
}

#[tauri::command]
pub async fn sync_workshop(state: State<'_, TauriState>) -> Result<SyncReport, DesktopError> {
    audited(
        "sync_workshop",
        String::new(),
        sync_summary,
        state.engine.sync_workshop(),
    )
    .await
}

#[tauri::command]
pub async fn search_workshop(
    query: SearchQuery,
    state: State<'_, TauriState>,
) -> Result<SearchResponse, DesktopError> {
    audited(
        "search_workshop",
        format!(
            "query={:?} tag={:?} sort={:?} page={} page_size={}",
            query.query, query.tag, query.sort, query.page, query.page_size
        ),
        search_summary,
        state.engine.search_workshop(&query),
    )
    .await
}

#[tauri::command]
pub async fn fetch_workshop_item(
    workshop_id: WorkshopId,
    state: State<'_, TauriState>,
) -> Result<WorkshopItemRecord, DesktopError> {
    audited(
        "fetch_workshop_item",
        format!("workshop_id={workshop_id}"),
        |item: &WorkshopItemRecord| format!("title={:?}", item.title),
        state.engine.fetch_workshop_item(workshop_id),
    )
    .await
}

#[tauri::command]
pub async fn download_workshop_item(
    workshop_id: WorkshopId,
    title_hint: Option<String>,
    state: State<'_, TauriState>,
) -> Result<AddonSummary, DesktopError> {
    audited(
        "download_workshop_item",
        format!("workshop_id={workshop_id} title={title_hint:?}"),
        addon_summary,
        state
            .engine
            .download_workshop_item_with_title(workshop_id, title_hint.as_deref()),
    )
    .await
}

#[tauri::command]
pub async fn update_workshop_item(
    workshop_id: WorkshopId,
    title_hint: Option<String>,
    state: State<'_, TauriState>,
) -> Result<AddonSummary, DesktopError> {
    audited(
        "update_workshop_item",
        format!("workshop_id={workshop_id} title={title_hint:?}"),
        addon_summary,
        state
            .engine
            .update_workshop_item_with_title(workshop_id, title_hint.as_deref()),
    )
    .await
}

#[tauri::command]
pub async fn cancel_download(
    workshop_id: WorkshopId,
    state: State<'_, TauriState>,
) -> Result<bool, DesktopError> {
    let cancelled = state.engine.cancel_download(workshop_id);
    tracing::info!(target: OP_TARGET, op = "cancel_download", %workshop_id, cancelled, "Download cancel requested");
    Ok(cancelled)
}

#[tauri::command]
pub async fn resolve_reconcile_issue(
    issue_id: IssueId,
    action: ReconcileAction,
    state: State<'_, TauriState>,
) -> Result<DashboardSnapshot, DesktopError> {
    audited(
        "resolve_reconcile_issue",
        format!("issue_id={issue_id:?} action={action:?}"),
        no_summary,
        async { state.engine.resolve_issue(&issue_id, action) },
    )
    .await
}

#[tauri::command]
pub async fn resolve_reconcile_issues_batch(
    items: Vec<ReconcileBatchItem>,
    state: State<'_, TauriState>,
) -> Result<ReconcileBatchReport, DesktopError> {
    audited(
        "resolve_reconcile_issues_batch",
        format!("count={}", items.len()),
        reconcile_summary,
        async { state.engine.resolve_issues_batch(&items) },
    )
    .await
}

#[tauri::command]
pub async fn move_addons(
    addon_ids: Vec<AddonId>,
    target_dir_type: AddonDirType,
    state: State<'_, TauriState>,
) -> Result<AddonBatchReport, DesktopError> {
    audited(
        "move_addons",
        format!("{} target={target_dir_type:?}", ids_detail(&addon_ids)),
        batch_summary,
        async { state.engine.move_addons(&addon_ids, target_dir_type) },
    )
    .await
}

#[tauri::command]
pub async fn toggle_addons(
    addon_ids: Vec<AddonId>,
    enabled: bool,
    allow_workshop_dir: Option<bool>,
    state: State<'_, TauriState>,
) -> Result<AddonBatchReport, DesktopError> {
    let allow_workshop_dir = allow_workshop_dir.unwrap_or(false);
    audited(
        "toggle_addons",
        format!(
            "{} enabled={enabled} allow_workshop_dir={allow_workshop_dir}",
            ids_detail(&addon_ids)
        ),
        batch_summary,
        async {
            state
                .engine
                .toggle_addons(&addon_ids, enabled, allow_workshop_dir)
        },
    )
    .await
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
    let preview: Vec<String> = renames
        .iter()
        .take(20)
        .map(|(id, name)| format!("{id}->{name}"))
        .collect();
    audited(
        "apply_renames",
        format!("count={} renames=[{}]", renames.len(), preview.join(", ")),
        batch_summary,
        async { state.engine.apply_renames(&renames) },
    )
    .await
}

#[tauri::command]
pub async fn delete_addons(
    addon_ids: Vec<AddonId>,
    delete_disk_file: bool,
    purge_known_record: bool,
    state: State<'_, TauriState>,
) -> Result<AddonBatchReport, DesktopError> {
    audited(
        "delete_addons",
        format!(
            "{} delete_disk_file={delete_disk_file} purge_known_record={purge_known_record}",
            ids_detail(&addon_ids)
        ),
        batch_summary,
        async {
            state
                .engine
                .delete_addons(&addon_ids, delete_disk_file, purge_known_record)
        },
    )
    .await
}

#[tauri::command]
pub async fn safe_delete_obsolete_dependency(
    workshop_id: WorkshopId,
    delete_disk_file: bool,
    purge_known_record: bool,
    state: State<'_, TauriState>,
) -> Result<AddonBatchReport, DesktopError> {
    audited(
        "safe_delete_obsolete_dependency",
        format!(
            "workshop_id={workshop_id} delete_disk_file={delete_disk_file} purge_known_record={purge_known_record}"
        ),
        batch_summary,
        async {
            state.engine.safe_delete_obsolete_dependency(
                workshop_id,
                delete_disk_file,
                purge_known_record,
            )
        },
    )
    .await
}

#[tauri::command]
pub async fn rebuild_bundles(
    state: State<'_, TauriState>,
) -> Result<Vec<BundleRecord>, DesktopError> {
    audited(
        "rebuild_bundles",
        String::new(),
        |bundles: &Vec<BundleRecord>| format!("bundles={}", bundles.len()),
        async { state.engine.rebuild_bundles() },
    )
    .await
}

#[tauri::command]
pub async fn create_manual_bundle(
    name: String,
    member_addon_ids: Vec<AddonId>,
    state: State<'_, TauriState>,
) -> Result<BundleRecord, DesktopError> {
    audited(
        "create_manual_bundle",
        format!("name={name:?} {}", ids_detail(&member_addon_ids)),
        |bundle: &BundleRecord| format!("bundle_id={:?}", bundle.id),
        async { state.engine.create_manual_bundle(name, member_addon_ids) },
    )
    .await
}

#[tauri::command]
pub async fn set_bundle_locked(
    bundle_id: BundleId,
    locked: bool,
    state: State<'_, TauriState>,
) -> Result<(), DesktopError> {
    audited(
        "set_bundle_locked",
        format!("bundle_id={bundle_id:?} locked={locked}"),
        no_summary,
        async { state.engine.set_bundle_locked(&bundle_id, locked) },
    )
    .await
}

#[tauri::command]
pub async fn delete_bundle(
    bundle_id: BundleId,
    state: State<'_, TauriState>,
) -> Result<(), DesktopError> {
    audited(
        "delete_bundle",
        format!("bundle_id={bundle_id:?}"),
        no_summary,
        async { state.engine.delete_bundle(&bundle_id) },
    )
    .await
}

#[tauri::command]
pub async fn track_collection(
    collection_id: WorkshopId,
    auto_sync: bool,
    state: State<'_, TauriState>,
) -> Result<TrackedCollectionRecord, DesktopError> {
    let record = audited(
        "track_collection",
        format!("collection_id={collection_id} auto_sync={auto_sync}"),
        no_summary,
        state.engine.track_collection(collection_id, auto_sync),
    )
    .await?;
    crate::spawn_workshop_hydration(&state.engine);
    Ok(record)
}

#[tauri::command]
pub async fn untrack_collection(
    collection_id: WorkshopId,
    state: State<'_, TauriState>,
) -> Result<(), DesktopError> {
    audited(
        "untrack_collection",
        format!("collection_id={collection_id}"),
        no_summary,
        async { state.engine.untrack_collection(collection_id) },
    )
    .await
}

#[tauri::command]
pub async fn save_user_group(
    group: UserGroup,
    state: State<'_, TauriState>,
) -> Result<UserGroup, DesktopError> {
    audited(
        "save_user_group",
        format!("group_id={:?} name={:?}", group.id, group.name),
        no_summary,
        async { state.engine.save_user_group(group) },
    )
    .await
}

#[tauri::command]
pub async fn delete_user_group(
    group_id: GroupId,
    state: State<'_, TauriState>,
) -> Result<(), DesktopError> {
    audited(
        "delete_user_group",
        format!("group_id={group_id:?}"),
        no_summary,
        async { state.engine.delete_user_group(&group_id) },
    )
    .await
}

#[tauri::command]
pub async fn save_profile(
    profile: LoadoutProfile,
    state: State<'_, TauriState>,
) -> Result<LoadoutProfile, DesktopError> {
    audited(
        "save_profile",
        format!(
            "profile_id={:?} name={:?} enabled_addons={}",
            profile.id,
            profile.name,
            profile.enabled_addon_ids.len()
        ),
        no_summary,
        async { state.engine.save_profile(profile) },
    )
    .await
}

#[tauri::command]
pub async fn delete_profile(
    profile_id: ProfileId,
    state: State<'_, TauriState>,
) -> Result<(), DesktopError> {
    audited(
        "delete_profile",
        format!("profile_id={profile_id:?}"),
        no_summary,
        async { state.engine.delete_profile(&profile_id) },
    )
    .await
}

#[tauri::command]
pub async fn apply_profile(
    profile_id: ProfileId,
    state: State<'_, TauriState>,
) -> Result<ProfileApplyReport, DesktopError> {
    audited(
        "apply_profile",
        format!("profile_id={profile_id:?}"),
        profile_summary,
        async { state.engine.apply_profile(&profile_id) },
    )
    .await
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
    audited(
        "check_local_filter_updates",
        String::new(),
        filter_status_summary,
        state.engine.check_local_filter_updates(),
    )
    .await
}

#[tauri::command]
pub async fn deploy_local_filter(
    request: DeployLocalFilterRequest,
    state: State<'_, TauriState>,
) -> Result<LocalFilterDeploymentStatus, DesktopError> {
    audited(
        "deploy_local_filter",
        format!(
            "repo_url={:?} track_latest={} target_commit={:?} start_after_deploy={} install_web_gpu={:?}",
            request.repo_url.as_deref().map(redact_url_str),
            request.track_latest,
            request.target_commit,
            request.start_after_deploy,
            request.install_web_gpu
        ),
        filter_status_summary,
        state.engine.deploy_local_filter(request),
    )
    .await
}

#[tauri::command]
pub async fn start_local_filter_service(
    state: State<'_, TauriState>,
) -> Result<LocalFilterDeploymentStatus, DesktopError> {
    audited(
        "start_local_filter_service",
        String::new(),
        filter_status_summary,
        state.engine.start_local_filter_service(),
    )
    .await
}

#[tauri::command]
pub async fn stop_local_filter_service(
    state: State<'_, TauriState>,
) -> Result<LocalFilterDeploymentStatus, DesktopError> {
    audited(
        "stop_local_filter_service",
        String::new(),
        filter_status_summary,
        state.engine.stop_local_filter_service(),
    )
    .await
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
    // Never log the token itself, only whether one was provided.
    let provided = token.as_deref().is_some_and(|t| !t.trim().is_empty());
    audited(
        "set_filter_llm_token",
        format!("token_provided={provided}"),
        no_summary,
        async { state.engine.set_filter_llm_token(token) },
    )
    .await
}

#[tauri::command]
pub async fn evaluate_filter_items(
    targets: Vec<FilterEvaluationTarget>,
    automatic: Option<bool>,
    state: State<'_, TauriState>,
) -> Result<FilterEvaluationBatchResponse, DesktopError> {
    let automatic = automatic.unwrap_or(false);
    audited(
        "evaluate_filter_items",
        format!("targets={} automatic={automatic}", targets.len()),
        filter_batch_summary,
        async {
            if automatic {
                state.engine.evaluate_automatic_filter_items(targets).await
            } else {
                state.engine.evaluate_filter_items(targets).await
            }
        },
    )
    .await
}

#[tauri::command]
pub async fn clear_filter_cache(
    state: State<'_, TauriState>,
) -> Result<FilterCacheClearResult, DesktopError> {
    audited(
        "clear_filter_cache",
        String::new(),
        |result: &FilterCacheClearResult| {
            format!(
                "cleared={} cache_generation={}",
                result.count, result.cache_generation
            )
        },
        async { state.engine.clear_filter_cache() },
    )
    .await
}

#[tauri::command]
pub async fn upsert_filter_exclusion(
    request: UpsertFilterExclusionRequest,
    state: State<'_, TauriState>,
) -> Result<DashboardSnapshot, DesktopError> {
    audited(
        "upsert_filter_exclusion",
        format!(
            "addon_id={} display_level={:?} bind_fingerprint={:?}",
            request.addon_id, request.display_level, request.bind_fingerprint
        ),
        no_summary,
        async { state.engine.upsert_filter_exclusion(request) },
    )
    .await
}

#[tauri::command]
pub async fn delete_filter_exclusion(
    addon_id: AddonId,
    state: State<'_, TauriState>,
) -> Result<DashboardSnapshot, DesktopError> {
    audited(
        "delete_filter_exclusion",
        format!("addon_id={addon_id}"),
        no_summary,
        async { state.engine.delete_filter_exclusion(&addon_id) },
    )
    .await
}

/// Logs the number of affected addons and up to 20 of their IDs.
fn ids_detail(ids: &[AddonId]) -> String {
    let shown: Vec<&str> = ids.iter().take(20).map(|id| id.0.as_str()).collect();
    let more = ids.len().saturating_sub(shown.len());
    if more > 0 {
        format!("count={} ids=[{}, …+{more}]", ids.len(), shown.join(", "))
    } else {
        format!("count={} ids=[{}]", ids.len(), shown.join(", "))
    }
}

fn addon_summary(addon: &AddonSummary) -> String {
    format!(
        "addon_id={} file={:?} size={}",
        addon.id, addon.current_file_name, addon.file_size
    )
}
