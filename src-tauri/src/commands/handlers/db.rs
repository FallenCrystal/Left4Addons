use super::*;
use tauri::{AppHandle, State, Manager};

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
            download_concurrency: DEFAULT_DOWNLOAD_CONCURRENCY,
            enable_dummy_bypass: false,
            suppress_sdk_unavailable_warning: false,
            disable_steamworks_sdk: false,
            force_steamworks_sdk_download: false,
            workshop_source_settings: WorkshopSourceSettings::default(),
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
    if settings_store.settings.download_concurrency == 0 {
        settings_store.settings.download_concurrency = DEFAULT_DOWNLOAD_CONCURRENCY;
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

pub async fn rescan_database_snapshot(app_handle: &AppHandle) -> Result<(Database, bool), String> {
    let state = app_handle.state::<crate::AppState>();
    let mut db = state.db.lock().await;
    let before =
        database_with_workshop_cache(&db, &state.workshop_cache_path, &state.known_addons_path);
    scan_addons_internal(
        &mut db,
        &state.settings_path,
        &state.groups_path,
        &state.known_addons_path,
        &state.cache_dir,
        &state.workshop_service,
    )
    .await?;
    let after =
        database_with_workshop_cache(&db, &state.workshop_cache_path, &state.known_addons_path);
    let changed = after != before;
    Ok((after, changed))
}

pub async fn scan_addons_internal(
    db: &mut Database,
    settings_path: &Path,
    groups_path: &Path,
    known_addons_path: &Path,
    cache_dir: &Path,
    _workshop_service: &WorkshopService,
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

    drop(new_workshop_ids);

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
        &state.known_addons_path,
    ))
}

