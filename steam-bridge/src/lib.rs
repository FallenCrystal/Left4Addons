use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use steamworks::{
    AccountId, AppIDs, AppId, Client, FileType, ItemState, PublishedFileId, SteamId, UGCQueryType,
    UGCStatisticType, UGCType, UserList, UserListOrder,
};

const APP_ID: u32 = 550;
const BRIDGE_VERSION: &str = "0.1.0";
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
const PERSONA_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

static RUNTIME: OnceLock<Mutex<Option<BridgeRuntime>>> = OnceLock::new();

struct BridgeRuntime {
    client: Client,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InitResponse {
    ok: bool,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_user_steam_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_user_account_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonResponse {
    ok: bool,
    payload: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeRequest {
    method: String,
    #[serde(default)]
    payload: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailsPayload {
    ids: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct QueryItemsPayload {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    section: Option<String>,
    #[serde(default)]
    page: Option<u32>,
    #[serde(default)]
    creator_id: Option<String>,
    #[serde(default)]
    active_tag: Option<String>,
    #[serde(default)]
    active_tag_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkshopIdPayload {
    workshop_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubscribedItemPayload {
    workshop_id: String,
    item_state: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    install_folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size_on_disk: Option<u64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct HomeSection {
    id: String,
    title_key: String,
    subtitle_key: String,
    icon: String,
    items: Vec<Value>,
    browse_params: BrowseParams,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BrowseParams {
    sort: String,
    section: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    days: Option<u32>,
}

#[derive(Debug, Clone)]
struct HomeSectionDef {
    id: &'static str,
    title_key: &'static str,
    subtitle_key: &'static str,
    icon: &'static str,
    sort: &'static str,
    section: &'static str,
    days: Option<u32>,
}

struct QueryExecution {
    items: Vec<Value>,
    child_ids: Vec<String>,
    total_results: u32,
    warnings: Vec<String>,
}

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
pub extern "C" fn l4a_steam_bridge_free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }

    unsafe {
        drop(CString::from_raw(ptr));
    }
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

fn query_home(runtime: &mut BridgeRuntime) -> Result<Value, String> {
    let sections = vec![
        HomeSectionDef {
            id: "past-week",
            title_key: "workshop.home.pastWeek",
            subtitle_key: "workshop.home.pastWeekDesc",
            icon: "Clock",
            sort: "trend",
            section: "readytouseitems",
            days: Some(7),
        },
        HomeSectionDef {
            id: "trending",
            title_key: "workshop.home.trending",
            subtitle_key: "workshop.home.trendingDesc",
            icon: "Flame",
            sort: "trend",
            section: "readytouseitems",
            days: Some(90),
        },
        HomeSectionDef {
            id: "most-subscribed",
            title_key: "workshop.home.mostSubscribed",
            subtitle_key: "workshop.home.mostSubscribedDesc",
            icon: "Users",
            sort: "totalprofiles",
            section: "readytouseitems",
            days: None,
        },
        HomeSectionDef {
            id: "recently-updated",
            title_key: "workshop.home.recentlyUpdated",
            subtitle_key: "workshop.home.recentlyUpdatedDesc",
            icon: "RefreshCw",
            sort: "lastupdated",
            section: "readytouseitems",
            days: None,
        },
        HomeSectionDef {
            id: "newest",
            title_key: "workshop.home.newest",
            subtitle_key: "workshop.home.newestDesc",
            icon: "Star",
            sort: "mostrecent",
            section: "readytouseitems",
            days: None,
        },
    ];

    let mut mapped_sections = Vec::new();
    let mut warnings = Vec::new();
    for section in sections {
        let query = build_query_handle(
            runtime,
            &QueryItemsPayload {
                query: None,
                sort: Some(section.sort.to_string()),
                section: Some(section.section.to_string()),
                page: Some(1),
                creator_id: None,
                active_tag: None,
                active_tag_name: None,
            },
            section.days,
        )?;
        let (items, section_warnings) = run_query(runtime, query)?;
        warnings.extend(section_warnings);
        mapped_sections.push(HomeSection {
            id: section.id.to_string(),
            title_key: section.title_key.to_string(),
            subtitle_key: section.subtitle_key.to_string(),
            icon: section.icon.to_string(),
            items,
            browse_params: BrowseParams {
                sort: section.sort.to_string(),
                section: section.section.to_string(),
                days: section.days,
            },
        });
    }

    Ok(json!({
        "source": "steam-sdk",
        "sections": mapped_sections,
        "warnings": dedupe_warnings(warnings),
    }))
}

fn query_items(runtime: &mut BridgeRuntime, payload: QueryItemsPayload) -> Result<Value, String> {
    let query = build_query_handle(runtime, &payload, None)?;
    let (mut items, warnings) = run_query(runtime, query)?;
    normalize_query_items(&mut items, payload.sort.as_deref());
    Ok(json!({
        "source": "steam-sdk",
        "items": items,
        "warnings": dedupe_warnings(warnings),
    }))
}

fn query_item(runtime: &mut BridgeRuntime, workshop_id: &str) -> Result<Value, String> {
    let (items, warnings) = query_details_internal(runtime, &[workshop_id.to_string()])?;
    let item = items
        .first()
        .cloned()
        .ok_or_else(|| "Workshop item not found".to_string())?;

    Ok(json!({
        "source": "steam-sdk",
        "item": item,
        "warnings": dedupe_warnings(warnings),
    }))
}

fn query_collection(runtime: &mut BridgeRuntime, workshop_id: &str) -> Result<Value, String> {
    let ugc = runtime.client.ugc();
    let mut query = ugc
        .query_item(parse_published_file_id(workshop_id)?)
        .map_err(|e| e.to_string())?;
    query = query.include_children(true).include_long_desc(true);
    let (collection, children, mut warnings) = run_query_with_children(runtime, query)?;
    let collection = collection
        .into_iter()
        .next()
        .ok_or_else(|| "Workshop collection not found".to_string())?;
    let items = if children.is_empty() {
        Vec::new()
    } else {
        let (items, child_warnings) = query_details_internal(runtime, &children)?;
        warnings.extend(child_warnings);
        items
    };

    Ok(json!({
        "source": "steam-sdk",
        "collection": collection,
        "items": items,
        "warnings": dedupe_warnings(warnings),
    }))
}

fn query_details(runtime: &mut BridgeRuntime, workshop_ids: &[String]) -> Result<Value, String> {
    Ok(Value::Array(
        query_details_internal(runtime, workshop_ids)?.0,
    ))
}

fn query_details_internal(
    runtime: &mut BridgeRuntime,
    workshop_ids: &[String],
) -> Result<(Vec<Value>, Vec<String>), String> {
    let ids = workshop_ids
        .iter()
        .map(|id| parse_published_file_id(id))
        .collect::<Result<Vec<_>, _>>()?;
    let ugc = runtime.client.ugc();
    let mut query = ugc.query_items(ids).map_err(|e| e.to_string())?;
    query = query
        .include_children(true)
        .include_long_desc(true)
        .include_metadata(true);
    let (items, _, warnings) = run_query_with_children(runtime, query)?;
    Ok((items, warnings))
}

fn request_download(runtime: &mut BridgeRuntime, workshop_id: &str) -> Result<Value, String> {
    let ugc = runtime.client.ugc();
    let published_file_id = parse_published_file_id(workshop_id)?;
    let state = ugc.item_state(published_file_id);
    if !state.contains(ItemState::SUBSCRIBED) {
        let (tx, rx) = mpsc::channel();
        ugc.subscribe_item(published_file_id, move |result| {
            let _ = tx.send(result.map(|_| ()));
        });
        let started_at = Instant::now();
        loop {
            runtime.client.run_callbacks();
            match rx.try_recv() {
                Ok(Ok(())) => break,
                Ok(Err(err)) => return Err(format!("SubscribeItem failed: {}", err)),
                Err(mpsc::TryRecvError::Empty) => {
                    if started_at.elapsed() > QUERY_TIMEOUT {
                        return Err("SubscribeItem timed out".to_string());
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("SubscribeItem callback channel disconnected".to_string())
                }
            }
        }
    }

    let accepted = ugc.download_item(published_file_id, true);
    Ok(json!({
        "accepted": accepted,
        "message": if accepted { "Steam download requested" } else { "Steam download request was rejected" },
    }))
}

fn get_download_status(runtime: &mut BridgeRuntime, workshop_id: &str) -> Result<Value, String> {
    let ugc = runtime.client.ugc();
    let published_file_id = parse_published_file_id(workshop_id)?;
    let state = ugc.item_state(published_file_id);
    let install_info = ugc.item_install_info(published_file_id);
    let download_info = ugc.item_download_info(published_file_id);

    Ok(json!({
        "subscribed": state.contains(ItemState::SUBSCRIBED),
        "installed": state.contains(ItemState::INSTALLED),
        "downloaded": download_info.map(|(downloaded, _)| downloaded),
        "total": download_info.map(|(_, total)| total),
        "installFolder": install_info.as_ref().map(|info| info.folder.clone()),
        "itemState": item_state_strings(state),
    }))
}

fn get_subscribed_items(runtime: &mut BridgeRuntime) -> Result<Value, String> {
    let ugc = runtime.client.ugc();
    let mut items = Vec::new();

    for published_file_id in ugc.subscribed_items(true) {
        let state = ugc.item_state(published_file_id);
        let install_info = ugc.item_install_info(published_file_id);
        items.push(SubscribedItemPayload {
            workshop_id: published_file_id.0.to_string(),
            item_state: item_state_strings(state),
            install_folder: install_info.as_ref().map(|info| info.folder.clone()),
            size_on_disk: install_info.as_ref().map(|info| info.size_on_disk),
        });
    }

    Ok(json!({
        "source": "steam-sdk",
        "items": items,
    }))
}

fn get_favorited_collections(runtime: &mut BridgeRuntime) -> Result<Value, String> {
    let ugc = runtime.client.ugc();
    let current_user = runtime.client.user().steam_id().account_id();
    let app_ids = AppIDs::Both {
        creator: AppId(APP_ID),
        consumer: AppId(APP_ID),
    };

    let mut page = 1;
    let mut items = Vec::new();
    let mut total_results = None;

    loop {
        let mut query = ugc
            .query_user(
                current_user,
                UserList::Favorited,
                UGCType::Collections,
                UserListOrder::LastUpdatedDesc,
                app_ids,
                page,
            )
            .map_err(|e| e.to_string())?;
        query = query
            .include_long_desc(true)
            .include_children(true)
            .include_metadata(true);

        let execution = run_query_with_children_page(runtime, query)?;
        if execution.items.is_empty() {
            break;
        }

        items.extend(execution.items);
        let expected_total = *total_results.get_or_insert(execution.total_results);
        if expected_total == 0 || items.len() as u32 >= expected_total {
            break;
        }
        page += 1;
    }

    Ok(json!({
        "source": "steam-sdk",
        "items": items,
    }))
}

fn build_query_handle(
    runtime: &mut BridgeRuntime,
    payload: &QueryItemsPayload,
    days_override: Option<u32>,
) -> Result<steamworks::QueryHandle, String> {
    let ugc = runtime.client.ugc();
    let page = payload.page.unwrap_or(1);
    let item_type = map_item_type(payload.section.as_deref());
    let app_ids = AppIDs::Both {
        creator: AppId(APP_ID),
        consumer: AppId(APP_ID),
    };

    let mut query = if let Some(creator_id) = payload.creator_id.as_deref() {
        let account_id = parse_account_id(creator_id)?;
        ugc.query_user(
            account_id,
            UserList::Published,
            item_type,
            map_user_list_order(payload.sort.as_deref()),
            app_ids,
            page,
        )
        .map_err(|e| e.to_string())?
    } else {
        ugc.query_all(
            map_query_type(payload.sort.as_deref()),
            item_type,
            app_ids,
            page,
        )
        .map_err(|e| e.to_string())?
    };

    query = query
        .include_long_desc(true)
        .include_children(true)
        .include_metadata(true);

    if let Some(text) = payload
        .query
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        query = query.set_search_text(text.trim());
    }
    if let Some(tag) = payload
        .active_tag_name
        .as_deref()
        .or(payload.active_tag.as_deref())
        .filter(|value| !value.trim().is_empty())
    {
        query = query.add_required_tag(tag.trim());
    }
    if let Some(days) = days_override {
        query = query.set_ranked_by_trend_days(days);
    }

    Ok(query)
}

fn run_query(
    runtime: &mut BridgeRuntime,
    query: steamworks::QueryHandle,
) -> Result<(Vec<Value>, Vec<String>), String> {
    let execution = run_query_with_children_page(runtime, query)?;
    Ok((execution.items, execution.warnings))
}

fn run_query_with_children(
    runtime: &mut BridgeRuntime,
    query: steamworks::QueryHandle,
) -> Result<(Vec<Value>, Vec<String>, Vec<String>), String> {
    let execution = run_query_with_children_page(runtime, query)?;
    Ok((execution.items, execution.child_ids, execution.warnings))
}

fn run_query_with_children_page(
    runtime: &mut BridgeRuntime,
    query: steamworks::QueryHandle,
) -> Result<QueryExecution, String> {
    let (tx, rx) = mpsc::channel();
    query.fetch(move |result| {
        let payload = result.map_err(|err| err.to_string()).and_then(|results| {
            let mut items = Vec::new();
            let mut child_ids = Vec::new();
            for index in 0..results.returned_results() {
                if let Some(item) = results.get(index) {
                    let children = results
                        .get_children(index)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|child| child.0.to_string())
                        .collect::<Vec<_>>();
                    child_ids.extend(children.iter().cloned());
                    items.push(query_result_to_json(
                        &results,
                        index,
                        &item,
                        results.preview_url(index),
                        &children,
                    ));
                }
            }
            Ok((items, child_ids, results.total_results()))
        });
        let _ = tx.send(payload);
    });

    let started_at = Instant::now();
    loop {
        runtime.client.run_callbacks();
        match rx.try_recv() {
            Ok(result) => {
                let (mut items, child_ids, total_results) = result?;
                let warnings = fill_creator_persona_names(runtime, &mut items);
                return Ok(QueryExecution {
                    items,
                    child_ids,
                    total_results,
                    warnings,
                });
            }
            Err(mpsc::TryRecvError::Empty) => {
                if started_at.elapsed() > QUERY_TIMEOUT {
                    return Err("Steam workshop query timed out".to_string());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err("Steam workshop query callback disconnected".to_string())
            }
        }
    }
}

fn fill_creator_persona_names(runtime: &mut BridgeRuntime, items: &mut [Value]) -> Vec<String> {
    let mut unique_ids = Vec::new();
    let mut seen_ids = HashSet::new();

    for item in items.iter() {
        let Some(steam_id) = item.get("creator_steam_id").and_then(Value::as_str) else {
            continue;
        };
        let trimmed = steam_id.trim();
        if trimmed.is_empty() || !seen_ids.insert(trimmed.to_string()) {
            continue;
        }
        unique_ids.push(trimmed.to_string());
    }

    if unique_ids.is_empty() {
        return Vec::new();
    }

    let friends = runtime.client.friends();
    let mut pending = Vec::new();
    let mut names = HashMap::new();

    for steam_id in &unique_ids {
        let id = match steam_id.parse::<u64>() {
            Ok(raw) => SteamId::from_raw(raw),
            Err(_) => continue,
        };
        let friend = friends.get_friend(id);
        let name = friend.name();
        if is_usable_persona_name(&name) {
            names.insert(steam_id.clone(), name);
            continue;
        }

        let _ = friends.request_user_information(id, true);
        pending.push((steam_id.clone(), id));
    }

    if !pending.is_empty() {
        let started_at = Instant::now();
        while started_at.elapsed() <= PERSONA_LOOKUP_TIMEOUT {
            runtime.client.run_callbacks();
            let mut unresolved = Vec::new();

            for (steam_id, id) in pending.into_iter() {
                let name = friends.get_friend(id).name();
                if is_usable_persona_name(&name) {
                    names.insert(steam_id, name);
                } else {
                    unresolved.push((steam_id, id));
                }
            }

            if unresolved.is_empty() {
                pending = unresolved;
                break;
            }

            pending = unresolved;
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    for item in items.iter_mut() {
        let Some(obj) = item.as_object_mut() else {
            continue;
        };
        let Some(steam_id) = obj.get("creator_steam_id").and_then(Value::as_str) else {
            continue;
        };
        if let Some(name) = names.get(steam_id) {
            obj.insert("creator_name".to_string(), Value::String(name.clone()));
        }
    }

    if pending.is_empty() {
        return Vec::new();
    }

    let sample_ids = pending
        .iter()
        .take(5)
        .map(|(steam_id, _)| steam_id.clone())
        .collect::<Vec<_>>()
        .join(", ");
    vec![format!(
        "Steamworks SDK creator persona lookup failed for {} author(s): {}",
        pending.len(),
        sample_ids
    )]
}

fn is_usable_persona_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty() && trimmed != "[unknown]"
}

fn dedupe_warnings(warnings: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    warnings
        .into_iter()
        .filter(|warning| {
            let trimmed = warning.trim();
            !trimmed.is_empty() && seen.insert(trimmed.to_string())
        })
        .collect()
}

fn query_result_to_json(
    results: &steamworks::QueryResults<'_>,
    index: u32,
    item: &steamworks::QueryResult,
    preview_url: Option<String>,
    child_ids: &[String],
) -> Value {
    let owner_id = item.owner.raw().to_string();
    let account_id = item.owner.account_id().raw().to_string();
    let subscriptions = results.statistic(index, UGCStatisticType::Subscriptions);
    let favorites = results.statistic(index, UGCStatisticType::Favorites);
    let lifetime_subscriptions = results.statistic(index, UGCStatisticType::UniqueSubscriptions);
    let lifetime_favorites = results.statistic(index, UGCStatisticType::UniqueFavorites);
    let views = results.statistic(index, UGCStatisticType::UniqueWebsiteViews);
    let comments = results.statistic(index, UGCStatisticType::Comments);
    let total_votes = item.num_upvotes as u64 + item.num_downvotes as u64;

    json!({
        "publishedfileid": item.published_file_id.0.to_string(),
        "title": item.title,
        "description": item.description,
        "short_description": item.description,
        "preview_url": preview_url.unwrap_or_default(),
        "creator": owner_id,
        "creator_name": "",
        "creator_steam_id": owner_id,
        "creator_account_id": account_id,
        "tags": item.tags.iter().map(|tag| json!({ "tag": tag })).collect::<Vec<_>>(),
        "file_size": item.file_size.to_string(),
        "time_created": item.time_created,
        "time_updated": item.time_updated,
        "num_children": item.num_children,
        "child_item_ids": child_ids,
        "subscriptions": subscriptions,
        "favorited": favorites,
        "favorites": favorites,
        "lifetime_subscriptions": lifetime_subscriptions,
        "lifetime_favorited": lifetime_favorites,
        "lifetime_favorites": lifetime_favorites,
        "views": views,
        "num_comments_public": comments,
        "comments": comments,
        "total_votes": total_votes,
        "score": item.score,
        "file_type": match item.file_type {
            FileType::Collection => "collection",
            _ => "item",
        },
    })
}

fn normalize_query_items(items: &mut [Value], sort: Option<&str>) {
    match sort.unwrap_or("trend") {
        "mostrecent" => items.sort_by(|left, right| {
            json_u64(right, "time_created")
                .cmp(&json_u64(left, "time_created"))
                .then_with(|| json_u64(right, "time_updated").cmp(&json_u64(left, "time_updated")))
                .then_with(|| {
                    json_string(left, "publishedfileid").cmp(json_string(right, "publishedfileid"))
                })
        }),
        "lastupdated" => items.sort_by(|left, right| {
            json_u64(right, "time_updated")
                .cmp(&json_u64(left, "time_updated"))
                .then_with(|| json_u64(right, "time_created").cmp(&json_u64(left, "time_created")))
                .then_with(|| {
                    json_string(left, "publishedfileid").cmp(json_string(right, "publishedfileid"))
                })
        }),
        "totalprofiles" => items.sort_by(|left, right| {
            json_u64(right, "lifetime_subscriptions")
                .cmp(&json_u64(left, "lifetime_subscriptions"))
                .then_with(|| {
                    json_u64(right, "subscriptions").cmp(&json_u64(left, "subscriptions"))
                })
                .then_with(|| json_u64(right, "time_updated").cmp(&json_u64(left, "time_updated")))
                .then_with(|| {
                    json_string(left, "publishedfileid").cmp(json_string(right, "publishedfileid"))
                })
        }),
        "toprated" => items.sort_by(|left, right| {
            json_f64(right, "score")
                .total_cmp(&json_f64(left, "score"))
                .then_with(|| json_u64(right, "total_votes").cmp(&json_u64(left, "total_votes")))
                .then_with(|| json_u64(right, "time_updated").cmp(&json_u64(left, "time_updated")))
                .then_with(|| {
                    json_string(left, "publishedfileid").cmp(json_string(right, "publishedfileid"))
                })
        }),
        _ => {}
    }
}

fn json_u64(value: &Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(Value::as_u64)
        .or_else(|| value.get(key).and_then(Value::as_str)?.parse::<u64>().ok())
        .unwrap_or(0)
}

fn json_f64(value: &Value, key: &str) -> f64 {
    value
        .get(key)
        .and_then(Value::as_f64)
        .or_else(|| value.get(key).and_then(Value::as_str)?.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn json_string<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn parse_published_file_id(workshop_id: &str) -> Result<PublishedFileId, String> {
    workshop_id
        .trim()
        .parse::<u64>()
        .map(PublishedFileId)
        .map_err(|_| format!("Invalid workshop id: {}", workshop_id))
}

fn parse_account_id(creator_id: &str) -> Result<AccountId, String> {
    let raw = creator_id
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("Creator id is not numeric: {}", creator_id))?;
    if raw > u32::MAX as u64 {
        Ok(SteamId::from_raw(raw).account_id())
    } else {
        Ok(AccountId::from_raw(raw as u32))
    }
}

fn map_item_type(section: Option<&str>) -> UGCType {
    match section.unwrap_or("readytouseitems") {
        "collections" => UGCType::Collections,
        _ => UGCType::ItemsReadyToUse,
    }
}

fn map_query_type(sort: Option<&str>) -> UGCQueryType {
    match sort.unwrap_or("trend") {
        "textsearch" => UGCQueryType::RankedByTextSearch,
        "totalprofiles" => UGCQueryType::RankedByTotalUniqueSubscriptions,
        "mostrecent" => UGCQueryType::RankedByPublicationDate,
        "toprated" => UGCQueryType::RankedByVote,
        "lastupdated" => UGCQueryType::RankedByLastUpdatedDate,
        _ => UGCQueryType::RankedByTrend,
    }
}

fn map_user_list_order(sort: Option<&str>) -> UserListOrder {
    match sort.unwrap_or("lastupdated") {
        "mostrecent" => UserListOrder::CreationOrderDesc,
        "lastupdated" => UserListOrder::LastUpdatedDesc,
        _ => UserListOrder::LastUpdatedDesc,
    }
}

fn item_state_strings(state: ItemState) -> Vec<&'static str> {
    let mut values = Vec::new();
    if state.contains(ItemState::SUBSCRIBED) {
        values.push("subscribed");
    }
    if state.contains(ItemState::LEGACY_ITEM) {
        values.push("legacy");
    }
    if state.contains(ItemState::INSTALLED) {
        values.push("installed");
    }
    if state.contains(ItemState::NEEDS_UPDATE) {
        values.push("needs-update");
    }
    if state.contains(ItemState::DOWNLOADING) {
        values.push("downloading");
    }
    if state.contains(ItemState::DOWNLOAD_PENDING) {
        values.push("download-pending");
    }
    values
}

fn into_c_string<T: Serialize>(value: &T) -> *mut c_char {
    let encoded = serde_json::to_string(value)
        .unwrap_or_else(|err| json!({ "ok": false, "error": err.to_string() }).to_string());
    CString::new(encoded).unwrap().into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_mostrecent_sorts_newest_first() {
        let mut items = vec![
            json!({ "publishedfileid": "1", "time_created": 10_u64, "time_updated": 50_u64 }),
            json!({ "publishedfileid": "2", "time_created": 30_u64, "time_updated": 20_u64 }),
            json!({ "publishedfileid": "3", "time_created": 20_u64, "time_updated": 60_u64 }),
        ];

        normalize_query_items(&mut items, Some("mostrecent"));

        assert_eq!(json_string(&items[0], "publishedfileid"), "2");
        assert_eq!(json_string(&items[1], "publishedfileid"), "3");
        assert_eq!(json_string(&items[2], "publishedfileid"), "1");
    }

    #[test]
    fn normalize_lastupdated_sorts_latest_update_first() {
        let mut items = vec![
            json!({ "publishedfileid": "1", "time_created": 10_u64, "time_updated": 50_u64 }),
            json!({ "publishedfileid": "2", "time_created": 30_u64, "time_updated": 20_u64 }),
            json!({ "publishedfileid": "3", "time_created": 20_u64, "time_updated": 60_u64 }),
        ];

        normalize_query_items(&mut items, Some("lastupdated"));

        assert_eq!(json_string(&items[0], "publishedfileid"), "3");
        assert_eq!(json_string(&items[1], "publishedfileid"), "1");
        assert_eq!(json_string(&items[2], "publishedfileid"), "2");
    }

    #[test]
    fn normalize_totalprofiles_prefers_subscription_counts() {
        let mut items = vec![
            json!({ "publishedfileid": "1", "subscriptions": 200_u64, "lifetime_subscriptions": 500_u64, "time_updated": 10_u64 }),
            json!({ "publishedfileid": "2", "subscriptions": 100_u64, "lifetime_subscriptions": 900_u64, "time_updated": 20_u64 }),
            json!({ "publishedfileid": "3", "subscriptions": 300_u64, "lifetime_subscriptions": 700_u64, "time_updated": 30_u64 }),
        ];

        normalize_query_items(&mut items, Some("totalprofiles"));

        assert_eq!(json_string(&items[0], "publishedfileid"), "2");
        assert_eq!(json_string(&items[1], "publishedfileid"), "3");
        assert_eq!(json_string(&items[2], "publishedfileid"), "1");
    }

    #[test]
    fn normalize_toprated_prefers_score_then_votes() {
        let mut items = vec![
            json!({ "publishedfileid": "1", "score": 0.90_f64, "total_votes": 100_u64, "time_updated": 10_u64 }),
            json!({ "publishedfileid": "2", "score": 0.95_f64, "total_votes": 80_u64, "time_updated": 20_u64 }),
            json!({ "publishedfileid": "3", "score": 0.95_f64, "total_votes": 120_u64, "time_updated": 30_u64 }),
        ];

        normalize_query_items(&mut items, Some("toprated"));

        assert_eq!(json_string(&items[0], "publishedfileid"), "3");
        assert_eq!(json_string(&items[1], "publishedfileid"), "2");
        assert_eq!(json_string(&items[2], "publishedfileid"), "1");
    }
}
