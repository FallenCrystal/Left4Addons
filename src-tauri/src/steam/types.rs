use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopCapabilities {
    pub bridge_available: bool,
    pub bridge_loaded: bool,
    pub bridge_initialized: bool,
    pub provider: String,
    #[serde(default)]
    pub bridge_version: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub current_user_steam_id: Option<String>,
    #[serde(default)]
    pub current_user_account_id: Option<String>,
    #[serde(default)]
    pub can_query_items: bool,
    #[serde(default)]
    pub can_query_home: bool,
    #[serde(default)]
    pub can_download: bool,
    #[serde(default)]
    pub can_enumerate_installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopBrowseParams {
    pub sort: String,
    pub section: String,
    #[serde(default)]
    pub days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopHomeSection {
    pub id: String,
    pub title_key: String,
    pub subtitle_key: String,
    pub icon: String,
    pub items: Vec<Value>,
    pub browse_params: WorkshopBrowseParams,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopHomeResponse {
    pub source: String,
    #[serde(default)]
    pub sections: Vec<WorkshopHomeSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopBrowseQuery {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub creator_id: Option<String>,
    #[serde(default)]
    pub active_tag: Option<String>,
    #[serde(default)]
    pub active_tag_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopItemsResponse {
    pub source: String,
    #[serde(default)]
    pub items: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopItemResponse {
    pub source: String,
    #[serde(default)]
    pub item: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkshopCollectionResponse {
    pub source: String,
    #[serde(default)]
    pub collection: Value,
    #[serde(default)]
    pub items: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeRequest {
    pub method: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BridgeResponse {
    pub ok: bool,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BridgeInitResponse {
    pub ok: bool,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub current_user_steam_id: Option<String>,
    #[serde(default)]
    pub current_user_account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BridgeDownloadStatus {
    #[serde(default)]
    pub subscribed: bool,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub downloaded: Option<u64>,
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub install_folder: Option<String>,
    #[serde(default)]
    pub item_state: Vec<String>,
}
