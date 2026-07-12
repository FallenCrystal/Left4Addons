use super::*;
use tauri::{AppHandle, Manager, State};

const WORKSHOP_ID_VALIDATION_RETRY_SECS: i64 = 30 * 60;
const WORKSHOP_ID_STATUS_PENDING: &str = "pending";
const WORKSHOP_ID_STATUS_VERIFIED: &str = "verified";
const WORKSHOP_ID_STATUS_REJECTED: &str = "rejected";
const WORKSHOP_ID_SOURCE_FILENAME: &str = "filename";
const WORKSHOP_ID_SOURCE_ADDON_URL: &str = "addonUrl";

fn normalize_filename_workshop_id(value: &str) -> Option<String> {
    if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    value
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
        .map(|id| id.to_string())
}

fn extract_workshop_id_from_vpk_filename(vpk_name: &str) -> Option<String> {
    let vpk_name = vpk_name.strip_suffix(".disabled").unwrap_or(vpk_name);
    if !vpk_name.ends_with(".vpk") {
        return None;
    }

    let stem = Path::new(vpk_name).file_stem()?.to_str()?;
    if let Some(id) = normalize_filename_workshop_id(stem) {
        return Some(id);
    }

    let bracketed = stem.strip_prefix('[')?;
    let (id, _) = bracketed.split_once(']')?;
    normalize_filename_workshop_id(id)
}

fn workshop_id_from_addon_url(addon: &Addon) -> Option<String> {
    addon
        .addon_info
        .get("addonurl0")
        .or_else(|| addon.addon_info.get("addonurl"))
        .and_then(|value| value.as_str())
        .and_then(extract_workshop_id_from_url)
}

fn workshop_detail_id(details: &Value) -> Option<String> {
    details
        .get("publishedfileid")
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_string)
                .or_else(|| value.as_u64().map(|id| id.to_string()))
        })
        .and_then(|id| normalize_filename_workshop_id(&id))
}

fn is_successful_workshop_detail(details: &Value) -> bool {
    match details.get("result") {
        None => true,
        Some(value) => value
            .as_u64()
            .map(|result| result == 1)
            .or_else(|| value.as_str().map(|result| result == "1"))
            .unwrap_or(false),
    }
}

fn is_valid_workshop_item_detail(candidate: &str, details: &Value) -> bool {
    workshop_detail_id(details).as_deref() == Some(candidate)
        && is_successful_workshop_detail(details)
        && !is_collection_detail(details)
}

fn workshop_id_candidate_is_due(
    addon: &Addon,
    force: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    if addon.is_dummy
        || addon.workshop_id_candidate.is_none()
        || matches!(
            addon.workshop_id_validation_status.as_deref(),
            Some(WORKSHOP_ID_STATUS_VERIFIED | WORKSHOP_ID_STATUS_REJECTED)
        )
    {
        return false;
    }

    if force {
        return true;
    }

    addon
        .workshop_id_last_attempt_at
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|attempted_at| {
            now.signed_duration_since(attempted_at.with_timezone(&chrono::Utc))
                .num_seconds()
                >= WORKSHOP_ID_VALIDATION_RETRY_SECS
        })
        .unwrap_or(true)
}

fn apply_workshop_id_validation(
    addons: &mut HashMap<String, Addon>,
    attempted_ids: &HashSet<String>,
    details_by_id: Option<&HashMap<String, Value>>,
    attempted_at: &str,
    id_migrations: &mut HashMap<String, String>,
) {
    let mut updated = HashMap::new();

    for (old_id, mut addon) in std::mem::take(addons) {
        let Some(candidate) = addon.workshop_id_candidate.clone() else {
            updated.insert(old_id, addon);
            continue;
        };
        if !attempted_ids.contains(&candidate) {
            updated.insert(old_id, addon);
            continue;
        }

        addon.workshop_id_last_attempt_at = Some(attempted_at.to_string());
        match details_by_id {
            Some(details_by_id) => {
                let valid_details = details_by_id
                    .get(&candidate)
                    .filter(|details| is_valid_workshop_item_detail(&candidate, details));
                if let Some(details) = valid_details {
                    addon.id = candidate.clone();
                    addon.workshop_id = Some(candidate.clone());
                    addon.workshop_id_validation_status =
                        Some(WORKSHOP_ID_STATUS_VERIFIED.to_string());
                    addon.steam_details = Some(details.clone());
                    if old_id != addon.id {
                        id_migrations.insert(old_id, addon.id.clone());
                    }
                } else {
                    let fallback = (addon.workshop_id_source.as_deref()
                        == Some(WORKSHOP_ID_SOURCE_FILENAME))
                    .then(|| workshop_id_from_addon_url(&addon))
                    .flatten()
                    .filter(|url_id| url_id != &candidate);
                    if let Some(url_id) = fallback {
                        addon.workshop_id_candidate = Some(url_id);
                        addon.workshop_id_source = Some(WORKSHOP_ID_SOURCE_ADDON_URL.to_string());
                        addon.workshop_id_validation_status =
                            Some(WORKSHOP_ID_STATUS_PENDING.to_string());
                        addon.workshop_id_last_attempt_at = None;
                    } else {
                        addon.workshop_id_validation_status =
                            Some(WORKSHOP_ID_STATUS_REJECTED.to_string());
                    }
                    addon.workshop_id = None;
                    addon.steam_details = None;
                }
            }
            None => {
                addon.workshop_id_validation_status = Some(WORKSHOP_ID_STATUS_PENDING.to_string());
            }
        }

        let new_id = addon.id.clone();
        updated.insert(new_id, addon);
    }

    *addons = updated;
}

pub(super) async fn validate_workshop_id_candidates(
    db: &mut Database,
    workshop_service: &WorkshopService,
    force: bool,
) {
    let now = chrono::Utc::now();
    let source_policy = SourcePolicy::from_settings(&db.settings);
    if !source_policy.allow_bridge() && !source_policy.allow_web_api() {
        return;
    }

    // A rejected filename candidate can immediately fall back to addonurl0.
    for _ in 0..2 {
        let mut candidate_ids = HashSet::new();
        for addon in db
            .addons
            .values()
            .chain(db.known_uninstalled_addons.values())
        {
            if workshop_id_candidate_is_due(addon, force, now) {
                if let Some(candidate) = addon.workshop_id_candidate.as_ref() {
                    candidate_ids.insert(candidate.clone());
                }
            }
        }
        if candidate_ids.is_empty() {
            break;
        }

        let mut candidate_ids = candidate_ids.into_iter().collect::<Vec<_>>();
        candidate_ids.sort();
        let attempted_ids = candidate_ids.iter().cloned().collect::<HashSet<_>>();
        let attempted_at = now.to_rfc3339();
        let details_by_id = match fetch_steam_details_hybrid(
            workshop_service,
            &candidate_ids,
            source_policy.allow_bridge(),
            source_policy.allow_web_api(),
            source_policy.source_order(),
        )
        .await
        {
            Ok(details) => Some(
                details
                    .into_iter()
                    .filter_map(|details| workshop_detail_id(&details).map(|id| (id, details)))
                    .collect::<HashMap<_, _>>(),
            ),
            Err(err) => {
                eprintln!("Failed to validate Workshop IDs from VPK metadata: {}", err);
                None
            }
        };

        let mut id_migrations = HashMap::new();
        apply_workshop_id_validation(
            &mut db.addons,
            &attempted_ids,
            details_by_id.as_ref(),
            &attempted_at,
            &mut id_migrations,
        );
        apply_workshop_id_validation(
            &mut db.known_uninstalled_addons,
            &attempted_ids,
            details_by_id.as_ref(),
            &attempted_at,
            &mut id_migrations,
        );

        if !id_migrations.is_empty() {
            for group in &mut db.groups {
                let mut seen = HashSet::new();
                group.addons = group
                    .addons
                    .iter()
                    .map(|id| id_migrations.get(id).cloned().unwrap_or_else(|| id.clone()))
                    .filter(|id| seen.insert(id.clone()))
                    .collect();
            }
        }
    }
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
            download_concurrency: DEFAULT_DOWNLOAD_CONCURRENCY,
            enable_dummy_bypass: false,
            suppress_sdk_unavailable_warning: false,
            disable_steamworks_sdk: false,
            force_steamworks_sdk_download: false,
            max_download_retries: 3,
            dependency_missing_behavior: "ask".to_string(),
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
                    workshop_id_candidate: entry.workshop_id_candidate.clone(),
                    workshop_id_source: entry.workshop_id_source.clone(),
                    workshop_id_validation_status: entry.workshop_id_validation_status.clone(),
                    workshop_id_last_attempt_at: entry.workshop_id_last_attempt_at.clone(),
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

    let mut db = Database {
        settings: settings_store.settings,
        addons: merged_addons,
        groups,
        known_uninstalled_addons,
        master_collections: settings_store.master_collections,
    };
    let normalized_master_collections = normalize_master_collection_group_refs(&mut db);

    if normalized_master_collections
        || !settings_existed
        || !groups_existed
        || !known_addons_existed
    {
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
    let active_vpk_ids = db
        .addons
        .values()
        .chain(db.known_uninstalled_addons.values())
        .map(|addon| (addon.vpk_name.clone(), addon.id.clone()))
        .collect::<HashMap<_, _>>();
    known_addons.retain(|id, entry| {
        active_vpk_ids
            .get(&entry.vpk_name)
            .map(|active_id| active_id == id)
            .unwrap_or(true)
    });

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
                workshop_id_candidate: addon.workshop_id_candidate.clone(),
                workshop_id_source: addon.workshop_id_source.clone(),
                workshop_id_validation_status: addon.workshop_id_validation_status.clone(),
                workshop_id_last_attempt_at: addon.workshop_id_last_attempt_at.clone(),
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
                workshop_id_candidate: addon.workshop_id_candidate.clone(),
                workshop_id_source: addon.workshop_id_source.clone(),
                workshop_id_validation_status: addon.workshop_id_validation_status.clone(),
                workshop_id_last_attempt_at: addon.workshop_id_last_attempt_at.clone(),
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
    workshop_service: &WorkshopService,
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

            let meta = extract_addon_metadata(&file_info.full_path, cache_dir);
            let filename_candidate = extract_workshop_id_from_vpk_filename(&vpk_name);
            let url_workshop_id = meta
                .addon_info
                .get("addonurl0")
                .or_else(|| meta.addon_info.get("addonurl"))
                .and_then(|value| value.as_str())
                .and_then(extract_workshop_id_from_url);

            if is_dummy_addon_info(&meta.addon_info) {
                continue;
            }

            let filename_rejected = filename_candidate.as_ref().is_some_and(|candidate| {
                cached.as_ref().is_some_and(|addon| {
                    addon.workshop_id_candidate.as_deref() == Some(candidate)
                        && addon.workshop_id_source.as_deref() == Some(WORKSHOP_ID_SOURCE_FILENAME)
                        && addon.workshop_id_validation_status.as_deref()
                            == Some(WORKSHOP_ID_STATUS_REJECTED)
                })
            });
            let (candidate, source) = if filename_rejected {
                url_workshop_id
                    .clone()
                    .map(|id| (id, WORKSHOP_ID_SOURCE_ADDON_URL.to_string()))
            } else {
                filename_candidate
                    .clone()
                    .map(|id| (id, WORKSHOP_ID_SOURCE_FILENAME.to_string()))
                    .or_else(|| {
                        url_workshop_id
                            .clone()
                            .map(|id| (id, WORKSHOP_ID_SOURCE_ADDON_URL.to_string()))
                    })
            }
            .unzip();
            let candidate_matches = cached.as_ref().is_some_and(|addon| {
                addon.workshop_id_candidate == candidate
                    && addon.workshop_id_source.as_deref() == source.as_deref()
            });
            let candidate_verified = candidate_matches
                && cached.as_ref().is_some_and(|addon| {
                    addon.workshop_id_validation_status.as_deref()
                        == Some(WORKSHOP_ID_STATUS_VERIFIED)
                        && addon.workshop_id == candidate
                });
            let workshop_id = if candidate_verified {
                candidate.clone()
            } else {
                None
            };

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
                workshop_id_candidate: candidate.clone(),
                workshop_id_source: source.clone(),
                workshop_id_validation_status: if candidate.is_none() {
                    None
                } else if candidate_matches {
                    cached
                        .as_ref()
                        .and_then(|addon| addon.workshop_id_validation_status.clone())
                } else {
                    Some(WORKSHOP_ID_STATUS_PENDING.to_string())
                },
                workshop_id_last_attempt_at: if candidate_matches {
                    cached
                        .as_ref()
                        .and_then(|addon| addon.workshop_id_last_attempt_at.clone())
                } else {
                    None
                },
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
                is_dummy: false,
            };

            known_addons.insert(
                id.clone(),
                KnownAddonEntry {
                    id: id.clone(),
                    vpk_name: vpk_name.clone(),
                    workshop_id: workshop_id.clone(),
                    workshop_id_candidate: addon.workshop_id_candidate.clone(),
                    workshop_id_source: addon.workshop_id_source.clone(),
                    workshop_id_validation_status: addon.workshop_id_validation_status.clone(),
                    workshop_id_last_attempt_at: addon.workshop_id_last_attempt_at.clone(),
                    addon_info: addon.addon_info.clone(),
                    has_image: addon.has_image,
                    image_path: addon.image_path.clone(),
                    steam_details: addon.steam_details.clone(),
                },
            );

            active_addons.insert(id.clone(), addon);
        } else if let Some(mut addon) = cached {
            addon.file_size = file_info.size;
            addon.current_path = file_info.full_path.to_string_lossy().to_string();
            addon.dir_type = file_info.dir_type;
            addon.is_enabled = file_info.is_enabled;
            addon.is_dummy = is_dummy_addon_info(&addon.addon_info);
            if addon.is_dummy {
                continue;
            }

            let filename_candidate = extract_workshop_id_from_vpk_filename(&vpk_name);
            let url_candidate = workshop_id_from_addon_url(&addon);
            let filename_rejected = filename_candidate.as_ref().is_some_and(|candidate| {
                addon.workshop_id_candidate.as_deref() == Some(candidate)
                    && addon.workshop_id_source.as_deref() == Some(WORKSHOP_ID_SOURCE_FILENAME)
                    && addon.workshop_id_validation_status.as_deref()
                        == Some(WORKSHOP_ID_STATUS_REJECTED)
            });
            let (candidate, source) = if filename_rejected {
                url_candidate.map(|id| (id, WORKSHOP_ID_SOURCE_ADDON_URL.to_string()))
            } else {
                filename_candidate
                    .map(|id| (id, WORKSHOP_ID_SOURCE_FILENAME.to_string()))
                    .or_else(|| {
                        url_candidate.map(|id| (id, WORKSHOP_ID_SOURCE_ADDON_URL.to_string()))
                    })
            }
            .unzip();
            if candidate.is_some() {
                let matches_cached =
                    addon.workshop_id_candidate == candidate && addon.workshop_id_source == source;
                let verified = matches_cached
                    && addon.workshop_id_validation_status.as_deref()
                        == Some(WORKSHOP_ID_STATUS_VERIFIED);
                addon.workshop_id_candidate = candidate.clone();
                addon.workshop_id_source = source;
                if verified {
                    addon.workshop_id = candidate;
                } else {
                    addon.workshop_id_validation_status =
                        Some(WORKSHOP_ID_STATUS_PENDING.to_string());
                    addon.workshop_id_last_attempt_at = None;
                    addon.workshop_id = None;
                }
            }

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
                                workshop_id_candidate: addon.workshop_id_candidate.clone(),
                                workshop_id_source: addon.workshop_id_source.clone(),
                                workshop_id_validation_status: addon
                                    .workshop_id_validation_status
                                    .clone(),
                                workshop_id_last_attempt_at: addon
                                    .workshop_id_last_attempt_at
                                    .clone(),
                                addon_info: addon.addon_info.clone(),
                                has_image: addon.has_image,
                                image_path: addon.image_path.clone(),
                                steam_details: addon.steam_details.clone(),
                            },
                        );
                    }
                }
            }

            let id = addon.id.clone();
            active_addons.insert(id, addon);
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
                    workshop_id_candidate: entry.workshop_id_candidate.clone(),
                    workshop_id_source: entry.workshop_id_source.clone(),
                    workshop_id_validation_status: entry.workshop_id_validation_status.clone(),
                    workshop_id_last_attempt_at: entry.workshop_id_last_attempt_at.clone(),
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
        if !addon.is_dummy
            && !is_dummy_addon_info(&addon.addon_info)
            && !active_addons.contains_key(id)
            && !uninstalled.contains_key(id)
        {
            uninstalled.insert(id.clone(), addon.clone());
        }
    }

    db.addons = active_addons;
    db.known_uninstalled_addons = uninstalled;
    validate_workshop_id_candidates(db, workshop_service, false).await;

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
    normalize_master_collection_group_refs(db);

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

#[cfg(test)]
mod tests {
    use super::{
        apply_workshop_id_validation, extract_workshop_id_from_vpk_filename,
        is_valid_workshop_item_detail, WORKSHOP_ID_SOURCE_ADDON_URL, WORKSHOP_ID_SOURCE_FILENAME,
        WORKSHOP_ID_STATUS_PENDING, WORKSHOP_ID_STATUS_REJECTED, WORKSHOP_ID_STATUS_VERIFIED,
    };
    use crate::commands::types::Addon;
    use serde_json::json;
    use std::collections::{HashMap, HashSet};

    fn addon(id: &str, candidate: &str) -> Addon {
        Addon {
            id: id.to_string(),
            vpk_name: format!("[{}]Manual Addon.vpk", candidate),
            workshop_id: None,
            workshop_id_candidate: Some(candidate.to_string()),
            workshop_id_source: Some(WORKSHOP_ID_SOURCE_FILENAME.to_string()),
            workshop_id_validation_status: Some(WORKSHOP_ID_STATUS_PENDING.to_string()),
            workshop_id_last_attempt_at: None,
            addon_info: json!({}),
            has_image: false,
            image_path: None,
            files_count: 0,
            file_size: 0,
            parsed_at: String::new(),
            current_path: String::new(),
            dir_type: "loading".to_string(),
            is_enabled: true,
            steam_details: None,
            workshop_details: None,
            is_dummy: false,
        }
    }

    #[test]
    fn extracts_only_explicit_workshop_id_filename_formats() {
        assert_eq!(
            extract_workshop_id_from_vpk_filename("3560883926.vpk"),
            Some("3560883926".to_string())
        );
        assert_eq!(
            extract_workshop_id_from_vpk_filename("[3560883926] Manual Addon.vpk"),
            Some("3560883926".to_string())
        );
        assert_eq!(
            extract_workshop_id_from_vpk_filename("[3560883926] Manual Addon.vpk.disabled"),
            Some("3560883926".to_string())
        );
        assert_eq!(
            extract_workshop_id_from_vpk_filename("3560883926 addon.vpk"),
            None
        );
        assert_eq!(
            extract_workshop_id_from_vpk_filename("addon-3560883926.vpk"),
            None
        );
        assert_eq!(
            extract_workshop_id_from_vpk_filename("[not-an-id] addon.vpk"),
            None
        );
    }

    #[test]
    fn accepts_only_existing_non_collection_details() {
        assert!(is_valid_workshop_item_detail(
            "3560883926",
            &json!({ "publishedfileid": "3560883926", "result": 1, "file_type": "item" })
        ));
        assert!(!is_valid_workshop_item_detail(
            "3560883926",
            &json!({ "publishedfileid": "3560883926", "result": 1, "file_type": "collection" })
        ));
        assert!(!is_valid_workshop_item_detail(
            "3560883926",
            &json!({ "publishedfileid": "3560883926", "result": 1, "file_type": 2 })
        ));
        assert!(!is_valid_workshop_item_detail(
            "3560883926",
            &json!({ "publishedfileid": "3560883926", "result": 9 })
        ));
        assert!(!is_valid_workshop_item_detail(
            "3560883926",
            &json!({ "publishedfileid": "123" })
        ));
    }

    #[test]
    fn confirmed_filename_id_migrates_the_local_addon_key() {
        let mut addons =
            HashMap::from([("local-hash".to_string(), addon("local-hash", "3560883926"))]);
        let attempted_ids = HashSet::from(["3560883926".to_string()]);
        let details = HashMap::from([(
            "3560883926".to_string(),
            json!({ "publishedfileid": "3560883926", "file_type": "item" }),
        )]);
        let mut migrations = HashMap::new();

        apply_workshop_id_validation(
            &mut addons,
            &attempted_ids,
            Some(&details),
            "2026-07-12T00:00:00Z",
            &mut migrations,
        );

        let resolved = addons.get("3560883926").unwrap();
        assert_eq!(resolved.workshop_id.as_deref(), Some("3560883926"));
        assert_eq!(
            resolved.workshop_id_validation_status.as_deref(),
            Some(WORKSHOP_ID_STATUS_VERIFIED)
        );
        assert_eq!(
            migrations.get("local-hash").map(String::as_str),
            Some("3560883926")
        );
    }

    #[test]
    fn rejects_collection_id_from_addon_url_after_filename_fallback() {
        let mut local = addon("local-hash", "3560883926");
        local.addon_info = json!({
            "addonurl0": "https://steamcommunity.com/sharedfiles/filedetails/?id=1234567890"
        });
        let mut addons = HashMap::from([("local-hash".to_string(), local)]);
        let attempted_ids = HashSet::from(["3560883926".to_string()]);

        apply_workshop_id_validation(
            &mut addons,
            &attempted_ids,
            Some(&HashMap::new()),
            "2026-07-12T00:00:00Z",
            &mut HashMap::new(),
        );

        let rejected = addons.get("local-hash").unwrap();
        assert_eq!(rejected.workshop_id, None);
        assert_eq!(
            rejected.workshop_id_candidate.as_deref(),
            Some("1234567890")
        );
        assert_eq!(
            rejected.workshop_id_source.as_deref(),
            Some(WORKSHOP_ID_SOURCE_ADDON_URL)
        );
        assert_eq!(
            rejected.workshop_id_validation_status.as_deref(),
            Some(WORKSHOP_ID_STATUS_PENDING)
        );

        apply_workshop_id_validation(
            &mut addons,
            &HashSet::from(["1234567890".to_string()]),
            Some(&HashMap::from([(
                "1234567890".to_string(),
                json!({ "publishedfileid": "1234567890", "file_type": "collection" }),
            )])),
            "2026-07-12T00:01:00Z",
            &mut HashMap::new(),
        );

        let rejected = addons.get("local-hash").unwrap();
        assert_eq!(rejected.workshop_id, None);
        assert_eq!(
            rejected.workshop_id_validation_status.as_deref(),
            Some(WORKSHOP_ID_STATUS_REJECTED)
        );
    }
}
