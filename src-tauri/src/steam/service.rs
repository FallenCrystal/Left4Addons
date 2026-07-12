use super::bridge::WorkshopBridge;
use super::types::{
    BridgeDownloadStatus, SubscribedWorkshopItemsResponse, WorkshopBrowseQuery,
    WorkshopCapabilities, WorkshopCollectionResponse, WorkshopHomeResponse, WorkshopItemResponse,
    WorkshopItemsResponse,
};
use crate::mirrors::MirrorClientExt;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Default)]
struct WorkshopServiceState {
    bridge: Option<WorkshopBridge>,
    bridge_error: Option<String>,
    load_attempted: bool,
}

#[derive(Default)]
pub struct WorkshopService {
    exe_dir: PathBuf,
    state: Mutex<WorkshopServiceState>,
}

impl WorkshopService {
    pub fn new(exe_dir: &Path) -> Self {
        Self {
            exe_dir: exe_dir.to_path_buf(),
            state: Mutex::new(WorkshopServiceState::default()),
        }
    }

    fn unavailable_capabilities(last_error: Option<String>) -> WorkshopCapabilities {
        WorkshopCapabilities {
            bridge_available: false,
            bridge_loaded: false,
            bridge_initialized: false,
            provider: "web-fallback".to_string(),
            bridge_version: None,
            last_error,
            current_user_steam_id: None,
            current_user_account_id: None,
            can_query_items: false,
            can_query_home: false,
            can_download: false,
            can_enumerate_installed: false,
            can_enumerate_subscribed: false,
        }
    }

    fn ensure_bridge(&self) -> Result<WorkshopBridge, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Steam bridge state mutex poisoned".to_string())?;

        if let Some(bridge) = &state.bridge {
            return Ok(bridge.clone());
        }

        if state.load_attempted {
            return Err(state
                .bridge_error
                .clone()
                .unwrap_or_else(|| "Steam bridge unavailable".to_string()));
        }

        match WorkshopBridge::load_near(&self.exe_dir) {
            Ok(bridge) => {
                state.bridge = Some(bridge.clone());
                state.bridge_error = None;
                state.load_attempted = true;
                Ok(bridge)
            }
            Err(err) => {
                state.bridge = None;
                state.bridge_error = Some(err.clone());
                state.load_attempted = true;
                Err(err)
            }
        }
    }

    pub fn capabilities(&self) -> WorkshopCapabilities {
        match self.ensure_bridge() {
            Ok(bridge) => bridge.capabilities(),
            Err(err) => Self::unavailable_capabilities(Some(err)),
        }
    }

    pub fn has_bridge(&self) -> bool {
        self.ensure_bridge().is_ok()
    }

    pub fn shutdown(&self) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(bridge) = state.bridge.take() {
                bridge.shutdown();
            }
            state.bridge_error = None;
            state.load_attempted = false;
        }
    }

    pub fn bridge_fetch_details(&self, workshop_ids: &[String]) -> Result<Vec<Value>, String> {
        let bridge = self.ensure_bridge()?;

        let payload = bridge.call("get_details", &json!({ "ids": workshop_ids }))?;
        serde_json::from_value(payload).map_err(|e| e.to_string())
    }

    pub fn bridge_query_home(&self) -> Result<WorkshopHomeResponse, String> {
        let bridge = self.ensure_bridge()?;

        let payload = bridge.call("query_home", &json!({}))?;
        serde_json::from_value(payload).map_err(|e| e.to_string())
    }

    pub fn bridge_query_items(
        &self,
        query: &WorkshopBrowseQuery,
    ) -> Result<WorkshopItemsResponse, String> {
        let bridge = self.ensure_bridge()?;

        let payload = bridge.call("query_items", query)?;
        serde_json::from_value(payload).map_err(|e| e.to_string())
    }

    pub fn bridge_query_item(&self, workshop_id: &str) -> Result<WorkshopItemResponse, String> {
        let bridge = self.ensure_bridge()?;

        let payload = bridge.call("query_item", &json!({ "workshopId": workshop_id }))?;
        serde_json::from_value(payload).map_err(|e| e.to_string())
    }

    pub fn bridge_query_collection(
        &self,
        workshop_id: &str,
    ) -> Result<WorkshopCollectionResponse, String> {
        let bridge = self.ensure_bridge()?;

        let payload = bridge.call("query_collection", &json!({ "workshopId": workshop_id }))?;
        serde_json::from_value(payload).map_err(|e| e.to_string())
    }

    pub fn bridge_request_download(&self, workshop_id: &str) -> Result<(), String> {
        let bridge = self.ensure_bridge()?;

        let payload = bridge.call("request_download", &json!({ "workshopId": workshop_id }))?;
        let accepted = payload
            .get("accepted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if accepted {
            Ok(())
        } else {
            Err(payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Steam bridge rejected download request")
                .to_string())
        }
    }

    pub fn bridge_download_status(
        &self,
        workshop_id: &str,
    ) -> Result<BridgeDownloadStatus, String> {
        let bridge = self.ensure_bridge()?;

        let payload = bridge.call("get_download_status", &json!({ "workshopId": workshop_id }))?;
        serde_json::from_value(payload).map_err(|e| e.to_string())
    }

    pub fn bridge_get_subscribed_items(&self) -> Result<SubscribedWorkshopItemsResponse, String> {
        let bridge = self.ensure_bridge()?;

        let payload = bridge.call("get_subscribed_items", &json!({}))?;
        serde_json::from_value(payload).map_err(|e| e.to_string())
    }

    pub fn bridge_get_favorited_collections(&self) -> Result<WorkshopItemsResponse, String> {
        let bridge = self.ensure_bridge()?;

        let payload = bridge.call("get_favorited_collections", &json!({}))?;
        serde_json::from_value(payload).map_err(|e| e.to_string())
    }
}

pub async fn fetch_steam_details_web(workshop_ids: &[String]) -> Result<Vec<Value>, String> {
    if workshop_ids.is_empty() {
        return Ok(Vec::new());
    }

    let url = "https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/";
    let mut params = Vec::new();
    params.push(("itemcount".to_string(), workshop_ids.len().to_string()));
    for (index, id) in workshop_ids.iter().enumerate() {
        params.push((format!("publishedfileids[{}]", index), id.clone()));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let res = client
        .post_mirrored(url)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Failed to send Steam API request: {}", e))?;

    if !res.status().is_success() {
        return Err(format!("Steam API responded with status {}", res.status()));
    }

    let json: Value = res
        .json()
        .await
        .map_err(|e| format!("Failed to parse Steam API response: {}", e))?;

    Ok(json["response"]["publishedfiledetails"]
        .as_array()
        .cloned()
        .unwrap_or_default())
}

pub async fn fetch_collection_children_web(collection_id: &str) -> Result<Vec<String>, String> {
    let url = "https://api.steampowered.com/ISteamRemoteStorage/GetCollectionDetails/v1/";
    let params = vec![
        ("collectioncount".to_string(), "1".to_string()),
        ("publishedfileids[0]".to_string(), collection_id.to_string()),
    ];

    let client = reqwest::Client::new();
    let res = client
        .post_mirrored(url)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Failed to query collection details: {}", e))?;

    if !res.status().is_success() {
        return Err(format!(
            "Steam Collection API responded with status {}",
            res.status()
        ));
    }

    let json: Value = res
        .json()
        .await
        .map_err(|e| format!("Failed to parse collection JSON: {}", e))?;

    let details_list = json["response"]["collectiondetails"]
        .as_array()
        .ok_or_else(|| "Invalid collectiondetails format".to_string())?;

    if details_list.is_empty() {
        return Err("Collection not found".to_string());
    }

    let mut ids = Vec::new();
    if let Some(children) = details_list[0]["children"].as_array() {
        for child in children {
            if let Some(id) = child["publishedfileid"].as_str() {
                ids.push(id.to_string());
            }
        }
    }

    Ok(ids)
}
