use crate::db::LocalDb;
use crate::error::CoreError;
use crate::file_changes::{occupied, regular_file, same_file, take_matching_dummy, FileChanges};
use crate::scanner::{scan_and_commit, scan_workspace};
use l4a_protocol::{
    AddonBatchReport, AddonDirType, AddonId, AddonOperationOutcome, AddonOperationResult,
    AddonSummary, AppConfig, InstallState, InvalidCharReplace, IssueId, RenamePreviewItem,
    RenameRuleConfig, WorkshopId,
};
use l4a_vpk::{inspect_dummy_vpk, DummyInspection, DummySpec};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static WIN_INVALID_RE: OnceLock<Regex> = OnceLock::new();

fn win_invalid_re() -> &'static Regex {
    WIN_INVALID_RE.get_or_init(|| Regex::new(r#"[\\/:*?"<>|]"#).unwrap())
}

pub(crate) fn append_os_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os: OsString = path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

pub(crate) fn strip_suffix_ascii_case<'a>(input: &'a str, suffix: &str) -> Option<&'a str> {
    let start = input.len().checked_sub(suffix.len())?;
    input
        .get(start..)?
        .eq_ignore_ascii_case(suffix)
        .then(|| &input[..start])
}

fn is_windows_reserved_name(stem: &str) -> bool {
    let head = stem.split('.').next().unwrap_or(stem);
    let upper = head.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// 按规则清洗文件名并保证输出以 `.vpk` 结尾。
pub fn sanitize_vpk_filename(raw_name: &str, rules: &RenameRuleConfig) -> String {
    let base = strip_suffix_ascii_case(raw_name, ".vpk.disabled")
        .or_else(|| strip_suffix_ascii_case(raw_name, ".disabled"))
        .or_else(|| strip_suffix_ascii_case(raw_name, ".vpk"))
        .unwrap_or(raw_name);

    let no_ctrl: String = base.chars().filter(|c| !c.is_control()).collect();
    let mut out = win_invalid_re().replace_all(&no_ctrl, "_").into_owned();

    if rules.clean_special_chars {
        let rep = match rules.invalid_char_replace {
            InvalidCharReplace::Space => " ",
            InvalidCharReplace::Empty => "",
            InvalidCharReplace::Underscore => "_",
        };
        let mut cleaned = String::with_capacity(out.len());
        for c in out.chars() {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '[' | ']' | ' ') {
                cleaned.push(c);
            } else {
                cleaned.push_str(rep);
            }
        }
        out = cleaned;
    }

    if rules.enable_trim {
        out = out.trim().to_string();
    }
    if rules.enable_remove_double_spaces {
        while out.contains("  ") {
            out = out.replace("  ", " ");
        }
    }
    if rules.max_filename_length > 0 {
        let max_base = rules.max_filename_length.saturating_sub(4).max(1);
        if out.chars().count() > max_base {
            out = out.chars().take(max_base).collect();
        }
    }
    if rules.enable_trim {
        out = out.trim().to_string();
    }

    let trimmed = out.trim_matches(|c| c == '.' || c == ' ');
    let final_base = if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "addon".to_string()
    } else if is_windows_reserved_name(trimmed) {
        format!("_{trimmed}")
    } else {
        trimmed.to_string()
    };

    format!("{final_base}.vpk")
}

/// 按规则组合可读文件名（支持 Workshop ID 前缀与分组前缀）。
pub fn build_target_vpk_name(
    workshop_id: Option<WorkshopId>,
    group_prefix: Option<&str>,
    title: &str,
    rules: &RenameRuleConfig,
) -> String {
    let mut parts = Vec::new();
    if rules.enable_workshop_id_prefix {
        if let Some(wid) = workshop_id {
            parts.push(format!("[{wid}]"));
        }
    }
    if rules.enable_group_prefix {
        if let Some(gp) = group_prefix.filter(|s| !s.trim().is_empty()) {
            parts.push(format!("[{}]", gp.trim()));
        }
    }
    parts.push(title.trim().to_string());
    sanitize_vpk_filename(&parts.join(" "), rules)
}

pub fn preview_batch_rename(
    db: &LocalDb,
    config: &AppConfig,
    rules: &RenameRuleConfig,
    addon_ids: &[AddonId],
) -> Result<Vec<RenamePreviewItem>, CoreError> {
    let addons = db.list_addons()?;
    let groups = db.list_user_groups()?;
    let mut first_group_by_addon: HashMap<AddonId, String> = HashMap::new();
    for g in groups {
        for aid in g.addon_ids {
            first_group_by_addon
                .entry(aid)
                .or_insert_with(|| g.name.clone());
        }
    }

    let target_set: HashSet<&AddonId> = addon_ids.iter().collect();
    let mut seen_targets = HashSet::new();
    let mut items = Vec::new();

    for addon in addons {
        if !target_set.is_empty() && !target_set.contains(&addon.id) {
            continue;
        }
        if addon.install_state != InstallState::Installed {
            continue;
        }
        let Some(ref cur_name) = addon.current_file_name else {
            continue;
        };
        let group_prefix = first_group_by_addon.get(&addon.id).map(String::as_str);
        let target_vpk =
            build_target_vpk_name(addon.workshop_id, group_prefix, &addon.title, rules);
        let conflict = plan_rename(config, &addon, &target_vpk, &mut seen_targets)
            .err()
            .map(|error| error.to_string());

        items.push(RenamePreviewItem {
            addon_id: addon.id,
            current_file_name: cur_name.clone(),
            target_file_name: target_vpk,
            conflict,
        });
    }

    Ok(items)
}

struct RenamePlan<'a> {
    addon: &'a AddonSummary,
    source: &'a Path,
    target: PathBuf,
    sibling: PathBuf,
    vpk_name: String,
}

fn plan_rename<'a>(
    config: &AppConfig,
    addon: &'a AddonSummary,
    vpk_name: &str,
    reserved: &mut HashSet<String>,
) -> Result<Option<RenamePlan<'a>>, CoreError> {
    if addon.primary_dir == Some(AddonDirType::Workshop) {
        return Err(CoreError::Conflict(format!(
            "模组 {} 位于 workshop/ 目录，禁止直接重命名，请先移至 loading/",
            addon.id
        )));
    }
    let source = addon_path(config, addon)?;
    let parent = source
        .parent()
        .ok_or_else(|| CoreError::InvalidInput("无效源目录".into()))?;
    let enabled_path = parent.join(vpk_name);
    let disabled_path = append_os_suffix(&enabled_path, ".disabled");
    let target_key = enabled_path.to_string_lossy().to_ascii_lowercase();
    let (target, sibling) = if addon.enabled {
        (enabled_path, disabled_path)
    } else {
        (disabled_path, enabled_path)
    };
    if source == target {
        return Ok(None);
    }
    let case_only = source
        .to_string_lossy()
        .eq_ignore_ascii_case(&target.to_string_lossy());
    if occupied(&target)?
        && !(case_only && same_file(&regular_file(source)?, &regular_file(&target)?))
    {
        return Err(target_conflict(&target));
    }
    require_free(&sibling)?;
    if !reserved.insert(target_key) {
        return Err(target_conflict(&target));
    }
    Ok(Some(RenamePlan {
        addon,
        source,
        target,
        sibling,
        vpk_name: vpk_name.to_string(),
    }))
}

pub fn apply_renames(
    db: &LocalDb,
    config: &AppConfig,
    image_cache_dir: &Path,
    renames: &[(AddonId, String)],
) -> Result<AddonBatchReport, CoreError> {
    let mut ids = HashSet::new();
    if renames.iter().any(|(id, _)| !ids.insert(id)) {
        return Err(CoreError::InvalidInput("批量重命名包含重复模组 ID".into()));
    }
    let addons = addons_by_id(db)?;
    let mut plans = Vec::with_capacity(renames.len());
    let mut reserved_dests = HashSet::new();
    for (id, raw_name) in renames {
        let vpk_name = sanitize_vpk_filename(raw_name, &config.rename_rules);
        match find_addon(&addons, id)
            .and_then(|addon| plan_rename(config, addon, &vpk_name, &mut reserved_dests))
        {
            Ok(plan) => plans.push((id, plan)),
            Err(error) => {
                let failure = AddonOperationOutcome::Failed {
                    error: error.into(),
                };
                let items = renames
                    .iter()
                    .map(|(addon_id, _)| AddonOperationResult {
                        addon_id: addon_id.clone(),
                        outcome: if addon_id == id {
                            failure.clone()
                        } else {
                            AddonOperationOutcome::NotAttempted
                        },
                    })
                    .collect();
                return Ok(AddonBatchReport {
                    items,
                    refresh_error: None,
                });
            }
        }
    }

    let mut renamed = Vec::new();
    let items = run_batch(plans, |_, plan| {
        let Some(plan) = plan else { return Ok(false) };
        let mut updated = plan.addon.clone();
        updated.current_file_name = Some(plan.vpk_name);
        updated.current_path = Some(plan.target.to_string_lossy().into_owned());
        publish_addon_path(db, &updated, plan.source, &plan.target, Some(&plan.sibling))?;
        renamed.push(updated);
        Ok(true)
    });
    Ok(refresh_batch(items, || {
        scan_and_commit(db, config, image_cache_dir, renamed, |_, _| Ok(()))
    }))
}

pub fn move_addons(
    db: &LocalDb,
    config: &AppConfig,
    image_cache_dir: &Path,
    addon_ids: &[AddonId],
    target_dir_type: AddonDirType,
) -> Result<AddonBatchReport, CoreError> {
    if addon_ids.is_empty() {
        return Ok(AddonBatchReport {
            items: Vec::new(),
            refresh_error: None,
        });
    }
    let target_dir = match target_dir_type {
        AddonDirType::Loading => Path::new(&config.loading_dir),
        AddonDirType::Workshop => Path::new(&config.workshop_dir),
    };
    if target_dir.as_os_str().is_empty() {
        return Err(CoreError::InvalidInput("目标模组目录未配置".into()));
    }
    fs::create_dir_all(target_dir)?;

    let addons = addons_by_id(db)?;
    let mut moved = Vec::new();
    let items = run_batch(addon_ids.iter().map(|id| (id, ())), |id, ()| {
        let addon = find_addon(&addons, id)?;
        let src = addon_path(config, addon)?;
        if addon.primary_dir == Some(target_dir_type) {
            return Ok(false);
        }
        let orig_name = src
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| CoreError::InvalidInput("无效源文件名".into()))?;

        let (dst, sibling, next_enabled, next_vpk_name) = match target_dir_type {
            AddonDirType::Workshop => {
                let target_vpk = match addon.workshop_id {
                    Some(wid) => format!("{wid}.vpk"),
                    None => strip_suffix_ascii_case(orig_name, ".disabled")
                        .unwrap_or(orig_name)
                        .to_string(),
                };
                let dst = target_dir.join(&target_vpk);
                let disabled_sibling = append_os_suffix(&dst, ".disabled");
                (dst, disabled_sibling, true, target_vpk)
            }
            AddonDirType::Loading => {
                let dst = target_dir.join(orig_name);
                let clean_vpk = strip_suffix_ascii_case(orig_name, ".disabled")
                    .unwrap_or(orig_name)
                    .to_string();
                let sibling = if addon.enabled {
                    append_os_suffix(&dst, ".disabled")
                } else {
                    target_dir.join(&clean_vpk)
                };
                (dst, sibling, addon.enabled, clean_vpk)
            }
        };

        require_free(&sibling)?;
        let source_dir = src
            .parent()
            .ok_or_else(|| CoreError::InvalidInput("无效源目录".into()))?;
        if fs::canonicalize(source_dir)? == fs::canonicalize(target_dir)? {
            return Err(CoreError::Conflict("源目录与目标目录冲突或已变化".into()));
        }

        let updated = FileChanges::run(|changes| {
            if occupied(&dst)? {
                let wid = addon
                    .workshop_id
                    .filter(|_| target_dir_type == AddonDirType::Workshop)
                    .ok_or_else(|| {
                        CoreError::Conflict(format!("目标文件已存在: {}", dst.display()))
                    })?;
                take_matching_dummy(changes, &dst, wid)?;
            }
            let (backup, content_hash) = take_addon_file(changes, addon, src)?;
            changes.publish_copy(&backup, &dst)?;

            let mut has_dummy = false;
            if let Some(wid) = addon
                .workshop_id
                .filter(|_| target_dir_type == AddonDirType::Loading && config.enable_dummy_bypass)
            {
                if config.workshop_dir.trim().is_empty() {
                    return Err(CoreError::InvalidInput(
                        "已启用 Dummy Bypass，但未配置 workshop_dir".into(),
                    ));
                }
                let dummy_path = Path::new(&config.workshop_dir).join(format!("{wid}.vpk"));
                take_matching_dummy(changes, &dummy_path, wid)?;
                changes.publish_dummy(
                    &dst,
                    &dummy_path,
                    &DummySpec {
                        workshop_id: wid,
                        original_hash: content_hash,
                        original_size: addon.file_size,
                        original_time_updated: addon.remote_time_updated,
                    },
                )?;
                has_dummy = true;
            }

            let mut updated = addon.clone();
            updated.primary_dir = Some(target_dir_type);
            updated.enabled = next_enabled;
            updated.current_file_name = Some(next_vpk_name);
            updated.current_path = Some(dst.to_string_lossy().into_owned());
            updated.has_dummy_bypass = has_dummy;
            changes.verify()?;
            require_free(&sibling)?;
            db.upsert_addon(&updated)?;
            Ok(updated)
        })?;
        moved.push(updated);
        Ok(true)
    });

    let moved_to_workshop: Vec<_> = moved
        .iter()
        .filter(|addon| addon.primary_dir == Some(AddonDirType::Workshop))
        .filter_map(|addon| addon.workshop_id)
        .collect();
    Ok(refresh_batch(items, || {
        scan_and_commit(db, config, image_cache_dir, moved, |tx, _| {
            for wid in moved_to_workshop {
                LocalDb::mark_issue_resolved_tx(tx, &IssueId(format!("new_ws_{wid}")))?;
            }
            Ok(())
        })
    }))
}

pub fn toggle_addons<'a>(
    db: &LocalDb,
    config: &AppConfig,
    image_cache_dir: &Path,
    toggles: impl IntoIterator<Item = (&'a AddonId, bool)>,
    allow_workshop_dir: bool,
) -> Result<AddonBatchReport, CoreError> {
    let addons = addons_by_id(db)?;
    let mut changed = Vec::new();
    let items = run_batch(toggles, |id, enabled| {
        let addon = find_addon(&addons, id)?;
        if addon.enabled == enabled {
            return Ok(false);
        }
        if addon.primary_dir == Some(AddonDirType::Workshop) && !allow_workshop_dir {
            return Err(CoreError::Conflict(format!(
                "模组 {id} 位于 workshop/ 目录，禁止直接切换启用状态，请先移至 loading/"
            )));
        }
        let src = addon_path(config, addon)?;
        let name = src
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| CoreError::InvalidInput("无效源文件名".into()))?;
        let dst = if enabled {
            let enabled_name = strip_suffix_ascii_case(name, ".disabled")
                .ok_or_else(|| CoreError::Conflict("文件启用状态已变化，请重新扫描".into()))?;
            src.with_file_name(enabled_name)
        } else {
            append_os_suffix(src, ".disabled")
        };
        require_free(&dst)?;
        let mut updated = addon.clone();
        updated.enabled = enabled;
        updated.current_path = Some(dst.to_string_lossy().into_owned());
        publish_addon_path(db, &updated, src, &dst, None)?;
        changed.push(updated);
        Ok(true)
    });

    Ok(refresh_batch(items, || {
        scan_and_commit(db, config, image_cache_dir, changed, |_, _| Ok(()))
    }))
}

pub fn delete_addons(
    db: &LocalDb,
    config: &AppConfig,
    image_cache_dir: &Path,
    addon_ids: &[AddonId],
    delete_disk_file: bool,
    purge_known_record: bool,
) -> Result<AddonBatchReport, CoreError> {
    let addons = addons_by_id(db)?;
    let items = run_batch(addon_ids.iter().map(|id| (id, ())), |id, ()| {
        let Some(addon) = addons.get(id) else {
            return Ok(false);
        };
        if purge_known_record && !delete_disk_file && addon.install_state == InstallState::Installed
        {
            return Err(CoreError::Conflict(format!("模组 {id} 仍处于安装状态")));
        }
        FileChanges::run(|changes| {
            let mut removed_file = false;
            if delete_disk_file {
                if addon.current_path.is_some() {
                    let path = addon_path(config, addon)?;
                    if occupied(path)? {
                        take_addon_file(changes, addon, path)?;
                        removed_file = true;
                    }
                }
                if let Some(wid) = addon
                    .workshop_id
                    .filter(|_| !config.workshop_dir.is_empty())
                {
                    let dummy_path = Path::new(&config.workshop_dir).join(format!("{wid}.vpk"));
                    if occupied(&dummy_path)?
                        && matches!(inspect_dummy_vpk(&dummy_path), DummyInspection::Dummy(_))
                    {
                        take_matching_dummy(changes, &dummy_path, wid)?;
                        removed_file = true;
                    }
                }
            }
            changes.verify()?;
            if purge_known_record {
                db.delete_addon_record(id)?;
            } else if delete_disk_file && addon.install_state != InstallState::UninstalledKnown {
                db.mark_addon_uninstalled(id)?;
            } else {
                return Ok(removed_file);
            }
            Ok(true)
        })
    });
    Ok(refresh_batch(items, || {
        scan_workspace(db, config, image_cache_dir)
    }))
}

fn addons_by_id(db: &LocalDb) -> Result<HashMap<AddonId, AddonSummary>, CoreError> {
    Ok(db
        .list_addons()?
        .into_iter()
        .map(|addon| (addon.id.clone(), addon))
        .collect())
}

fn find_addon<'a>(
    addons: &'a HashMap<AddonId, AddonSummary>,
    id: &AddonId,
) -> Result<&'a AddonSummary, CoreError> {
    addons
        .get(id)
        .ok_or_else(|| CoreError::NotFound(format!("未找到模组: {id}")))
}

pub(crate) fn addon_path<'a>(
    config: &AppConfig,
    addon: &'a AddonSummary,
) -> Result<&'a Path, CoreError> {
    let path = addon
        .current_path
        .as_deref()
        .map(Path::new)
        .ok_or_else(|| CoreError::InvalidInput(format!("模组未安装: {}", addon.id)))?;
    let directory = match addon.primary_dir {
        Some(AddonDirType::Loading) => &config.loading_dir,
        Some(AddonDirType::Workshop) => &config.workshop_dir,
        None => return Err(CoreError::Conflict("模组目录已变化，请重新扫描".into())),
    };
    if directory.is_empty() || path.parent() != Some(Path::new(directory)) {
        return Err(CoreError::Conflict("模组目录已变化，请重新扫描".into()));
    }
    Ok(path)
}

pub(crate) fn take_addon_file(
    changes: &mut FileChanges,
    addon: &AddonSummary,
    source: &Path,
) -> Result<(PathBuf, String), CoreError> {
    let backup = changes.take(source)?;
    match inspect_dummy_vpk(&backup) {
        DummyInspection::RealVpk {
            content_hash,
            file_size,
            ..
        } if addon.content_hash.as_ref() == Some(&content_hash) && addon.file_size == file_size => {
            Ok((backup, content_hash))
        }
        _ => Err(CoreError::Conflict(format!(
            "源文件已变化，请重新扫描: {}",
            source.display()
        ))),
    }
}

fn publish_addon_path(
    db: &LocalDb,
    addon: &AddonSummary,
    source: &Path,
    target: &Path,
    sibling: Option<&Path>,
) -> Result<(), CoreError> {
    FileChanges::run(|changes| {
        let (backup, _) = take_addon_file(changes, addon, source)?;
        changes.publish_copy(&backup, target)?;
        changes.verify()?;
        if let Some(sibling) = sibling {
            require_free(sibling)?;
        }
        db.upsert_addon(addon)
    })
}

fn require_free(path: &Path) -> Result<(), CoreError> {
    if occupied(path)? {
        return Err(target_conflict(path));
    }
    Ok(())
}

fn target_conflict(path: &Path) -> CoreError {
    CoreError::Conflict(format!("目标文件已存在或重复: {}", path.display()))
}

fn run_batch<'a, T>(
    inputs: impl IntoIterator<Item = (&'a AddonId, T)>,
    mut apply: impl FnMut(&AddonId, T) -> Result<bool, CoreError>,
) -> Vec<AddonOperationResult> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut failed = false;
    for (id, input) in inputs {
        if !seen.insert(id) {
            continue;
        }
        let outcome = if failed {
            AddonOperationOutcome::NotAttempted
        } else {
            match apply(id, input) {
                Ok(true) => AddonOperationOutcome::Applied,
                Ok(false) => AddonOperationOutcome::Unchanged,
                Err(error) => {
                    failed = true;
                    AddonOperationOutcome::Failed {
                        error: error.into(),
                    }
                }
            }
        };
        items.push(AddonOperationResult {
            addon_id: id.clone(),
            outcome,
        });
    }
    items
}

fn refresh_batch<T>(
    items: Vec<AddonOperationResult>,
    refresh: impl FnOnce() -> Result<T, CoreError>,
) -> AddonBatchReport {
    let refresh_error = if items.is_empty() {
        None
    } else {
        refresh().err().map(Into::into)
    };
    AddonBatchReport {
        items,
        refresh_error,
    }
}

pub fn search_by_map_code(db: &LocalDb, query: &str) -> Result<Vec<AddonSummary>, CoreError> {
    let needle = query.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let addons = db.list_addons()?;
    Ok(addons
        .into_iter()
        .filter(|a| {
            a.maps.iter().any(|m| {
                m.code.to_ascii_lowercase().contains(&needle)
                    || m.name.to_ascii_lowercase().contains(&needle)
            })
        })
        .collect())
}
