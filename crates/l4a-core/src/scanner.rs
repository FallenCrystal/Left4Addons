use crate::db::repo::{derive_categories_from_info, PhysicalFileRow};
use crate::db::LocalDb;
use crate::error::CoreError;
use crate::ops::strip_suffix_ascii_case;
use l4a_protocol::{
    AddonDirType, AddonId, AddonSummary, AppConfig, FileRole, InstallState, PhysicalFileRecord,
    WorkshopId,
};
use l4a_vpk::{extract_addon_metadata, inspect_dummy_vpk, DummyInspection};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub files_scanned: usize,
    pub real_addons_active: usize,
    pub dummy_files_found: usize,
}

/// 扫描 `loading_dir` 与 `workshop_dir` 下的全部 `.vpk` / `.vpk.disabled` 文件。
///
/// 关键改进：不再跳过 Dummy 文件，而是记录全部物理文件状态进入 `physical_files`，
/// 为 Dummy Bypass v2 对账引擎提供完整输入。
pub fn scan_workspace(
    db: &LocalDb,
    config: &AppConfig,
    image_cache_dir: &Path,
) -> Result<ScanResult, CoreError> {
    scan_and_commit(db, config, image_cache_dir, None, |_, _| Ok(())).map(|(scan, ())| scan)
}

pub(crate) fn scan_and_commit<T>(
    db: &LocalDb,
    config: &AppConfig,
    image_cache_dir: &Path,
    seeds: impl IntoIterator<Item = AddonSummary>,
    finish: impl FnOnce(&rusqlite::Connection, &HashMap<AddonId, AddonSummary>) -> Result<T, CoreError>,
) -> Result<(ScanResult, T), CoreError> {
    let loading_dir = Path::new(&config.loading_dir);
    let workshop_dir = Path::new(&config.workshop_dir);

    if !config.loading_dir.is_empty() {
        fs::create_dir_all(loading_dir)?;
    }
    if !config.workshop_dir.is_empty() {
        fs::create_dir_all(workshop_dir)?;
    }
    fs::create_dir_all(image_cache_dir)?;

    let existing_files: HashMap<String, PhysicalFileRow> = db
        .list_physical_files()?
        .into_iter()
        .map(|row| (row.record.path.clone(), row))
        .collect();
    let mut existing_addons: HashMap<AddonId, AddonSummary> = db
        .list_addons()?
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect();

    let mut seeds_by_path = HashMap::new();
    for addon in seeds {
        if let Some(path) = &addon.current_path {
            seeds_by_path.insert(path.clone(), addon.clone());
        }
        existing_addons.insert(addon.id.clone(), addon);
    }
    let addons_by_path: HashMap<_, _> = existing_addons
        .values()
        .filter_map(|addon| addon.current_path.as_deref().map(|path| (path, addon)))
        .collect();

    let mut disk_entries: Vec<(PathBuf, AddonDirType, String, bool, u64, i64)> = Vec::new();
    for (dir, dir_type) in [
        (loading_dir, AddonDirType::Loading),
        (workshop_dir, AddonDirType::Workshop),
    ] {
        if dir.as_os_str().is_empty() || !dir.is_dir() {
            continue;
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            if !ft.is_file() {
                continue;
            }
            let path = entry.path();
            let Some(raw_name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let lower = raw_name.to_ascii_lowercase();
            let (enabled, clean_vpk_name) = if lower.ends_with(".vpk") {
                (true, raw_name.to_string())
            } else if lower.ends_with(".vpk.disabled") {
                let stripped = &raw_name[..raw_name.len() - ".disabled".len()];
                (false, stripped.to_string())
            } else {
                continue;
            };

            let meta = entry.metadata()?;
            let file_size = meta.len();
            let modified_at = modified_at(&meta);

            disk_entries.push((
                path,
                dir_type,
                clean_vpk_name,
                enabled,
                file_size,
                modified_at,
            ));
        }
    }

    disk_entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let mut physical_rows = Vec::with_capacity(disk_entries.len());
    let mut real_addons_by_id: HashMap<AddonId, AddonSummary> = HashMap::new();
    let mut dummy_workshop_ids: HashSet<WorkshopId> = HashSet::new();

    for (path, dir_type, vpk_name, enabled, file_size, modified_at) in disk_entries {
        let path_str = path.to_string_lossy().into_owned();
        let prev = existing_files.get(&path_str);
        let seed = seeds_by_path.get(&path_str);
        // A file operation can commit the new path even when the inventory refresh fails.
        let known_addon = seed.or_else(|| addons_by_path.get(path_str.as_str()).copied());
        let unchanged = seed.is_none()
            && prev.is_some_and(|p| {
                p.record.file_size == file_size
                    && p.record.modified_at == modified_at
                    && p.record.enabled == enabled
                    && p.record.dir_type == dir_type
            });

        let (role, workshop_id, content_hash, dummy_original_hash, dummy_original_ts, meta_opt) =
            if unchanged {
                let p = prev.unwrap();
                let cached_addon = p.addon_id.as_ref().and_then(|aid| existing_addons.get(aid));
                let need_reparse = matches!(p.record.role, FileRole::RealAddon)
                    && (cached_addon.is_none()
                        || cached_addon.is_some_and(|a| {
                            a.cover_image.as_ref().is_some_and(|c| {
                                !c.starts_with("http") && !image_cache_dir.join(c).exists()
                            })
                        }));
                if !need_reparse {
                    (
                        p.record.role,
                        p.record.workshop_id,
                        p.record.content_hash.clone(),
                        p.record.dummy_original_hash.clone(),
                        p.record.dummy_original_time_updated,
                        None,
                    )
                } else {
                    inspect_disk_vpk(
                        &path,
                        &vpk_name,
                        image_cache_dir,
                        db,
                        &existing_addons,
                        &existing_files,
                        known_addon,
                    )?
                }
            } else {
                inspect_disk_vpk(
                    &path,
                    &vpk_name,
                    image_cache_dir,
                    db,
                    &existing_addons,
                    &existing_files,
                    known_addon,
                )?
            };

        if matches!(role, FileRole::DummyV1 | FileRole::DummyV2) {
            if let Some(wid) = workshop_id.or_else(|| extract_workshop_id_from_filename(&vpk_name))
            {
                dummy_workshop_ids.insert(wid);
            }
            physical_rows.push(PhysicalFileRow {
                record: PhysicalFileRecord {
                    path: path_str,
                    dir_type,
                    file_name: vpk_name,
                    enabled,
                    role,
                    workshop_id: workshop_id
                        .or_else(|| extract_workshop_id_from_filename(&raw_name_fallback(&path))),
                    content_hash,
                    dummy_original_hash,
                    dummy_original_time_updated: dummy_original_ts,
                    file_size,
                    modified_at,
                },
                addon_id: None,
            });
            continue;
        }

        if role == FileRole::Corrupted {
            physical_rows.push(PhysicalFileRow {
                record: PhysicalFileRecord {
                    path: path_str,
                    dir_type,
                    file_name: vpk_name,
                    enabled,
                    role: FileRole::Corrupted,
                    workshop_id: workshop_id
                        .or_else(|| extract_workshop_id_from_filename(&raw_name_fallback(&path))),
                    content_hash: None,
                    dummy_original_hash: None,
                    dummy_original_time_updated: None,
                    file_size,
                    modified_at,
                },
                addon_id: None,
            });
            continue;
        }

        let resolved_addon = if let Some(meta) = meta_opt {
            build_addon_summary(
                workshop_id,
                &vpk_name,
                &path_str,
                dir_type,
                enabled,
                file_size,
                meta,
                known_addon,
                &existing_addons,
            )
        } else if let Some(cached) = prev
            .and_then(|p| p.addon_id.as_ref())
            .and_then(|aid| existing_addons.get(aid))
            .filter(|cached| {
                cached.current_path.as_deref() == Some(path_str.as_str())
                    && cached.content_hash == content_hash
            })
            .cloned()
        {
            let mut next = cached;
            next.install_state = InstallState::Installed;
            next.primary_dir = Some(dir_type);
            next.enabled = enabled;
            next.current_file_name = Some(vpk_name.clone());
            next.current_path = Some(path_str.clone());
            next.file_size = file_size;
            next
        } else {
            let meta = extract_addon_metadata(&path, image_cache_dir);
            build_addon_summary(
                workshop_id,
                &vpk_name,
                &path_str,
                dir_type,
                enabled,
                file_size,
                meta,
                known_addon,
                &existing_addons,
            )
        };

        let aid = resolved_addon.id.clone();
        physical_rows.push(PhysicalFileRow {
            record: PhysicalFileRecord {
                path: path_str,
                dir_type,
                file_name: vpk_name,
                enabled,
                role,
                workshop_id: resolved_addon.workshop_id,
                content_hash: content_hash.or_else(|| resolved_addon.content_hash.clone()),
                dummy_original_hash: None,
                dummy_original_time_updated: None,
                file_size,
                modified_at,
            },
            addon_id: Some(aid.clone()),
        });

        // 当同一逻辑 Addon 在 loading 和 workshop 都存在物理文件时，主视图优先展示 loading/ 托管版本。
        match real_addons_by_id.get(&aid) {
            Some(existing) if existing.primary_dir == Some(AddonDirType::Loading) => {}
            _ => {
                real_addons_by_id.insert(aid, resolved_addon);
            }
        }
    }

    for addon in real_addons_by_id.values_mut() {
        addon.has_dummy_bypass = addon
            .workshop_id
            .is_some_and(|w| dummy_workshop_ids.contains(&w));
    }

    let dummy_files_found = physical_rows
        .iter()
        .filter(|r| matches!(r.record.role, FileRole::DummyV1 | FileRole::DummyV2))
        .count();
    let issues = crate::reconcile::collect_issues(&physical_rows, config);
    let value = db.write(|tx| {
        for addon in real_addons_by_id.values() {
            LocalDb::upsert_addon_tx(tx, addon)?;
        }
        for old_id in existing_addons.keys() {
            if !real_addons_by_id.contains_key(old_id) {
                LocalDb::mark_addon_uninstalled_tx(tx, old_id)?;
            }
        }
        LocalDb::replace_physical_files_tx(tx, &physical_rows)?;
        LocalDb::replace_open_reconcile_issues_tx(tx, &issues)?;
        finish(tx, &real_addons_by_id)
    })?;

    Ok((
        ScanResult {
            files_scanned: physical_rows.len(),
            real_addons_active: real_addons_by_id.len(),
            dummy_files_found,
        },
        value,
    ))
}

pub(crate) fn modified_at(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i64)
        .unwrap_or(0)
}

fn raw_name_fallback(path: &Path) -> String {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    strip_suffix_ascii_case(name, ".disabled")
        .unwrap_or(name)
        .to_string()
}

#[allow(clippy::type_complexity)]
fn inspect_disk_vpk(
    path: &Path,
    vpk_name: &str,
    image_cache_dir: &Path,
    db: &LocalDb,
    existing_addons: &HashMap<AddonId, AddonSummary>,
    existing_files: &HashMap<String, PhysicalFileRow>,
    known_addon: Option<&AddonSummary>,
) -> Result<
    (
        FileRole,
        Option<WorkshopId>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<l4a_vpk::AddonMetadata>,
    ),
    CoreError,
> {
    match inspect_dummy_vpk(path) {
        DummyInspection::Dummy(meta) => {
            let role = if meta.schema_version >= 2 {
                FileRole::DummyV2
            } else {
                FileRole::DummyV1
            };
            let wid = meta
                .workshop_id
                .or_else(|| extract_workshop_id_from_filename(vpk_name));
            Ok((
                role,
                wid,
                None,
                meta.original_hash,
                meta.original_time_updated,
                None,
            ))
        }
        DummyInspection::Corrupted(_) => Ok((FileRole::Corrupted, None, None, None, None, None)),
        DummyInspection::RealVpk { content_hash, .. } => {
            let meta = extract_addon_metadata(path, image_cache_dir);
            let prev_addon_wid = existing_files
                .get(&path.to_string_lossy().into_owned())
                .and_then(|p| p.record.workshop_id);
            let real_id = if let Some(addon) =
                known_addon.filter(|addon| addon.content_hash.as_ref() == Some(&content_hash))
            {
                addon.workshop_id
            } else {
                resolve_workshop_id(
                    vpk_name,
                    &meta.addon_info,
                    &content_hash,
                    prev_addon_wid,
                    db,
                    existing_addons,
                )
            };
            Ok((
                FileRole::RealAddon,
                real_id,
                Some(content_hash),
                None,
                None,
                Some(meta),
            ))
        }
    }
}

fn resolve_workshop_id(
    vpk_name: &str,
    addon_info: &BTreeMap<String, String>,
    content_hash: &str,
    prev_path_wid: Option<WorkshopId>,
    db: &LocalDb,
    existing_addons: &HashMap<AddonId, AddonSummary>,
) -> Option<WorkshopId> {
    if let Some(wid) = extract_workshop_id_from_filename(vpk_name) {
        return Some(wid);
    }
    if let Some(url) = addon_info
        .get("addonurl0")
        .or_else(|| addon_info.get("addonurl"))
    {
        if let Some(wid) = extract_workshop_id_from_url(url) {
            return Some(wid);
        }
    }
    if let Some(found) = existing_addons
        .values()
        .find(|a| a.content_hash.as_deref() == Some(content_hash) && a.workshop_id.is_some())
        .and_then(|a| a.workshop_id)
    {
        return Some(found);
    }
    if let Some(wid) = db
        .lookup_addon_workshop_id_by_hash(content_hash)
        .ok()
        .flatten()
    {
        return Some(wid);
    }
    prev_path_wid
}

#[allow(clippy::too_many_arguments)]
fn build_addon_summary(
    workshop_id: Option<WorkshopId>,
    vpk_name: &str,
    path_str: &str,
    dir_type: AddonDirType,
    enabled: bool,
    file_size: u64,
    meta: l4a_vpk::AddonMetadata,
    known_addon: Option<&AddonSummary>,
    existing_addons: &HashMap<AddonId, AddonSummary>,
) -> AddonSummary {
    let id = known_addon
        .filter(|addon| {
            addon.workshop_id == workshop_id
                && addon.content_hash.as_deref() == Some(meta.content_hash.as_str())
        })
        .map(|addon| addon.id.clone())
        .unwrap_or_else(|| match workshop_id {
            Some(wid) => AddonId::from_workshop_id(wid),
            None => AddonId::from_content_hash(&meta.content_hash),
        });
    let prev = existing_addons.get(&id);
    let title = meta
        .addon_info
        .get("addontitle")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| prev.map(|p| p.title.clone()))
        .unwrap_or_else(|| {
            strip_suffix_ascii_case(vpk_name, ".vpk")
                .unwrap_or(vpk_name)
                .to_string()
        });
    let author = meta
        .addon_info
        .get("addonauthor")
        .or_else(|| meta.addon_info.get("author"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| prev.and_then(|p| p.author.clone()));
    let version = meta
        .addon_info
        .get("addonversion")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let categories = derive_categories_from_info(&meta.addon_info);

    AddonSummary {
        id,
        workshop_id,
        title,
        author,
        version,
        install_state: InstallState::Installed,
        primary_dir: Some(dir_type),
        enabled,
        current_file_name: Some(vpk_name.to_string()),
        current_path: Some(path_str.to_string()),
        content_hash: Some(meta.content_hash),
        file_size,
        files_count: meta.files_count,
        cover_image: meta
            .image_path
            .or_else(|| prev.and_then(|p| p.cover_image.clone())),
        categories,
        maps: meta.maps,
        has_dummy_bypass: false,
        remote_status: prev
            .map(|p| p.remote_status)
            .unwrap_or(l4a_protocol::WorkshopItemStatus::Unknown),
        local_time_updated: prev.and_then(|p| p.local_time_updated),
        remote_time_updated: prev.and_then(|p| p.remote_time_updated),
        update_available: prev.is_some_and(|p| p.update_available),
        addon_info: meta.addon_info,
    }
}

pub fn extract_workshop_id_from_filename(vpk_name: &str) -> Option<WorkshopId> {
    let base = strip_suffix_ascii_case(vpk_name, ".disabled").unwrap_or(vpk_name);
    let base = strip_suffix_ascii_case(base, ".vpk").unwrap_or(base);

    if base.chars().all(|c| c.is_ascii_digit()) && !base.is_empty() {
        return WorkshopId::from_str(base).ok();
    }
    if let Some(rest) = base.strip_prefix('[') {
        if let Some((digits, _)) = rest.split_once(']') {
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                return WorkshopId::from_str(digits).ok();
            }
        }
    }
    let digits: String = base.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 6 {
        return WorkshopId::from_str(&digits).ok();
    }
    None
}

pub fn extract_workshop_id_from_url(url_str: &str) -> Option<WorkshopId> {
    let trimmed = url_str.trim();
    if let Ok(parsed) = url::Url::parse(trimmed) {
        for (k, v) in parsed.query_pairs() {
            if k.eq_ignore_ascii_case("id") {
                let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    return WorkshopId::from_str(&digits).ok();
                }
            }
        }
        return None;
    }

    for marker in ["?id=", "&id="] {
        if let Some(idx) = trimmed.find(marker) {
            let rest = &trimmed[idx + marker.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                return WorkshopId::from_str(&digits).ok();
            }
        }
    }
    None
}
