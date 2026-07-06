use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use serde::{Serialize, Deserialize};
use tauri::{AppHandle, State, Manager, Emitter};
use regex::Regex;
use crate::vpk::{extract_addon_metadata, generate_dummy_vpk, calculate_file_hash};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Settings {
    #[serde(rename = "workshopDir")]
    pub workshop_dir: String,
    #[serde(rename = "loadingDir")]
    pub loading_dir: String,
    #[serde(rename = "enableDummyBypass", default)]
    pub enable_dummy_bypass: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub addons: Vec<String>,
    pub tags: Option<Vec<String>>,
    #[serde(rename = "workshopCollectionId")]
    pub workshop_collection_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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
pub struct LocalDatabase {
    pub settings: Settings,
    pub groups: Vec<Group>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Database {
    pub settings: Settings,
    pub addons: HashMap<String, Addon>,
    pub groups: Vec<Group>,
    #[serde(rename = "knownUninstalledAddons")]
    pub known_uninstalled_addons: HashMap<String, Addon>,
}

pub fn load_db(db_path: &Path, known_addons_path: &Path, app_handle: &AppHandle) -> Database {
    let runtime_dir = db_path.parent().unwrap_or(Path::new(""));
    let default_loading = runtime_dir.join("addons-loading").to_string_lossy().to_string();
    let default_workshop = Path::new(&default_loading).join("workshop").to_string_lossy().to_string();
    
    let default_db = Database {
        settings: Settings {
            workshop_dir: default_workshop,
            loading_dir: default_loading,
            enable_dummy_bypass: false,
        },
        addons: HashMap::new(),
        groups: Vec::new(),
        known_uninstalled_addons: HashMap::new(),
    };

    let db_json_existed = db_path.exists();
    
    let mut local_db: LocalDatabase = if db_path.exists() {
        match fs::read_to_string(db_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|_| {
                if let Ok(old_db) = serde_json::from_str::<serde_json::Value>(&content) {
                    let settings = serde_json::from_value(old_db["settings"].clone()).unwrap_or_default();
                    let groups = serde_json::from_value(old_db["groups"].clone()).unwrap_or_default();
                    LocalDatabase { settings, groups }
                } else {
                    LocalDatabase::default()
                }
            }),
            Err(_) => LocalDatabase::default(),
        }
    } else {
        LocalDatabase {
            settings: default_db.settings.clone(),
            groups: Vec::new(),
        }
    };

    let known_addons: HashMap<String, KnownAddonEntry> = if known_addons_path.exists() {
        match fs::read_to_string(known_addons_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    } else {
        let mut migrated = HashMap::new();
        if db_path.exists() {
            if let Ok(content) = fs::read_to_string(db_path) {
                if let Ok(old_db) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(addons_obj) = old_db["addons"].as_object() {
                        for (_k, v) in addons_obj {
                            if let Ok(mut addon) = serde_json::from_value::<Addon>(v.clone()) {
                                if addon.id.is_empty() {
                                    addon.id = addon.workshop_id.clone().unwrap_or_else(|| addon.vpk_name.clone());
                                }
                                let id = addon.id.clone();
                                migrated.insert(id.clone(), KnownAddonEntry {
                                    id,
                                    vpk_name: addon.vpk_name.clone(),
                                    workshop_id: addon.workshop_id.clone(),
                                    addon_info: addon.addon_info,
                                    has_image: addon.has_image,
                                    image_path: addon.image_path,
                                    steam_details: addon.steam_details,
                                });
                            }
                        }
                    }
                }
            }
        }
        if !migrated.is_empty() {
            let _ = fs::write(known_addons_path, serde_json::to_string_pretty(&migrated).unwrap_or_default());
        }
        migrated
    };

    let old_default_loading = app_handle.path().app_data_dir()
        .map(|p| p.join("addons-loading").to_string_lossy().to_string())
        .unwrap_or_default();

    if local_db.settings.loading_dir.is_empty() || local_db.settings.loading_dir == old_default_loading {
        local_db.settings.loading_dir = default_db.settings.loading_dir.clone();
    }
    let loading_path = Path::new(&local_db.settings.loading_dir);
    local_db.settings.workshop_dir = loading_path.join("workshop").to_string_lossy().to_string();

    let merged_addons = HashMap::new();

    let mut known_uninstalled_addons = HashMap::new();
    for (id, entry) in &known_addons {
        if !merged_addons.contains_key(id) {
            known_uninstalled_addons.insert(id.clone(), Addon {
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
                is_dummy: false,
            });
        }
    }

    let db = Database {
        settings: local_db.settings,
        addons: merged_addons,
        groups: local_db.groups,
        known_uninstalled_addons,
    };

    if !db_json_existed {
        save_db_internal(db_path, known_addons_path, &db);
    }

    db
}

pub fn save_db_internal(db_path: &Path, known_addons_path: &Path, db: &Database) {
    let local_db = LocalDatabase {
        settings: db.settings.clone(),
        groups: db.groups.clone(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&local_db) {
        let _ = fs::write(db_path, json);
    }

    let mut known_addons: HashMap<String, KnownAddonEntry> = if known_addons_path.exists() {
        match fs::read_to_string(known_addons_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    } else {
        HashMap::new()
    };

    for (id, addon) in &db.addons {
        if addon.workshop_id.is_none() {
            known_addons.remove(&addon.vpk_name);
        }
        known_addons.insert(id.clone(), KnownAddonEntry {
            id: id.clone(),
            vpk_name: addon.vpk_name.clone(),
            workshop_id: addon.workshop_id.clone(),
            addon_info: addon.addon_info.clone(),
            has_image: addon.has_image,
            image_path: addon.image_path.clone(),
            steam_details: addon.steam_details.clone(),
        });
    }

    for (id, addon) in &db.known_uninstalled_addons {
        if addon.workshop_id.is_none() {
            known_addons.remove(&addon.vpk_name);
        }
        known_addons.insert(id.clone(), KnownAddonEntry {
            id: id.clone(),
            vpk_name: addon.vpk_name.clone(),
            workshop_id: addon.workshop_id.clone(),
            addon_info: addon.addon_info.clone(),
            has_image: addon.has_image,
            image_path: addon.image_path.clone(),
            steam_details: addon.steam_details.clone(),
        });
    }

    if let Ok(json) = serde_json::to_string_pretty(&known_addons) {
        let _ = fs::write(known_addons_path, json);
    }
}

async fn fetch_steam_details(workshop_ids: &[String]) -> Result<Vec<serde_json::Value>, String> {
    if workshop_ids.is_empty() {
        return Ok(Vec::new());
    }

    let url = "https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/";
    let mut params = Vec::new();
    params.push(("itemcount".to_string(), workshop_ids.len().to_string()));
    for (index, id) in workshop_ids.iter().enumerate() {
        params.push((format!("publishedfileids[{}]", index), id.clone()));
    }

    let client = reqwest::Client::new();
    let res = client.post(url)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Failed to send Steam API request: {}", e))?;

    if !res.status().is_success() {
        return Err(format!("Steam API responded with status {}", res.status()));
    }

    let json: serde_json::Value = res.json()
        .await
        .map_err(|e| format!("Failed to parse Steam API response: {}", e))?;

    let details = json["response"]["publishedfiledetails"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    Ok(details)
}

pub async fn scan_addons_internal(
    db: &mut Database,
    db_path: &Path,
    known_addons_path: &Path,
    cache_dir: &Path,
) -> Result<(), String> {
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

    let mut known_addons: HashMap<String, KnownAddonEntry> = if known_addons_path.exists() {
        match fs::read_to_string(known_addons_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    } else {
        HashMap::new()
    };

    let mut active_addons = HashMap::new();
    let mut new_workshop_ids = HashSet::new();

    for file_info in files_on_disk {
        let vpk_name = file_info.vpk_name.clone();
        let cached = db.addons.values().find(|a| a.vpk_name == vpk_name).cloned()
            .or_else(|| db.known_uninstalled_addons.values().find(|a| a.vpk_name == vpk_name).cloned());

        let has_capitalized_keys = cached.as_ref().map_or(false, |addon| {
            addon.addon_info.as_object().map_or(false, |obj| {
                obj.keys().any(|k| k.chars().any(|c| c.is_uppercase()))
            })
        });

        let needs_metadata = match &cached {
            Some(addon) => {
                addon.addon_info.is_null() 
                || addon.addon_info.as_object().map_or(true, |m| m.is_empty())
                || has_capitalized_keys
                || (addon.image_path.is_none() && !addon.has_image)
                || (addon.has_image && addon.image_path.as_ref().map_or(true, |p| {
                    p.starts_with("/cache/") && !cache_dir.join(p.trim_start_matches("/cache/")).exists()
                }))
            },
            None => true,
        };

        if needs_metadata {
            println!("Parsing metadata for: {}", vpk_name);
            let meta = extract_addon_metadata(&file_info.full_path, cache_dir);
            
            let mut workshop_id = None;
            let base_name = Path::new(&vpk_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if base_name.chars().all(|c| c.is_ascii_digit()) {
                workshop_id = Some(base_name.to_string());
            }

            // Fallback: extract workshop ID from url keys inside addon_info (all keys are lowercase now)
            if workshop_id.is_none() {
                if let Some(url_val) = meta.addon_info.get("addonurl0").or_else(|| meta.addon_info.get("addonurl")) {
                    if let Some(url_str) = url_val.as_str() {
                        workshop_id = extract_workshop_id_from_url(url_str);
                    }
                }
            }

            let is_dummy = meta.addon_info.get("addondescription")
                .and_then(|v| v.as_str())
                .map(|v| v == "A dummy addon generated by Left 4 Addons")
                .unwrap_or(false);

            let id = workshop_id.clone().unwrap_or_else(|| meta.hash.clone());

            if workshop_id.is_none() && !id.is_empty() {
                known_addons.remove(&vpk_name);
            }

            let entry = known_addons.get(&id);
            let addon_info = entry.map(|e| e.addon_info.clone()).unwrap_or(meta.addon_info);
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
                is_dummy,
            };

            if let Some(ref w_id) = workshop_id {
                new_workshop_ids.insert(w_id.clone());
            }

            known_addons.insert(id.clone(), KnownAddonEntry {
                id: id.clone(),
                vpk_name: vpk_name.clone(),
                workshop_id: workshop_id.clone(),
                addon_info: addon.addon_info.clone(),
                has_image: addon.has_image,
                image_path: addon.image_path.clone(),
                steam_details: addon.steam_details.clone(),
            });

            active_addons.insert(id.clone(), addon);
        } else if let Some(mut addon) = cached {
            if addon.workshop_id.is_none() && (addon.id.ends_with(".vpk") || addon.id.ends_with(".vpk.disabled") || addon.id.len() != 32) {
                let hash = calculate_file_hash(&file_info.full_path);
                if !hash.is_empty() {
                    let old_id = addon.id.clone();
                    addon.id = hash.clone();
                    known_addons.remove(&old_id);
                    known_addons.insert(hash.clone(), KnownAddonEntry {
                        id: hash.clone(),
                        vpk_name: addon.vpk_name.clone(),
                        workshop_id: None,
                        addon_info: addon.addon_info.clone(),
                        has_image: addon.has_image,
                        image_path: addon.image_path.clone(),
                        steam_details: addon.steam_details.clone(),
                    });
                }
            }

            addon.file_size = file_info.size;
            addon.current_path = file_info.full_path.to_string_lossy().to_string();
            addon.dir_type = file_info.dir_type;
            addon.is_enabled = file_info.is_enabled;
            addon.is_dummy = addon.addon_info.get("addondescription")
                .and_then(|v| v.as_str())
                .map(|v| v == "A dummy addon generated by Left 4 Addons")
                .unwrap_or(false);

            if addon.workshop_id.is_none() {
                if let Some(url_val) = addon.addon_info.get("addonurl0").or_else(|| addon.addon_info.get("addonurl")) {
                    if let Some(url_str) = url_val.as_str() {
                        addon.workshop_id = extract_workshop_id_from_url(url_str);
                        if let Some(ref w_id) = addon.workshop_id {
                            new_workshop_ids.insert(w_id.clone());
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
        if let Ok(steam_details_list) = fetch_steam_details(&ids_array).await {
            for details in steam_details_list {
                if let Some(w_id) = details.get("publishedfileid").and_then(|id| id.as_str()) {
                    for addon in active_addons.values_mut() {
                        if addon.workshop_id.as_deref() == Some(w_id) {
                            addon.steam_details = Some(details.clone());
                            if !addon.has_image {
                                if let Some(preview_url) = details.get("preview_url").and_then(|u| u.as_str()) {
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
            uninstalled.insert(id.clone(), Addon {
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
                is_dummy: false,
            });
        }
    }
    for (id, addon) in &db.known_uninstalled_addons {
        if !active_addons.contains_key(id) && !uninstalled.contains_key(id) {
            uninstalled.insert(id.clone(), addon.clone());
        }
    }

    db.addons = active_addons;
    db.known_uninstalled_addons = uninstalled;

    let mut vpk_to_id = HashMap::new();
    for addon in db.addons.values() {
        vpk_to_id.insert(addon.vpk_name.clone(), addon.id.clone());
    }
    for addon in db.known_uninstalled_addons.values() {
        vpk_to_id.insert(addon.vpk_name.clone(), addon.id.clone());
    }

    for g in &mut db.groups {
        g.addons = g.addons.iter().map(|item| {
            let clean_item = item.strip_suffix(".disabled").unwrap_or(item);
            if let Some(new_id) = vpk_to_id.get(clean_item) {
                new_id.clone()
            } else {
                item.clone()
            }
        }).collect();
    }
    db.groups.retain(|g| !g.addons.is_empty());

    save_db_internal(db_path, known_addons_path, db);
    Ok(())
}

fn extract_workshop_id_from_url(url: &str) -> Option<String> {
    if let Some(pos) = url.find("id=") {
        let start = pos + 3;
        let end = url[start..].find('&').map(|idx| start + idx).unwrap_or(url.len());
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
    let re_part_word = Regex::new(r"(?i)(?:[-#_/:,\s]+(?:part|pt|partie|pts|vol|volume|chapter|ch|act))$").unwrap();
    
    loop {
        let prev_len = s.len();
        
        s = re_ver.replace(&s, "").into_owned();
        s = re_part_num.replace(&s, "").into_owned();
        s = re_part_word.replace(&s, "").into_owned();
        
        s = s.trim_end_matches(|c: char| c.is_whitespace() || c == ':' || c == '_' || c == '-' || c == '/' || c == '\\' || c == '#' || c == '+' || c == ',' || c == '.').to_string();
        
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
        vpk_name: String,
        title: String,
        description: String,
    }

    let mut candidates = Vec::new();
    for addon in ungrouped {
        let title = addon.steam_details
            .as_ref()
            .and_then(|d| d.get("title").and_then(|t| t.as_str()))
            .or_else(|| addon.addon_info.get("addontitle").and_then(|t| t.as_str()))
            .unwrap_or(&addon.vpk_name)
            .to_string();

        let description = addon.addon_info
            .get("addondescription")
            .and_then(|t| t.as_str())
            .or_else(|| addon.addon_info.get("addontagline").and_then(|t| t.as_str()))
            .or_else(|| {
                addon.steam_details
                    .as_ref()
                    .and_then(|d| d.get("description").and_then(|t| t.as_str()))
            })
            .unwrap_or("")
            .to_string();

        candidates.push(Candidate {
            vpk_name: addon.vpk_name.clone(),
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

            let group_id = format!("group_{}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos());
                
            let mut addons = Vec::new();
            for &idx in &idxs {
                addons.push(candidates[idx].vpk_name.clone());
                indices_to_remove.insert(idx);
            }

            db.groups.push(Group {
                id: group_id,
                name: group_name,
                addons,
                tags: None,
                workshop_collection_id: None,
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
                    title_groups.entry(cleaned_prefix).or_default().push(c.vpk_name);
                }
            }
        }
    }

    for (prefix, addons) in title_groups {
        if addons.len() >= 2 {
            let group_id = format!("group_{}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos());
                
            db.groups.push(Group {
                id: group_id,
                name: prefix,
                addons,
                tags: None,
                workshop_collection_id: None,
            });
        }
    }
}

#[tauri::command]
pub async fn get_settings(
    state: State<'_, crate::AppState>,
) -> Result<Settings, String> {
    let db = state.db.lock().await;
    Ok(db.settings.clone())
}

#[tauri::command]
pub async fn save_settings(
    loading_dir: String,
    enable_dummy_bypass: bool,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;
    let loading_path = Path::new(&loading_dir);
    let workshop_dir = loading_path.join("workshop").to_string_lossy().to_string();
    
    db.settings.workshop_dir = workshop_dir;
    db.settings.loading_dir = loading_dir;
    db.settings.enable_dummy_bypass = enable_dummy_bypass;
    save_db_internal(&state.db_path, &state.known_addons_path, &db);

    scan_addons_internal(&mut db, &state.db_path, &state.known_addons_path, &state.cache_dir).await?;
    Ok(db.clone())
}

#[tauri::command]
pub async fn get_addons(
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;
    scan_addons_internal(&mut db, &state.db_path, &state.known_addons_path, &state.cache_dir).await?;
    Ok(db.clone())
}

#[tauri::command]
pub async fn move_addons(
    ids: Vec<String>,
    target_dir_type: String,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;
    let target_dir = if target_dir_type == "loading" {
        PathBuf::from(&db.settings.loading_dir)
    } else {
        PathBuf::from(&db.settings.workshop_dir)
    };

    if !target_dir.exists() {
        fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
    }

    for id in ids {
        if let Some(addon) = db.addons.get(&id) {
            let current_path = PathBuf::from(&addon.current_path);
            if current_path.exists() {
                let file_name = current_path.file_name().unwrap();
                let dest_path = target_dir.join(file_name);
                
                // If moving back to workshop, check if dest_path is a dummy addon and remove it first
                if target_dir_type == "workshop" && dest_path.exists() {
                    let is_dummy = if let Ok((files, mut file)) = crate::vpk::parse_vpk(&dest_path) {
                        let addoninfo_key = files.keys().find(|k| {
                            let lower = k.to_lowercase();
                            lower == "addoninfo.txt" || lower.ends_with("/addoninfo.txt") || lower.ends_with("\\addoninfo.txt")
                        });
                        if let Some(key) = addoninfo_key {
                            if let Some(entry) = files.get(key) {
                                if let Ok(content_bytes) = crate::vpk::get_file_content(&mut file, entry) {
                                    let text = String::from_utf8_lossy(&content_bytes);
                                    let parsed = crate::vpk::parse_key_values(&text);
                                    parsed.get("addondescription")
                                        .and_then(|v| v.as_str())
                                        .map(|v| v == "A dummy addon generated by Left 4 Addons")
                                        .unwrap_or(false)
                                } else { false }
                            } else { false }
                        } else { false }
                    } else { false };
                    
                    if is_dummy {
                        let _ = fs::remove_file(&dest_path);
                    }
                }

                if !dest_path.exists() {
                    if fs::rename(&current_path, &dest_path).is_ok() {
                        // If moving out of workshop and dummy bypass is enabled, generate dummy addon in workshop directory
                        if target_dir_type == "loading" && addon.dir_type == "workshop" && db.settings.enable_dummy_bypass {
                            if let Some(ref w_id) = addon.workshop_id {
                                let workshop_dir = PathBuf::from(&db.settings.workshop_dir);
                                let dummy_vpk_path = workshop_dir.join(format!("{}.vpk", w_id));
                                if dest_path.exists() {
                                    let _ = generate_dummy_vpk(&dest_path, &dummy_vpk_path);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    scan_addons_internal(&mut db, &state.db_path, &state.known_addons_path, &state.cache_dir).await?;
    Ok(db.clone())
}

#[tauri::command]
pub async fn toggle_addons(
    ids: Vec<String>,
    enabled: bool,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;
    
    for id in ids {
        if let Some(addon) = db.addons.get(&id) {
            let current_path = PathBuf::from(&addon.current_path);
            if current_path.exists() {
                let current_dir = current_path.parent().unwrap();
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

                if !dest_path.exists() {
                    let _ = fs::rename(&current_path, &dest_path);
                }
            }
        }
    }

    scan_addons_internal(&mut db, &state.db_path, &state.known_addons_path, &state.cache_dir).await?;
    Ok(db.clone())
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

            let old_base = Path::new(&addon.vpk_name).file_stem().and_then(|s| s.to_str()).unwrap_or("").replace(".disabled", "");
            let new_base = Path::new(&sanitized).file_stem().and_then(|s| s.to_str()).unwrap_or("");
            
            use md5::{Md5, Digest};
            let mut hasher_old = Md5::new();
            hasher_old.update(old_base.as_bytes());
            let old_hash = hasher_old.finalize();
            
            let mut hasher_new = Md5::new();
            hasher_new.update(new_base.as_bytes());
            let new_hash = hasher_new.finalize();

            let old_img = state.cache_dir.join(format!("{:x}_image.jpg", old_hash));
            let new_img = state.cache_dir.join(format!("{:x}_image.jpg", new_hash));

            if old_img.exists() {
                if let Ok(_) = fs::rename(&old_img, &new_img) {
                    if let Some(addon_ref) = db.addons.get_mut(&sanitized) {
                        addon_ref.image_path = Some(format!("/cache/{:x}_image.jpg", new_hash));
                    }
                }
            }
        }
    } else {
        return Err("Addon not found".to_string());
    }

    scan_addons_internal(&mut db, &state.db_path, &state.known_addons_path, &state.cache_dir).await?;
    Ok(db.clone())
}

#[derive(Deserialize)]
pub struct RenameItem {
    #[serde(rename = "vpkName")]
    pub vpk_name: String,
    #[serde(rename = "newVpkName")]
    pub new_vpk_name: String,
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

    for item in renames {
        let sanitized = sanitize_filename(&item.new_vpk_name);
        
        if let Some(addon) = db.addons.get(&item.vpk_name).cloned() {
            let current_path = PathBuf::from(&addon.current_path);
            if current_path.exists() {
                let dir = current_path.parent().unwrap();
                let new_filename = if addon.is_enabled {
                    sanitized.clone()
                } else {
                    format!("{}.disabled", sanitized)
                };
                let dest_path = dir.join(&new_filename);

                if dest_path.exists() {
                    continue; // Skip if already exists
                }

                if fs::rename(&current_path, &dest_path).is_err() {
                    continue;
                }

                let mut updated_addon = addon.clone();
                updated_addon.vpk_name = sanitized.clone();
                updated_addon.current_path = dest_path.to_string_lossy().to_string();
                
                db.addons.insert(sanitized.clone(), updated_addon);
                db.addons.remove(&item.vpk_name);

                for g in &mut db.groups {
                    g.addons = g.addons.iter().map(|name| {
                        if name == &item.vpk_name {
                            sanitized.clone()
                        } else {
                            name.clone()
                        }
                    }).collect();
                }

                let old_base = Path::new(&item.vpk_name).file_stem().and_then(|s| s.to_str()).unwrap_or("").replace(".disabled", "");
                let new_base = Path::new(&sanitized).file_stem().and_then(|s| s.to_str()).unwrap_or("");
                
                use md5::{Md5, Digest};
                let mut hasher_old = Md5::new();
                hasher_old.update(old_base.as_bytes());
                let old_hash = hasher_old.finalize();
                
                let mut hasher_new = Md5::new();
                hasher_new.update(new_base.as_bytes());
                let new_hash = hasher_new.finalize();

                let old_img = state.cache_dir.join(format!("{:x}_image.jpg", old_hash));
                let new_img = state.cache_dir.join(format!("{:x}_image.jpg", new_hash));

                if old_img.exists() {
                    if let Ok(_) = fs::rename(&old_img, &new_img) {
                        if let Some(addon_ref) = db.addons.get_mut(&sanitized) {
                            addon_ref.image_path = Some(format!("/cache/{:x}_image.jpg", new_hash));
                        }
                    }
                }
            }
        }
    }

    scan_addons_internal(&mut db, &state.db_path, &state.known_addons_path, &state.cache_dir).await?;
    Ok(db.clone())
}

#[tauri::command]
pub async fn group_action(
    action: String,
    name: Option<String>,
    group_id: Option<String>,
    ids: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    workshop_collection_id: Option<String>,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;

    if action == "create" {
        let name = name.ok_or_else(|| "Missing name".to_string())?;
        let ids = ids.ok_or_else(|| "Missing ids".to_string())?;

        let group_id = format!("group_{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos());

        for g in &mut db.groups {
            g.addons.retain(|n| !ids.contains(n));
        }

        let filtered_vpks: Vec<String> = ids.into_iter()
            .filter(|n| db.addons.contains_key(n) || db.known_uninstalled_addons.contains_key(n) || n.ends_with(".vpk"))
            .collect();

        db.groups.push(Group {
            id: group_id,
            name,
            addons: filtered_vpks,
            tags,
            workshop_collection_id,
        });

    } else if action == "delete" {
        let group_id = group_id.ok_or_else(|| "Missing groupId".to_string())?;
        db.groups.retain(|g| g.id != group_id);

    } else if action == "add-addons" {
        let group_id = group_id.ok_or_else(|| "Missing groupId".to_string())?;
        let ids = ids.ok_or_else(|| "Missing ids".to_string())?;

        for g in &mut db.groups {
            g.addons.retain(|n| !ids.contains(n));
        }

        let valid_ids: Vec<String> = ids.into_iter()
            .filter(|n| db.addons.contains_key(n) || db.known_uninstalled_addons.contains_key(n) || n.ends_with(".vpk"))
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

        let valid_addon_names: HashSet<String> = db.addons.keys().cloned()
            .chain(db.known_uninstalled_addons.keys().cloned())
            .collect();

        if let Some(group) = db.groups.iter_mut().find(|g| g.id == group_id) {
            if let Some(n) = name {
                group.name = n;
            }
            group.tags = tags;
            group.workshop_collection_id = workshop_collection_id;

            if let Some(vpks) = ids {
                let filtered_vpks: Vec<String> = vpks.into_iter()
                    .filter(|n| valid_addon_names.contains(n) || n.ends_with(".vpk"))
                    .collect();
                group.addons = filtered_vpks;
            }
        }

    } else if action == "auto-group" {
        auto_group_internal(&mut db);
    } else {
        return Err("Unknown action".to_string());
    }

    db.groups.retain(|g| !g.addons.is_empty());

    save_db_internal(&state.db_path, &state.known_addons_path, &db);
    scan_addons_internal(&mut db, &state.db_path, &state.known_addons_path, &state.cache_dir).await?;
    Ok(db.clone())
}

#[tauri::command]
pub async fn open_workshop(
    workshop_id: String,
) -> Result<(), String> {
    let url = format!("https://steamcommunity.com/sharedfiles/filedetails/?id={}", workshop_id);
    open::that(&url).map_err(|e| format!("Failed to open workshop URL: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn open_url(
    url: String,
) -> Result<(), String> {
    open::that(&url).map_err(|e| format!("Failed to open URL: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn steam_sync(
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;
    
    // First, scan addons to populate database with any new/removed files
    scan_addons_internal(&mut db, &state.db_path, &state.known_addons_path, &state.cache_dir).await?;
    
    let mut ids = Vec::new();
    for addon in db.addons.values() {
        if let Some(ref w_id) = addon.workshop_id {
            ids.push(w_id.clone());
        }
    }

    if ids.is_empty() {
        return Ok(db.clone());
    }

    println!("Syncing Steam details manually for {} items...", ids.len());
    let steam_details_list = fetch_steam_details(&ids).await?;
    
    for details in steam_details_list {
        if let Some(w_id) = details.get("publishedfileid").and_then(|id| id.as_str()) {
            for addon in db.addons.values_mut() {
                if addon.workshop_id.as_deref() == Some(w_id) {
                    addon.steam_details = Some(details.clone());
                    if !addon.has_image {
                        if let Some(preview_url) = details.get("preview_url").and_then(|u| u.as_str()) {
                            addon.image_path = Some(preview_url.to_string());
                        }
                    }
                    break;
                }
            }
        }
    }

    save_db_internal(&state.db_path, &state.known_addons_path, &db);
    scan_addons_internal(&mut db, &state.db_path, &state.known_addons_path, &state.cache_dir).await?;
    Ok(db.clone())
}

#[tauri::command]
pub async fn fetch_workshop_html(url: String) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
        .map_err(|e| e.to_string())?;
    let res = client.get(&url)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch URL: {}", e))?;
    let body = res.text()
        .await
        .map_err(|e| format!("Failed to get response text: {}", e))?;
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
        for id in &ids {
            if let Some(addon) = db.addons.get(id) {
                let path = PathBuf::from(&addon.current_path);
                if path.exists() {
                    let _ = fs::remove_file(&path);
                }
                let disabled_path = path.with_extension("vpk.disabled");
                if disabled_path.exists() {
                    let _ = fs::remove_file(&disabled_path);
                }
            }
        }
    }

    if remove_from_known {
        let mut known_addons: HashMap<String, KnownAddonEntry> = if state.known_addons_path.exists() {
            fs::read_to_string(&state.known_addons_path)
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_default()
        } else {
            HashMap::new()
        };

        for id in &ids {
            known_addons.remove(id);
            db.known_uninstalled_addons.remove(id);
        }

        let _ = fs::write(&state.known_addons_path, serde_json::to_string_pretty(&known_addons).unwrap_or_default());
    }

    scan_addons_internal(&mut db, &state.db_path, &state.known_addons_path, &state.cache_dir).await?;
    Ok(db.clone())
}

#[tauri::command]
pub async fn download_addon(
    workshop_id: String,
    state: State<'_, crate::AppState>,
    app_handle: AppHandle,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;

    let details_list = fetch_steam_details(&[workshop_id.clone()]).await?;
    if details_list.is_empty() {
        return Err("Failed to retrieve details for workshop item".to_string());
    }
    let details = &details_list[0];
    
    let file_url = details.get("file_url")
        .and_then(|u| u.as_str())
        .ok_or_else(|| "Workshop item has no download URL (might be hidden or deleted)".to_string())?;
        
    let title = details.get("title").and_then(|t| t.as_str()).unwrap_or("Workshop Item");

    let workshop_dir = PathBuf::from(&db.settings.workshop_dir);
    if !workshop_dir.exists() {
        fs::create_dir_all(&workshop_dir).map_err(|e| e.to_string())?;
    }
    let dest_filename = format!("{}.vpk", workshop_id);
    let dest_path = workshop_dir.join(&dest_filename);

    println!("Downloading: {} (URL: {})", title, file_url);
    let client = reqwest::Client::new();
    let mut response = client.get(file_url)
        .send()
        .await
        .map_err(|e| format!("Download request failed: {}", e))?;

    let total_size = response.content_length().unwrap_or(0);
    let mut file = fs::File::create(&dest_path).map_err(|e| format!("Failed to create local file: {}", e))?;
    let mut downloaded = 0;

    #[derive(Serialize, Clone)]
    struct DownloadProgress {
        #[serde(rename = "workshopId")]
        workshop_id: String,
        percent: u32,
        downloaded: u64,
        total: u64,
    }

    while let Some(chunk) = response.chunk().await.map_err(|e| format!("Download chunk failed: {}", e))? {
        use std::io::Write;
        file.write_all(&chunk).map_err(|e| format!("Failed to write chunk: {}", e))?;
        downloaded += chunk.len() as u64;

        let percent = if total_size > 0 {
            (downloaded as f64 / total_size as f64 * 100.0) as u32
        } else {
            0
        };

        let _ = app_handle.emit("download-progress", DownloadProgress {
            workshop_id: workshop_id.clone(),
            percent,
            downloaded,
            total: total_size,
        });
    }

    let mut known_addons: HashMap<String, KnownAddonEntry> = if state.known_addons_path.exists() {
        fs::read_to_string(&state.known_addons_path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default()
    } else {
        HashMap::new()
    };

    let has_image = details.get("preview_url").is_some();
    let image_path = details.get("preview_url").and_then(|u| u.as_str()).map(|s| s.to_string());
    
    let id = workshop_id.clone();
    known_addons.insert(id.clone(), KnownAddonEntry {
        id: id.clone(),
        vpk_name: dest_filename.clone(),
        workshop_id: Some(workshop_id.clone()),
        addon_info: serde_json::Value::Null,
        has_image,
        image_path,
        steam_details: Some(details.clone()),
    });

    let _ = fs::write(&state.known_addons_path, serde_json::to_string_pretty(&known_addons).unwrap_or_default());

    scan_addons_internal(&mut db, &state.db_path, &state.known_addons_path, &state.cache_dir).await?;
    Ok(db.clone())
}

async fn fetch_collection_details_internal(collection_id: &str) -> Result<Vec<String>, String> {
    let url = "https://api.steampowered.com/ISteamRemoteStorage/GetCollectionDetails/v1/";
    let mut params = Vec::new();
    params.push(("collectioncount".to_string(), "1".to_string()));
    params.push(("publishedfileids[0]".to_string(), collection_id.to_string()));

    let client = reqwest::Client::new();
    let res = client.post(url)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Failed to query collection details: {}", e))?;

    if !res.status().is_success() {
        return Err(format!("Steam Collection API responded with status {}", res.status()));
    }

    let json: serde_json::Value = res.json()
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

#[tauri::command]
pub async fn fetch_collection(collection_id: String) -> Result<serde_json::Value, String> {
    let child_ids = fetch_collection_details_internal(&collection_id).await?;

    let mut query_ids = vec![collection_id.clone()];
    query_ids.extend(child_ids.clone());

    let details = fetch_steam_details(&query_ids).await?;

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

    let details_list = fetch_steam_details(&[workshop_id.clone()]).await?;
    if details_list.is_empty() {
        return Err("Failed to retrieve details for workshop item".to_string());
    }
    let details = &details_list[0];

    let mut known_addons: HashMap<String, KnownAddonEntry> = if state.known_addons_path.exists() {
        fs::read_to_string(&state.known_addons_path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default()
    } else {
        HashMap::new()
    };

    let dest_filename = format!("{}.vpk", workshop_id);
    let has_image = details.get("preview_url").is_some();
    let image_path = details.get("preview_url").and_then(|u| u.as_str()).map(|s| s.to_string());

    let id = workshop_id.clone();
    known_addons.insert(id.clone(), KnownAddonEntry {
        id: id.clone(),
        vpk_name: dest_filename.clone(),
        workshop_id: Some(workshop_id.clone()),
        addon_info: serde_json::Value::Null,
        has_image,
        image_path,
        steam_details: Some(details.clone()),
    });

    let _ = fs::write(&state.known_addons_path, serde_json::to_string_pretty(&known_addons).unwrap_or_default());

    scan_addons_internal(&mut db, &state.db_path, &state.known_addons_path, &state.cache_dir).await?;
    Ok(db.clone())
}

