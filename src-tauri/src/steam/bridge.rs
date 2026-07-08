use super::types::{BridgeInitResponse, BridgeRequest, BridgeResponse, WorkshopCapabilities};
use libloading::Library;
use serde::Serialize;
use serde_json::Value;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const IDLE_SHUTDOWN_DELAY: Duration = Duration::from_secs(60);

type InitFn = unsafe extern "C" fn() -> *mut c_char;
type RequestFn = unsafe extern "C" fn(*const c_char) -> *mut c_char;
type FreeStringFn = unsafe extern "C" fn(*mut c_char);
type ShutdownFn = unsafe extern "C" fn();

struct BridgeApi {
    _library: Library,
    init: InitFn,
    request_json: RequestFn,
    free_string: FreeStringFn,
    shutdown: ShutdownFn,
}

impl Drop for BridgeApi {
    fn drop(&mut self) {
        unsafe {
            (self.shutdown)();
        }
    }
}

#[derive(Clone)]
pub struct WorkshopBridge {
    api: Arc<BridgeApi>,
    invocation_gate: Arc<Mutex<()>>,
    activity_token: Arc<AtomicU64>,
    capabilities: WorkshopCapabilities,
}

impl WorkshopBridge {
    pub fn load_near(exe_dir: &Path) -> Result<Self, String> {
        let mut last_error = None;
        for candidate in candidate_library_paths(exe_dir) {
            match unsafe { Self::load_from_path(&candidate) } {
                Ok(bridge) => return Ok(bridge),
                Err(err) => last_error = Some(format!("{}: {}", candidate.display(), err)),
            }
        }

        Err(last_error.unwrap_or_else(|| "Steam bridge DLL not found".to_string()))
    }

    unsafe fn load_from_path(path: &Path) -> Result<Self, String> {
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

        let api = Arc::new(BridgeApi {
            _library: library,
            init,
            request_json,
            free_string,
            shutdown,
        });

        let init_value = invoke_raw_string(&api, None)?;
        let init_response: BridgeInitResponse =
            serde_json::from_str(&init_value).map_err(|e| e.to_string())?;
        if !init_response.ok {
            return Err(init_response
                .error
                .unwrap_or_else(|| "Steam bridge initialization failed".to_string()));
        }

        let capabilities = WorkshopCapabilities {
            bridge_available: true,
            bridge_loaded: true,
            bridge_initialized: false,
            provider: "steam-sdk".to_string(),
            bridge_version: init_response.version.clone(),
            last_error: None,
            current_user_steam_id: None,
            current_user_account_id: None,
            can_query_items: true,
            can_query_home: true,
            can_download: true,
            can_enumerate_installed: true,
        };

        Ok(Self {
            api,
            invocation_gate: Arc::new(Mutex::new(())),
            activity_token: Arc::new(AtomicU64::new(0)),
            capabilities,
        })
    }

    pub fn capabilities(&self) -> WorkshopCapabilities {
        self.capabilities.clone()
    }

    pub fn call<T: Serialize>(&self, method: &str, payload: &T) -> Result<Value, String> {
        let request = BridgeRequest {
            method: method.to_string(),
            payload: serde_json::to_value(payload).map_err(|e| e.to_string())?,
        };
        let encoded = serde_json::to_string(&request).map_err(|e| e.to_string())?;
        let result = {
            let _guard = self
                .invocation_gate
                .lock()
                .map_err(|_| "Steam bridge invocation mutex poisoned".to_string())?;
            invoke_raw_string(&self.api, Some(&encoded))
        }?;
        let token = self.activity_token.fetch_add(1, Ordering::SeqCst) + 1;
        self.schedule_idle_shutdown(token);
        let response: BridgeResponse = serde_json::from_str(&result).map_err(|e| e.to_string())?;
        if response.ok {
            Ok(response.payload)
        } else {
            Err(response
                .error
                .unwrap_or_else(|| format!("Steam bridge call {} failed", method)))
        }
    }

    pub fn shutdown(&self) {
        self.activity_token.fetch_add(1, Ordering::SeqCst);
        if let Ok(_guard) = self.invocation_gate.lock() {
            unsafe {
                (self.api.shutdown)();
            }
        }
    }

    fn schedule_idle_shutdown(&self, token: u64) {
        let api = Arc::clone(&self.api);
        let invocation_gate = Arc::clone(&self.invocation_gate);
        let activity_token = Arc::clone(&self.activity_token);

        std::thread::spawn(move || {
            std::thread::sleep(IDLE_SHUTDOWN_DELAY);
            let Ok(_guard) = invocation_gate.lock() else {
                return;
            };
            if activity_token.load(Ordering::SeqCst) == token {
                unsafe {
                    (api.shutdown)();
                }
            }
        });
    }
}

unsafe fn load_library(path: &Path) -> Result<Library, String> {
    #[cfg(target_os = "windows")]
    {
        use libloading::os::windows::{
            Library as WindowsLibrary, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        };

        let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS;
        let library = WindowsLibrary::load_with_flags(path, flags)
            .map(Library::from)
            .map_err(|e| format_windows_load_error(path, &e.to_string()))?;
        return Ok(library);
    }

    #[cfg(not(target_os = "windows"))]
    {
        Library::new(path).map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "windows")]
fn format_windows_load_error(path: &Path, source: &str) -> String {
    let hint = if path
        .parent()
        .is_some_and(|parent| parent.file_name().is_some_and(|name| name == "steam"))
    {
        " The bridge DLL was found, but one of its dependent DLLs could not be loaded from the same directory."
    } else {
        ""
    };

    format!(
        "{}. This usually means a dependent DLL is missing or the DLL architecture does not match. Required runtime files include steam_api64.dll, and GNU builds may also require MinGW runtime DLLs such as libgcc_s_seh-1.dll and libwinpthread-1.dll.{}",
        source, hint
    )
}

fn candidate_library_paths(exe_dir: &Path) -> Vec<PathBuf> {
    let names = if cfg!(target_os = "windows") {
        vec!["l4a-steam-bridge.dll"]
    } else if cfg!(target_os = "macos") {
        vec!["libl4a-steam-bridge.dylib"]
    } else {
        vec!["libl4a-steam-bridge.so", "l4a-steam-bridge.dll"]
    };

    let mut paths = Vec::new();
    for name in names {
        paths.push(exe_dir.join(name));
        paths.push(exe_dir.join("steam").join(name));
    }
    paths
}

fn invoke_raw_string(api: &BridgeApi, request: Option<&str>) -> Result<String, String> {
    let ptr = if let Some(request) = request {
        let encoded = CString::new(request).map_err(|e| e.to_string())?;
        unsafe { (api.request_json)(encoded.as_ptr()) }
    } else {
        unsafe { (api.init)() }
    };

    if ptr.is_null() {
        return Err("Steam bridge returned a null response".to_string());
    }

    let result = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();

    unsafe {
        (api.free_string)(ptr);
    }

    Ok(result)
}
