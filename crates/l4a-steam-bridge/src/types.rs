use serde::{Deserialize, Serialize};
use serde_json::Value;
use steamworks::Client;

pub(crate) struct BridgeRuntime {
    pub(crate) client: Client,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitResponse {
    pub(crate) ok: bool,
    pub(crate) version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) current_user_steam_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) current_user_account_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JsonResponse {
    pub(crate) ok: bool,
    pub(crate) payload: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BridgeRequest {
    pub(crate) method: String,
    #[serde(default)]
    pub(crate) payload: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DetailsPayload {
    pub(crate) ids: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueryItemsPayload {
    #[serde(default)]
    pub(crate) query: Option<String>,
    #[serde(default)]
    pub(crate) sort: Option<String>,
    #[serde(default)]
    pub(crate) section: Option<String>,
    #[serde(default)]
    pub(crate) page: Option<u32>,
    #[serde(default)]
    pub(crate) creator_id: Option<String>,
    #[serde(default)]
    pub(crate) active_tag: Option<String>,
    #[serde(default)]
    pub(crate) active_tag_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkshopIdPayload {
    pub(crate) workshop_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubscribedItemPayload {
    pub(crate) workshop_id: String,
    pub(crate) item_state: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) install_folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) size_on_disk: Option<u64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HomeSection {
    pub(crate) id: String,
    pub(crate) title_key: String,
    pub(crate) subtitle_key: String,
    pub(crate) icon: String,
    pub(crate) items: Vec<Value>,
    pub(crate) browse_params: BrowseParams,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowseParams {
    pub(crate) sort: String,
    pub(crate) section: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) days: Option<u32>,
}

#[derive(Debug, Clone)]
pub(crate) struct HomeSectionDef {
    pub(crate) id: &'static str,
    pub(crate) title_key: &'static str,
    pub(crate) subtitle_key: &'static str,
    pub(crate) icon: &'static str,
    pub(crate) sort: &'static str,
    pub(crate) section: &'static str,
    pub(crate) days: Option<u32>,
}

pub(crate) struct QueryExecution {
    pub(crate) items: Vec<Value>,
    pub(crate) child_ids: Vec<String>,
    pub(crate) total_results: u32,
    pub(crate) warnings: Vec<String>,
}

pub(crate) type QueryItemsWithChildren = (Vec<Value>, Vec<String>, Vec<String>);
