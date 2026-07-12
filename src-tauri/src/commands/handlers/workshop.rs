use super::*;
use crate::mirrors::MirrorClientExt;
use tauri::State;

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
    let db = state.db.lock().await;
    if !SourcePolicy::from_settings(&db.settings).allow_bridge() {
        return Ok(WorkshopCapabilities {
            bridge_available: false,
            bridge_loaded: false,
            bridge_initialized: false,
            provider: "web-fallback".to_string(),
            bridge_version: None,
            last_error: Some("Steamworks SDK workshop source is disabled by settings".to_string()),
            current_user_steam_id: None,
            current_user_account_id: None,
            can_query_items: false,
            can_query_home: false,
            can_download: false,
            can_enumerate_installed: false,
            can_enumerate_subscribed: false,
        });
    }
    drop(db);
    Ok(state.workshop_service.capabilities())
}

#[tauri::command]
pub async fn query_workshop_home(
    state: State<'_, crate::AppState>,
) -> Result<WorkshopHomeResponse, String> {
    let db = state.db.lock().await;
    if !SourcePolicy::from_settings(&db.settings).allow_bridge() {
        return Err("Steamworks SDK workshop source is disabled".to_string());
    }
    drop(db);
    state.workshop_service.bridge_query_home()
}

#[tauri::command]
pub async fn query_workshop_items(
    query: WorkshopBrowseQuery,
    state: State<'_, crate::AppState>,
) -> Result<WorkshopItemsResponse, String> {
    let db = state.db.lock().await;
    if !SourcePolicy::from_settings(&db.settings).allow_bridge() {
        return Err("Steamworks SDK workshop source is disabled".to_string());
    }
    drop(db);
    state.workshop_service.bridge_query_items(&query)
}

#[tauri::command]
pub async fn query_workshop_item(
    workshop_id: String,
    state: State<'_, crate::AppState>,
) -> Result<WorkshopItemResponse, String> {
    let db = state.db.lock().await;
    if !SourcePolicy::from_settings(&db.settings).allow_bridge() {
        return Err("Steamworks SDK workshop source is disabled".to_string());
    }
    drop(db);
    state.workshop_service.bridge_query_item(&workshop_id)
}

#[tauri::command]
pub async fn query_workshop_details(
    workshop_ids: Vec<String>,
    state: State<'_, crate::AppState>,
) -> Result<WorkshopItemsResponse, String> {
    let db = state.db.lock().await;
    if !SourcePolicy::from_settings(&db.settings).allow_bridge() {
        return Err("Steamworks SDK workshop source is disabled".to_string());
    }
    drop(db);

    Ok(WorkshopItemsResponse {
        source: "steam-sdk".to_string(),
        items: state.workshop_service.bridge_fetch_details(&workshop_ids)?,
        warnings: Vec::new(),
    })
}

#[tauri::command]
pub async fn query_workshop_collection(
    workshop_id: String,
    state: State<'_, crate::AppState>,
) -> Result<WorkshopCollectionResponse, String> {
    let db = state.db.lock().await;
    if !SourcePolicy::from_settings(&db.settings).allow_bridge() {
        return Err("Steamworks SDK workshop source is disabled".to_string());
    }
    drop(db);
    state.workshop_service.bridge_query_collection(&workshop_id)
}

#[tauri::command]
pub async fn steam_sync(state: State<'_, crate::AppState>) -> Result<Database, String> {
    let (mut ids, source_policy, can_enumerate_subscribed) = {
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
        validate_workshop_id_candidates(&mut db, &state.workshop_service, true).await;

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
        let source_policy = SourcePolicy::from_settings(&db.settings);
        let can_enumerate_subscribed = source_policy.allow_bridge()
            && state
                .workshop_service
                .capabilities()
                .can_enumerate_subscribed;

        (ids, source_policy, can_enumerate_subscribed)
    };

    let mut subscribed_ids = Vec::new();
    if can_enumerate_subscribed {
        match state.workshop_service.bridge_get_subscribed_items() {
            Ok(response) => {
                subscribed_ids = response
                    .items
                    .into_iter()
                    .map(|item| item.workshop_id)
                    .filter(|id| !id.trim().is_empty())
                    .collect();
            }
            Err(err) => {
                eprintln!(
                    "Failed to enumerate subscribed workshop items via Steam SDK: {}",
                    err
                );
            }
        }
    }

    ids.extend(subscribed_ids.iter().cloned());
    ids.sort();
    ids.dedup();

    if ids.is_empty() {
        let db = state.db.lock().await;
        return Ok(database_with_workshop_cache(
            &db,
            &state.workshop_cache_path,
            &state.known_addons_path,
        ));
    }

    if !source_policy.allow_bridge() && !source_policy.allow_web_api() {
        let db = state.db.lock().await;
        return Ok(database_with_workshop_cache(
            &db,
            &state.workshop_cache_path,
            &state.known_addons_path,
        ));
    }

    println!("Syncing Steam details manually for {} items...", ids.len());
    let steam_details_list = fetch_steam_details_hybrid(
        &state.workshop_service,
        &ids,
        source_policy.allow_bridge(),
        source_policy.allow_web_api(),
        source_policy.source_order(),
    )
    .await?;
    let mut details_by_id = HashMap::new();
    for details in steam_details_list {
        if let Some(w_id) = details.get("publishedfileid").and_then(|id| id.as_str()) {
            details_by_id.insert(w_id.to_string(), details);
        }
    }

    let mut favorited_collection_ids = Vec::new();
    if source_policy.allow_bridge() {
        match state.workshop_service.bridge_get_favorited_collections() {
            Ok(response) => {
                for item in response.items {
                    if let Some(collection_id) =
                        item.get("publishedfileid").and_then(|id| id.as_str())
                    {
                        favorited_collection_ids.push(collection_id.to_string());
                        details_by_id
                            .entry(collection_id.to_string())
                            .or_insert(item);
                    }
                }
            }
            Err(err) => {
                eprintln!(
                    "Failed to enumerate favorited workshop collections via Steam SDK: {}",
                    err
                );
            }
        }
    }
    favorited_collection_ids.sort();
    favorited_collection_ids.dedup();

    let mut collection_children: HashMap<String, Vec<String>> = HashMap::new();
    let mut extra_detail_ids = Vec::new();
    for collection_id in &favorited_collection_ids {
        match state
            .workshop_service
            .bridge_query_collection(collection_id)
        {
            Ok(payload) => {
                if let Some(w_id) = payload
                    .collection
                    .get("publishedfileid")
                    .and_then(|id| id.as_str())
                {
                    details_by_id.insert(w_id.to_string(), payload.collection.clone());
                }
                let mut child_ids = Vec::new();
                for item in payload.items {
                    if let Some(child_id) = item.get("publishedfileid").and_then(|id| id.as_str()) {
                        child_ids.push(child_id.to_string());
                        if !details_by_id.contains_key(child_id) {
                            extra_detail_ids.push(child_id.to_string());
                        }
                        details_by_id.insert(child_id.to_string(), item);
                    }
                }
                child_ids.sort();
                child_ids.dedup();
                if !child_ids.is_empty() {
                    collection_children.insert(collection_id.clone(), child_ids);
                }
            }
            Err(err) => {
                eprintln!(
                    "Failed to query favorited collection {} via Steam SDK: {}",
                    collection_id, err
                );
                if let Ok(child_ids) = fetch_collection_children_hybrid(
                    &state.workshop_service,
                    collection_id,
                    source_policy.allow_bridge(),
                    source_policy.allow_web_api(),
                    source_policy.source_order(),
                )
                .await
                {
                    let mut child_ids = child_ids;
                    child_ids.sort();
                    child_ids.dedup();
                    for child_id in &child_ids {
                        if !details_by_id.contains_key(child_id) {
                            extra_detail_ids.push(child_id.clone());
                        }
                    }
                    if !child_ids.is_empty() {
                        collection_children.insert(collection_id.clone(), child_ids);
                    }
                }
            }
        }
    }

    if !extra_detail_ids.is_empty() {
        extra_detail_ids.sort();
        extra_detail_ids.dedup();
        if let Ok(extra_details) = fetch_steam_details_hybrid(
            &state.workshop_service,
            &extra_detail_ids,
            source_policy.allow_bridge(),
            source_policy.allow_web_api(),
            source_policy.source_order(),
        )
        .await
        {
            for details in extra_details {
                if let Some(w_id) = details.get("publishedfileid").and_then(|id| id.as_str()) {
                    details_by_id.insert(w_id.to_string(), details);
                }
            }
        }
    }

    let mut db = state.db.lock().await;
    let mut known_addons = load_known_addons(&state.known_addons_path);
    for (workshop_id, details) in &details_by_id {
        if !is_collection_detail(details) {
            upsert_known_addon_entry(&mut known_addons, workshop_id, Some(details));
        }
    }
    for workshop_id in &subscribed_ids {
        if details_by_id
            .get(workshop_id)
            .map(|details| !is_collection_detail(details))
            .unwrap_or(true)
        {
            upsert_known_addon_entry(
                &mut known_addons,
                workshop_id,
                details_by_id.get(workshop_id),
            );
        }
    }
    for child_ids in collection_children.values() {
        for workshop_id in child_ids {
            upsert_known_addon_entry(
                &mut known_addons,
                workshop_id,
                details_by_id.get(workshop_id),
            );
        }
    }

    fs::write(
        &state.known_addons_path,
        serde_json::to_string_pretty(&known_addons).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    if !collection_children.is_empty() {
        let ws_mc_id = ensure_workshop_import_master_collection(&mut db);
        for (collection_id, child_ids) in collection_children {
            let title = details_by_id
                .get(&collection_id)
                .and_then(|details| details.get("title"))
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("Workshop Collection")
                .to_string();

            if let Some(group) = db.groups.iter_mut().find(|group| {
                group.workshop_collection_id.as_deref() == Some(collection_id.as_str())
            }) {
                group.addons = child_ids.clone();
                if !title.is_empty() {
                    group.name = title.clone();
                }
                group.workshop_collection_id = Some(collection_id.clone());
                ensure_group_in_master_collection(group, &ws_mc_id);
            } else {
                let group_id = format!(
                    "group_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                );
                db.groups.push(Group {
                    id: group_id,
                    name: title,
                    addons: child_ids,
                    tags: None,
                    workshop_collection_id: Some(collection_id),
                    master_collection_ids: Some(vec![ws_mc_id.clone()]),
                    source: Some("workshop-import".to_string()),
                });
            }
        }

        let ws_group_ids: Vec<String> = db
            .groups
            .iter()
            .filter(|group| {
                group
                    .master_collection_ids
                    .as_ref()
                    .is_some_and(|ids| ids.iter().any(|id| id == &ws_mc_id))
            })
            .map(|group| group.id.clone())
            .collect();
        if let Some(master_collection) = db
            .master_collections
            .iter_mut()
            .find(|mc| mc.id == ws_mc_id)
        {
            master_collection.group_ids = ws_group_ids;
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
        &state.known_addons_path,
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

    let (allow_html, sdk_query_available) = {
        let db = state.db.lock().await;
        let source_policy = SourcePolicy::from_settings(&db.settings);
        let sdk_query_available = if source_policy.allow_bridge() {
            let capabilities = state.workshop_service.capabilities();
            capabilities.can_query_items || capabilities.can_query_home
        } else {
            false
        };
        (
            source_policy.allow_html(&source, sdk_query_available),
            sdk_query_available,
        )
    };
    if !allow_html {
        let err = if sdk_query_available {
            "Steam Community HTML fetching is disabled for this scope while Steamworks SDK is available. Adjust the allowed HTML fetch scope in settings to allow it.".to_string()
        } else {
            "Steam Community HTML fetching is disabled by source settings".to_string()
        };
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

    if is_background_workshop_fetch_source(&source) {
        let workshop_id = extract_workshop_id_from_url(parsed.as_str())
            .ok_or_else(|| "Background workshop fetch requires a workshop item URL".to_string())?;
        if let Err(err) = ensure_background_workshop_fetch_allowed(
            &source,
            &workshop_id,
            &state.known_addons_path,
            &state.groups_path,
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
    let wait = match reserve_workshop_html_fetch_slot() {
        Ok(wait) => wait,
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
    if let Some(wait) = wait {
        std::thread::sleep(wait);
    }
    let client = crate::mirrors::MirrorManager::client_builder_for(parsed.as_str())
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
        .map_err(|e| e.to_string())?;

    let res = match client.get_mirrored(parsed.as_str()).send().await {
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

    if let Some(message) = extract_steamcommunity_error_message(&body) {
        if message.to_lowercase().contains("too many request") {
            pause_workshop_html_fetches(&message);
        }
        let _ = append_workshop_crawl_log_internal(
            &state.workshop_crawl_log_path,
            serde_json::json!({
                "at": started_at,
                "source": source,
                "url": url,
                "ok": false,
                "status": status,
                "error": message,
            }),
        );
        return Err(message);
    }

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
pub async fn fetch_collection(
    collection_id: String,
    state: State<'_, crate::AppState>,
) -> Result<serde_json::Value, String> {
    let source_policy = {
        let db = state.db.lock().await;
        SourcePolicy::from_settings(&db.settings)
    };

    if source_policy.allow_bridge() {
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
        source_policy.allow_bridge(),
        source_policy.allow_web_api(),
        source_policy.source_order(),
    )
    .await?;

    let mut query_ids = vec![collection_id.clone()];
    query_ids.extend(child_ids.clone());

    let details = fetch_steam_details_hybrid(
        &state.workshop_service,
        &query_ids,
        source_policy.allow_bridge(),
        source_policy.allow_web_api(),
        source_policy.source_order(),
    )
    .await?;

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
    let source_policy = {
        let db = state.db.lock().await;
        SourcePolicy::from_settings(&db.settings)
    };

    let details = fetch_steam_details_hybrid(
        &state.workshop_service,
        std::slice::from_ref(&workshop_id),
        source_policy.allow_bridge(),
        source_policy.allow_web_api(),
        source_policy.source_order(),
    )
    .await
    .ok()
    .and_then(|mut details| details.drain(..).next());

    let mut known_addons = load_known_addons(&state.known_addons_path);

    let dest_filename = format!("{}.vpk", workshop_id);
    let has_image = details
        .as_ref()
        .and_then(|details| details.get("preview_url"))
        .is_some();
    let image_path = details
        .as_ref()
        .and_then(|details| details.get("preview_url"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string());

    let id = workshop_id.clone();
    known_addons.insert(
        id.clone(),
        KnownAddonEntry {
            id: id.clone(),
            vpk_name: dest_filename.clone(),
            workshop_id: Some(workshop_id.clone()),
            workshop_id_candidate: None,
            workshop_id_source: None,
            workshop_id_validation_status: None,
            workshop_id_last_attempt_at: None,
            addon_info: serde_json::Value::Null,
            has_image,
            image_path,
            steam_details: details.clone(),
        },
    );

    let _ = fs::write(
        &state.known_addons_path,
        serde_json::to_string_pretty(&known_addons).unwrap_or_default(),
    );

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

#[tauri::command]
pub async fn get_workshop_cache(
    state: State<'_, crate::AppState>,
) -> Result<HashMap<String, serde_json::Value>, String> {
    Ok(workshop_cache_with_known_addons(
        &state.workshop_cache_path,
        &state.known_addons_path,
    ))
}

#[tauri::command]
pub async fn record_workshop_items_seen(
    items: Vec<WorkshopSeenItem>,
    source: Option<String>,
    state: State<'_, crate::AppState>,
) -> Result<Database, String> {
    // Browse snapshots and dependency details both replace the cache file. Keep
    // their read-modify-write sections atomic with respect to one another.
    let cache_write_guard = state
        .workshop_cache_write_lock
        .lock()
        .map_err(|_| "Failed to acquire workshop cache write lock".to_string())?;
    let mut cache = load_workshop_cache(&state.workshop_cache_path);
    let now = chrono::Utc::now().to_rfc3339();
    let source = source.unwrap_or_else(|| "unknown".to_string());

    for item in items {
        if item.workshop_id.trim().is_empty() {
            continue;
        }

        let allow_rich_content = is_known_workshop_id(
            &state.known_addons_path,
            &state.groups_path,
            &item.workshop_id,
        );
        let obj = cache_entry_object(&mut cache, &item.workshop_id);
        persist_seen_workshop_item_entry(obj, &item, &source, &now, allow_rich_content);
    }

    merge_known_addon_snapshots_into_cache(&mut cache, &state.known_addons_path);
    propagate_author_names(&mut cache);
    save_workshop_cache(&state.workshop_cache_path, &cache)?;
    drop(cache_write_guard);
    let db = state.db.lock().await;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
        &state.known_addons_path,
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

    // See record_workshop_items_seen: avoid a late seen-item write erasing this
    // item's requiredItems relation.
    let cache_write_guard = state
        .workshop_cache_write_lock
        .lock()
        .map_err(|_| "Failed to acquire workshop cache write lock".to_string())?;
    let mut cache = load_workshop_cache(&state.workshop_cache_path);
    let now = chrono::Utc::now().to_rfc3339();
    let source = source.unwrap_or_else(|| "unknown".to_string());
    ensure_background_workshop_fetch_allowed(
        &source,
        &workshop_id,
        &state.known_addons_path,
        &state.groups_path,
    )?;
    let allow_rich_content =
        is_known_workshop_id(&state.known_addons_path, &state.groups_path, &workshop_id);

    {
        let obj = cache_entry_object(&mut cache, &workshop_id);
        persist_workshop_page_details_entry(
            obj,
            &workshop_id,
            &details,
            &source,
            &now,
            allow_rich_content,
        );
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

    if let Some(collection_items) = details.get("collectionItems").and_then(|v| v.as_array()) {
        for item in collection_items {
            let Some(child_id) = item.get("workshopId").and_then(|v| v.as_str()) else {
                continue;
            };
            let child_obj = cache_entry_object(&mut cache, child_id);
            insert_non_empty_string(child_obj, "workshopId", child_id);
            if let Some(title) = item.get("title").and_then(|v| v.as_str()) {
                insert_non_empty_string(child_obj, "title", title);
            }
            if let Some(author_name) = item.get("authorName").and_then(|v| v.as_str()) {
                let author_ids = [
                    item.get("authorSteamId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    item.get("authorAccountId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    item.get("authorVanityId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                ];
                insert_author_name_if_useful(child_obj, author_name, &author_ids);
            }
            if let Some(author_url) = item.get("authorUrl").and_then(|v| v.as_str()) {
                insert_non_empty_string(child_obj, "creatorProfileUrl", author_url);
                insert_non_empty_string(child_obj, "authorUrl", author_url);
            }
            if let Some(author_steam_id) = item.get("authorSteamId").and_then(|v| v.as_str()) {
                insert_non_empty_string(child_obj, "creatorSteamId", author_steam_id);
                insert_non_empty_string(child_obj, "creatorId", author_steam_id);
            }
            if let Some(author_vanity_id) = item.get("authorVanityId").and_then(|v| v.as_str()) {
                insert_non_empty_string(child_obj, "creatorVanityId", author_vanity_id);
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

    merge_known_addon_snapshots_into_cache(&mut cache, &state.known_addons_path);
    propagate_author_names(&mut cache);
    save_workshop_cache(&state.workshop_cache_path, &cache)?;
    drop(cache_write_guard);
    let db = state.db.lock().await;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
        &state.known_addons_path,
    ))
}
