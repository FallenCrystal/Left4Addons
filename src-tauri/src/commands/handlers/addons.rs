use super::*;
use tauri::{AppHandle, State};

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

        if !move_requires_dir_change(addon, &target_dir_type) {
            continue;
        }

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
        &state.known_addons_path,
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

        if !toggle_requires_state_change(addon, enabled) {
            continue;
        }

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
        &state.known_addons_path,
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
        &state.known_addons_path,
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

        if !rename_requires_name_change(&addon, &sanitized) {
            continue;
        }

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
        &state.known_addons_path,
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
                let source_policy = SourcePolicy::from_settings(&db.settings);
                if let Ok(steam_details_list) = fetch_steam_details_hybrid(
                    &state.workshop_service,
                    &new_workshop_ids,
                    source_policy.allow_bridge(),
                    source_policy.allow_web_api(),
                    source_policy.source_order(),
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
        &state.known_addons_path,
    ))
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
        &state.known_addons_path,
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
        let (workshop_dir, source_policy, force_sdk_download) = {
            let db = state.db.lock().await;
            (
                PathBuf::from(&db.settings.workshop_dir),
                SourcePolicy::from_settings(&db.settings),
                db.settings.force_steamworks_sdk_download,
            )
        };
        let details_list = fetch_steam_details_hybrid(
            &state.workshop_service,
            std::slice::from_ref(&workshop_id),
            source_policy.allow_bridge(),
            source_policy.allow_web_api(),
            source_policy.source_order(),
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
        let file_url = details
            .get("file_url")
            .and_then(|u| u.as_str())
            .map(|s| s.to_string());
        let prefer_sdk_download = force_sdk_download || file_url.is_none();

        if !workshop_dir.exists() {
            fs::create_dir_all(&workshop_dir).map_err(|e| e.to_string())?;
        }
        if !state.download_cache_dir.exists() {
            fs::create_dir_all(&state.download_cache_dir).map_err(|e| {
                format!(
                    "Failed to create download cache directory {}: {}",
                    state.download_cache_dir.display(),
                    e
                )
            })?;
        }

        let dest_filename = format!("{}.vpk", workshop_id);
        let dest_path = workshop_dir.join(&dest_filename);
        let partial_path = partial_download_path(&state.download_cache_dir, &workshop_id);
        let partial_metadata_path =
            partial_download_metadata_path(&state.download_cache_dir, &workshop_id);
        remove_dummy_workshop_targets(&dest_path)?;
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

        if prefer_sdk_download {
            match attempt_bridge_download(
                &state,
                &state.workshop_service,
                &workshop_id,
                &workshop_dir,
                &app_handle,
                source_policy.allow_bridge(),
            )
            .await
            {
                Ok(true) => {
                    let _ = cleanup_partial_download(&state.download_cache_dir, &workshop_id);
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
                        &state.known_addons_path,
                    ));
                }
                Ok(false) => {}
                Err(err) if err == DOWNLOAD_CANCELLED_ERR => return Err(err),
                Err(_) => {}
            }
        }

        let file_url = match file_url {
            Some(url) => url,
            None if !prefer_sdk_download => {
                match attempt_bridge_download(
                    &state,
                    &state.workshop_service,
                    &workshop_id,
                    &workshop_dir,
                    &app_handle,
                    source_policy.allow_bridge(),
                )
                .await
                {
                    Ok(true) => {
                        let _ = cleanup_partial_download(&state.download_cache_dir, &workshop_id);
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
                            &state.known_addons_path,
                        ));
                    }
                    Ok(false) => {}
                    Err(err) if err == DOWNLOAD_CANCELLED_ERR => return Err(err),
                    Err(_) => {}
                }

                return Err(
                    "Workshop item has no direct download URL, and Steamworks SDK fallback did not install it".to_string()
                );
            }
            None => {
                return Err(
                    "Workshop item has no download URL (Steamworks SDK did not install it and no direct Web API file_url is available)".to_string()
                );
            }
        };

        println!("Downloading: {} (URL: {})", title, file_url);
        let client = reqwest::Client::new();

        let mut resume_from = 0u64;
        if partial_path.exists() || partial_metadata_path.exists() {
            match load_partial_download_metadata(&partial_metadata_path) {
                Ok(metadata)
                    if metadata.workshop_id == workshop_id
                        && metadata.target_filename == dest_filename
                        && metadata.file_url == file_url =>
                {
                    resume_from = fs::metadata(&partial_path)
                        .map(|meta| meta.len())
                        .unwrap_or(0);
                }
                Ok(_) | Err(_) => {
                    cleanup_partial_download(&state.download_cache_dir, &workshop_id)?;
                }
            }
        }

        let mut response = if resume_from > 0 {
            let resumed = client
                .get(&file_url)
                .header(reqwest::header::RANGE, format!("bytes={}-", resume_from))
                .send()
                .await
                .map_err(|e| format!("Download request failed: {}", e))?;

            match resumed.status() {
                reqwest::StatusCode::PARTIAL_CONTENT => resumed,
                reqwest::StatusCode::OK => {
                    cleanup_partial_download(&state.download_cache_dir, &workshop_id)?;
                    resume_from = 0;
                    client
                        .get(&file_url)
                        .send()
                        .await
                        .map_err(|e| format!("Download request failed: {}", e))?
                }
                _ => resumed,
            }
        } else {
            client
                .get(&file_url)
                .send()
                .await
                .map_err(|e| format!("Download request failed: {}", e))?
        };

        if !response.status().is_success() {
            if !prefer_sdk_download {
                match attempt_bridge_download(
                    &state,
                    &state.workshop_service,
                    &workshop_id,
                    &workshop_dir,
                    &app_handle,
                    source_policy.allow_bridge(),
                )
                .await
                {
                    Ok(true) => {
                        let _ = cleanup_partial_download(&state.download_cache_dir, &workshop_id);
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
                            &state.known_addons_path,
                        ));
                    }
                    Ok(false) => {}
                    Err(err) if err == DOWNLOAD_CANCELLED_ERR => return Err(err),
                    Err(_) => {}
                }
            }

            return Err(format!(
                "Download server responded with status {}",
                response.status()
            ));
        }

        let response_etag = normalized_etag(response.headers());
        let response_last_modified = normalized_last_modified(response.headers());
        let total_size = if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            let content_range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| "Partial download response missing Content-Range".to_string())?;
            let (range_start, range_total) = parse_content_range_start(content_range)
                .ok_or_else(|| format!("Invalid Content-Range header: {}", content_range))?;

            if range_start != resume_from {
                cleanup_partial_download(&state.download_cache_dir, &workshop_id)?;
                return Err(format!(
                    "Resume offset mismatch: expected {}, got {}",
                    resume_from, range_start
                ));
            }

            if let Ok(existing_metadata) = load_partial_download_metadata(&partial_metadata_path) {
                if let (Some(existing), Some(current)) =
                    (existing_metadata.etag.as_deref(), response_etag.as_deref())
                {
                    if existing != current {
                        cleanup_partial_download(&state.download_cache_dir, &workshop_id)?;
                        return Err(
                            "Remote file changed while resuming download (ETag mismatch)"
                                .to_string(),
                        );
                    }
                }
                if let (Some(existing), Some(current)) = (
                    existing_metadata.last_modified.as_deref(),
                    response_last_modified.as_deref(),
                ) {
                    if existing != current {
                        cleanup_partial_download(&state.download_cache_dir, &workshop_id)?;
                        return Err(
                            "Remote file changed while resuming download (Last-Modified mismatch)"
                                .to_string(),
                        );
                    }
                }
                if let Some(existing_total) = existing_metadata.total_size {
                    if existing_total != range_total {
                        cleanup_partial_download(&state.download_cache_dir, &workshop_id)?;
                        return Err(
                            "Remote file changed while resuming download (size mismatch)"
                                .to_string(),
                        );
                    }
                }
            }

            range_total
        } else {
            response.content_length().unwrap_or(0)
        };

        let metadata = DownloadResumeMetadata {
            workshop_id: workshop_id.clone(),
            target_filename: dest_filename.clone(),
            file_url: file_url.clone(),
            total_size: (total_size > 0).then_some(total_size),
            etag: response_etag,
            last_modified: response_last_modified,
        };
        save_partial_download_metadata(&partial_metadata_path, &metadata)?;

        let mut file = if resume_from > 0 {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&partial_path)
                .map_err(|e| format!("Failed to reopen partial download file: {}", e))?
        } else {
            fs::File::create(&partial_path)
                .map_err(|e| format!("Failed to create local file: {}", e))?
        };
        let mut downloaded = resume_from;

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
                let _ = cleanup_partial_download(&state.download_cache_dir, &workshop_id);
                return Err(DOWNLOAD_CANCELLED_ERR.to_string());
            }

            use std::io::Write;
            file.write_all(&chunk)
                .map_err(|e| format!("Failed to write chunk: {}", e))?;
            downloaded += chunk.len() as u64;

            let percent = if total_size > 0 {
                ((downloaded as f64 / total_size as f64) * 100.0) as u32
            } else {
                0
            }
            .min(99);

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
            let _ = cleanup_partial_download(&state.download_cache_dir, &workshop_id);
            return Err(DOWNLOAD_CANCELLED_ERR.to_string());
        }

        crate::watcher::suppress_internal_refresh_for(
            &state,
            Duration::from_millis(DOWNLOAD_FINALIZE_SUPPRESS_MS),
        );
        move_or_copy_file(&partial_path, &dest_path)
            .map_err(|e| format!("Failed to finalize downloaded file: {}", e))?;
        let _ = fs::remove_file(&partial_metadata_path);

        let _ = app_handle.emit(
            "download-progress",
            DownloadProgress {
                workshop_id: workshop_id.clone(),
                percent: 100,
                downloaded: total_size,
                total: total_size,
                source: "web-fallback".to_string(),
                phase: "finalize".to_string(),
            },
        );

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
            &state.known_addons_path,
        ))
    }
    .await;

    let _ = clear_download_cancellation(&state, &workshop_id);
    result
}
