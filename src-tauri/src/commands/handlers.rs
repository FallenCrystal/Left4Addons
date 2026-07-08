use crate::steam::{
    fetch_collection_children_web, fetch_steam_details_web, BridgeDownloadStatus,
    WorkshopBrowseQuery, WorkshopCapabilities, WorkshopCollectionResponse, WorkshopHomeResponse,
    WorkshopItemResponse, WorkshopItemsResponse, WorkshopService,
};
use crate::vpk::{extract_addon_metadata, generate_dummy_vpk};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, State};

use super::types::{
    is_dummy_addon_info, Addon, Database, Group, KnownAddonEntry, MasterCollection, RenameItem,
    Settings, SettingsStore, WorkshopSeenItem,
};

const DOWNLOAD_CANCELLED_ERR: &str = "Download cancelled";

fn resolve_cache_file(cache_dir: &Path, image_path: &str) -> Result<PathBuf, String> {
    let filename = image_path.strip_prefix("/cache/").unwrap_or(image_path);
    let rel_path = Path::new(filename);

    if filename.is_empty()
        || filename.contains("..")
        || filename.contains('/')
        || filename.contains('\\')
        || rel_path.components().count() != 1
    {
        return Err("Invalid cache image path".to_string());
    }

    let cache_root = fs::canonicalize(cache_dir)
        .map_err(|e| format!("Failed to resolve cache directory: {}", e))?;
    let file_path = fs::canonicalize(cache_root.join(filename))
        .map_err(|e| format!("Failed to resolve cache image: {}", e))?;

    if !file_path.starts_with(&cache_root) {
        return Err("Cache image path escapes cache directory".to_string());
    }

    Ok(file_path)
}

fn validate_open_url(url: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;
    match parsed.scheme() {
        "http" | "https" | "steam" => Ok(parsed),
        scheme => Err(format!("Unsupported URL scheme: {}", scheme)),
    }
}

fn validate_steamcommunity_url(url: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;
    if parsed.scheme() != "https" {
        return Err("Only HTTPS Steam Community URLs are allowed".to_string());
    }

    let host = parsed.host_str().unwrap_or_default();
    if host == "steamcommunity.com" || host.ends_with(".steamcommunity.com") {
        Ok(parsed)
    } else {
        Err("Only steamcommunity.com URLs are allowed".to_string())
    }
}

fn is_background_workshop_fetch_source(source: &str) -> bool {
    matches!(source, "startup-auto" | "background-refresh")
}

fn is_known_workshop_id(known_addons_path: &Path, workshop_id: &str) -> bool {
    let workshop_id = workshop_id.trim();
    if workshop_id.is_empty() {
        return false;
    }

    let known_addons = load_known_addons(known_addons_path);
    if known_addons.contains_key(workshop_id)
        || known_addons.contains_key(&format!("{}.vpk", workshop_id))
    {
        return true;
    }

    known_addons.values().any(|entry| {
        entry.id == workshop_id
            || entry.vpk_name == format!("{}.vpk", workshop_id)
            || entry.workshop_id.as_deref() == Some(workshop_id)
    })
}

fn ensure_background_workshop_fetch_allowed(
    source: &str,
    workshop_id: &str,
    known_addons_path: &Path,
) -> Result<(), String> {
    if !is_background_workshop_fetch_source(source) {
        return Ok(());
    }

    if is_known_workshop_id(known_addons_path, workshop_id) {
        return Ok(());
    }

    Err(format!(
        "Background workshop fetch blocked: workshop item {} is not present in known_addons",
        workshop_id
    ))
}

fn request_download_cancellation(state: &crate::AppState, workshop_id: &str) -> Result<(), String> {
    let mut cancelled = state
        .cancelled_downloads
        .lock()
        .map_err(|_| "Failed to acquire cancelled downloads lock".to_string())?;
    cancelled.insert(workshop_id.to_string());
    Ok(())
}

fn clear_download_cancellation(state: &crate::AppState, workshop_id: &str) -> Result<(), String> {
    let mut cancelled = state
        .cancelled_downloads
        .lock()
        .map_err(|_| "Failed to acquire cancelled downloads lock".to_string())?;
    cancelled.remove(workshop_id);
    Ok(())
}

fn is_download_cancelled(state: &crate::AppState, workshop_id: &str) -> Result<bool, String> {
    let cancelled = state
        .cancelled_downloads
        .lock()
        .map_err(|_| "Failed to acquire cancelled downloads lock".to_string())?;
    Ok(cancelled.contains(workshop_id))
}

fn is_dummy_vpk(path: &Path) -> bool {
    if let Ok((files, mut file)) = crate::vpk::parse_vpk(path) {
        let addoninfo_key = files.keys().find(|k| {
            let lower = k.to_lowercase();
            lower == "addoninfo.txt"
                || lower.ends_with("/addoninfo.txt")
                || lower.ends_with("\\addoninfo.txt")
        });
        if let Some(key) = addoninfo_key {
            if let Some(entry) = files.get(key) {
                if let Ok(content_bytes) = crate::vpk::get_file_content(&mut file, entry) {
                    let text = String::from_utf8_lossy(&content_bytes);
                    let parsed = crate::vpk::parse_key_values(&text);
                    return is_dummy_addon_info(&parsed);
                }
            }
        }
    }
    false
}

fn load_known_addons(known_addons_path: &Path) -> HashMap<String, KnownAddonEntry> {
    if known_addons_path.exists() {
        if let Ok(content) = fs::read_to_string(known_addons_path) {
            if let Ok(parsed) = serde_json::from_str::<HashMap<String, KnownAddonEntry>>(&content) {
                return parsed
                    .into_iter()
                    .filter(|(_, entry)| !is_dummy_addon_info(&entry.addon_info))
                    .collect();
            }
        }
    }
    HashMap::new()
}

fn load_settings_store(settings_path: &Path) -> SettingsStore {
    if !settings_path.exists() {
        return SettingsStore::default();
    }

    let Ok(content) = fs::read_to_string(settings_path) else {
        return SettingsStore::default();
    };

    serde_json::from_str::<SettingsStore>(&content).unwrap_or_else(|_| {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
            SettingsStore {
                settings: serde_json::from_value(
                    value.get("settings").cloned().unwrap_or_default(),
                )
                .unwrap_or_default(),
                master_collections: serde_json::from_value(
                    value.get("masterCollections").cloned().unwrap_or_default(),
                )
                .unwrap_or_default(),
            }
        } else {
            SettingsStore::default()
        }
    })
}

fn load_groups(groups_path: &Path) -> Vec<Group> {
    if !groups_path.exists() {
        return Vec::new();
    }

    let Ok(content) = fs::read_to_string(groups_path) else {
        return Vec::new();
    };

    serde_json::from_str::<Vec<Group>>(&content).unwrap_or_else(|_| {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
            serde_json::from_value(value.get("groups").cloned().unwrap_or_default())
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    })
}

fn load_legacy_db_value(runtime_dir: &Path) -> Option<serde_json::Value> {
    let legacy_db_path = runtime_dir.join("db.json");
    if !legacy_db_path.exists() {
        return None;
    }

    let content = fs::read_to_string(legacy_db_path).ok()?;
    serde_json::from_str::<serde_json::Value>(&content).ok()
}

fn load_known_addons_from_legacy_db(
    legacy_db: &serde_json::Value,
) -> HashMap<String, KnownAddonEntry> {
    let mut migrated = HashMap::new();

    if let Some(addons_obj) = legacy_db.get("addons").and_then(|v| v.as_object()) {
        for value in addons_obj.values() {
            if let Ok(mut addon) = serde_json::from_value::<Addon>(value.clone()) {
                if addon.id.is_empty() {
                    addon.id = addon
                        .workshop_id
                        .clone()
                        .unwrap_or_else(|| addon.vpk_name.clone());
                }
                if addon.is_dummy || is_dummy_addon_info(&addon.addon_info) {
                    continue;
                }
                let id = addon.id.clone();
                migrated.insert(
                    id.clone(),
                    KnownAddonEntry {
                        id,
                        vpk_name: addon.vpk_name.clone(),
                        workshop_id: addon.workshop_id.clone(),
                        addon_info: addon.addon_info,
                        has_image: addon.has_image,
                        image_path: addon.image_path,
                        steam_details: addon.steam_details,
                    },
                );
            }
        }
    }

    migrated
}

pub fn load_db(
    settings_path: &Path,
    groups_path: &Path,
    known_addons_path: &Path,
    runtime_dir: &Path,
    app_handle: &AppHandle,
) -> Database {
    let default_loading = runtime_dir
        .join("addons-loading")
        .to_string_lossy()
        .to_string();
    let default_workshop = Path::new(&default_loading)
        .join("workshop")
        .to_string_lossy()
        .to_string();

    let default_db = Database {
        settings: Settings {
            workshop_dir: default_workshop,
            loading_dir: default_loading,
            enable_dummy_bypass: false,
            suppress_sdk_unavailable_warning: false,
            disable_steamworks_sdk: false,
        },
        addons: HashMap::new(),
        groups: Vec::new(),
        known_uninstalled_addons: HashMap::new(),
        master_collections: Vec::new(),
    };

    let legacy_db = load_legacy_db_value(runtime_dir);
    let settings_existed = settings_path.exists();
    let groups_existed = groups_path.exists();
    let known_addons_existed = known_addons_path.exists();

    let mut settings_store = if settings_existed {
        load_settings_store(settings_path)
    } else if let Some(legacy_db) = legacy_db.as_ref() {
        SettingsStore {
            settings: serde_json::from_value(
                legacy_db.get("settings").cloned().unwrap_or_default(),
            )
            .unwrap_or_default(),
            master_collections: serde_json::from_value(
                legacy_db
                    .get("masterCollections")
                    .cloned()
                    .unwrap_or_default(),
            )
            .unwrap_or_default(),
        }
    } else {
        SettingsStore {
            settings: default_db.settings.clone(),
            master_collections: Vec::new(),
        }
    };
    let groups = if groups_existed {
        load_groups(groups_path)
    } else if let Some(legacy_db) = legacy_db.as_ref() {
        serde_json::from_value(legacy_db.get("groups").cloned().unwrap_or_default())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let known_addons: HashMap<String, KnownAddonEntry> = if known_addons_path.exists() {
        load_known_addons(known_addons_path)
    } else if let Some(legacy_db) = legacy_db.as_ref() {
        let migrated = load_known_addons_from_legacy_db(legacy_db);
        if !migrated.is_empty() {
            let _ = fs::write(
                known_addons_path,
                serde_json::to_string_pretty(&migrated).unwrap_or_default(),
            );
        }
        migrated
    } else {
        HashMap::new()
    };

    let old_default_loading = app_handle
        .path()
        .app_data_dir()
        .map(|p| p.join("addons-loading").to_string_lossy().to_string())
        .unwrap_or_default();

    if settings_store.settings.loading_dir.is_empty()
        || settings_store.settings.loading_dir == old_default_loading
    {
        settings_store.settings.loading_dir = default_db.settings.loading_dir.clone();
    }

    let loading_path = Path::new(&settings_store.settings.loading_dir);
    settings_store.settings.workshop_dir =
        loading_path.join("workshop").to_string_lossy().to_string();

    let merged_addons = HashMap::new();
    let mut known_uninstalled_addons = HashMap::new();
    for (id, entry) in &known_addons {
        if !merged_addons.contains_key(id) {
            known_uninstalled_addons.insert(
                id.clone(),
                Addon {
                    id: id.clone(),
                    vpk_name: entry.vpk_name.clone(),
                    workshop_id: entry.workshop_id.clone(),
                    addon_info: entry.addon_info.clone(),
                    has_image: entry.has_image,
                    image_path: entry.image_path.clone(),
                    files_count: 0,
                    file_size: 0,
                    parsed_at: "".to_string(),
                    current_path: "".to_string(),
                    dir_type: "none".to_string(),
                    is_enabled: false,
                    steam_details: entry.steam_details.clone(),
                    workshop_details: None,
                    is_dummy: false,
                },
            );
        }
    }

    let db = Database {
        settings: settings_store.settings,
        addons: merged_addons,
        groups,
        known_uninstalled_addons,
        master_collections: settings_store.master_collections,
    };

    if !settings_existed || !groups_existed || !known_addons_existed {
        save_db_internal(settings_path, groups_path, known_addons_path, &db);
    }

    db
}

pub fn save_db_internal(
    settings_path: &Path,
    groups_path: &Path,
    known_addons_path: &Path,
    db: &Database,
) {
    if let Some(parent) = settings_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Some(parent) = groups_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Some(parent) = known_addons_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let settings_store = SettingsStore {
        settings: db.settings.clone(),
        master_collections: db.master_collections.clone(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&settings_store) {
        let _ = fs::write(settings_path, json);
    }

    if let Ok(json) = serde_json::to_string_pretty(&db.groups) {
        let _ = fs::write(groups_path, json);
    }

    let mut known_addons = load_known_addons(known_addons_path);

    for (id, addon) in &db.addons {
        if addon.workshop_id.is_none() {
            known_addons.remove(&addon.vpk_name);
        }
        known_addons.insert(
            id.clone(),
            KnownAddonEntry {
                id: id.clone(),
                vpk_name: addon.vpk_name.clone(),
                workshop_id: addon.workshop_id.clone(),
                addon_info: addon.addon_info.clone(),
                has_image: addon.has_image,
                image_path: addon.image_path.clone(),
                steam_details: addon.steam_details.clone(),
            },
        );
    }

    for (id, addon) in &db.known_uninstalled_addons {
        if addon.workshop_id.is_none() {
            known_addons.remove(&addon.vpk_name);
        }
        known_addons.insert(
            id.clone(),
            KnownAddonEntry {
                id: id.clone(),
                vpk_name: addon.vpk_name.clone(),
                workshop_id: addon.workshop_id.clone(),
                addon_info: addon.addon_info.clone(),
                has_image: addon.has_image,
                image_path: addon.image_path.clone(),
                steam_details: addon.steam_details.clone(),
            },
        );
    }

    if let Ok(json) = serde_json::to_string_pretty(&known_addons) {
        let _ = fs::write(known_addons_path, json);
    }
}

fn load_workshop_cache(cache_path: &Path) -> HashMap<String, serde_json::Value> {
    if !cache_path.exists() {
        return HashMap::new();
    }

    let Ok(content) = fs::read_to_string(cache_path) else {
        return HashMap::new();
    };

    if let Ok(map) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&content) {
        return map;
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
        if let Some(items) = value.get("items").and_then(|v| v.as_object()) {
            return items
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
        }
    }

    HashMap::new()
}

fn save_workshop_cache(
    cache_path: &Path,
    cache: &HashMap<String, serde_json::Value>,
) -> Result<(), String> {
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create cache directory: {}", e))?;
    }
    let json = serde_json::to_string_pretty(cache)
        .map_err(|e| format!("Failed to serialize workshop cache: {}", e))?;
    fs::write(cache_path, json).map_err(|e| format!("Failed to write workshop cache: {}", e))
}

fn insert_non_empty_string(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: &str,
) {
    let value = value.trim();
    if !value.is_empty() {
        obj.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
}

fn insert_optional_value(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<serde_json::Value>,
) {
    if let Some(value) = value {
        if !value.is_null() {
            obj.insert(key.to_string(), value);
        }
    }
}

fn insert_optional_u64(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<u64>,
) {
    if let Some(value) = value {
        obj.insert(key.to_string(), serde_json::json!(value));
    }
}

fn insert_optional_vec_string(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<Vec<String>>,
) {
    if let Some(values) = value {
        let cleaned: Vec<String> = values
            .into_iter()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect();
        if !cleaned.is_empty() {
            obj.insert(key.to_string(), serde_json::json!(cleaned));
        }
    }
}

fn cache_entry_object<'a>(
    cache: &'a mut HashMap<String, serde_json::Value>,
    workshop_id: &str,
) -> &'a mut serde_json::Map<String, serde_json::Value> {
    let entry = cache
        .entry(workshop_id.to_string())
        .or_insert_with(|| serde_json::json!({ "workshopId": workshop_id }));
    if !entry.is_object() {
        *entry = serde_json::json!({ "workshopId": workshop_id });
    }
    entry
        .as_object_mut()
        .expect("workshop cache entry must be an object")
}

fn append_workshop_crawl_log_internal(
    crawl_log_path: &Path,
    record: serde_json::Value,
) -> Result<(), String> {
    if let Some(parent) = crawl_log_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create crawl log directory: {}", e))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(crawl_log_path)
        .map_err(|e| format!("Failed to open crawl log: {}", e))?;
    let line = serde_json::to_string(&record)
        .map_err(|e| format!("Failed to serialize crawl log record: {}", e))?;
    writeln!(file, "{}", line).map_err(|e| format!("Failed to append crawl log: {}", e))
}

fn merge_workshop_details_into_addon(addon: &mut Addon, details: &serde_json::Value) {
    addon.workshop_details = Some(details.clone());

    if addon.image_path.is_none() {
        if let Some(preview_url) = details
            .get("previewUrl")
            .or_else(|| details.get("imagePath"))
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
        {
            addon.image_path = Some(preview_url.to_string());
            addon.has_image = true;
        }
    }

    let creator_name = details
        .get("creatorName")
        .or_else(|| details.get("authorName"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string());

    let creator_id = details
        .get("creatorSteamId")
        .or_else(|| details.get("creatorId"))
        .or_else(|| details.get("authorId"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string());

    let creator_vanity_id = details
        .get("creatorVanityId")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string());

    let creator_account_id = details
        .get("creatorAccountId")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string());

    if creator_name.is_some() || creator_id.is_some() {
        let mut steam_details = addon
            .steam_details
            .clone()
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
        if !steam_details.is_object() {
            steam_details = serde_json::Value::Object(serde_json::Map::new());
        }
        if let Some(obj) = steam_details.as_object_mut() {
            if let Some(name) = creator_name {
                let current = obj
                    .get("creator_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if current.trim().is_empty() || current == creator_id.as_deref().unwrap_or_default()
                {
                    obj.insert("creator_name".to_string(), serde_json::Value::String(name));
                }
            }
            if let Some(id) = creator_id {
                obj.entry("creator".to_string())
                    .or_insert_with(|| serde_json::Value::String(id));
            }
            if let Some(vanity_id) = creator_vanity_id {
                obj.entry("creator_vanity_id".to_string())
                    .or_insert_with(|| serde_json::Value::String(vanity_id));
            }
            if let Some(account_id) = creator_account_id {
                obj.entry("creator_account_id".to_string())
                    .or_insert_with(|| serde_json::Value::String(account_id));
            }
            if let Some(description) = details
                .get("description")
                .and_then(|v| v.as_str())
                .filter(|v| !v.trim().is_empty())
            {
                obj.entry("description".to_string())
                    .or_insert_with(|| serde_json::Value::String(description.trim().to_string()));
            }
        }
        addon.steam_details = Some(steam_details);
    }
}

fn database_with_workshop_cache(db: &Database, workshop_cache_path: &Path) -> Database {
    let cache = load_workshop_cache(workshop_cache_path);
    if cache.is_empty() {
        return db.clone();
    }

    let mut response = db.clone();
    for addon in response.addons.values_mut() {
        if let Some(workshop_id) = addon.workshop_id.as_deref() {
            if let Some(details) = cache.get(workshop_id) {
                merge_workshop_details_into_addon(addon, details);
            }
        }
    }
    for addon in response.known_uninstalled_addons.values_mut() {
        if let Some(workshop_id) = addon.workshop_id.as_deref() {
            if let Some(details) = cache.get(workshop_id) {
                merge_workshop_details_into_addon(addon, details);
            }
        }
    }
    response
}

pub async fn rescan_database_snapshot(app_handle: &AppHandle) -> Result<(Database, bool), String> {
    let state = app_handle.state::<crate::AppState>();
    let mut db = state.db.lock().await;
    let before = database_with_workshop_cache(&db, &state.workshop_cache_path);
    scan_addons_internal(
        &mut db,
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &state.cache_dir,
        &state.workshop_service,
    )
    .await?;
    let after = database_with_workshop_cache(&db, &state.workshop_cache_path);
    let changed = after != before;
    Ok((after, changed))
}

async fn fetch_steam_details(workshop_ids: &[String]) -> Result<Vec<serde_json::Value>, String> {
    fetch_steam_details_web(workshop_ids).await
}

async fn fetch_steam_details_hybrid(
    workshop_service: &WorkshopService,
    workshop_ids: &[String],
    allow_bridge: bool,
) -> Result<Vec<Value>, String> {
    if workshop_ids.is_empty() {
        return Ok(Vec::new());
    }

    if !allow_bridge {
        return fetch_steam_details(workshop_ids).await;
    }

    match workshop_service.bridge_fetch_details(workshop_ids) {
        Ok(details) if !details.is_empty() => Ok(details),
        Ok(_) | Err(_) => fetch_steam_details(workshop_ids).await,
    }
}

async fn fetch_collection_children_hybrid(
    workshop_service: &WorkshopService,
    collection_id: &str,
    allow_bridge: bool,
) -> Result<Vec<String>, String> {
    if !allow_bridge {
        return fetch_collection_children_web(collection_id).await;
    }

    if let Ok(payload) = workshop_service.bridge_query_collection(collection_id) {
        let mut ids = Vec::new();
        for item in payload.items {
            if let Some(id) = item.get("publishedfileid").and_then(|v| v.as_str()) {
                ids.push(id.to_string());
            }
        }
        if !ids.is_empty() {
            return Ok(ids);
        }
    }

    fetch_collection_children_web(collection_id).await
}

async fn attempt_bridge_download(
    state: &crate::AppState,
    workshop_service: &WorkshopService,
    workshop_id: &str,
    workshop_dir: &Path,
    app_handle: &AppHandle,
    allow_bridge: bool,
) -> Result<bool, String> {
    if !allow_bridge || !workshop_service.has_bridge() {
        return Ok(false);
    }

    workshop_service.bridge_request_download(workshop_id)?;

    #[derive(Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    struct DownloadProgress {
        workshop_id: String,
        percent: u32,
        downloaded: u64,
        total: u64,
        source: String,
        phase: String,
    }

    let expected_path = workshop_dir.join(format!("{}.vpk", workshop_id));
    let expected_disabled_path = workshop_dir.join(format!("{}.vpk.disabled", workshop_id));

    for _ in 0..40 {
        if is_download_cancelled(state, workshop_id)? {
            return Err(DOWNLOAD_CANCELLED_ERR.to_string());
        }

        let status: BridgeDownloadStatus =
            match workshop_service.bridge_download_status(workshop_id) {
                Ok(status) => status,
                Err(err) => return Err(err),
            };

        if let (Some(downloaded), Some(total)) = (status.downloaded, status.total) {
            let percent = if total > 0 {
                ((downloaded as f64 / total as f64) * 100.0) as u32
            } else {
                0
            };
            let _ = app_handle.emit(
                "download-progress",
                DownloadProgress {
                    workshop_id: workshop_id.to_string(),
                    percent,
                    downloaded,
                    total,
                    source: "steam-sdk".to_string(),
                    phase: "download".to_string(),
                },
            );
        }

        if expected_path.exists() || expected_disabled_path.exists() {
            let _ = app_handle.emit(
                "download-progress",
                DownloadProgress {
                    workshop_id: workshop_id.to_string(),
                    percent: 100,
                    downloaded: status.total.unwrap_or(0),
                    total: status.total.unwrap_or(0),
                    source: "steam-sdk".to_string(),
                    phase: "download".to_string(),
                },
            );
            return Ok(true);
        }

        if status.installed && expected_path.exists() {
            return Ok(true);
        }

        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    Ok(false)
}

pub async fn scan_addons_internal(
    db: &mut Database,
    settings_path: &Path,
    groups_path: &Path,
    known_addons_path: &Path,
    cache_dir: &Path,
    workshop_service: &WorkshopService,
) -> Result<(), String> {
    let allow_bridge = !db.settings.disable_steamworks_sdk;
    let workshop_dir = Path::new(&db.settings.workshop_dir);
    let loading_dir = Path::new(&db.settings.loading_dir);

    if !workshop_dir.exists() {
        let _ = fs::create_dir_all(workshop_dir);
    }
    if !loading_dir.exists() {
        let _ = fs::create_dir_all(loading_dir);
    }
    if !cache_dir.exists() {
        let _ = fs::create_dir_all(cache_dir);
    }

    struct DiskFileInfo {
        vpk_name: String,
        full_path: PathBuf,
        size: u64,
        dir_type: String,
        is_enabled: bool,
    }

    let mut files_on_disk = Vec::new();

    // Scan workshop dir
    if let Ok(entries) = fs::read_dir(workshop_dir) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_file() {
                    let path = entry.path();
                    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if filename.ends_with(".vpk") || filename.ends_with(".vpk.disabled") {
                        let is_enabled = filename.ends_with(".vpk");
                        let vpk_name = if is_enabled {
                            filename.to_string()
                        } else {
                            filename.strip_suffix(".disabled").unwrap().to_string()
                        };
                        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                        files_on_disk.push(DiskFileInfo {
                            vpk_name,
                            full_path: path,
                            size,
                            dir_type: "workshop".to_string(),
                            is_enabled,
                        });
                    }
                }
            }
        }
    }

    // Scan loading dir
    if let Ok(entries) = fs::read_dir(loading_dir) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_file() {
                    let path = entry.path();
                    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if filename.ends_with(".vpk") || filename.ends_with(".vpk.disabled") {
                        let is_enabled = filename.ends_with(".vpk");
                        let vpk_name = if is_enabled {
                            filename.to_string()
                        } else {
                            filename.strip_suffix(".disabled").unwrap().to_string()
                        };
                        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                        files_on_disk.push(DiskFileInfo {
                            vpk_name,
                            full_path: path,
                            size,
                            dir_type: "loading".to_string(),
                            is_enabled,
                        });
                    }
                }
            }
        }
    }

    let mut known_addons = load_known_addons(known_addons_path);

    let mut active_addons = HashMap::new();
    let mut new_workshop_ids = HashSet::new();

    for file_info in files_on_disk {
        let vpk_name = file_info.vpk_name.clone();
        let cached = db
            .addons
            .values()
            .find(|a| a.vpk_name == vpk_name)
            .cloned()
            .or_else(|| {
                db.known_uninstalled_addons
                    .values()
                    .find(|a| a.vpk_name == vpk_name)
                    .cloned()
            });

        let has_capitalized_keys = cached.as_ref().is_some_and(|addon| {
            addon
                .addon_info
                .as_object()
                .is_some_and(|obj| obj.keys().any(|k| k.chars().any(|c| c.is_uppercase())))
        });

        let needs_metadata = match &cached {
            Some(addon) => {
                addon.addon_info.is_null()
                    || addon.addon_info.as_object().map_or(true, |m| m.is_empty())
                    || has_capitalized_keys
                    || (addon.image_path.is_none() && !addon.has_image)
                    || (addon.has_image
                        && addon.image_path.as_ref().map_or(true, |p| {
                            p.starts_with("/cache/")
                                && !cache_dir.join(p.trim_start_matches("/cache/")).exists()
                        }))
            }
            None => true,
        };

        if needs_metadata {
            println!("Parsing metadata for: {}", vpk_name);

            let mut workshop_id = None;
            let base_name = Path::new(&vpk_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if base_name.chars().all(|c| c.is_ascii_digit()) {
                workshop_id = Some(base_name.to_string());
            }

            let meta = extract_addon_metadata(&file_info.full_path, cache_dir);

            // Fallback: extract workshop ID from url keys inside addon_info (all keys are lowercase now)
            if workshop_id.is_none() {
                if let Some(url_val) = meta
                    .addon_info
                    .get("addonurl0")
                    .or_else(|| meta.addon_info.get("addonurl"))
                {
                    if let Some(url_str) = url_val.as_str() {
                        workshop_id = extract_workshop_id_from_url(url_str);
                    }
                }
            }

            let is_dummy = meta
                .addon_info
                .get("addondescription")
                .and_then(|v| v.as_str())
                .map(|v| v == "A dummy addon generated by Left 4 Addons")
                .unwrap_or(false);

            let id = workshop_id.clone().unwrap_or_else(|| meta.hash.clone());

            if workshop_id.is_none() && !id.is_empty() {
                known_addons.remove(&vpk_name);
            }

            let entry = known_addons.get(&id);
            let addon_info = entry
                .map(|e| e.addon_info.clone())
                .unwrap_or(meta.addon_info);
            let has_image = entry.map(|e| e.has_image).unwrap_or(meta.has_image);
            let image_path = entry.and_then(|e| e.image_path.clone()).or(meta.image_path);
            let steam_details = entry.and_then(|e| e.steam_details.clone());

            let addon = Addon {
                id: id.clone(),
                vpk_name: vpk_name.clone(),
                workshop_id: workshop_id.clone(),
                addon_info,
                has_image,
                image_path,
                files_count: meta.files_count,
                file_size: file_info.size,
                parsed_at: chrono::Utc::now().to_rfc3339(),
                current_path: file_info.full_path.to_string_lossy().to_string(),
                dir_type: file_info.dir_type.clone(),
                is_enabled: file_info.is_enabled,
                steam_details,
                workshop_details: None,
                is_dummy,
            };

            if let Some(ref w_id) = workshop_id {
                if !is_dummy {
                    new_workshop_ids.insert(w_id.clone());
                }
            }

            if !is_dummy {
                known_addons.insert(
                    id.clone(),
                    KnownAddonEntry {
                        id: id.clone(),
                        vpk_name: vpk_name.clone(),
                        workshop_id: workshop_id.clone(),
                        addon_info: addon.addon_info.clone(),
                        has_image: addon.has_image,
                        image_path: addon.image_path.clone(),
                        steam_details: addon.steam_details.clone(),
                    },
                );
            }

            active_addons.insert(id.clone(), addon);
        } else if let Some(mut addon) = cached {
            if addon.workshop_id.is_none()
                && (addon.id.ends_with(".vpk")
                    || addon.id.ends_with(".vpk.disabled")
                    || addon.id.len() != 32)
            {
                let meta = extract_addon_metadata(&file_info.full_path, cache_dir);
                let hash = meta.hash;
                if !hash.is_empty() {
                    let old_id = addon.id.clone();
                    addon.id = hash.clone();
                    known_addons.remove(&old_id);
                    if !addon.is_dummy && !is_dummy_addon_info(&addon.addon_info) {
                        known_addons.insert(
                            hash.clone(),
                            KnownAddonEntry {
                                id: hash.clone(),
                                vpk_name: addon.vpk_name.clone(),
                                workshop_id: None,
                                addon_info: addon.addon_info.clone(),
                                has_image: addon.has_image,
                                image_path: addon.image_path.clone(),
                                steam_details: addon.steam_details.clone(),
                            },
                        );
                    }
                }
            }

            addon.file_size = file_info.size;
            addon.current_path = file_info.full_path.to_string_lossy().to_string();
            addon.dir_type = file_info.dir_type;
            addon.is_enabled = file_info.is_enabled;
            addon.is_dummy = addon
                .addon_info
                .get("addondescription")
                .and_then(|v| v.as_str())
                .map(|v| v == "A dummy addon generated by Left 4 Addons")
                .unwrap_or(false);

            if addon.workshop_id.is_none() {
                if let Some(url_val) = addon
                    .addon_info
                    .get("addonurl0")
                    .or_else(|| addon.addon_info.get("addonurl"))
                {
                    if let Some(url_str) = url_val.as_str() {
                        addon.workshop_id = extract_workshop_id_from_url(url_str);
                        if let Some(ref w_id) = addon.workshop_id {
                            if !addon.is_dummy {
                                new_workshop_ids.insert(w_id.clone());
                            }
                        }
                    }
                }
            }

            let id = addon.id.clone();
            active_addons.insert(id, addon);
        }
    }

    if !new_workshop_ids.is_empty() {
        let ids_array: Vec<String> = new_workshop_ids.into_iter().collect();
        println!("Syncing Steam details for {} items...", ids_array.len());
        if let Ok(steam_details_list) =
            fetch_steam_details_hybrid(workshop_service, &ids_array, allow_bridge).await
        {
            for details in steam_details_list {
                if let Some(w_id) = details.get("publishedfileid").and_then(|id| id.as_str()) {
                    for addon in active_addons.values_mut() {
                        if addon.workshop_id.as_deref() == Some(w_id) {
                            addon.steam_details = Some(details.clone());
                            if !addon.has_image {
                                if let Some(preview_url) =
                                    details.get("preview_url").and_then(|u| u.as_str())
                                {
                                    addon.image_path = Some(preview_url.to_string());
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }
    }

    let mut uninstalled = HashMap::new();
    for (id, entry) in &known_addons {
        if !active_addons.contains_key(id) {
            uninstalled.insert(
                id.clone(),
                Addon {
                    id: id.clone(),
                    vpk_name: entry.vpk_name.clone(),
                    workshop_id: entry.workshop_id.clone(),
                    addon_info: entry.addon_info.clone(),
                    has_image: entry.has_image,
                    image_path: entry.image_path.clone(),
                    files_count: 0,
                    file_size: 0,
                    parsed_at: "".to_string(),
                    current_path: "".to_string(),
                    dir_type: "none".to_string(),
                    is_enabled: false,
                    steam_details: entry.steam_details.clone(),
                    workshop_details: None,
                    is_dummy: false,
                },
            );
        }
    }
    for (id, addon) in &db.known_uninstalled_addons {
        if !active_addons.contains_key(id) && !uninstalled.contains_key(id) {
            uninstalled.insert(id.clone(), addon.clone());
        }
    }

    db.addons = active_addons;
    db.known_uninstalled_addons = uninstalled;

    // Sync Steam details for all addons with workshopId but no steamDetails
    let mut ids_to_sync: Vec<String> = Vec::new();
    for addon in db.addons.values() {
        if addon.workshop_id.is_some() && addon.steam_details.is_none() && !addon.is_dummy {
            ids_to_sync.push(addon.workshop_id.clone().unwrap());
        }
    }
    for addon in db.known_uninstalled_addons.values() {
        if addon.workshop_id.is_some() && addon.steam_details.is_none() {
            ids_to_sync.push(addon.workshop_id.clone().unwrap());
        }
    }
    if !ids_to_sync.is_empty() {
        ids_to_sync.sort();
        ids_to_sync.dedup();
        println!("Syncing Steam details for {} items...", ids_to_sync.len());
        if let Ok(steam_details_list) =
            fetch_steam_details_hybrid(workshop_service, &ids_to_sync, allow_bridge).await
        {
            for details in steam_details_list {
                if let Some(w_id) = details.get("publishedfileid").and_then(|id| id.as_str()) {
                    // Update installed addons
                    for addon in db.addons.values_mut() {
                        if addon.workshop_id.as_deref() == Some(w_id) {
                            addon.steam_details = Some(details.clone());
                            if !addon.has_image {
                                if let Some(preview_url) =
                                    details.get("preview_url").and_then(|u| u.as_str())
                                {
                                    addon.image_path = Some(preview_url.to_string());
                                }
                            }
                            break;
                        }
                    }
                    // Update uninstalled addons
                    for addon in db.known_uninstalled_addons.values_mut() {
                        if addon.workshop_id.as_deref() == Some(w_id) {
                            addon.steam_details = Some(details.clone());
                            if !addon.has_image {
                                if let Some(preview_url) =
                                    details.get("preview_url").and_then(|u| u.as_str())
                                {
                                    addon.image_path = Some(preview_url.to_string());
                                    addon.has_image = true;
                                }
                            }
                            if let Some(file_size_str) =
                                details.get("file_size").and_then(|v| v.as_str())
                            {
                                if let Ok(file_size) = file_size_str.parse::<u64>() {
                                    if file_size > 0 {
                                        addon.file_size = file_size;
                                    }
                                }
                            }
                            // Update vpk_name to title if it's still just a workshop ID
                            if addon.vpk_name == w_id || addon.vpk_name == format!("{}.vpk", w_id) {
                                if let Some(title) = details.get("title").and_then(|v| v.as_str()) {
                                    if !title.is_empty() {
                                        addon.vpk_name = title.to_string();
                                    }
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }
    }

    let mut vpk_to_id = HashMap::new();
    for addon in db.addons.values() {
        vpk_to_id.insert(addon.vpk_name.clone(), addon.id.clone());
    }
    for addon in db.known_uninstalled_addons.values() {
        vpk_to_id.insert(addon.vpk_name.clone(), addon.id.clone());
    }

    for g in &mut db.groups {
        g.addons = g
            .addons
            .iter()
            .map(|item| {
                let clean_item = item.strip_suffix(".disabled").unwrap_or(item);
                if let Some(new_id) = vpk_to_id.get(clean_item) {
                    new_id.clone()
                } else {
                    item.clone()
                }
            })
            .collect();
    }
    db.groups.retain(|g| !g.addons.is_empty());

    save_db_internal(settings_path, groups_path, known_addons_path, db);
    Ok(())
}

fn extract_workshop_id_from_url(url: &str) -> Option<String> {
    if let Some(pos) = url.find("id=") {
        let start = pos + 3;
        let end = url[start..]
            .find('&')
            .map(|idx| start + idx)
            .unwrap_or(url.len());
        let id_str = &url[start..end];
        if id_str.chars().all(|c| c.is_ascii_digit()) && !id_str.is_empty() {
            return Some(id_str.to_string());
        }
    }
    None
}

fn clean_group_name(name: &str) -> String {
    let mut s = name.trim().to_string();

    let re_ver = Regex::new(r"(?i)\s+v?\d+(?:\.\d+)*$").unwrap();
    let re_part_num = Regex::new(r"(?i)(?:[-#_/:,\s]+(?:part|pt|partie|pts|vol|volume|chapter|ch|act)?\s*(?:\d+|[ivxldcm]+)(?:\/\d+)?)$").unwrap();
    let re_part_word =
        Regex::new(r"(?i)(?:[-#_/:,\s]+(?:part|pt|partie|pts|vol|volume|chapter|ch|act))$")
            .unwrap();

    loop {
        let prev_len = s.len();

        s = re_ver.replace(&s, "").into_owned();
        s = re_part_num.replace(&s, "").into_owned();
        s = re_part_word.replace(&s, "").into_owned();

        s = s
            .trim_end_matches(|c: char| {
                c.is_whitespace()
                    || c == ':'
                    || c == '_'
                    || c == '-'
                    || c == '/'
                    || c == '\\'
                    || c == '#'
                    || c == '+'
                    || c == ','
                    || c == '.'
            })
            .to_string();

        if s.len() == prev_len {
            break;
        }
    }

    s
}

fn auto_group_internal(db: &mut Database) {
    let mut grouped_vpks = HashSet::new();
    for g in &db.groups {
        for addon in &g.addons {
            grouped_vpks.insert(addon.clone());
        }
    }

    let mut ungrouped = Vec::new();
    for (name, addon) in &db.addons {
        if !grouped_vpks.contains(name) {
            ungrouped.push(addon.clone());
        }
    }

    struct Candidate {
        id: String,
        title: String,
        description: String,
    }

    let mut candidates = Vec::new();
    for addon in ungrouped {
        let title = addon
            .steam_details
            .as_ref()
            .and_then(|d| d.get("title").and_then(|t| t.as_str()))
            .or_else(|| addon.addon_info.get("addontitle").and_then(|t| t.as_str()))
            .unwrap_or(&addon.vpk_name)
            .to_string();

        let description = addon
            .addon_info
            .get("addondescription")
            .and_then(|t| t.as_str())
            .or_else(|| {
                addon
                    .addon_info
                    .get("addontagline")
                    .and_then(|t| t.as_str())
            })
            .or_else(|| {
                addon
                    .steam_details
                    .as_ref()
                    .and_then(|d| d.get("description").and_then(|t| t.as_str()))
            })
            .unwrap_or("")
            .to_string();

        candidates.push(Candidate {
            id: addon.id.clone(),
            title,
            description,
        });
    }

    let mut desc_groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, c) in candidates.iter().enumerate() {
        let desc_trim = c.description.trim();
        if desc_trim.len() > 10 {
            let key = desc_trim.to_lowercase();
            desc_groups.entry(key).or_default().push(i);
        }
    }

    let mut indices_to_remove = HashSet::new();

    for (_desc, idxs) in desc_groups {
        if idxs.len() >= 2 {
            let mut common_prefix = candidates[idxs[0]].title.clone();
            for &idx in &idxs[1..] {
                let current_title = &candidates[idx].title;
                let mut common_len = 0;
                for (c1, c2) in common_prefix.chars().zip(current_title.chars()) {
                    if c1 == c2 {
                        common_len += c1.len_utf8();
                    } else {
                        break;
                    }
                }
                common_prefix.truncate(common_len);
            }

            let mut group_name = clean_group_name(&common_prefix);
            if group_name.is_empty() {
                group_name = "Campaign Pack".to_string();
            }

            let group_id = format!(
                "group_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );

            let mut addons = Vec::new();
            for &idx in &idxs {
                addons.push(candidates[idx].id.clone());
                indices_to_remove.insert(idx);
            }

            db.groups.push(Group {
                id: group_id,
                name: group_name,
                addons,
                tags: None,
                workshop_collection_id: None,
                master_collection_ids: None,
                source: Some("auto-group".to_string()),
            });
        }
    }

    let mut remaining_candidates = Vec::new();
    for (i, c) in candidates.into_iter().enumerate() {
        if !indices_to_remove.contains(&i) {
            remaining_candidates.push(c);
        }
    }

    let re_part = Regex::new(r"(?i)^(.*?)\s*(?:[-#_]*\s*(?:part|pt|partie|pts)\s*(\d+|[ivxldcm]+)(?:\/\d+)?|\s+v?\d+\.\d+|\s+v\d+)$").unwrap();
    let mut title_groups: HashMap<String, Vec<String>> = HashMap::new();
    for c in remaining_candidates {
        if let Some(caps) = re_part.captures(&c.title) {
            let prefix = caps.get(1).unwrap().as_str().trim().to_string();
            if prefix.len() >= 3 {
                let cleaned_prefix = clean_group_name(&prefix);
                if cleaned_prefix.len() >= 3 {
                    title_groups.entry(cleaned_prefix).or_default().push(c.id);
                }
            }
        }
    }

    for (prefix, addons) in title_groups {
        if addons.len() >= 2 {
            let group_id = format!(
                "group_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );

            db.groups.push(Group {
                id: group_id,
                name: prefix,
                addons,
                tags: None,
                workshop_collection_id: None,
                master_collection_ids: None,
                source: Some("auto-group".to_string()),
            });
        }
    }
}

#[tauri::command]
pub async fn get_settings(state: State<'_, crate::AppState>) -> Result<Settings, String> {
    let db = state.db.lock().await;
    Ok(db.settings.clone())
}

#[tauri::command]
pub async fn save_settings(
    loading_dir: String,
    enable_dummy_bypass: bool,
    suppress_sdk_unavailable_warning: bool,
    disable_steamworks_sdk: bool,
    state: State<'_, crate::AppState>,
    app_handle: AppHandle,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;
    let loading_path = PathBuf::from(&loading_dir);
    let workshop_dir = loading_path.join("workshop").to_string_lossy().to_string();

    db.settings.workshop_dir = workshop_dir;
    db.settings.loading_dir = loading_dir;
    db.settings.enable_dummy_bypass = enable_dummy_bypass;
    db.settings.suppress_sdk_unavailable_warning = suppress_sdk_unavailable_warning;
    db.settings.disable_steamworks_sdk = disable_steamworks_sdk;
    if disable_steamworks_sdk {
        state.workshop_service.shutdown();
    }
    save_db_internal(
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &db,
    );

    scan_addons_internal(
        &mut db,
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &state.cache_dir,
        &state.workshop_service,
    )
    .await?;
    if let Err(err) = crate::watcher::rebind_addon_watcher(&app_handle, &loading_path) {
        crate::watcher::emit_watch_error(&app_handle, &err);
    }
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
    ))
}

#[tauri::command]
pub async fn get_addons(state: State<'_, crate::AppState>) -> Result<Database, String> {
    let mut db = state.db.lock().await;
    scan_addons_internal(
        &mut db,
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &state.cache_dir,
        &state.workshop_service,
    )
    .await?;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
    ))
}

#[tauri::command]
pub async fn get_cache_image(
    image_path: String,
    state: State<'_, crate::AppState>,
) -> Result<Vec<u8>, String> {
    let file_path = resolve_cache_file(&state.cache_dir, &image_path)?;
    fs::read(&file_path).map_err(|e| format!("Failed to read cache image: {}", e))
}

#[tauri::command]
pub async fn move_addons(
    ids: Vec<String>,
    target_dir_type: String,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;

    if target_dir_type != "loading" && target_dir_type != "workshop" {
        return Err(format!(
            "Invalid target directory type: {}",
            target_dir_type
        ));
    }

    let target_dir = if target_dir_type == "loading" {
        PathBuf::from(&db.settings.loading_dir)
    } else {
        PathBuf::from(&db.settings.workshop_dir)
    };

    if !target_dir.exists() {
        fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
    }

    struct MovePlan {
        id: String,
        current_path: PathBuf,
        dest_path: PathBuf,
        from_dir_type: String,
        workshop_id: Option<String>,
    }

    let mut plans = Vec::new();
    let mut errors = Vec::new();

    for id in ids {
        let Some(addon) = db.addons.get(&id) else {
            errors.push(format!("Addon not found: {}", id));
            continue;
        };

        let current_path = PathBuf::from(&addon.current_path);
        if !current_path.exists() {
            errors.push(format!("Source file does not exist for {}", addon.vpk_name));
            continue;
        }

        let Some(file_name) = current_path.file_name() else {
            errors.push(format!("Invalid source file path for {}", addon.vpk_name));
            continue;
        };

        let dest_path = target_dir.join(file_name);
        if dest_path.exists() && !(target_dir_type == "workshop" && is_dummy_vpk(&dest_path)) {
            errors.push(format!(
                "Target file already exists: {}",
                dest_path.display()
            ));
            continue;
        }

        plans.push(MovePlan {
            id,
            current_path,
            dest_path,
            from_dir_type: addon.dir_type.clone(),
            workshop_id: addon.workshop_id.clone(),
        });
    }

    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }

    crate::watcher::suppress_internal_refresh(&state);
    for plan in plans {
        if target_dir_type == "workshop" && plan.dest_path.exists() && is_dummy_vpk(&plan.dest_path)
        {
            fs::remove_file(&plan.dest_path)
                .map_err(|e| format!("Failed to remove dummy addon for {}: {}", plan.id, e))?;
        }

        fs::rename(&plan.current_path, &plan.dest_path)
            .map_err(|e| format!("Failed to move {}: {}", plan.id, e))?;

        if target_dir_type == "loading"
            && plan.from_dir_type == "workshop"
            && db.settings.enable_dummy_bypass
        {
            if let Some(ref w_id) = plan.workshop_id {
                let workshop_dir = PathBuf::from(&db.settings.workshop_dir);
                let dummy_vpk_path = workshop_dir.join(format!("{}.vpk", w_id));
                generate_dummy_vpk(&plan.dest_path, &dummy_vpk_path).map_err(|e| {
                    format!("Failed to generate dummy addon for {}: {}", plan.id, e)
                })?;
            }
        }
    }

    scan_addons_internal(
        &mut db,
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &state.cache_dir,
        &state.workshop_service,
    )
    .await?;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
    ))
}

#[tauri::command]
pub async fn toggle_addons(
    ids: Vec<String>,
    enabled: bool,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;

    let mut plans = Vec::new();
    let mut errors = Vec::new();

    for id in ids {
        let Some(addon) = db.addons.get(&id) else {
            errors.push(format!("Addon not found: {}", id));
            continue;
        };

        let current_path = PathBuf::from(&addon.current_path);
        if !current_path.exists() {
            errors.push(format!("Source file does not exist for {}", addon.vpk_name));
            continue;
        }

        let Some(current_dir) = current_path.parent() else {
            errors.push(format!("Invalid source file path for {}", addon.vpk_name));
            continue;
        };

        let dest_path = if enabled {
            if !addon.current_path.ends_with(".disabled") {
                continue;
            }
            current_dir.join(&addon.vpk_name)
        } else {
            if addon.current_path.ends_with(".disabled") {
                continue;
            }
            current_dir.join(format!("{}.disabled", addon.vpk_name))
        };

        if dest_path.exists() {
            errors.push(format!(
                "Target file already exists: {}",
                dest_path.display()
            ));
            continue;
        }

        plans.push((id, current_path, dest_path));
    }

    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }

    crate::watcher::suppress_internal_refresh(&state);
    for (id, current_path, dest_path) in plans {
        fs::rename(&current_path, &dest_path)
            .map_err(|e| format!("Failed to toggle {}: {}", id, e))?;
    }

    scan_addons_internal(
        &mut db,
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &state.cache_dir,
        &state.workshop_service,
    )
    .await?;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
    ))
}

#[tauri::command]
pub async fn rename_addon(
    id: String,
    new_vpk_name: String,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;

    let sanitize_filename = |name: &str| {
        let re = Regex::new(r"[\\/:*?<>|]").unwrap();
        let s = re.replace_all(name, "_").trim().to_string();
        if s.ends_with(".vpk") {
            s
        } else {
            format!("{}.vpk", s)
        }
    };

    let sanitized = sanitize_filename(&new_vpk_name);

    if let Some(addon) = db.addons.get(&id).cloned() {
        let current_path = PathBuf::from(&addon.current_path);
        if current_path.exists() {
            crate::watcher::suppress_internal_refresh(&state);
            let dir = current_path.parent().unwrap();
            let new_filename = if addon.is_enabled {
                sanitized.clone()
            } else {
                format!("{}.disabled", sanitized)
            };
            let dest_path = dir.join(&new_filename);

            if dest_path.exists() {
                return Err(format!("A file named \"{}\" already exists", new_filename));
            }

            fs::rename(&current_path, &dest_path).map_err(|e| e.to_string())?;

            let mut updated_addon = addon.clone();
            updated_addon.vpk_name = sanitized.clone();
            updated_addon.current_path = dest_path.to_string_lossy().to_string();

            db.addons.insert(id.clone(), updated_addon);

            let old_base = Path::new(&addon.vpk_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .replace(".disabled", "");
            let new_base = Path::new(&sanitized)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");

            use md5::{Digest, Md5};
            let mut hasher_old = Md5::new();
            hasher_old.update(old_base.as_bytes());
            let old_hash = hasher_old.finalize();

            let mut hasher_new = Md5::new();
            hasher_new.update(new_base.as_bytes());
            let new_hash = hasher_new.finalize();

            let old_img = state.cache_dir.join(format!("{:x}_image.jpg", old_hash));
            let new_img = state.cache_dir.join(format!("{:x}_image.jpg", new_hash));

            if old_img.exists() && fs::rename(&old_img, &new_img).is_ok() {
                if let Some(addon_ref) = db.addons.get_mut(&id) {
                    addon_ref.image_path = Some(format!("/cache/{:x}_image.jpg", new_hash));
                }
            }
        }
    } else {
        return Err("Addon not found".to_string());
    }

    scan_addons_internal(
        &mut db,
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &state.cache_dir,
        &state.workshop_service,
    )
    .await?;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
    ))
}

#[tauri::command]
pub async fn rename_addons(
    renames: Vec<RenameItem>,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;

    let sanitize_filename = |name: &str| {
        let re = Regex::new(r"[\\/:*?<>|]").unwrap();
        let s = re.replace_all(name, "_").trim().to_string();
        if s.ends_with(".vpk") {
            s
        } else {
            format!("{}.vpk", s)
        }
    };

    struct RenamePlan {
        id: String,
        current_path: PathBuf,
        dest_path: PathBuf,
        sanitized: String,
        old_vpk_name: String,
    }

    let mut plans = Vec::new();
    let mut errors = Vec::new();
    let mut destination_paths = HashSet::new();

    for item in renames {
        let sanitized = sanitize_filename(&item.new_vpk_name);

        let Some(addon) = db.addons.get(&item.id).cloned() else {
            errors.push(format!("Addon not found: {}", item.id));
            continue;
        };

        let current_path = PathBuf::from(&addon.current_path);
        if !current_path.exists() {
            errors.push(format!("Source file does not exist for {}", addon.vpk_name));
            continue;
        }

        let Some(dir) = current_path.parent() else {
            errors.push(format!("Invalid source file path for {}", addon.vpk_name));
            continue;
        };

        let new_filename = if addon.is_enabled {
            sanitized.clone()
        } else {
            format!("{}.disabled", sanitized)
        };
        let dest_path = dir.join(&new_filename);

        if dest_path.exists() {
            errors.push(format!(
                "Target file already exists: {}",
                dest_path.display()
            ));
            continue;
        }

        if !destination_paths.insert(dest_path.clone()) {
            errors.push(format!("Duplicate rename target: {}", dest_path.display()));
            continue;
        }

        plans.push(RenamePlan {
            id: item.id,
            current_path,
            dest_path,
            sanitized,
            old_vpk_name: addon.vpk_name,
        });
    }

    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }

    crate::watcher::suppress_internal_refresh(&state);
    for plan in plans {
        fs::rename(&plan.current_path, &plan.dest_path)
            .map_err(|e| format!("Failed to rename {}: {}", plan.id, e))?;

        if let Some(addon) = db.addons.get_mut(&plan.id) {
            addon.vpk_name = plan.sanitized.clone();
            addon.current_path = plan.dest_path.to_string_lossy().to_string();
        }

        let old_base = Path::new(&plan.old_vpk_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .replace(".disabled", "");
        let new_base = Path::new(&plan.sanitized)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        use md5::{Digest, Md5};
        let mut hasher_old = Md5::new();
        hasher_old.update(old_base.as_bytes());
        let old_hash = hasher_old.finalize();

        let mut hasher_new = Md5::new();
        hasher_new.update(new_base.as_bytes());
        let new_hash = hasher_new.finalize();

        let old_img = state.cache_dir.join(format!("{:x}_image.jpg", old_hash));
        let new_img = state.cache_dir.join(format!("{:x}_image.jpg", new_hash));

        if old_img.exists() {
            fs::rename(&old_img, &new_img)
                .map_err(|e| format!("Failed to rename cached image for {}: {}", plan.id, e))?;
            if let Some(addon_ref) = db.addons.get_mut(&plan.id) {
                addon_ref.image_path = Some(format!("/cache/{:x}_image.jpg", new_hash));
            }
        }
    }

    scan_addons_internal(
        &mut db,
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &state.cache_dir,
        &state.workshop_service,
    )
    .await?;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
    ))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn group_action(
    action: String,
    name: Option<String>,
    group_id: Option<String>,
    ids: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    workshop_collection_id: Option<String>,
    source: Option<String>,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;

    if action == "create" {
        let name = name.ok_or_else(|| "Missing name".to_string())?;
        let ids = ids.ok_or_else(|| "Missing ids".to_string())?;
        let source = source.unwrap_or_else(|| "manual".to_string());

        let group_id = format!(
            "group_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );

        for g in &mut db.groups {
            g.addons.retain(|n| !ids.contains(n));
        }

        // For workshop imports, add unknown IDs to known_uninstalled_addons
        let raw_ids = ids.clone();
        let mut new_workshop_ids: Vec<String> = Vec::new();
        if source == "workshop-import" {
            for raw_id in &raw_ids {
                let workshop_id = raw_id.replace(".vpk", "");
                // Check both with and without .vpk suffix
                let vpk_key = format!("{}.vpk", workshop_id);
                if !db.addons.contains_key(&vpk_key)
                    && !db.known_uninstalled_addons.contains_key(&vpk_key)
                    && !db.addons.contains_key(&workshop_id)
                    && !db.known_uninstalled_addons.contains_key(&workshop_id)
                {
                    db.known_uninstalled_addons.insert(
                        workshop_id.clone(),
                        Addon {
                            id: workshop_id.clone(),
                            vpk_name: workshop_id.clone(),
                            workshop_id: Some(workshop_id.clone()),
                            addon_info: serde_json::Value::Object(serde_json::Map::new()),
                            has_image: false,
                            image_path: None,
                            files_count: 0,
                            file_size: 0,
                            parsed_at: String::new(),
                            current_path: String::new(),
                            dir_type: "none".to_string(),
                            is_enabled: false,
                            steam_details: None,
                            workshop_details: None,
                            is_dummy: false,
                        },
                    );
                    new_workshop_ids.push(workshop_id);
                }
            }
            // Fetch Steam details for newly added workshop items
            if !new_workshop_ids.is_empty() {
                let allow_bridge = !db.settings.disable_steamworks_sdk;
                if let Ok(steam_details_list) =
                    fetch_steam_details_hybrid(
                        &state.workshop_service,
                        &new_workshop_ids,
                        allow_bridge,
                    )
                    .await
                {
                    for details in steam_details_list {
                        let wid = details["publishedfileid"].as_str().unwrap_or("");
                        if let Some(addon) = db.known_uninstalled_addons.get_mut(wid) {
                            // Store raw Steam API response
                            addon.steam_details = Some(details.clone());
                            if let Some(preview_url) = details["preview_url"].as_str() {
                                if !preview_url.is_empty() {
                                    addon.image_path = Some(preview_url.to_string());
                                    addon.has_image = true;
                                }
                            }
                            if let Some(file_size_str) = details["file_size"].as_str() {
                                if let Ok(file_size) = file_size_str.parse::<u64>() {
                                    addon.file_size = file_size;
                                }
                            }
                            // Update vpk_name to title if available
                            if let Some(title) = details["title"].as_str() {
                                if !title.is_empty() {
                                    addon.vpk_name = title.to_string();
                                }
                            }
                        }
                    }
                }
            }
        }

        let filtered_vpks: Vec<String> = ids
            .into_iter()
            .filter_map(|n| {
                // Already a valid key
                if db.addons.contains_key(&n)
                    || db.known_uninstalled_addons.contains_key(&n)
                    || n.ends_with(".vpk")
                {
                    return Some(n);
                }
                // Raw workshop ID → try with .vpk suffix
                let vpk_key = format!("{}.vpk", n);
                if db.addons.contains_key(&vpk_key)
                    || db.known_uninstalled_addons.contains_key(&vpk_key)
                {
                    return Some(vpk_key);
                }
                None
            })
            .collect();

        // Auto-assign to master collections based on source
        // Ensure system master collections exist
        if source == "auto-group" || source == "workshop-import" {
            if !db
                .master_collections
                .iter()
                .any(|mc| mc.name_key.as_deref() == Some("masterCollections.systemCampaignAuto"))
            {
                let mc_id = format!(
                    "mc_system_auto_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                );
                db.master_collections.push(MasterCollection {
                    id: mc_id,
                    name: "战役 (自动识别)".to_string(),
                    name_key: Some("masterCollections.systemCampaignAuto".to_string()),
                    group_ids: Vec::new(),
                    is_system: true,
                    icon: Some("Sparkles".to_string()),
                });
            }
            if !db
                .master_collections
                .iter()
                .any(|mc| mc.name_key.as_deref() == Some("masterCollections.systemWorkshopImport"))
            {
                let mc_id = format!(
                    "mc_system_ws_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                );
                db.master_collections.push(MasterCollection {
                    id: mc_id,
                    name: "从创意工坊导入".to_string(),
                    name_key: Some("masterCollections.systemWorkshopImport".to_string()),
                    group_ids: Vec::new(),
                    is_system: true,
                    icon: Some("Globe".to_string()),
                });
            }
        }

        let mut mc_ids: Vec<String> = Vec::new();
        if source == "auto-group" {
            if let Some(mc) = db
                .master_collections
                .iter()
                .find(|mc| mc.name_key.as_deref() == Some("masterCollections.systemCampaignAuto"))
            {
                mc_ids.push(mc.id.clone());
            }
        } else if source == "workshop-import" {
            if let Some(mc) = db
                .master_collections
                .iter()
                .find(|mc| mc.name_key.as_deref() == Some("masterCollections.systemWorkshopImport"))
            {
                mc_ids.push(mc.id.clone());
            }
        }

        db.groups.push(Group {
            id: group_id.clone(),
            name,
            addons: filtered_vpks,
            tags,
            workshop_collection_id,
            master_collection_ids: if mc_ids.is_empty() {
                None
            } else {
                Some(mc_ids.clone())
            },
            source: Some(source),
        });

        // Also update master collections' group_ids
        for mc_id in &mc_ids {
            if let Some(mc) = db.master_collections.iter_mut().find(|mc| &mc.id == mc_id) {
                if !mc.group_ids.contains(&group_id) {
                    mc.group_ids.push(group_id.clone());
                }
            }
        }
    } else if action == "delete" {
        let group_id = group_id.ok_or_else(|| "Missing groupId".to_string())?;
        // Remove group from all master collections
        for mc in &mut db.master_collections {
            mc.group_ids.retain(|gid| gid != &group_id);
        }
        db.groups.retain(|g| g.id != group_id);
    } else if action == "add-addons" {
        let group_id = group_id.ok_or_else(|| "Missing groupId".to_string())?;
        let ids = ids.ok_or_else(|| "Missing ids".to_string())?;

        for g in &mut db.groups {
            g.addons.retain(|n| !ids.contains(n));
        }

        let valid_ids: Vec<String> = ids
            .into_iter()
            .filter_map(|n| {
                if db.addons.contains_key(&n)
                    || db.known_uninstalled_addons.contains_key(&n)
                    || n.ends_with(".vpk")
                {
                    return Some(n);
                }
                let vpk_key = format!("{}.vpk", n);
                if db.addons.contains_key(&vpk_key)
                    || db.known_uninstalled_addons.contains_key(&vpk_key)
                {
                    return Some(vpk_key);
                }
                None
            })
            .collect();

        if let Some(target_group) = db.groups.iter_mut().find(|g| g.id == group_id) {
            for name in valid_ids {
                if !target_group.addons.contains(&name) {
                    target_group.addons.push(name);
                }
            }
        } else {
            return Err("Group not found".to_string());
        }
    } else if action == "remove-addons" {
        let group_id = group_id.ok_or_else(|| "Missing groupId".to_string())?;
        let ids = ids.ok_or_else(|| "Missing ids".to_string())?;

        if let Some(group) = db.groups.iter_mut().find(|g| g.id == group_id) {
            group.addons.retain(|n| !ids.contains(n));
        }
    } else if action == "rename-group" || action == "update-group" {
        let group_id = group_id.ok_or_else(|| "Missing groupId".to_string())?;

        let valid_addon_names: HashSet<String> = db
            .addons
            .keys()
            .cloned()
            .chain(db.known_uninstalled_addons.keys().cloned())
            .collect();

        if let Some(group) = db.groups.iter_mut().find(|g| g.id == group_id) {
            if let Some(n) = name {
                group.name = n;
            }
            group.tags = tags;
            group.workshop_collection_id = workshop_collection_id;

            if let Some(vpks) = ids {
                let filtered_vpks: Vec<String> = vpks
                    .into_iter()
                    .filter(|n| valid_addon_names.contains(n) || n.ends_with(".vpk"))
                    .collect();
                group.addons = filtered_vpks;
            }
        }
    } else if action == "auto-group" {
        auto_group_internal(&mut db);
        // Auto-assign auto-grouped groups to system master collection
        let mc_name_key = "masterCollections.systemCampaignAuto".to_string();
        if !db
            .master_collections
            .iter()
            .any(|mc| mc.name_key.as_deref() == Some("masterCollections.systemCampaignAuto"))
        {
            let mc_id = format!(
                "mc_system_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );
            db.master_collections.push(MasterCollection {
                id: mc_id.clone(),
                name: "战役 (自动识别)".to_string(),
                name_key: Some(mc_name_key),
                group_ids: Vec::new(),
                is_system: true,
                icon: Some("Sparkles".to_string()),
            });
        }
        // Collect auto-group IDs first to avoid borrow conflicts
        let auto_group_ids: Vec<String> = db
            .groups
            .iter()
            .filter(|g| g.source.as_deref() == Some("auto-group"))
            .map(|g| g.id.clone())
            .collect();
        let mc_id = db
            .master_collections
            .iter()
            .find(|mc| mc.name_key.as_deref() == Some("masterCollections.systemCampaignAuto"))
            .map(|mc| mc.id.clone())
            .unwrap();
        // Update master collection group_ids
        if let Some(mc) = db.master_collections.iter_mut().find(|mc| mc.id == mc_id) {
            for gid in &auto_group_ids {
                if !mc.group_ids.contains(gid) {
                    mc.group_ids.push(gid.clone());
                }
            }
        }
        // Update groups' master_collection_ids
        for g in &mut db.groups {
            if g.source.as_deref() == Some("auto-group") {
                if g.master_collection_ids.is_none() {
                    g.master_collection_ids = Some(vec![mc_id.clone()]);
                } else if let Some(ref mut ids) = g.master_collection_ids {
                    if !ids.contains(&mc_id) {
                        ids.push(mc_id.clone());
                    }
                }
            }
        }
    } else if action == "create-master-collection" {
        let name = name.ok_or_else(|| "Missing name".to_string())?;
        let mc_id = format!(
            "mc_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let group_ids = ids.unwrap_or_default();
        // Update groups to reference this master collection
        for gid in &group_ids {
            if let Some(group) = db.groups.iter_mut().find(|g| g.id == *gid) {
                if group.master_collection_ids.is_none() {
                    group.master_collection_ids = Some(vec![mc_id.clone()]);
                } else if let Some(ref mut ids) = group.master_collection_ids {
                    if !ids.contains(&mc_id) {
                        ids.push(mc_id.clone());
                    }
                }
            }
        }
        db.master_collections.push(MasterCollection {
            id: mc_id,
            name,
            name_key: None,
            group_ids,
            is_system: false,
            icon: None,
        });
    } else if action == "delete-master-collection" {
        let mc_id = group_id.ok_or_else(|| "Missing masterCollectionId".to_string())?;
        // Remove master collection reference from all groups
        for group in &mut db.groups {
            if let Some(ref mut ids) = group.master_collection_ids {
                ids.retain(|id| id != &mc_id);
                if ids.is_empty() {
                    group.master_collection_ids = None;
                }
            }
        }
        db.master_collections.retain(|mc| mc.id != mc_id);
    } else if action == "rename-master-collection" {
        let mc_id = group_id.ok_or_else(|| "Missing masterCollectionId".to_string())?;
        let name = name.ok_or_else(|| "Missing name".to_string())?;
        if let Some(mc) = db.master_collections.iter_mut().find(|mc| mc.id == mc_id) {
            if mc.is_system {
                return Err("Cannot rename system master collection".to_string());
            }
            mc.name = name;
        } else {
            return Err("Master collection not found".to_string());
        }
    } else if action == "add-to-master-collection" {
        let mc_id = group_id.ok_or_else(|| "Missing masterCollectionId".to_string())?;
        let group_ids = ids.ok_or_else(|| "Missing ids".to_string())?;
        // First update master collection group_ids
        if let Some(mc) = db.master_collections.iter_mut().find(|mc| mc.id == mc_id) {
            for gid in &group_ids {
                if !mc.group_ids.contains(gid) {
                    mc.group_ids.push(gid.clone());
                }
            }
        } else {
            return Err("Master collection not found".to_string());
        }
        // Then update groups' master_collection_ids
        for gid in &group_ids {
            if let Some(group) = db.groups.iter_mut().find(|g| g.id == *gid) {
                if group.master_collection_ids.is_none() {
                    group.master_collection_ids = Some(vec![mc_id.clone()]);
                } else if let Some(ref mut ids) = group.master_collection_ids {
                    if !ids.contains(&mc_id) {
                        ids.push(mc_id.clone());
                    }
                }
            }
        }
    } else if action == "remove-from-master-collection" {
        let mc_id = group_id.ok_or_else(|| "Missing masterCollectionId".to_string())?;
        let group_ids = ids.ok_or_else(|| "Missing ids".to_string())?;
        if let Some(mc) = db.master_collections.iter_mut().find(|mc| mc.id == mc_id) {
            mc.group_ids.retain(|gid| !group_ids.contains(gid));
        }
        for gid in &group_ids {
            if let Some(group) = db.groups.iter_mut().find(|g| g.id == *gid) {
                if let Some(ref mut ids) = group.master_collection_ids {
                    ids.retain(|id| id != &mc_id);
                    if ids.is_empty() {
                        group.master_collection_ids = None;
                    }
                }
            }
        }
    } else if action == "ensure-system-master-collections" {
        // Ensure system master collections exist
        if !db
            .master_collections
            .iter()
            .any(|mc| mc.name_key.as_deref() == Some("masterCollections.systemCampaignAuto"))
        {
            let mc_id = format!(
                "mc_system_auto_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );
            db.master_collections.push(MasterCollection {
                id: mc_id,
                name: "战役 (自动识别)".to_string(),
                name_key: Some("masterCollections.systemCampaignAuto".to_string()),
                group_ids: Vec::new(),
                is_system: true,
                icon: Some("Sparkles".to_string()),
            });
        }
        if !db
            .master_collections
            .iter()
            .any(|mc| mc.name_key.as_deref() == Some("masterCollections.systemWorkshopImport"))
        {
            let mc_id = format!(
                "mc_system_ws_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );
            db.master_collections.push(MasterCollection {
                id: mc_id,
                name: "从创意工坊导入".to_string(),
                name_key: Some("masterCollections.systemWorkshopImport".to_string()),
                group_ids: Vec::new(),
                is_system: true,
                icon: Some("Globe".to_string()),
            });
        }
        // Sync group memberships
        let auto_mc_id = db
            .master_collections
            .iter()
            .find(|mc| mc.name_key.as_deref() == Some("masterCollections.systemCampaignAuto"))
            .map(|mc| mc.id.clone());
        let ws_mc_id = db
            .master_collections
            .iter()
            .find(|mc| mc.name_key.as_deref() == Some("masterCollections.systemWorkshopImport"))
            .map(|mc| mc.id.clone());
        for g in &mut db.groups {
            let source = g.source.clone().unwrap_or_default();
            if source == "auto-group" {
                if let Some(ref mc_id) = auto_mc_id {
                    if let Some(ref mut ids) = g.master_collection_ids {
                        if !ids.contains(mc_id) {
                            ids.push(mc_id.clone());
                        }
                    } else {
                        g.master_collection_ids = Some(vec![mc_id.clone()]);
                    }
                }
            } else if source == "workshop-import" {
                if let Some(ref mc_id) = ws_mc_id {
                    if let Some(ref mut ids) = g.master_collection_ids {
                        if !ids.contains(mc_id) {
                            ids.push(mc_id.clone());
                        }
                    } else {
                        g.master_collection_ids = Some(vec![mc_id.clone()]);
                    }
                }
            }
        }
        // Rebuild group_ids for system master collections
        if let Some(ref mc_id) = auto_mc_id {
            let auto_group_ids: Vec<String> = db
                .groups
                .iter()
                .filter(|g| g.source.as_deref() == Some("auto-group"))
                .map(|g| g.id.clone())
                .collect();
            if let Some(mc) = db.master_collections.iter_mut().find(|mc| mc.id == *mc_id) {
                mc.group_ids = auto_group_ids;
            }
        }
        if let Some(ref mc_id) = ws_mc_id {
            let ws_group_ids: Vec<String> = db
                .groups
                .iter()
                .filter(|g| g.source.as_deref() == Some("workshop-import"))
                .map(|g| g.id.clone())
                .collect();
            if let Some(mc) = db.master_collections.iter_mut().find(|mc| mc.id == *mc_id) {
                mc.group_ids = ws_group_ids;
            }
        }
    } else {
        return Err("Unknown action".to_string());
    }

    db.groups.retain(|g| !g.addons.is_empty());

    save_db_internal(
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &db,
    );
    scan_addons_internal(
        &mut db,
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &state.cache_dir,
        &state.workshop_service,
    )
    .await?;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
    ))
}

#[tauri::command]
pub async fn open_workshop(workshop_id: String) -> Result<(), String> {
    let url = format!(
        "https://steamcommunity.com/sharedfiles/filedetails/?id={}",
        workshop_id
    );
    open::that(&url).map_err(|e| format!("Failed to open workshop URL: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn open_url(url: String) -> Result<(), String> {
    let parsed = validate_open_url(&url)?;
    open::that(parsed.as_str()).map_err(|e| format!("Failed to open URL: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn get_workshop_capabilities(
    state: State<'_, crate::AppState>,
) -> Result<WorkshopCapabilities, String> {
    Ok(state.workshop_service.capabilities())
}

#[tauri::command]
pub async fn query_workshop_home(
    state: State<'_, crate::AppState>,
) -> Result<WorkshopHomeResponse, String> {
    state.workshop_service.bridge_query_home()
}

#[tauri::command]
pub async fn query_workshop_items(
    query: WorkshopBrowseQuery,
    state: State<'_, crate::AppState>,
) -> Result<WorkshopItemsResponse, String> {
    state.workshop_service.bridge_query_items(&query)
}

#[tauri::command]
pub async fn query_workshop_item(
    workshop_id: String,
    state: State<'_, crate::AppState>,
) -> Result<WorkshopItemResponse, String> {
    state.workshop_service.bridge_query_item(&workshop_id)
}

#[tauri::command]
pub async fn query_workshop_collection(
    workshop_id: String,
    state: State<'_, crate::AppState>,
) -> Result<WorkshopCollectionResponse, String> {
    state.workshop_service.bridge_query_collection(&workshop_id)
}

#[tauri::command]
pub async fn steam_sync(state: State<'_, crate::AppState>) -> Result<Database, String> {
    let (ids, allow_bridge) = {
        let mut db = state.db.lock().await;

        // First, scan addons to populate database with any new/removed files
        scan_addons_internal(
            &mut db,
            &state.settings_path,
            &state.groups_path,
            &state.known_addons_path,
            &state.cache_dir,
            &state.workshop_service,
        )
        .await?;

        let mut ids = Vec::new();
        for addon in db.addons.values() {
            if let Some(ref w_id) = addon.workshop_id {
                ids.push(w_id.clone());
            }
        }
        for addon in db.known_uninstalled_addons.values() {
            if let Some(ref w_id) = addon.workshop_id {
                ids.push(w_id.clone());
            }
        }
        ids.sort();
        ids.dedup();

        if ids.is_empty() {
            return Ok(database_with_workshop_cache(
                &db,
                &state.workshop_cache_path,
            ));
        }

        (ids, !db.settings.disable_steamworks_sdk)
    };

    println!("Syncing Steam details manually for {} items...", ids.len());
    let steam_details_list =
        fetch_steam_details_hybrid(&state.workshop_service, &ids, allow_bridge).await?;

    let mut db = state.db.lock().await;
    for details in steam_details_list {
        if let Some(w_id) = details.get("publishedfileid").and_then(|id| id.as_str()) {
            // Update installed addons
            for addon in db.addons.values_mut() {
                if addon.workshop_id.as_deref() == Some(w_id) {
                    addon.steam_details = Some(details.clone());
                    if !addon.has_image {
                        if let Some(preview_url) =
                            details.get("preview_url").and_then(|u| u.as_str())
                        {
                            addon.image_path = Some(preview_url.to_string());
                        }
                    }
                    break;
                }
            }
            // Update uninstalled addons
            for addon in db.known_uninstalled_addons.values_mut() {
                if addon.workshop_id.as_deref() == Some(w_id) {
                    addon.steam_details = Some(details.clone());
                    if !addon.has_image {
                        if let Some(preview_url) =
                            details.get("preview_url").and_then(|u| u.as_str())
                        {
                            addon.image_path = Some(preview_url.to_string());
                            addon.has_image = true;
                        }
                    }
                    if let Some(file_size_str) = details.get("file_size").and_then(|v| v.as_str()) {
                        if let Ok(file_size) = file_size_str.parse::<u64>() {
                            if file_size > 0 {
                                addon.file_size = file_size;
                            }
                        }
                    }
                    // Update vpk_name to title if it's still just a workshop ID
                    if addon.vpk_name == w_id || addon.vpk_name == format!("{}.vpk", w_id) {
                        if let Some(title) = details.get("title").and_then(|v| v.as_str()) {
                            if !title.is_empty() {
                                addon.vpk_name = title.to_string();
                            }
                        }
                    }
                    break;
                }
            }
        }
    }

    save_db_internal(
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &db,
    );
    scan_addons_internal(
        &mut db,
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &state.cache_dir,
        &state.workshop_service,
    )
    .await?;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
    ))
}

#[tauri::command]
pub async fn fetch_workshop_html(
    url: String,
    source: Option<String>,
    state: State<'_, crate::AppState>,
) -> Result<String, String> {
    let started_at = chrono::Utc::now().to_rfc3339();
    let source = source.unwrap_or_else(|| "unknown".to_string());
    let parsed = match validate_steamcommunity_url(&url) {
        Ok(parsed) => parsed,
        Err(err) => {
            let _ = append_workshop_crawl_log_internal(
                &state.workshop_crawl_log_path,
                serde_json::json!({
                    "at": started_at,
                    "source": source,
                    "url": url,
                    "ok": false,
                    "error": err,
                }),
            );
            return Err(err);
        }
    };
    if is_background_workshop_fetch_source(&source) {
        let workshop_id = extract_workshop_id_from_url(parsed.as_str())
            .ok_or_else(|| "Background workshop fetch requires a workshop item URL".to_string())?;
        if let Err(err) = ensure_background_workshop_fetch_allowed(
            &source,
            &workshop_id,
            &state.known_addons_path,
        ) {
            let _ = append_workshop_crawl_log_internal(
                &state.workshop_crawl_log_path,
                serde_json::json!({
                    "at": started_at,
                    "source": source,
                    "url": url,
                    "ok": false,
                    "error": err,
                }),
            );
            return Err(err);
        }
    }
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
        .map_err(|e| e.to_string())?;

    let res = match client.get(parsed).send().await {
        Ok(res) => res,
        Err(e) => {
            let err = format!("Failed to fetch URL: {}", e);
            let _ = append_workshop_crawl_log_internal(
                &state.workshop_crawl_log_path,
                serde_json::json!({
                    "at": started_at,
                    "source": source,
                    "url": url,
                    "ok": false,
                    "error": err,
                }),
            );
            return Err(err);
        }
    };

    let status = res.status().as_u16();
    let body = match res.text().await {
        Ok(body) => body,
        Err(e) => {
            let err = format!("Failed to get response text: {}", e);
            let _ = append_workshop_crawl_log_internal(
                &state.workshop_crawl_log_path,
                serde_json::json!({
                    "at": started_at,
                    "source": source,
                    "url": url,
                    "ok": false,
                    "status": status,
                    "error": err,
                }),
            );
            return Err(err);
        }
    };

    let _ = append_workshop_crawl_log_internal(
        &state.workshop_crawl_log_path,
        serde_json::json!({
            "at": started_at,
            "source": source,
            "url": url,
            "ok": true,
            "status": status,
            "bytes": body.len(),
        }),
    );

    Ok(body)
}

#[tauri::command]
pub async fn delete_addons(
    ids: Vec<String>,
    delete_file: bool,
    remove_from_known: bool,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;

    if delete_file {
        crate::watcher::suppress_internal_refresh(&state);
        for id in &ids {
            if let Some(addon) = db.addons.get(id) {
                let path = PathBuf::from(&addon.current_path);
                if path.exists() {
                    fs::remove_file(&path)
                        .map_err(|e| format!("Failed to delete {}: {}", path.display(), e))?;
                }
                let disabled_path = path.with_extension("vpk.disabled");
                if disabled_path.exists() {
                    fs::remove_file(&disabled_path).map_err(|e| {
                        format!("Failed to delete {}: {}", disabled_path.display(), e)
                    })?;
                }
            }
        }
    }

    if remove_from_known {
        let mut known_addons = load_known_addons(&state.known_addons_path);

        for id in &ids {
            known_addons.remove(id);
            db.known_uninstalled_addons.remove(id);
        }

        fs::write(
            &state.known_addons_path,
            serde_json::to_string_pretty(&known_addons).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }

    scan_addons_internal(
        &mut db,
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &state.cache_dir,
        &state.workshop_service,
    )
    .await?;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
    ))
}

#[tauri::command]
pub async fn download_addon(
    workshop_id: String,
    state: State<'_, crate::AppState>,
    app_handle: AppHandle,
) -> Result<Database, String> {
    clear_download_cancellation(&state, &workshop_id)?;

    let result = async {
        let (workshop_dir, allow_bridge) = {
            let db = state.db.lock().await;
            (
                PathBuf::from(&db.settings.workshop_dir),
                !db.settings.disable_steamworks_sdk,
            )
        };
        let details_list =
            fetch_steam_details_hybrid(
                &state.workshop_service,
                std::slice::from_ref(&workshop_id),
                allow_bridge,
            )
            .await?;
        if details_list.is_empty() {
            return Err("Failed to retrieve details for workshop item".to_string());
        }
        let details = details_list[0].clone();

        let title = details
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("Workshop Item");

        if !workshop_dir.exists() {
            fs::create_dir_all(&workshop_dir).map_err(|e| e.to_string())?;
        }
        let dest_filename = format!("{}.vpk", workshop_id);
        let dest_path = workshop_dir.join(&dest_filename);
        if dest_path.exists() {
            return Err(format!(
                "Addon file already exists: {}",
                dest_path.display()
            ));
        }
        let disabled_dest_path = dest_path.with_extension("vpk.disabled");
        if disabled_dest_path.exists() {
            return Err(format!(
                "Disabled addon file already exists: {}",
                disabled_dest_path.display()
            ));
        }
        let tmp_path = workshop_dir.join(format!("{}.download", dest_filename));
        if tmp_path.exists() {
            fs::remove_file(&tmp_path)
                .map_err(|e| format!("Failed to remove stale download file: {}", e))?;
        }

        match attempt_bridge_download(
            &state,
            &state.workshop_service,
            &workshop_id,
            &workshop_dir,
            &app_handle,
            allow_bridge,
        )
        .await
        {
            Ok(true) => {
                crate::watcher::suppress_internal_refresh(&state);
                let mut db = state.db.lock().await;
                scan_addons_internal(
                    &mut db,
                    &state.settings_path,
                    &state.groups_path,
                    &state.known_addons_path,
                    &state.cache_dir,
                    &state.workshop_service,
                )
                .await?;
                return Ok(database_with_workshop_cache(
                    &db,
                    &state.workshop_cache_path,
                ));
            }
            Ok(false) => {}
            Err(err) if err == DOWNLOAD_CANCELLED_ERR => return Err(err),
            Err(_) => {}
        }

        let file_url = details
            .get("file_url")
            .and_then(|u| u.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                "Workshop item has no download URL (SDK did not install it and Web API has no direct file URL)".to_string()
            })?;

        println!("Downloading: {} (URL: {})", title, file_url);
        let client = reqwest::Client::new();
        let mut response = client
            .get(&file_url)
            .send()
            .await
            .map_err(|e| format!("Download request failed: {}", e))?;
        if !response.status().is_success() {
            return Err(format!(
                "Download server responded with status {}",
                response.status()
            ));
        }

        let total_size = response.content_length().unwrap_or(0);
        let mut file = fs::File::create(&tmp_path)
            .map_err(|e| format!("Failed to create local file: {}", e))?;
        let mut downloaded = 0;

        #[derive(Serialize, Clone)]
        struct DownloadProgress {
            #[serde(rename = "workshopId")]
            workshop_id: String,
            percent: u32,
            downloaded: u64,
            total: u64,
            source: String,
            phase: String,
        }

        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("Download chunk failed: {}", e))?
        {
            if is_download_cancelled(&state, &workshop_id)? {
                drop(file);
                fs::remove_file(&tmp_path).ok();
                return Err(DOWNLOAD_CANCELLED_ERR.to_string());
            }

            use std::io::Write;
            file.write_all(&chunk)
                .map_err(|e| format!("Failed to write chunk: {}", e))?;
            downloaded += chunk.len() as u64;

            let percent = if total_size > 0 {
                (downloaded as f64 / total_size as f64 * 100.0) as u32
            } else {
                0
            };

            let _ = app_handle.emit(
                "download-progress",
                DownloadProgress {
                    workshop_id: workshop_id.clone(),
                    percent,
                    downloaded,
                    total: total_size,
                    source: "web-fallback".to_string(),
                    phase: "fallback-download".to_string(),
                },
            );
        }
        file.flush()
            .map_err(|e| format!("Failed to flush downloaded file: {}", e))?;
        drop(file);

        if is_download_cancelled(&state, &workshop_id)? {
            fs::remove_file(&tmp_path).ok();
            return Err(DOWNLOAD_CANCELLED_ERR.to_string());
        }

        crate::watcher::suppress_internal_refresh(&state);
        fs::rename(&tmp_path, &dest_path)
            .map_err(|e| format!("Failed to finalize downloaded file: {}", e))?;

        let mut known_addons = load_known_addons(&state.known_addons_path);

        let has_image = details.get("preview_url").is_some();
        let image_path = details
            .get("preview_url")
            .and_then(|u| u.as_str())
            .map(|s| s.to_string());

        let id = workshop_id.clone();
        known_addons.insert(
            id.clone(),
            KnownAddonEntry {
                id: id.clone(),
                vpk_name: dest_filename.clone(),
                workshop_id: Some(workshop_id.clone()),
                addon_info: serde_json::Value::Null,
                has_image,
                image_path,
                steam_details: Some(details.clone()),
            },
        );

        fs::write(
            &state.known_addons_path,
            serde_json::to_string_pretty(&known_addons).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        let mut db = state.db.lock().await;
        scan_addons_internal(
            &mut db,
            &state.settings_path,
            &state.groups_path,
            &state.known_addons_path,
            &state.cache_dir,
            &state.workshop_service,
        )
        .await?;
        Ok(database_with_workshop_cache(
            &db,
            &state.workshop_cache_path,
        ))
    }
    .await;

    let _ = clear_download_cancellation(&state, &workshop_id);
    result
}

#[tauri::command]
pub async fn fetch_collection(
    collection_id: String,
    state: State<'_, crate::AppState>,
) -> Result<serde_json::Value, String> {
    let allow_bridge = {
        let db = state.db.lock().await;
        !db.settings.disable_steamworks_sdk
    };

    if allow_bridge {
        if let Ok(payload) = state
            .workshop_service
            .bridge_query_collection(&collection_id)
        {
            return Ok(serde_json::json!({
                "collection": payload.collection,
                "items": payload.items,
            }));
        }
    }

    let child_ids = fetch_collection_children_hybrid(
        &state.workshop_service,
        &collection_id,
        allow_bridge,
    )
    .await?;

    let mut query_ids = vec![collection_id.clone()];
    query_ids.extend(child_ids.clone());

    let details =
        fetch_steam_details_hybrid(&state.workshop_service, &query_ids, allow_bridge).await?;

    let mut collection_details = serde_json::Value::Null;
    let mut items = Vec::new();

    for detail in details {
        if let Some(id) = detail.get("publishedfileid").and_then(|id| id.as_str()) {
            if id == collection_id {
                collection_details = detail;
            } else {
                items.push(detail);
            }
        }
    }

    Ok(serde_json::json!({
        "collection": collection_details,
        "items": items,
    }))
}

#[tauri::command]
pub async fn add_known_addon(
    workshop_id: String,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;
    let allow_bridge = !db.settings.disable_steamworks_sdk;

    let details_list =
        fetch_steam_details_hybrid(
            &state.workshop_service,
            std::slice::from_ref(&workshop_id),
            allow_bridge,
        )
        .await?;
    if details_list.is_empty() {
        return Err("Failed to retrieve details for workshop item".to_string());
    }
    let details = &details_list[0];

    let mut known_addons = load_known_addons(&state.known_addons_path);

    let dest_filename = format!("{}.vpk", workshop_id);
    let has_image = details.get("preview_url").is_some();
    let image_path = details
        .get("preview_url")
        .and_then(|u| u.as_str())
        .map(|s| s.to_string());

    let id = workshop_id.clone();
    known_addons.insert(
        id.clone(),
        KnownAddonEntry {
            id: id.clone(),
            vpk_name: dest_filename.clone(),
            workshop_id: Some(workshop_id.clone()),
            addon_info: serde_json::Value::Null,
            has_image,
            image_path,
            steam_details: Some(details.clone()),
        },
    );

    let _ = fs::write(
        &state.known_addons_path,
        serde_json::to_string_pretty(&known_addons).unwrap_or_default(),
    );

    scan_addons_internal(
        &mut db,
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &state.cache_dir,
        &state.workshop_service,
    )
    .await?;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
    ))
}

#[tauri::command]
pub async fn get_workshop_cache(
    state: State<'_, crate::AppState>,
) -> Result<HashMap<String, serde_json::Value>, String> {
    Ok(load_workshop_cache(&state.workshop_cache_path))
}

#[tauri::command]
pub async fn record_workshop_items_seen(
    items: Vec<WorkshopSeenItem>,
    source: Option<String>,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut cache = load_workshop_cache(&state.workshop_cache_path);
    let now = chrono::Utc::now().to_rfc3339();
    let source = source.unwrap_or_else(|| "unknown".to_string());

    for item in items {
        if item.workshop_id.trim().is_empty() {
            continue;
        }

        let obj = cache_entry_object(&mut cache, &item.workshop_id);
        insert_non_empty_string(obj, "workshopId", &item.workshop_id);
        insert_non_empty_string(obj, "title", &item.title);
        insert_non_empty_string(obj, "previewUrl", &item.image_path);
        insert_non_empty_string(obj, "imagePath", &item.image_path);
        insert_non_empty_string(obj, "creatorName", &item.author_name);
        insert_non_empty_string(obj, "authorName", &item.author_name);
        insert_non_empty_string(obj, "creatorId", &item.author_id);
        insert_non_empty_string(obj, "authorId", &item.author_id);
        insert_non_empty_string(obj, "creatorProfileUrl", &item.author_url);
        insert_non_empty_string(obj, "authorUrl", &item.author_url);
        if let Some(vanity_id) = item.author_vanity_id.as_deref() {
            insert_non_empty_string(obj, "creatorVanityId", vanity_id);
        }
        if let Some(account_id) = item.author_account_id.as_deref() {
            insert_non_empty_string(obj, "creatorAccountId", account_id);
        }
        if let Some(steam_id) = item.author_steam_id.as_deref() {
            insert_non_empty_string(obj, "creatorSteamId", steam_id);
        }
        if item.author_id.chars().all(|c| c.is_ascii_digit()) && !item.author_id.is_empty() {
            insert_non_empty_string(obj, "creatorSteamId", &item.author_id);
        }
        if item.author_url.contains("/profiles/") {
            if let Some(id) = item
                .author_url
                .split("/profiles/")
                .nth(1)
                .and_then(|s| s.split('/').next())
            {
                if id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty() {
                    insert_non_empty_string(obj, "creatorSteamId", id);
                }
            }
        }
        insert_optional_value(
            obj,
            "shortDescription",
            item.short_description.map(serde_json::Value::String),
        );
        insert_optional_value(
            obj,
            "fileSizeDisplay",
            item.file_size.map(serde_json::Value::String),
        );
        insert_optional_value(obj, "tags", item.tags.map(|v| serde_json::json!(v)));
        insert_optional_u64(obj, "subscriptions", item.subscriptions);
        insert_optional_u64(obj, "favorites", item.favorites);
        insert_optional_u64(obj, "lifetimeSubscriptions", item.lifetime_subscriptions);
        insert_optional_u64(obj, "lifetimeFavorites", item.lifetime_favorites);
        insert_optional_u64(obj, "views", item.views);
        insert_optional_u64(obj, "comments", item.comments);
        insert_optional_u64(obj, "totalVotes", item.total_votes);
        insert_optional_u64(obj, "timeCreated", item.time_created);
        insert_optional_u64(obj, "timeUpdated", item.time_updated);
        insert_optional_u64(obj, "childCount", item.child_count);
        insert_optional_u64(obj, "previewCount", item.preview_count);
        insert_optional_vec_string(obj, "childItemIds", item.child_item_ids);
        insert_optional_vec_string(obj, "galleryPreviewUrls", item.gallery_preview_urls);
        insert_non_empty_string(obj, "lastSeenSource", &source);
        insert_non_empty_string(obj, "lastSeenAt", &now);
    }

    save_workshop_cache(&state.workshop_cache_path, &cache)?;
    let db = state.db.lock().await;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
    ))
}

#[tauri::command]
pub async fn persist_workshop_page_details(
    workshop_id: String,
    details: serde_json::Value,
    source: Option<String>,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    if workshop_id.trim().is_empty() {
        return Err("Missing workshop id".to_string());
    }

    let mut cache = load_workshop_cache(&state.workshop_cache_path);
    let now = chrono::Utc::now().to_rfc3339();
    let source = source.unwrap_or_else(|| "unknown".to_string());
    ensure_background_workshop_fetch_allowed(&source, &workshop_id, &state.known_addons_path)?;

    {
        let obj = cache_entry_object(&mut cache, &workshop_id);
        insert_non_empty_string(obj, "workshopId", &workshop_id);
        insert_non_empty_string(obj, "lastPageFetchedAt", &now);
        insert_non_empty_string(obj, "lastPageSource", &source);
        if let Some(title) = details.get("title").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "title", title);
        }
        if let Some(preview_url) = details.get("previewUrl").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "previewUrl", preview_url);
            insert_non_empty_string(obj, "imagePath", preview_url);
        }
        if let Some(description) = details.get("description").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "description", description);
        }
        if let Some(description_html) = details.get("descriptionHtml").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "descriptionHtml", description_html);
        }
        if let Some(creator_name) = details.get("creatorName").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "creatorName", creator_name);
            insert_non_empty_string(obj, "authorName", creator_name);
        }
        if let Some(profile_url) = details.get("creatorProfileUrl").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "creatorProfileUrl", profile_url);
            insert_non_empty_string(obj, "authorUrl", profile_url);
        }
        if let Some(steam_id) = details.get("creatorSteamId").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "creatorSteamId", steam_id);
            insert_non_empty_string(obj, "creatorId", steam_id);
        }
        if let Some(vanity_id) = details.get("creatorVanityId").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "creatorVanityId", vanity_id);
            if obj
                .get("creatorId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .is_empty()
            {
                insert_non_empty_string(obj, "creatorId", vanity_id);
            }
        }
        if let Some(account_id) = details.get("creatorAccountId").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "creatorAccountId", account_id);
        }
        if let Some(file_size) = details.get("fileSizeDisplay").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "fileSizeDisplay", file_size);
        }
        if let Some(posted_text) = details.get("postedDateText").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "postedDateText", posted_text);
        }
        if let Some(updated_text) = details.get("updatedDateText").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "updatedDateText", updated_text);
        }
        insert_optional_value(
            obj,
            "changeNoteCount",
            details.get("changeNoteCount").cloned(),
        );
        insert_optional_value(obj, "ratingStars", details.get("ratingStars").cloned());
        insert_optional_value(obj, "ratingCount", details.get("ratingCount").cloned());
        insert_optional_value(
            obj,
            "uniqueVisitors",
            details.get("uniqueVisitors").cloned(),
        );
        insert_optional_value(
            obj,
            "currentSubscribers",
            details.get("currentSubscribers").cloned(),
        );
        insert_optional_value(
            obj,
            "currentFavorites",
            details.get("currentFavorites").cloned(),
        );

        if let Some(gallery) = details.get("imageGallery").cloned() {
            obj.insert("imageGallery".to_string(), gallery.clone());
            obj.insert("galleryUrls".to_string(), gallery);
        }
        if let Some(tags) = details.get("tags").cloned() {
            obj.insert("pageTags".to_string(), tags);
        }
        if let Some(required) = details.get("requiredItems").cloned() {
            obj.insert("requiredItems".to_string(), required);
        }
        if let Some(parent_collections) = details.get("parentCollections").cloned() {
            obj.insert("parentCollections".to_string(), parent_collections);
        }
        if let Some(background) = details.get("backgroundImageUrl").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "backgroundImageUrl", background);
        }
    }

    if let Some(required_items) = details.get("requiredItems").and_then(|v| v.as_array()) {
        for item in required_items {
            let Some(child_id) = item.get("workshopId").and_then(|v| v.as_str()) else {
                continue;
            };
            let child_obj = cache_entry_object(&mut cache, child_id);
            insert_non_empty_string(child_obj, "workshopId", child_id);
            if let Some(title) = item.get("title").and_then(|v| v.as_str()) {
                insert_non_empty_string(child_obj, "title", title);
            }
            insert_non_empty_string(child_obj, "lastSeenSource", &source);
            insert_non_empty_string(child_obj, "lastSeenAt", &now);
        }
    }

    if let Some(parent_collections) = details.get("parentCollections").and_then(|v| v.as_array()) {
        for item in parent_collections {
            let Some(collection_id) = item.get("workshopId").and_then(|v| v.as_str()) else {
                continue;
            };
            let collection_obj = cache_entry_object(&mut cache, collection_id);
            insert_non_empty_string(collection_obj, "workshopId", collection_id);
            if let Some(title) = item.get("title").and_then(|v| v.as_str()) {
                insert_non_empty_string(collection_obj, "title", title);
            }
            insert_non_empty_string(collection_obj, "lastSeenSource", &source);
            insert_non_empty_string(collection_obj, "lastSeenAt", &now);
        }
    }

    save_workshop_cache(&state.workshop_cache_path, &cache)?;
    let db = state.db.lock().await;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
    ))
}

#[tauri::command]
pub async fn get_background_tasks(
    state: State<'_, crate::AppState>,
) -> Result<serde_json::Value, String> {
    if !state.background_tasks_path.exists() {
        return Ok(serde_json::json!([]));
    }
    let content = fs::read_to_string(&state.background_tasks_path)
        .map_err(|e| format!("Failed to read background tasks: {}", e))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse background tasks: {}", e))
}

#[tauri::command]
pub async fn save_background_task_snapshot(
    tasks: serde_json::Value,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    if let Some(parent) = state.background_tasks_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create task directory: {}", e))?;
    }
    let json = serde_json::to_string_pretty(&tasks)
        .map_err(|e| format!("Failed to serialize background tasks: {}", e))?;
    fs::write(&state.background_tasks_path, json)
        .map_err(|e| format!("Failed to write background tasks: {}", e))
}

#[tauri::command]
pub async fn cancel_download(
    workshop_id: String,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    request_download_cancellation(&state, &workshop_id)
}

#[tauri::command]
pub async fn append_workshop_crawl_log(
    record: serde_json::Value,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    append_workshop_crawl_log_internal(&state.workshop_crawl_log_path, record)
}

#[cfg(test)]
mod tests {
    use super::{ensure_background_workshop_fetch_allowed, is_background_workshop_fetch_source};
    use serde_json::json;
    use std::fs;

    fn write_known_addons_file(name: &str, value: serde_json::Value) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "left4addons-{}-{}-known_addons.json",
            name,
            std::process::id()
        ));
        fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();
        path
    }

    #[test]
    fn background_source_detection_is_limited_to_silent_refreshes() {
        assert!(is_background_workshop_fetch_source("startup-auto"));
        assert!(is_background_workshop_fetch_source("background-refresh"));
        assert!(!is_background_workshop_fetch_source("workshop-detail"));
        assert!(!is_background_workshop_fetch_source("workshop-home"));
    }

    #[test]
    fn background_fetch_only_allows_known_addons() {
        let path = write_known_addons_file(
            "allowed",
            json!({
                "12345": {
                    "id": "12345",
                    "vpkName": "12345.vpk",
                    "workshopId": "12345",
                    "addonInfo": {},
                    "hasImage": false,
                    "imagePath": null,
                    "steamDetails": null
                }
            }),
        );

        let allowed = ensure_background_workshop_fetch_allowed("startup-auto", "12345", &path);
        let blocked = ensure_background_workshop_fetch_allowed("startup-auto", "99999", &path);
        let manual = ensure_background_workshop_fetch_allowed("workshop-detail", "99999", &path);

        fs::remove_file(&path).ok();

        assert!(allowed.is_ok());
        assert!(blocked.is_err());
        assert!(manual.is_ok());
    }
}
