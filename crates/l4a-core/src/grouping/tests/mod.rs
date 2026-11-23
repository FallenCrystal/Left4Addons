mod campaign_matching;
mod partitioning_invariants;

use chrono::Utc;
use l4a_protocol::{
    AddonDirType, AddonId, AddonSummary, InstallState, MapEntry, MetadataSource,
    WorkshopDependency, WorkshopFileType, WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use std::collections::BTreeMap;

fn make_addon(
    id: &str,
    wid: Option<u64>,
    file_name: &str,
    addon_title: Option<&str>,
    author: Option<&str>,
    categories: &[&str],
    map_codes: &[&str],
) -> AddonSummary {
    let mut info = BTreeMap::new();
    if let Some(t) = addon_title {
        info.insert("addontitle".into(), t.into());
    }
    if let Some(a) = author {
        info.insert("addonauthor".into(), a.into());
    }
    AddonSummary {
        id: AddonId(id.into()),
        workshop_id: wid.and_then(|w| WorkshopId::new(w).ok()),
        title: addon_title
            .map(str::to_string)
            .unwrap_or_else(|| file_name.trim_end_matches(".vpk").to_string()),
        addon_title: addon_title.map(str::to_string),
        workshop_title: None,
        author: author.map(str::to_string),
        version: Some("1.0".into()),
        install_state: InstallState::Installed,
        primary_dir: Some(AddonDirType::Loading),
        enabled: true,
        current_file_name: Some(file_name.into()),
        current_path: Some(format!("/addons/{file_name}")),
        content_hash: Some(format!("hash_{id}")),
        file_size: 1024,
        files_count: 10,
        cover_image: None,
        local_cover_image: None,
        categories: categories.iter().map(|c| (*c).to_string()).collect(),
        maps: map_codes
            .iter()
            .map(|c| MapEntry {
                code: (*c).into(),
                name: (*c).into(),
                image_path: None,
                image_hint: None,
            })
            .collect(),
        has_dummy_bypass: false,
        remote_status: WorkshopItemStatus::Unknown,
        local_time_updated: None,
        remote_time_updated: None,
        update_available: false,
        addon_info: info,
    }
}

fn make_ws_snapshot(
    wid: u64,
    title: &str,
    creator_id: &str,
    tags: &[&str],
    deps: &[u64],
) -> WorkshopItemRecord {
    WorkshopItemRecord {
        workshop_id: WorkshopId::new(wid).unwrap(),
        file_type: WorkshopFileType::Item,
        status: WorkshopItemStatus::Active,
        title: title.into(),
        description: Some("Campaign multi-part workshop item description".into()),
        creator_steam_id: Some(creator_id.into()),
        creator_name: Some(format!("Creator_{creator_id}")),
        preview_url: None,
        previews: Vec::new(),
        file_url: None,
        file_size: Some(2048),
        time_created: Some(1_700_000_000),
        time_updated: Some(1_700_000_100),
        tags: tags.iter().map(|t| (*t).to_string()).collect(),
        dependencies: deps
            .iter()
            .map(|d| WorkshopDependency {
                workshop_id: WorkshopId::new(*d).unwrap(),
                title: None,
                required: true,
            })
            .collect(),
        collection_children: Vec::new(),
        subscriptions: None,
        favorites: None,
        source: MetadataSource::SteamworksSdk,
        fetched_at: Utc::now(),
    }
}
