use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Settings {
    #[serde(rename = "workshopDir")]
    pub workshop_dir: String,
    #[serde(rename = "loadingDir")]
    pub loading_dir: String,
    #[serde(rename = "enableDummyBypass", default)]
    pub enable_dummy_bypass: bool,
    #[serde(rename = "suppressSdkUnavailableWarning", default)]
    pub suppress_sdk_unavailable_warning: bool,
    #[serde(rename = "disableSteamworksSdk", default)]
    pub disable_steamworks_sdk: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Group {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub addons: Vec<String>,
    pub tags: Option<Vec<String>>,
    #[serde(rename = "workshopCollectionId")]
    pub workshop_collection_id: Option<String>,
    #[serde(rename = "masterCollectionIds", default)]
    pub master_collection_ids: Option<Vec<String>>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MasterCollection {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(rename = "nameKey", default)]
    pub name_key: Option<String>,
    #[serde(rename = "groupIds", default)]
    pub group_ids: Vec<String>,
    #[serde(rename = "isSystem", default)]
    pub is_system: bool,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Addon {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "vpkName")]
    pub vpk_name: String,
    #[serde(rename = "workshopId")]
    pub workshop_id: Option<String>,
    #[serde(rename = "addonInfo")]
    pub addon_info: serde_json::Value,
    #[serde(rename = "hasImage")]
    pub has_image: bool,
    #[serde(rename = "imagePath")]
    pub image_path: Option<String>,
    #[serde(rename = "filesCount")]
    pub files_count: usize,
    #[serde(rename = "fileSize")]
    pub file_size: u64,
    #[serde(rename = "parsedAt")]
    pub parsed_at: String,
    #[serde(rename = "currentPath")]
    pub current_path: String,
    #[serde(rename = "dirType")]
    pub dir_type: String,
    #[serde(rename = "isEnabled")]
    pub is_enabled: bool,
    #[serde(rename = "steamDetails")]
    pub steam_details: Option<serde_json::Value>,
    #[serde(rename = "workshopDetails", default)]
    pub workshop_details: Option<serde_json::Value>,
    #[serde(rename = "isDummy", default)]
    pub is_dummy: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LocalAddon {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "vpkName")]
    pub vpk_name: String,
    #[serde(rename = "workshopId")]
    pub workshop_id: Option<String>,
    #[serde(rename = "filesCount")]
    pub files_count: usize,
    #[serde(rename = "fileSize")]
    pub file_size: u64,
    #[serde(rename = "parsedAt")]
    pub parsed_at: String,
    #[serde(rename = "currentPath")]
    pub current_path: String,
    #[serde(rename = "dirType")]
    pub dir_type: String,
    #[serde(rename = "isEnabled")]
    pub is_enabled: bool,
    #[serde(rename = "isDummy", default)]
    pub is_dummy: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct KnownAddonEntry {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "vpkName")]
    pub vpk_name: String,
    #[serde(rename = "workshopId")]
    pub workshop_id: Option<String>,
    #[serde(rename = "addonInfo")]
    pub addon_info: serde_json::Value,
    #[serde(rename = "hasImage")]
    pub has_image: bool,
    #[serde(rename = "imagePath")]
    pub image_path: Option<String>,
    #[serde(rename = "steamDetails")]
    pub steam_details: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct WorkshopSeenItem {
    #[serde(rename = "workshopId")]
    pub workshop_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(rename = "imagePath", default)]
    pub image_path: String,
    #[serde(rename = "authorName", default)]
    pub author_name: String,
    #[serde(rename = "authorId", default)]
    pub author_id: String,
    #[serde(rename = "authorUrl", default)]
    pub author_url: String,
    #[serde(rename = "authorSteamId", default)]
    pub author_steam_id: Option<String>,
    #[serde(rename = "authorVanityId", default)]
    pub author_vanity_id: Option<String>,
    #[serde(rename = "authorAccountId", default)]
    pub author_account_id: Option<String>,
    #[serde(rename = "shortDescription", default)]
    pub short_description: Option<String>,
    #[serde(rename = "fileSize", default)]
    pub file_size: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub subscriptions: Option<u64>,
    #[serde(default)]
    pub favorites: Option<u64>,
    #[serde(rename = "lifetimeSubscriptions", default)]
    pub lifetime_subscriptions: Option<u64>,
    #[serde(rename = "lifetimeFavorites", default)]
    pub lifetime_favorites: Option<u64>,
    #[serde(default)]
    pub views: Option<u64>,
    #[serde(default)]
    pub comments: Option<u64>,
    #[serde(rename = "totalVotes", default)]
    pub total_votes: Option<u64>,
    #[serde(rename = "timeCreated", default)]
    pub time_created: Option<u64>,
    #[serde(rename = "timeUpdated", default)]
    pub time_updated: Option<u64>,
    #[serde(rename = "childCount", default)]
    pub child_count: Option<u64>,
    #[serde(rename = "previewCount", default)]
    pub preview_count: Option<u64>,
    #[serde(rename = "childItemIds", default)]
    pub child_item_ids: Option<Vec<String>>,
    #[serde(rename = "galleryPreviewUrls", default)]
    pub gallery_preview_urls: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SettingsStore {
    pub settings: Settings,
    #[serde(rename = "masterCollections", default)]
    pub master_collections: Vec<MasterCollection>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Database {
    pub settings: Settings,
    pub addons: HashMap<String, Addon>,
    pub groups: Vec<Group>,
    #[serde(rename = "knownUninstalledAddons")]
    pub known_uninstalled_addons: HashMap<String, Addon>,
    #[serde(rename = "masterCollections", default)]
    pub master_collections: Vec<MasterCollection>,
}

#[derive(Deserialize)]
pub struct RenameItem {
    pub id: String,
    #[serde(rename = "newVpkName")]
    pub new_vpk_name: String,
}

pub(crate) fn is_dummy_addon_info(addon_info: &serde_json::Value) -> bool {
    addon_info
        .get("addondescription")
        .or_else(|| addon_info.get("addonDescription"))
        .and_then(|v| v.as_str())
        .map(|v| v == "A dummy addon generated by Left 4 Addons")
        .unwrap_or(false)
}
