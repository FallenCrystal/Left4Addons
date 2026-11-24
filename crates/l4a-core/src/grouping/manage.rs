use crate::db::LocalDb;
use crate::error::CoreError;
use crate::ops::toggle_addons;
use chrono::Utc;
use l4a_protocol::{
    AddonDirType, AddonId, AddonOperationOutcome, AddonSummary, GroupId, InstallState,
    LoadoutProfile, ProfileApplyReport, ProfileId, TrackedCollectionRecord, UserGroup, WorkshopId,
};
use l4a_workshop::WorkshopResolver;
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub async fn track_workshop_collection(
    db: &LocalDb,
    resolver: &WorkshopResolver,
    collection_id: WorkshopId,
    auto_sync: bool,
) -> Result<TrackedCollectionRecord, CoreError> {
    let (remote, _warnings) = resolver.fetch_collection(collection_id).await?;
    let _workspace = db.lock_workspace()?;
    let mut removed_upstream_ids = db
        .list_tracked_collections()?
        .into_iter()
        .find(|col| col.collection_id == collection_id)
        .map(|col| col.removed_upstream_ids)
        .unwrap_or_default();
    removed_upstream_ids.retain(|id| !remote.child_ids.contains(id));
    let record = TrackedCollectionRecord {
        collection_id,
        title: remote
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| format!("Collection #{collection_id}")),
        description: remote.description,
        auto_sync,
        member_workshop_ids: remote.child_ids,
        removed_upstream_ids,
        last_synced_at: Some(Utc::now()),
    };
    db.write(|tx| {
        for item in &remote.items {
            LocalDb::upsert_workshop_snapshot_tx(tx, item, false)?;
        }
        LocalDb::save_tracked_collection_tx(tx, &record)
    })?;
    Ok(record)
}

pub fn untrack_workshop_collection(
    db: &LocalDb,
    collection_id: WorkshopId,
) -> Result<(), CoreError> {
    let _workspace = db.lock_workspace()?;
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

/// 一键应用预设方案 (仅切换 `loading/` 目录中的模组)。
/// 跳过 `workshop/` 目录中的模组，以免触发 Steam 重新下载。
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
    let by_id: HashMap<AddonId, AddonSummary> =
        addons.iter().map(|a| (a.id.clone(), a.clone())).collect();

    let mut missing_addons = Vec::new();
    let mut skipped_workshop_only = Vec::new();
    let mut to_enable = Vec::new();
    let mut to_disable = Vec::new();

    for aid in &desired {
        match by_id.get(aid) {
            None => missing_addons.push(aid.clone()),
            Some(a) if a.install_state != InstallState::Installed => {
                missing_addons.push(aid.clone())
            }
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

    let toggles = to_enable
        .iter()
        .map(|id| (id, true))
        .chain(to_disable.iter().map(|id| (id, false)));
    let batch = toggle_addons(db, config, image_cache_dir, toggles, false)?;
    let mut enabled_count = 0;
    let mut disabled_count = 0;
    for item in &batch.items {
        if item.outcome == AddonOperationOutcome::Applied {
            if desired.contains(&item.addon_id) {
                enabled_count += 1;
            } else {
                disabled_count += 1;
            }
        }
    }

    skipped_workshop_only.sort();
    skipped_workshop_only.dedup();

    Ok(ProfileApplyReport {
        profile_id: profile_id.clone(),
        enabled_count,
        disabled_count,
        skipped_workshop_only,
        missing_addons,
        batch,
    })
}
