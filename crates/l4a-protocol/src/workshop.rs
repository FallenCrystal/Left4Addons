use crate::ids::WorkshopId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopItemStatus {
    Active,
    Removed,
    PrivateOrUnavailable,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopFileType {
    #[default]
    Item,
    Collection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataSource {
    #[serde(alias = "user_contribution")]
    CatalogServer,
    SteamKit,
    SteamworksSdk,
    SteamWebApi,
    WebScraper,
    LocalVpk,
    AdminOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopDependency {
    pub workshop_id: WorkshopId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default = "default_true")]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopPreview {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub youtube_video_id: Option<String>,
    #[serde(default)]
    pub preview_type: u32,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopItemRecord {
    pub workshop_id: WorkshopId,
    #[serde(default)]
    pub file_type: WorkshopFileType,
    #[serde(default)]
    pub status: WorkshopItemStatus,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_steam_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub previews: Vec<WorkshopPreview>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_created: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_updated: Option<i64>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<WorkshopDependency>,
    #[serde(default)]
    pub collection_children: Vec<WorkshopId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriptions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favorites: Option<u64>,
    pub source: MetadataSource,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopCollectionRecord {
    pub collection_id: WorkshopId,
    pub status: WorkshopItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    #[serde(default)]
    pub child_ids: Vec<WorkshopId>,
    #[serde(default)]
    pub items: Vec<WorkshopItemRecord>,
    pub source: MetadataSource,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[allow(clippy::large_enum_variant)]
pub enum WorkshopFetchOutcome {
    Found {
        item: WorkshopItemRecord,
    },
    Removed {
        workshop_id: WorkshopId,
        source: MetadataSource,
        observed_at: DateTime<Utc>,
        reason: Option<String>,
    },
    PrivateOrUnavailable {
        workshop_id: WorkshopId,
        source: MetadataSource,
        observed_at: DateTime<Utc>,
        reason: Option<String>,
    },
}

impl WorkshopFetchOutcome {
    pub fn workshop_id(&self) -> WorkshopId {
        match self {
            Self::Found { item } => item.workshop_id,
            Self::Removed { workshop_id, .. } | Self::PrivateOrUnavailable { workshop_id, .. } => {
                *workshop_id
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub status: Option<WorkshopItemStatus>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_type: Option<WorkshopFileType>,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            query: None,
            tag: None,
            status: None,
            sort: None,
            page: default_page(),
            page_size: default_page_size(),
            language: None,
            search_target: None,
            file_type: None,
        }
    }
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    30
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub items: Vec<WorkshopItemRecord>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub source: MetadataSource,
}
