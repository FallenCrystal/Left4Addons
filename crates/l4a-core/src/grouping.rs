use crate::db::LocalDb;
use crate::error::CoreError;
use crate::ops::toggle_addons;
use chrono::Utc;
use l4a_protocol::{
    AddonDirType, AddonId, AddonSummary, BundleId, BundleKind, BundleRecord, GroupId,
    InstallState, LoadoutProfile, MapEntry, ProfileApplyReport, ProfileId,
    TrackedCollectionRecord, UserGroup, WorkshopId,
};
use l4a_workshop::WorkshopResolver;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// 按四层清晰模型管理：
/// 1. `Bundle`: 物理多包整合（支持 `locked` 保护与跨分包地图/封面聚合）
/// 2. `TrackedCollection`: 上游工坊合集追踪
/// 3. `UserGroup`: 多对多自定义分类/标签
/// 4. `LoadoutProfile`: 一键启用预设
pub fn rebuild_auto_bundles(db: &LocalDb) -> Result<Vec<BundleRecord>, CoreError> {
    let existing = db.list_bundles()?;
    let mut locked_member_ids: HashSet<AddonId> = HashSet::new();
    for b in &existing {
        if b.locked || b.kind == BundleKind::Manual {
            for aid in &b.member_addon_ids {
                locked_member_ids.insert(aid.clone());
            }
        }
    }

    db.delete_unlocked_auto_bundles()?;

    let addons = db.list_addons()?;
    let addon_map: HashMap<AddonId, AddonSummary> = addons
        .iter()
        .map(|a| (a.id.clone(), a.clone()))
        .collect();
    let wid_to_addon: HashMap<WorkshopId, AddonSummary> = addons
        .iter()
        .filter_map(|a| a.workshop_id.map(|w| (w, a.clone())))
        .collect();

    let mut assigned: HashSet<AddonId> = locked_member_ids;

    // 第一阶段：基于已安装 Workshop 依赖图聚合（主模组 + 本地已下载的依赖分包）
    let edges = db.list_all_active_dependency_edges()?;
    let mut deps_by_parent: HashMap<WorkshopId, Vec<WorkshopId>> = HashMap::new();
    for (p, d) in edges {
        deps_by_parent.entry(p).or_default().push(d);
    }

    let mut dep_parents: Vec<_> = deps_by_parent.into_iter().collect();
    dep_parents.sort_by_key(|(p, _)| *p);

    for (parent_wid, dep_wids) in dep_parents {
        let Some(parent_addon) = wid_to_addon.get(&parent_wid) else {
            continue;
        };
        if parent_addon.install_state != InstallState::Installed
            || assigned.contains(&parent_addon.id)
        {
            continue;
        }
        let mut members = vec![parent_addon.id.clone()];
        for dw in dep_wids {
            if let Some(dep_addon) = wid_to_addon.get(&dw) {
                if dep_addon.install_state == InstallState::Installed
                    && !assigned.contains(&dep_addon.id)
                    && !members.contains(&dep_addon.id)
                {
                    members.push(dep_addon.id.clone());
                }
            }
        }
        if members.len() >= 2 {
            for aid in &members {
                assigned.insert(aid.clone());
            }
            let bundle = assemble_bundle(
                BundleId(format!("bundle_dep_{parent_wid}")),
                parent_addon.title.clone(),
                BundleKind::DependencyGraph,
                false,
                members,
                &addon_map,
            );
            db.save_bundle(&bundle)?;
        }
    }

    // 第二阶段：战役 / 地图分包启发式聚合（同作者 + `Part N` / `Textures` 后缀，永不触碰已锁定的 Bundle）
    let re_part = Regex::new(
        r"(?i)^(.*?)\s*(?:[-#_/:,\s]*(?:part|pt|partie|pts|pack|vol|volume|chapter|ch)\s*(?:\d+|[ivxldcm]+)(?:/\d+)?|\s+textures?)$",
    )
    .unwrap();

    let mut by_key: HashMap<(String, String), Vec<AddonSummary>> = HashMap::new();
    for addon in &addons {
        if addon.install_state != InstallState::Installed || assigned.contains(&addon.id) {
            continue;
        }
        if !is_campaign_or_map(addon) {
            continue;
        }
        if let Some(caps) = re_part.captures(&addon.title) {
            let root = clean_root_title(caps.get(1).map_or("", |m| m.as_str()));
            if root.chars().count() < 3 {
                continue;
            }
            let author_key = addon
                .author
                .as_deref()
                .map(|s| s.trim().to_ascii_lowercase())
                .unwrap_or_default();
            by_key
                .entry((root.to_ascii_lowercase(), author_key))
                .or_default()
                .push(addon.clone());
        }
    }

    let mut heuristic_groups: Vec<_> = by_key.into_iter().collect();
    heuristic_groups.sort_by(|a, b| a.0.cmp(&b.0));

    for ((root_lower, author_key), mut parts) in heuristic_groups {
        if parts.len() < 2 {
            continue;
        }
        parts.sort_by(|a, b| a.title.cmp(&b.title));
        let display_name = clean_root_title(
            re_part
                .captures(&parts[0].title)
                .and_then(|c| c.get(1))
                .map_or(&parts[0].title, |m| m.as_str()),
        );
        let member_ids: Vec<AddonId> = parts.into_iter().map(|a| a.id).collect();
        for aid in &member_ids {
            assigned.insert(aid.clone());
        }
        let digest = hex::encode(Sha256::digest(
            format!("{root_lower}|{author_key}").as_bytes(),
        ));
        let bundle = assemble_bundle(
            BundleId(format!("bundle_camp_{}", &digest[..12])),
            display_name,
            BundleKind::CampaignParts,
            false,
            member_ids,
            &addon_map,
        );
        db.save_bundle(&bundle)?;
    }

    // 对所有已有手动/锁定 Bundle 也刷新其聚合地图与封面
    for mut b in db.list_bundles()? {
        if b.locked || b.kind == BundleKind::Manual {
            b = assemble_bundle(
                b.id,
                b.name,
                b.kind,
                b.locked,
                b.member_addon_ids,
                &addon_map,
            );
            db.save_bundle(&b)?;
        }
    }

    db.list_bundles()
}

pub fn create_manual_bundle(
    db: &LocalDb,
    name: String,
    member_addon_ids: Vec<AddonId>,
) -> Result<BundleRecord, CoreError> {
    let addons: HashMap<AddonId, AddonSummary> = db
        .list_addons()?
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect();
    let bundle = assemble_bundle(
        BundleId::generate(),
        name,
        BundleKind::Manual,
        true,
        member_addon_ids,
        &addons,
    );
    db.save_bundle(&bundle)?;
    Ok(bundle)
}

pub fn set_bundle_locked(db: &LocalDb, id: &BundleId, locked: bool) -> Result<(), CoreError> {
    db.set_bundle_locked(id, locked)
}

fn assemble_bundle(
    id: BundleId,
    name: String,
    kind: BundleKind,
    locked: bool,
    member_addon_ids: Vec<AddonId>,
    addon_map: &HashMap<AddonId, AddonSummary>,
) -> BundleRecord {
    let mut aggregated_maps: Vec<MapEntry> = Vec::new();
    let mut seen_codes: HashSet<String> = HashSet::new();
    let mut cover_image: Option<String> = None;

    for aid in &member_addon_ids {
        let Some(addon) = addon_map.get(aid) else {
            continue;
        };
        if cover_image.is_none() && addon.cover_image.is_some() {
            cover_image = addon.cover_image.clone();
        }
        for m in &addon.maps {
            let key = m.code.to_ascii_lowercase();
            if seen_codes.insert(key.clone()) {
                aggregated_maps.push(m.clone());
            } else if let Some(existing) = aggregated_maps
                .iter_mut()
                .find(|x| x.code.eq_ignore_ascii_case(&key))
            {
                if existing.image_path.is_none() && m.image_path.is_some() {
                    existing.image_path = m.image_path.clone();
                }
                if existing.name == existing.code && m.name != m.code {
                    existing.name = m.name.clone();
                }
            }
        }
    }

    BundleRecord {
        id,
        name,
        kind,
        locked,
        member_addon_ids,
        maps: aggregated_maps,
        cover_image,
    }
}

fn is_campaign_or_map(addon: &AddonSummary) -> bool {
    addon
        .categories
        .iter()
        .any(|c| c == "Campaign" || c == "Map")
}

fn clean_root_title(raw: &str) -> String {
    raw.trim()
        .trim_end_matches(|c: char| {
            c.is_whitespace() || matches!(c, ':' | '_' | '-' | '/' | '\\' | '#' | '+' | ',' | '.')
        })
        .trim()
        .to_string()
}

pub async fn track_workshop_collection(
    db: &LocalDb,
    resolver: &WorkshopResolver,
    collection_id: WorkshopId,
    auto_sync: bool,
) -> Result<TrackedCollectionRecord, CoreError> {
    let (remote, _warnings) = resolver.fetch_collection(collection_id).await?;
    for item in &remote.items {
        db.upsert_workshop_snapshot(item, false)?;
    }
    let record = TrackedCollectionRecord {
        collection_id,
        title: remote
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| format!("Collection #{collection_id}")),
        description: remote.description,
        auto_sync,
        member_workshop_ids: remote.child_ids,
        removed_upstream_ids: Vec::new(),
        last_synced_at: Some(Utc::now()),
    };
    db.save_tracked_collection(&record)?;
    Ok(record)
}

pub fn untrack_workshop_collection(db: &LocalDb, collection_id: WorkshopId) -> Result<(), CoreError> {
    db.delete_tracked_collection(collection_id)
}

pub fn save_user_group(db: &LocalDb, group: UserGroup) -> Result<UserGroup, CoreError> {
    db.save_user_group(&group)?;
    Ok(group)
}

pub fn delete_user_group(db: &LocalDb, id: &GroupId) -> Result<(), CoreError> {
    db.delete_user_group(id)
}

pub fn save_profile(db: &LocalDb, profile: LoadoutProfile) -> Result<LoadoutProfile, CoreError> {
    db.save_profile(&profile)?;
    Ok(profile)
}

pub fn delete_profile(db: &LocalDb, id: &ProfileId) -> Result<(), CoreError> {
    db.delete_profile(id)
}

/// 一键应用预设方案：只修改托管在 `loading/` 目录下的模组启用状态；
/// 仅在 `workshop/` 目录的模组不会被直接重命名为 `.disabled` 以防触发 Steam 重新下载。
pub fn apply_profile(
    db: &LocalDb,
    profile_id: &ProfileId,
    image_cache_dir: &Path,
    config: &l4a_protocol::AppConfig,
) -> Result<ProfileApplyReport, CoreError> {
    let profiles = db.list_profiles()?;
    let profile = profiles
        .into_iter()
        .find(|p| &p.id == profile_id)
        .ok_or_else(|| CoreError::NotFound(format!("预设不存在: {profile_id}")))?;

    let desired: HashSet<AddonId> = profile.enabled_addon_ids.iter().cloned().collect();
    let addons = db.list_addons()?;
    let by_id: HashMap<AddonId, AddonSummary> = addons
        .iter()
        .map(|a| (a.id.clone(), a.clone()))
        .collect();

    let mut missing_addons = Vec::new();
    let mut skipped_workshop_only = Vec::new();
    let mut to_enable = Vec::new();
    let mut to_disable = Vec::new();

    for aid in &desired {
        match by_id.get(aid) {
            None => missing_addons.push(aid.clone()),
            Some(a) if a.install_state != InstallState::Installed => missing_addons.push(aid.clone()),
            Some(a) if a.primary_dir == Some(AddonDirType::Workshop) && !a.enabled => {
                skipped_workshop_only.push(aid.clone());
            }
            _ => {}
        }
    }

    for addon in &addons {
        if addon.install_state != InstallState::Installed {
            continue;
        }
        if addon.primary_dir != Some(AddonDirType::Loading) {
            if desired.contains(&addon.id) != addon.enabled {
                skipped_workshop_only.push(addon.id.clone());
            }
            continue;
        }

        let should_enable = desired.contains(&addon.id);
        if should_enable && !addon.enabled {
            to_enable.push(addon.id.clone());
        } else if !should_enable && addon.enabled {
            to_disable.push(addon.id.clone());
        }
    }

    let enabled_count = to_enable.len();
    let disabled_count = to_disable.len();

    if !to_enable.is_empty() {
        toggle_addons(db, config, image_cache_dir, &to_enable, true, false)?;
    }
    if !to_disable.is_empty() {
        toggle_addons(db, config, image_cache_dir, &to_disable, false, false)?;
    }

    skipped_workshop_only.sort();
    skipped_workshop_only.dedup();

    Ok(ProfileApplyReport {
        profile_id: profile_id.clone(),
        enabled_count,
        disabled_count,
        skipped_workshop_only,
        missing_addons,
    })
}
