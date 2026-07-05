use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use serde::{Serialize, Deserialize};
use tauri::{AppHandle, State, Manager};
use regex::Regex;
use crate::vpk::{extract_addon_metadata};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Settings {
    #[serde(rename = "workshopDir")]
    pub workshop_dir: String,
    #[serde(rename = "loadingDir")]
    pub loading_dir: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Addon {
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
    pub dir_type: String, // "workshop" or "loading"
    #[serde(rename = "isEnabled")]
    pub is_enabled: bool,
    #[serde(rename = "steamDetails")]
    pub steam_details: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub addons: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Database {
    pub settings: Settings,
    pub addons: HashMap<String, Addon>,
    pub groups: Vec<Group>,
}

pub fn load_db(db_path: &Path, app_handle: &AppHandle) -> Database {
    let runtime_dir = db_path.parent().unwrap_or(Path::new(""));
    let default_loading = runtime_dir.join("addons-loading").to_string_lossy().to_string();
    let default_workshop = Path::new(&default_loading).join("workshop").to_string_lossy().to_string();
    
    let default_db = Database {
        settings: Settings {
            workshop_dir: default_workshop,
            loading_dir: default_loading,
        },
        addons: HashMap::new(),
        groups: Vec::new(),
    };

    if !db_path.exists() {
        if let Ok(json) = serde_json::to_string_pretty(&default_db) {
            let _ = fs::write(db_path, json);
        }
        return default_db;
    }

    let old_default_loading = app_handle.path().app_data_dir()
        .map(|p| p.join("addons-loading").to_string_lossy().to_string())
        .unwrap_or_default();

    match fs::read_to_string(db_path) {
        Ok(content) => match serde_json::from_str::<Database>(&content) {
            Ok(mut data) => {
                if data.settings.loading_dir.is_empty() || data.settings.loading_dir == old_default_loading {
                    data.settings.loading_dir = default_db.settings.loading_dir.clone();
                }
                // Enforce workshop_dir is loading_dir/workshop
                let loading_path = Path::new(&data.settings.loading_dir);
                data.settings.workshop_dir = loading_path.join("workshop").to_string_lossy().to_string();
                data
            }
            Err(_) => default_db,
        },
        Err(_) => default_db,
    }
}

pub fn save_db(db_path: &Path, db: &Database) {
    if let Ok(json) = serde_json::to_string_pretty(db) {
        let _ = fs::write(db_path, json);
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

    let mut active_addons = HashMap::new();
    let mut new_workshop_ids = HashSet::new();

    for file_info in files_on_disk {
        let vpk_name = file_info.vpk_name.clone();
        let cached = db.addons.get(&vpk_name).cloned();

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

            let addon = Addon {
                vpk_name: vpk_name.clone(),
                workshop_id: workshop_id.clone(),
                addon_info: meta.addon_info,
                has_image: meta.has_image,
                image_path: meta.image_path,
                files_count: meta.files_count,
                file_size: file_info.size,
                parsed_at: chrono::Utc::now().to_rfc3339(),
                current_path: file_info.full_path.to_string_lossy().to_string(),
                dir_type: file_info.dir_type.clone(),
                is_enabled: file_info.is_enabled,
                steam_details: None,
            };

            if let Some(ref w_id) = workshop_id {
                new_workshop_ids.insert(w_id.clone());
            }

            active_addons.insert(vpk_name, addon);
        } else if let Some(mut addon) = cached {
            addon.file_size = file_info.size;
            addon.current_path = file_info.full_path.to_string_lossy().to_string();
            addon.dir_type = file_info.dir_type;
            addon.is_enabled = file_info.is_enabled;

            // Fallback for cached addons that don't have workshop_id yet
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

            active_addons.insert(vpk_name, addon);
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

    db.addons = active_addons;

    for g in &mut db.groups {
        g.addons.retain(|vpk| db.addons.contains_key(vpk));
    }
    db.groups.retain(|g| !g.addons.is_empty());

    save_db(db_path, db);
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
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;
    let loading_path = Path::new(&loading_dir);
    let workshop_dir = loading_path.join("workshop").to_string_lossy().to_string();
    
    db.settings.workshop_dir = workshop_dir;
    db.settings.loading_dir = loading_dir;
    save_db(&state.db_path, &db);

    scan_addons_internal(&mut db, &state.db_path, &state.cache_dir).await?;
    Ok(db.clone())
}

#[tauri::command]
pub async fn get_addons(
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;
    scan_addons_internal(&mut db, &state.db_path, &state.cache_dir).await?;
    Ok(db.clone())
}

#[tauri::command]
pub async fn move_addons(
    vpk_names: Vec<String>,
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

    for vpk_name in vpk_names {
        if let Some(addon) = db.addons.get(&vpk_name) {
            let current_path = PathBuf::from(&addon.current_path);
            if current_path.exists() {
                let file_name = current_path.file_name().unwrap();
                let dest_path = target_dir.join(file_name);
                if !dest_path.exists() {
                    let _ = fs::rename(&current_path, &dest_path);
                }
            }
        }
    }

    scan_addons_internal(&mut db, &state.db_path, &state.cache_dir).await?;
    Ok(db.clone())
}

#[tauri::command]
pub async fn toggle_addons(
    vpk_names: Vec<String>,
    enabled: bool,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;
    
    for vpk_name in vpk_names {
        if let Some(addon) = db.addons.get(&vpk_name) {
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

    scan_addons_internal(&mut db, &state.db_path, &state.cache_dir).await?;
    Ok(db.clone())
}

#[tauri::command]
pub async fn rename_addon(
    vpk_name: String,
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
    
    if let Some(addon) = db.addons.get(&vpk_name).cloned() {
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
            
            db.addons.insert(sanitized.clone(), updated_addon);
            db.addons.remove(&vpk_name);

            for g in &mut db.groups {
                g.addons = g.addons.iter().map(|name| {
                    if name == &vpk_name {
                        sanitized.clone()
                    } else {
                        name.clone()
                    }
                }).collect();
            }

            let old_base = Path::new(&vpk_name).file_stem().and_then(|s| s.to_str()).unwrap_or("").replace(".disabled", "");
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

    scan_addons_internal(&mut db, &state.db_path, &state.cache_dir).await?;
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

    scan_addons_internal(&mut db, &state.db_path, &state.cache_dir).await?;
    Ok(db.clone())
}

#[tauri::command]
pub async fn group_action(
    action: String,
    name: Option<String>,
    group_id: Option<String>,
    vpk_names: Option<Vec<String>>,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;

    if action == "create" {
        let name = name.ok_or_else(|| "Missing name".to_string())?;
        let vpk_names = vpk_names.ok_or_else(|| "Missing vpkNames".to_string())?;

        let group_id = format!("group_{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos());

        for g in &mut db.groups {
            g.addons.retain(|n| !vpk_names.contains(n));
        }

        let filtered_vpks: Vec<String> = vpk_names.into_iter()
            .filter(|n| db.addons.contains_key(n))
            .collect();

        db.groups.push(Group {
            id: group_id,
            name,
            addons: filtered_vpks,
        });

    } else if action == "delete" {
        let group_id = group_id.ok_or_else(|| "Missing groupId".to_string())?;
        db.groups.retain(|g| g.id != group_id);

    } else if action == "add-addons" {
        let group_id = group_id.ok_or_else(|| "Missing groupId".to_string())?;
        let vpk_names = vpk_names.ok_or_else(|| "Missing vpkNames".to_string())?;

        for g in &mut db.groups {
            g.addons.retain(|n| !vpk_names.contains(n));
        }

        let valid_vpk_names: Vec<String> = vpk_names.into_iter()
            .filter(|n| db.addons.contains_key(n))
            .collect();

        if let Some(target_group) = db.groups.iter_mut().find(|g| g.id == group_id) {
            for name in valid_vpk_names {
                if !target_group.addons.contains(&name) {
                    target_group.addons.push(name);
                }
            }
        } else {
            return Err("Group not found".to_string());
        }


    } else if action == "remove-addons" {
        let group_id = group_id.ok_or_else(|| "Missing groupId".to_string())?;
        let vpk_names = vpk_names.ok_or_else(|| "Missing vpkNames".to_string())?;

        if let Some(group) = db.groups.iter_mut().find(|g| g.id == group_id) {
            group.addons.retain(|n| !vpk_names.contains(n));
        }

    } else if action == "rename-group" {
        let group_id = group_id.ok_or_else(|| "Missing groupId".to_string())?;
        let name = name.ok_or_else(|| "Missing name".to_string())?;

        if let Some(group) = db.groups.iter_mut().find(|g| g.id == group_id) {
            group.name = name;
        }

    } else if action == "auto-group" {
        auto_group_internal(&mut db);
    } else {
        return Err("Unknown action".to_string());
    }

    db.groups.retain(|g| !g.addons.is_empty());

    save_db(&state.db_path, &db);
    scan_addons_internal(&mut db, &state.db_path, &state.cache_dir).await?;
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
    scan_addons_internal(&mut db, &state.db_path, &state.cache_dir).await?;
    
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

    save_db(&state.db_path, &db);
    scan_addons_internal(&mut db, &state.db_path, &state.cache_dir).await?;
    Ok(db.clone())
}

