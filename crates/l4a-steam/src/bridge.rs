use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use l4a_protocol::{
    MetadataSource, WorkshopCollectionRecord, WorkshopDependency, WorkshopFetchOutcome,
    WorkshopFileType, WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use libloading::Library;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("Steamworks SDK 未启用或不可用: {0}")]
    Unavailable(String),
    #[error("调用 Steamworks bridge 失败: {0}")]
    CallFailed(String),
    #[error("解析 Steamworks 响应失败: {0}")]
    Serialization(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SteamCapabilities {
    pub bridge_available: bool,
    pub bridge_initialized: bool,
    pub bridge_version: Option<String>,
    pub current_user_steam_id: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubscribedItemInfo {
    pub workshop_id: WorkshopId,
    pub item_state: Vec<String>,
    pub install_folder: Option<String>,
    pub size_on_disk: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeDownloadStatus {
    pub subscribed: bool,
    pub installed: bool,
    pub downloaded: Option<u64>,
    pub total: Option<u64>,
    pub install_folder: Option<String>,
    pub item_state: Vec<String>,
}

#[async_trait]
pub trait SteamBridge: Send + Sync {
    async fn capabilities(&self) -> SteamCapabilities;
    async fn fetch_items(
        &self,
        ids: &[WorkshopId],
    ) -> Result<Vec<WorkshopFetchOutcome>, BridgeError>;
    async fn fetch_collection(
        &self,
        collection_id: WorkshopId,
    ) -> Result<WorkshopCollectionRecord, BridgeError>;
    async fn subscribed_items(&self) -> Result<Vec<SubscribedItemInfo>, BridgeError>;
    async fn request_download(&self, id: WorkshopId) -> Result<(), BridgeError>;
    async fn download_status(&self, id: WorkshopId) -> Result<BridgeDownloadStatus, BridgeError>;
    async fn shutdown(&self);
}

#[derive(Debug, Default)]
pub struct NullSteamBridge {
    reason: String,
}

impl NullSteamBridge {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl SteamBridge for NullSteamBridge {
    async fn capabilities(&self) -> SteamCapabilities {
        SteamCapabilities {
            bridge_available: false,
            bridge_initialized: false,
            bridge_version: None,
            current_user_steam_id: None,
            last_error: Some(self.reason.clone()),
        }
    }

    async fn fetch_items(
        &self,
        _ids: &[WorkshopId],
    ) -> Result<Vec<WorkshopFetchOutcome>, BridgeError> {
        Err(BridgeError::Unavailable(self.reason.clone()))
    }

    async fn fetch_collection(
        &self,
        _collection_id: WorkshopId,
    ) -> Result<WorkshopCollectionRecord, BridgeError> {
        Err(BridgeError::Unavailable(self.reason.clone()))
    }

    async fn subscribed_items(&self) -> Result<Vec<SubscribedItemInfo>, BridgeError> {
        Err(BridgeError::Unavailable(self.reason.clone()))
    }

    async fn request_download(&self, _id: WorkshopId) -> Result<(), BridgeError> {
        Err(BridgeError::Unavailable(self.reason.clone()))
    }

    async fn download_status(&self, _id: WorkshopId) -> Result<BridgeDownloadStatus, BridgeError> {
        Err(BridgeError::Unavailable(self.reason.clone()))
    }

    async fn shutdown(&self) {}
}

type InitFn = unsafe extern "C" fn() -> *mut c_char;
type RequestFn = unsafe extern "C" fn(*const c_char) -> *mut c_char;
type FreeStringFn = unsafe extern "C" fn(*mut c_char);
type ShutdownFn = unsafe extern "C" fn();

struct BridgeApi {
    // 声明顺序决定 Drop 顺序：先卸载 bridge 主库，再卸载支撑运行库（如 libsteam_api）。
    _library: Library,
    _dependencies: Vec<Library>,
    init: InitFn,
    request_json: RequestFn,
    free_string: FreeStringFn,
    shutdown: ShutdownFn,
    call_gate: Mutex<()>,
    shutdown_called: AtomicBool,
}

impl BridgeApi {
    fn shutdown_once(&self) {
        let _gate = self.call_gate.lock();
        if !self.shutdown_called.swap(true, Ordering::SeqCst) {
            unsafe {
                (self.shutdown)();
            }
        }
    }
}

impl Drop for BridgeApi {
    fn drop(&mut self) {
        self.shutdown_once();
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawInitResponse {
    ok: bool,
    version: Option<String>,
    error: Option<String>,
    current_user_steam_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RawRequest {
    method: String,
    payload: Value,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawResponse {
    ok: bool,
    payload: Value,
    error: Option<String>,
}

struct LoadedBridge {
    api: Arc<BridgeApi>,
    capabilities: SteamCapabilities,
}

struct DynamicBridgeState {
    base_dir: PathBuf,
    enabled: Mutex<bool>,
    inner: Mutex<Option<LoadedBridge>>,
    last_error: Mutex<Option<String>>,
}

/// 按需加载 `l4a-steam-bridge` 动态库并隔离 FFI 调用 (支持显式 `shutdown` 以规避 VAC 信任冲突)。
pub struct DynamicSteamBridge {
    state: Arc<DynamicBridgeState>,
}

impl DynamicSteamBridge {
    pub fn new(base_dir: impl AsRef<Path>, enabled: bool) -> Self {
        Self {
            state: Arc::new(DynamicBridgeState {
                base_dir: base_dir.as_ref().to_path_buf(),
                enabled: Mutex::new(enabled),
                inner: Mutex::new(None),
                last_error: Mutex::new(None),
            }),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        if let Ok(mut flag) = self.state.enabled.lock() {
            *flag = enabled;
        }
        if !enabled {
            self.state.unload_sync();
        }
    }
}

impl DynamicBridgeState {
    fn unload_sync(&self) {
        let loaded = {
            let Ok(mut guard) = self.inner.lock() else {
                return;
            };
            guard.take()
        };
        if let Some(loaded) = loaded {
            loaded.api.shutdown_once();
            drop(loaded);
        }
    }

    fn ensure_loaded(&self) -> Result<(Arc<BridgeApi>, SteamCapabilities), BridgeError> {
        let enabled = self.enabled.lock().map(|g| *g).unwrap_or(false);
        if !enabled {
            return Err(BridgeError::Unavailable(
                "配置中已禁用 Steamworks SDK".into(),
            ));
        }

        let mut guard = self
            .inner
            .lock()
            .map_err(|_| BridgeError::Unavailable("Steam bridge 状态锁异常".into()))?;
        if let Some(ref loaded) = *guard {
            return Ok((loaded.api.clone(), loaded.capabilities.clone()));
        }

        let lib_path = bridge_library_path(&self.base_dir);
        if !lib_path.exists() {
            let msg = format!("未找到 Steam bridge 库: {}", lib_path.display());
            if let Ok(mut err) = self.last_error.lock() {
                *err = Some(msg.clone());
            }
            return Err(BridgeError::Unavailable(msg));
        }

        match unsafe { load_bridge_api(&lib_path) } {
            Ok((api, caps)) => {
                let api = Arc::new(api);
                *guard = Some(LoadedBridge {
                    api: api.clone(),
                    capabilities: caps.clone(),
                });
                if let Ok(mut err) = self.last_error.lock() {
                    *err = None;
                }
                Ok((api, caps))
            }
            Err(msg) => {
                if let Ok(mut err) = self.last_error.lock() {
                    *err = Some(msg.clone());
                }
                Err(BridgeError::Unavailable(msg))
            }
        }
    }

    fn call_sync(&self, method: &str, payload: Value) -> Result<Value, BridgeError> {
        let (api, _) = self.ensure_loaded()?;
        let req = serde_json::to_string(&RawRequest {
            method: method.to_string(),
            payload,
        })
        .map_err(|e| BridgeError::Serialization(e.to_string()))?;

        let raw = invoke_raw_string(&api, Some(&req)).map_err(BridgeError::CallFailed)?;
        let resp: RawResponse =
            serde_json::from_str(&raw).map_err(|e| BridgeError::Serialization(e.to_string()))?;
        if resp.ok {
            Ok(resp.payload)
        } else {
            Err(BridgeError::CallFailed(
                resp.error.unwrap_or_else(|| format!("调用 {method} 失败")),
            ))
        }
    }
}

#[async_trait]
impl SteamBridge for DynamicSteamBridge {
    async fn capabilities(&self) -> SteamCapabilities {
        let state = self.state.clone();
        match tokio::task::spawn_blocking(move || state.ensure_loaded().map(|(_, c)| c)).await {
            Ok(Ok(caps)) => caps,
            Ok(Err(err)) => SteamCapabilities {
                bridge_available: false,
                bridge_initialized: false,
                bridge_version: None,
                current_user_steam_id: None,
                last_error: Some(err.to_string()),
            },
            Err(err) => SteamCapabilities {
                bridge_available: false,
                bridge_initialized: false,
                bridge_version: None,
                current_user_steam_id: None,
                last_error: Some(err.to_string()),
            },
        }
    }

    async fn fetch_items(
        &self,
        ids: &[WorkshopId],
    ) -> Result<Vec<WorkshopFetchOutcome>, BridgeError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let id_strings: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        let state = self.state.clone();
        let val = tokio::task::spawn_blocking(move || {
            state.call_sync("get_details", json!({ "ids": id_strings }))
        })
        .await
        .map_err(|e| BridgeError::CallFailed(e.to_string()))??;

        let arr = val.as_array().cloned().unwrap_or_default();
        let mut outcomes = Vec::with_capacity(arr.len());
        for raw in arr {
            if let Some(item) = parse_sdk_item_value(&raw) {
                outcomes.push(WorkshopFetchOutcome::Found { item });
            }
        }
        Ok(outcomes)
    }

    async fn fetch_collection(
        &self,
        collection_id: WorkshopId,
    ) -> Result<WorkshopCollectionRecord, BridgeError> {
        let cid_str = collection_id.to_string();
        let state = self.state.clone();
        let val = tokio::task::spawn_blocking(move || {
            state.call_sync("query_collection", json!({ "workshopId": cid_str }))
        })
        .await
        .map_err(|e| BridgeError::CallFailed(e.to_string()))??;

        let title = val
            .get("collection")
            .and_then(|c| c.get("title"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let description = val
            .get("collection")
            .and_then(|c| c.get("description"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let preview_url = val
            .get("collection")
            .and_then(|c| c.get("preview_url"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);

        let raw_items = val
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let items: Vec<WorkshopItemRecord> =
            raw_items.iter().filter_map(parse_sdk_item_value).collect();
        let child_ids = items.iter().map(|i| i.workshop_id).collect();

        Ok(WorkshopCollectionRecord {
            collection_id,
            status: WorkshopItemStatus::Active,
            title,
            description,
            preview_url,
            child_ids,
            items,
            source: MetadataSource::SteamworksSdk,
            fetched_at: Utc::now(),
        })
    }

    async fn subscribed_items(&self) -> Result<Vec<SubscribedItemInfo>, BridgeError> {
        let state = self.state.clone();
        let val =
            tokio::task::spawn_blocking(move || state.call_sync("get_subscribed_items", json!({})))
                .await
                .map_err(|e| BridgeError::CallFailed(e.to_string()))??;

        let arr = val
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for entry in arr {
            let Some(wid) = entry
                .get("workshopId")
                .and_then(Value::as_str)
                .and_then(|s| WorkshopId::from_str(s).ok())
            else {
                continue;
            };
            let item_state = entry
                .get("itemState")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default();
            out.push(SubscribedItemInfo {
                workshop_id: wid,
                item_state,
                install_folder: entry
                    .get("installFolder")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                size_on_disk: entry.get("sizeOnDisk").and_then(Value::as_u64),
            });
        }
        Ok(out)
    }

    async fn request_download(&self, id: WorkshopId) -> Result<(), BridgeError> {
        let wid_str = id.to_string();
        let state = self.state.clone();
        let val = tokio::task::spawn_blocking(move || {
            state.call_sync("request_download", json!({ "workshopId": wid_str }))
        })
        .await
        .map_err(|e| BridgeError::CallFailed(e.to_string()))??;

        if val
            .get("accepted")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            Ok(())
        } else {
            Err(BridgeError::CallFailed(
                val.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Steam SDK 拒绝下载请求")
                    .to_string(),
            ))
        }
    }

    async fn download_status(&self, id: WorkshopId) -> Result<BridgeDownloadStatus, BridgeError> {
        let wid_str = id.to_string();
        let state = self.state.clone();
        let val = tokio::task::spawn_blocking(move || {
            state.call_sync("get_download_status", json!({ "workshopId": wid_str }))
        })
        .await
        .map_err(|e| BridgeError::CallFailed(e.to_string()))??;
        serde_json::from_value(val).map_err(|e| BridgeError::Serialization(e.to_string()))
    }

    async fn shutdown(&self) {
        let state = self.state.clone();
        let _ = tokio::task::spawn_blocking(move || state.unload_sync()).await;
    }
}

pub fn parse_sdk_item_value(val: &Value) -> Option<WorkshopItemRecord> {
    let raw_id = val.get("publishedfileid")?.as_str()?;
    let workshop_id = WorkshopId::from_str(raw_id).ok()?;
    let title = val
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let description = val
        .get("description")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let creator_steam_id = val
        .get("creator_steam_id")
        .or_else(|| val.get("creator"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let creator_name = val
        .get("creator_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let preview_url = val
        .get("preview_url")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let file_size = val.get("file_size").and_then(Value::as_u64).or_else(|| {
        val.get("file_size")
            .and_then(Value::as_str)?
            .parse::<u64>()
            .ok()
    });
    let time_created = val.get("time_created").and_then(Value::as_i64);
    let time_updated = val.get("time_updated").and_then(Value::as_i64);
    let tags = val
        .get("tags")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    v.as_str()
                        .or_else(|| v.get("tag").and_then(Value::as_str))
                        .map(ToString::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    let file_type = match val.get("file_type").and_then(Value::as_str) {
        Some("collection") => WorkshopFileType::Collection,
        _ => WorkshopFileType::Item,
    };
    let child_ids: Vec<WorkshopId> = val
        .get("child_item_ids")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .filter_map(|s| WorkshopId::from_str(s).ok())
                .collect()
        })
        .unwrap_or_default();

    let (dependencies, collection_children) = if file_type == WorkshopFileType::Collection {
        (Vec::new(), child_ids)
    } else {
        (
            child_ids
                .into_iter()
                .map(|id| WorkshopDependency {
                    workshop_id: id,
                    title: None,
                    required: true,
                })
                .collect(),
            Vec::new(),
        )
    };

    let fetched_at = Utc
        .timestamp_opt(Utc::now().timestamp(), 0)
        .single()
        .unwrap_or_else(Utc::now);

    Some(WorkshopItemRecord {
        workshop_id,
        file_type,
        status: WorkshopItemStatus::Active,
        title,
        description,
        creator_steam_id,
        creator_name,
        preview_url,
        file_url: None,
        file_size,
        time_created,
        time_updated,
        tags,
        dependencies,
        collection_children,
        subscriptions: val.get("subscriptions").and_then(Value::as_u64),
        favorites: val.get("favorites").and_then(Value::as_u64),
        source: MetadataSource::SteamworksSdk,
        fetched_at,
    })
}

fn bridge_library_path(base_dir: &Path) -> PathBuf {
    let name = if cfg!(target_os = "windows") {
        "l4a-steam-bridge.dll"
    } else if cfg!(target_os = "macos") {
        "libl4a-steam-bridge.dylib"
    } else {
        "libl4a-steam-bridge.so"
    };
    base_dir.join("steam").join(name)
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
fn steam_api_library_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "steam_api64.dll"
    } else if cfg!(target_os = "macos") {
        "libsteam_api.dylib"
    } else {
        "libsteam_api.so"
    }
}

unsafe fn load_bridge_api(path: &Path) -> Result<(BridgeApi, SteamCapabilities), String> {
    let dependencies = load_support_libraries(path)?;
    let library = load_library(path)?;
    let init = *library
        .get::<InitFn>(b"l4a_steam_bridge_init\0")
        .map_err(|e| e.to_string())?;
    let request_json = *library
        .get::<RequestFn>(b"l4a_steam_bridge_request_json\0")
        .map_err(|e| e.to_string())?;
    let free_string = *library
        .get::<FreeStringFn>(b"l4a_steam_bridge_free_string\0")
        .map_err(|e| e.to_string())?;
    let shutdown = *library
        .get::<ShutdownFn>(b"l4a_steam_bridge_shutdown\0")
        .map_err(|e| e.to_string())?;

    let api = BridgeApi {
        _library: library,
        _dependencies: dependencies,
        init,
        request_json,
        free_string,
        shutdown,
        call_gate: Mutex::new(()),
        shutdown_called: AtomicBool::new(false),
    };

    let raw_init = invoke_raw_string(&api, None)?;
    let parsed: RawInitResponse = serde_json::from_str(&raw_init).map_err(|e| e.to_string())?;
    if !parsed.ok {
        return Err(parsed
            .error
            .unwrap_or_else(|| "Steam bridge 初始化失败".to_string()));
    }

    let caps = SteamCapabilities {
        bridge_available: true,
        bridge_initialized: true,
        bridge_version: parsed.version,
        current_user_steam_id: parsed.current_user_steam_id,
        last_error: None,
    };
    Ok((api, caps))
}

unsafe fn load_library(path: &Path) -> Result<Library, String> {
    #[cfg(target_os = "windows")]
    {
        use libloading::os::windows::{
            Library as WindowsLibrary, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        };
        let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS;
        WindowsLibrary::load_with_flags(path, flags)
            .map(Library::from)
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Library::new(path).map_err(|e| e.to_string())
    }
}

unsafe fn load_support_libraries(path: &Path) -> Result<Vec<Library>, String> {
    #[cfg(target_os = "windows")]
    {
        let _ = path;
        Ok(Vec::new())
    }
    #[cfg(not(target_os = "windows"))]
    {
        use libloading::os::unix::{Library as UnixLibrary, RTLD_GLOBAL, RTLD_NOW};
        let parent = path
            .parent()
            .ok_or_else(|| "无法解析 Steam bridge 父目录".to_string())?;
        let steam_api_path = parent.join(steam_api_library_name());
        if !steam_api_path.exists() {
            return Err(format!("缺少 Steam 运行库: {}", steam_api_path.display()));
        }
        let lib = UnixLibrary::open(Some(&steam_api_path), RTLD_NOW | RTLD_GLOBAL)
            .map(Library::from)
            .map_err(|e| e.to_string())?;
        Ok(vec![lib])
    }
}

fn invoke_raw_string(api: &BridgeApi, request: Option<&str>) -> Result<String, String> {
    let _gate = api
        .call_gate
        .lock()
        .map_err(|_| "Steam bridge 调用锁异常".to_string())?;
    if api.shutdown_called.load(Ordering::SeqCst) {
        return Err("Steam bridge 正在关闭或已卸载".to_string());
    }
    let ptr = if let Some(req) = request {
        let cstr = CString::new(req).map_err(|e| e.to_string())?;
        unsafe { (api.request_json)(cstr.as_ptr()) }
    } else {
        unsafe { (api.init)() }
    };
    if ptr.is_null() {
        return Err("Steam bridge 返回空指针".to_string());
    }
    let s = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe {
        (api.free_string)(ptr);
    }
    Ok(s)
}
