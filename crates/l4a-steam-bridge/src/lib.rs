mod convert;
mod queries;
mod types;
mod ugc;

use crate::queries::{
    get_download_status, get_favorited_collections, get_subscribed_items, query_collection,
    query_details, query_home, query_item, query_items, request_download,
};
use crate::types::{
    BridgeRequest, BridgeRuntime, DetailsPayload, InitResponse, JsonResponse, QueryItemsPayload,
    WorkshopIdPayload,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use steamworks::{AppId, Client};

pub(crate) const APP_ID: u32 = 550;
const BRIDGE_VERSION: &str = "0.1.0";
pub(crate) const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const PERSONA_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

static RUNTIME: OnceLock<Mutex<Option<BridgeRuntime>>> = OnceLock::new();

#[no_mangle]
pub extern "C" fn l4a_steam_bridge_init() -> *mut c_char {
    let response = match ensure_runtime() {
        Ok(runtime) => {
            let user = runtime.as_ref().map(|runtime| runtime.client.user());
            let Some(user) = user else {
                return into_c_string(&InitResponse {
                    ok: false,
                    version: BRIDGE_VERSION.to_string(),
                    error: Some("Steam bridge runtime unavailable".to_string()),
                    current_user_steam_id: None,
                    current_user_account_id: None,
                });
            };
            let steam_id = user.steam_id();
            InitResponse {
                ok: true,
                version: BRIDGE_VERSION.to_string(),
                error: None,
                current_user_steam_id: Some(steam_id.raw().to_string()),
                current_user_account_id: Some(steam_id.account_id().raw().to_string()),
            }
        }
        Err(err) => InitResponse {
            ok: false,
            version: BRIDGE_VERSION.to_string(),
            error: Some(err),
            current_user_steam_id: None,
            current_user_account_id: None,
        },
    };

    into_c_string(&response)
}

#[no_mangle]
pub extern "C" fn l4a_steam_bridge_request_json(request: *const c_char) -> *mut c_char {
    let result = handle_request(request)
        .map(|payload| JsonResponse {
            ok: true,
            payload,
            error: None,
        })
        .unwrap_or_else(|err| JsonResponse {
            ok: false,
            payload: Value::Null,
            error: Some(err),
        });

    into_c_string(&result)
}

#[no_mangle]
/// # Safety
///
/// `ptr` must be either null or a pointer returned by this bridge's string-returning functions.
/// A non-null pointer must be passed exactly once and must not be used after this call.
pub unsafe extern "C" fn l4a_steam_bridge_free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }

    // The caller must pass a pointer returned by `into_c_string` exactly once.
    drop(unsafe { CString::from_raw(ptr) });
}

#[no_mangle]
pub extern "C" fn l4a_steam_bridge_shutdown() {
    if let Some(lock) = RUNTIME.get() {
        if let Ok(mut runtime) = lock.lock() {
            runtime.take();
        }
    }
    std::env::remove_var("SteamAppId");
    std::env::remove_var("SteamGameId");
}

fn handle_request(request: *const c_char) -> Result<Value, String> {
    if request.is_null() {
        return Err("Steam bridge request pointer was null".to_string());
    }

    let request = unsafe { CStr::from_ptr(request) }
        .to_string_lossy()
        .into_owned();
    let parsed: BridgeRequest = serde_json::from_str(&request).map_err(|e| e.to_string())?;
    with_runtime(|runtime| match parsed.method.as_str() {
        "get_details" => {
            let payload: DetailsPayload =
                serde_json::from_value(parsed.payload).map_err(|e| e.to_string())?;
            query_details(runtime, &payload.ids)
        }
        "query_home" => query_home(runtime),
        "query_items" => {
            let payload: QueryItemsPayload =
                serde_json::from_value(parsed.payload).map_err(|e| e.to_string())?;
            query_items(runtime, payload)
        }
        "query_item" => {
            let payload: WorkshopIdPayload =
                serde_json::from_value(parsed.payload).map_err(|e| e.to_string())?;
            query_item(runtime, &payload.workshop_id)
        }
        "query_collection" => {
            let payload: WorkshopIdPayload =
                serde_json::from_value(parsed.payload).map_err(|e| e.to_string())?;
            query_collection(runtime, &payload.workshop_id)
        }
        "request_download" => {
            let payload: WorkshopIdPayload =
                serde_json::from_value(parsed.payload).map_err(|e| e.to_string())?;
            request_download(runtime, &payload.workshop_id)
        }
        "get_download_status" => {
            let payload: WorkshopIdPayload =
                serde_json::from_value(parsed.payload).map_err(|e| e.to_string())?;
            get_download_status(runtime, &payload.workshop_id)
        }
        "get_subscribed_items" => get_subscribed_items(runtime),
        "get_favorited_collections" => get_favorited_collections(runtime),
        method => Err(format!("Unsupported Steam bridge method: {}", method)),
    })
}

fn ensure_runtime() -> Result<std::sync::MutexGuard<'static, Option<BridgeRuntime>>, String> {
    let lock = RUNTIME.get_or_init(|| Mutex::new(None));
    let mut runtime = lock
        .lock()
        .map_err(|_| "Steam bridge mutex poisoned".to_string())?;

    if runtime.is_none() {
        let client =
            Client::init_app(AppId(APP_ID)).map_err(|e| format!("Steam init failed: {}", e))?;
        *runtime = Some(BridgeRuntime { client });
    }

    Ok(runtime)
}

fn with_runtime<T>(f: impl FnOnce(&mut BridgeRuntime) -> Result<T, String>) -> Result<T, String> {
    let mut runtime = ensure_runtime()?;
    let runtime = runtime
        .as_mut()
        .ok_or_else(|| "Steam bridge runtime unavailable".to_string())?;
    f(runtime)
}

fn into_c_string<T: Serialize>(value: &T) -> *mut c_char {
    let encoded = serde_json::to_string(value)
        .unwrap_or_else(|err| json!({ "ok": false, "error": err.to_string() }).to_string());
    CString::new(encoded).unwrap().into_raw()
}
