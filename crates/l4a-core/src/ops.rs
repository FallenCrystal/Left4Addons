use crate::db::LocalDb;
use crate::error::CoreError;
use l4a_protocol::{
    AddonDirType, AddonId, AddonSummary, AppConfig, InstallState, InvalidCharReplace,
    RenamePreviewItem, RenameRuleConfig, WorkshopId,
};
use l4a_vpk::{extract_addon_metadata, generate_dummy_vpk, inspect_dummy_vpk, DummyInspection, DummySpec};
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
    if input.len() >= suffix.len()
        && input[input.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    {
        Some(&input[..input.len() - suffix.len()])
    } else {
        None
    }
}

pub(crate) fn write_dummy_atomically(
    original_vpk_path: &Path,
    dummy_vpk_path: &Path,
    spec: &DummySpec,
) -> Result<(), CoreError> {
    if let Some(parent) = dummy_vpk_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let tmp = append_os_suffix(dummy_vpk_path, ".l4a_tmp");
    if tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }
    if let Err(err) = generate_dummy_vpk(original_vpk_path, &tmp, spec) {
        let _ = fs::remove_file(&tmp);
        return Err(CoreError::Vpk(err));
    }
    let bak = append_os_suffix(dummy_vpk_path, ".l4a_bak");
    if bak.exists() {
        let _ = fs::remove_file(&bak);
    }
    let had_dst = dummy_vpk_path.exists();
    if had_dst {
        fs::rename(dummy_vpk_path, &bak)?;
    }
    if let Err(err) = fs::rename(&tmp, dummy_vpk_path) {
        if had_dst && bak.exists() {
            let _ = fs::rename(&bak, dummy_vpk_path);
        }
        let _ = fs::remove_file(&tmp);
        return Err(err.into());
    }
    if had_dst && bak.exists() {
        let _ = fs::remove_file(&bak);
    }
    Ok(())
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
    rules: &RenameRuleConfig,
    addon_ids: &[AddonId],
) -> Result<Vec<RenamePreviewItem>, CoreError> {
    let addons = db.list_addons()?;
    let groups = db.list_user_groups()?;
    let mut first_group_by_addon: HashMap<AddonId, String> = HashMap::new();
    for g in groups {
        for aid in g.addon_ids {
            first_group_by_addon.entry(aid).or_insert_with(|| g.name.clone());
        }
    }

    let target_set: HashSet<&AddonId> = addon_ids.iter().collect();
    let mut seen_targets: HashMap<(String, String), AddonId> = HashMap::new();
    let mut items = Vec::new();

    for addon in addons {
        if !target_set.is_empty() && !target_set.contains(&addon.id) {
            continue;
        }
        if addon.install_state != InstallState::Installed {
            continue;
        }
        let Some(ref cur_path) = addon.current_path else {
            continue;
        };
        let Some(ref cur_name) = addon.current_file_name else {
            continue;
        };
        let group_prefix = first_group_by_addon.get(&addon.id).map(String::as_str);
        let target_vpk = build_target_vpk_name(addon.workshop_id, group_prefix, &addon.title, rules);
        let target_disk_name = if addon.enabled {
            target_vpk.clone()
        } else {
            format!("{target_vpk}.disabled")
        };

        let dir_key = Path::new(cur_path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut conflict = None;
        if addon.primary_dir == Some(AddonDirType::Workshop) {
            conflict = Some("位于 workshop/ 目录的模组禁止直接重命名，请先移至 loading/".to_string());
        } else if let Some(prev) = seen_targets.get(&(dir_key.clone(), target_disk_name.to_ascii_lowercase())) {
            conflict = Some(format!("与 {prev} 重命名目标冲突"));
        } else {
            seen_targets.insert(
                (dir_key.clone(), target_disk_name.to_ascii_lowercase()),
                addon.id.clone(),
            );
            let dest = Path::new(&dir_key).join(&target_disk_name);
            let sibling_enabled = Path::new(&dir_key).join(&target_vpk);
            let sibling_disabled = append_os_suffix(&sibling_enabled, ".disabled");
            let case_only_self = !cur_name.eq(&target_vpk) && cur_name.eq_ignore_ascii_case(&target_vpk);
            if target_vpk != *cur_name
                && !case_only_self
                && (dest.exists() || sibling_enabled.exists() || sibling_disabled.exists())
            {
                conflict = Some(format!("目标文件已存在: {target_disk_name}"));
            }
        }

        items.push(RenamePreviewItem {
            addon_id: addon.id,
            current_file_name: cur_name.clone(),
            target_file_name: target_vpk,
            conflict,
        });
    }

    Ok(items)
}

pub fn apply_renames(
    db: &LocalDb,
    renames: &[(AddonId, String)],
    rules: &RenameRuleConfig,
) -> Result<usize, CoreError> {
    let addons: HashMap<AddonId, AddonSummary> = db
        .list_addons()?
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect();

    struct Plan {
        addon: AddonSummary,
        src: PathBuf,
        dst: PathBuf,
        sanitized_vpk: String,
        case_only_rename: bool,
    }

    let mut plans = Vec::new();
    let mut reserved_dests = HashSet::new();

    for (aid, raw_name) in renames {
        let Some(addon) = addons.get(aid) else {
            return Err(CoreError::NotFound(format!("未找到 Addon: {aid}")));
        };
        if addon.primary_dir == Some(AddonDirType::Workshop) {
            return Err(CoreError::Conflict(format!(
                "模组 {aid} 位于 workshop/ 目录，禁止直接重命名，请先移至 loading/"
            )));
        }
        let Some(ref cur_path) = addon.current_path else {
            return Err(CoreError::InvalidInput(format!("Addon 未安装: {aid}")));
        };
        let sanitized_vpk = sanitize_vpk_filename(raw_name, rules);
        let src = PathBuf::from(cur_path);
        let dir = src
            .parent()
            .ok_or_else(|| CoreError::InvalidInput("无效源路径".into()))?;
        let disk_name = if addon.enabled {
            sanitized_vpk.clone()
        } else {
            format!("{sanitized_vpk}.disabled")
        };
        let src_name = src.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if src_name == disk_name {
            continue;
        }
        let dst = dir.join(&disk_name);
        let case_only_rename = src_name.eq_ignore_ascii_case(&disk_name);
        let sibling_enabled = dir.join(&sanitized_vpk);
        let sibling_disabled = append_os_suffix(&sibling_enabled, ".disabled");

        if !reserved_dests.insert(dst.to_string_lossy().to_ascii_lowercase())
            || (!case_only_rename
                && (dst.exists() || sibling_enabled.exists() || sibling_disabled.exists()))
        {
            return Err(CoreError::Conflict(format!(
                "目标文件已存在或重复: {}",
                dst.display()
            )));
        }
        plans.push(Plan {
            addon: addon.clone(),
            src,
            dst,
            sanitized_vpk,
            case_only_rename,
        });
    }

    let count = plans.len();
    for plan in plans {
        if plan.case_only_rename {
            let tmp = append_os_suffix(&plan.src, ".l4a_case_tmp");
            fs::rename(&plan.src, &tmp)?;
            if let Err(err) = fs::rename(&tmp, &plan.dst) {
                let _ = fs::rename(&tmp, &plan.src);
                return Err(err.into());
            }
        } else {
            fs::rename(&plan.src, &plan.dst)?;
        }
        let mut updated = plan.addon;
        updated.current_file_name = Some(plan.sanitized_vpk);
        updated.current_path = Some(plan.dst.to_string_lossy().into_owned());
        db.upsert_addon(&updated)?;
    }

    Ok(count)
}

pub fn move_addons(
    db: &LocalDb,
    config: &AppConfig,
    image_cache_dir: &Path,
    addon_ids: &[AddonId],
    target_dir_type: AddonDirType,
) -> Result<usize, CoreError> {
    let target_dir = match target_dir_type {
        AddonDirType::Loading => Path::new(&config.loading_dir),
        AddonDirType::Workshop => Path::new(&config.workshop_dir),
    };
    if target_dir.as_os_str().is_empty() {
        return Err(CoreError::InvalidInput("目标模组目录未配置".into()));
    }
    fs::create_dir_all(target_dir)?;

    let addons: HashMap<AddonId, AddonSummary> = db
        .list_addons()?
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect();

    let mut moved = 0usize;
    for aid in addon_ids {
        let addon = addons
            .get(aid)
            .ok_or_else(|| CoreError::NotFound(format!("未找到 Addon: {aid}")))?;
        if addon.primary_dir == Some(target_dir_type) {
            continue;
        }
        let Some(ref cur_path) = addon.current_path else {
            return Err(CoreError::InvalidInput(format!("Addon 未在磁盘安装: {aid}")));
        };
        let src = PathBuf::from(cur_path);
        let orig_name = src
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| CoreError::InvalidInput("无效源文件名".into()))?;

        let (dst, next_enabled, next_vpk_name) = match target_dir_type {
            AddonDirType::Workshop => {
                let target_vpk = match addon.workshop_id {
                    Some(wid) => format!("{wid}.vpk"),
                    None => strip_suffix_ascii_case(orig_name, ".disabled")
                        .unwrap_or(orig_name)
                        .to_string(),
                };
                let dst = target_dir.join(&target_vpk);
                let disabled_sibling = append_os_suffix(&dst, ".disabled");
                if disabled_sibling.exists() {
                    return Err(CoreError::Conflict(format!(
                        "目标目录已存在禁用态冲突文件: {}",
                        disabled_sibling.display()
                    )));
                }
                (dst, true, target_vpk)
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
                if sibling.exists() {
                    return Err(CoreError::Conflict(format!(
                        "目标目录已存在同名启/禁用文件: {}",
                        sibling.display()
                    )));
                }
                (dst, addon.enabled, clean_vpk)
            }
        };

        if dst.exists() {
            let removable_dummy = target_dir_type == AddonDirType::Workshop
                && matches!(inspect_dummy_vpk(&dst), DummyInspection::Dummy(_));
            if removable_dummy {
                fs::remove_file(&dst)?;
            } else {
                return Err(CoreError::Conflict(format!(
                    "目标文件已存在: {}",
                    dst.display()
                )));
            }
        }

        if fs::rename(&src, &dst).is_err() {
            let tmp = append_os_suffix(&dst, ".l4a_tmp");
            fs::copy(&src, &tmp)?;
            if let Err(err) = fs::rename(&tmp, &dst) {
                let _ = fs::remove_file(&tmp);
                return Err(err.into());
            }
            fs::remove_file(&src)?;
        }

        let mut has_dummy = false;
        if target_dir_type == AddonDirType::Loading && config.enable_dummy_bypass {
            if let Some(wid) = addon.workshop_id {
                if config.workshop_dir.trim().is_empty() {
                    return Err(CoreError::InvalidInput("已启用 Dummy Bypass，但未配置 workshop_dir".into()));
                }
                let dummy_path = Path::new(&config.workshop_dir).join(format!("{wid}.vpk"));
                let content_hash = addon.content_hash.clone().unwrap_or_else(|| {
                    extract_addon_metadata(&dst, image_cache_dir).content_hash
                });
                let size = fs::metadata(&dst)?.len();
                let ts = db
                    .get_workshop_snapshot(wid)?
                    .and_then(|s| s.time_updated);
                write_dummy_atomically(
                    &dst,
                    &dummy_path,
                    &DummySpec {
                        workshop_id: wid,
                        original_hash: content_hash,
                        original_size: size,
                        original_time_updated: ts,
                    },
                )?;
                has_dummy = true;
            }
        }

        let mut updated = addon.clone();
        updated.primary_dir = Some(target_dir_type);
        updated.enabled = next_enabled;
        updated.current_file_name = Some(next_vpk_name);
        updated.current_path = Some(dst.to_string_lossy().into_owned());
        updated.has_dummy_bypass = has_dummy;
        db.upsert_addon(&updated)?;
        moved += 1;
    }

    Ok(moved)
}

pub fn toggle_addons(
    db: &LocalDb,
    _config: &AppConfig,
    _image_cache_dir: &Path,
    addon_ids: &[AddonId],
    enabled: bool,
    allow_workshop_dir: bool,
) -> Result<usize, CoreError> {
    let addons: HashMap<AddonId, AddonSummary> = db
        .list_addons()?
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect();

    let mut changed = 0usize;
    for aid in addon_ids {
        let addon = addons
            .get(aid)
            .ok_or_else(|| CoreError::NotFound(format!("未找到 Addon: {aid}")))?;
        if addon.enabled == enabled {
            continue;
        }
        if addon.primary_dir == Some(AddonDirType::Workshop) && !allow_workshop_dir {
            return Err(CoreError::Conflict(format!(
                "模组 {aid} 位于 workshop/ 目录，禁止直接禁用以免触发 Steam 重复下载，请先移至 loading/"
            )));
        }
        let Some(ref cur_path) = addon.current_path else {
            continue;
        };
        let Some(ref vpk_name) = addon.current_file_name else {
            continue;
        };
        let src = PathBuf::from(cur_path);
        let parent = src
            .parent()
            .ok_or_else(|| CoreError::InvalidInput("无效源目录".into()))?;
        let dst = if enabled {
            parent.join(vpk_name)
        } else {
            parent.join(format!("{vpk_name}.disabled"))
        };
        if dst.exists() {
            return Err(CoreError::Conflict(format!(
                "目标文件已存在: {}",
                dst.display()
            )));
        }
        fs::rename(&src, &dst)?;
        let mut updated = addon.clone();
        updated.enabled = enabled;
        updated.current_path = Some(dst.to_string_lossy().into_owned());
        db.upsert_addon(&updated)?;
        changed += 1;
    }

    Ok(changed)
}

pub fn delete_addons(
    db: &LocalDb,
    config: &AppConfig,
    addon_ids: &[AddonId],
    delete_disk_file: bool,
    purge_known_record: bool,
) -> Result<usize, CoreError> {
    let addons: HashMap<AddonId, AddonSummary> = db
        .list_addons()?
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect();

    let mut deleted = 0usize;
    for aid in addon_ids {
        let Some(addon) = addons.get(aid) else {
            continue;
        };
        if delete_disk_file {
            if let Some(ref cur_path) = addon.current_path {
                let p = Path::new(cur_path);
                if p.exists() {
                    fs::remove_file(p)?;
                }
            }
            if let Some(wid) = addon.workshop_id {
                let dummy_path = Path::new(&config.workshop_dir).join(format!("{wid}.vpk"));
                if dummy_path.exists()
                    && matches!(inspect_dummy_vpk(&dummy_path), DummyInspection::Dummy(_))
                {
                    let _ = fs::remove_file(&dummy_path);
                }
            }
        }
        if purge_known_record {
            db.delete_addon_record(aid)?;
        } else if delete_disk_file {
            db.mark_addon_uninstalled(aid)?;
        }
        deleted += 1;
    }
    Ok(deleted)
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
