use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use steamworks::{
    AccountId, AppIDs, AppId, Client, FileType, ItemState, PublishedFileId, SteamId, UGCQueryType,
    UGCType, UserList, UserListOrder,
};

const APP_ID: u32 = 550;
const BRIDGE_VERSION: &str = "0.1.0";
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

static RUNTIME: OnceLock<Mutex<Option<BridgeRuntime>>> = OnceLock::new();

struct BridgeRuntime {
    client: Client,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InitResponse {
    ok: bool,
    version: String,
    current_user_steam_id: String,
    current_user_account_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorResponse {
    ok: bool,
    error: String,
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

#[no_mangle]
pub extern "C" fn l4a_steam_bridge_init() -> *mut c_char {
    match ensure_runtime() {
        Ok(runtime) => {
            let user = runtime
                .as_ref()
                .expect("runtime should be initialized")
                .client
                .user();
            let steam_id = user.steam_id();
            into_c_string(&InitResponse {
                ok: true,
                version: BRIDGE_VERSION.to_string(),
                current_user_steam_id: steam_id.raw().to_string(),
                current_user_account_id: steam_id.account_id().raw().to_string(),
            })
        }
        Err(err) => into_c_string(&ErrorResponse {
            ok: false,
            error: err,
        }),
    }
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
        let items = run_query(runtime, query)?;
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
    }))
}

fn query_items(runtime: &mut BridgeRuntime, payload: QueryItemsPayload) -> Result<Value, String> {
    let query = build_query_handle(runtime, &payload, None)?;
    let items = run_query(runtime, query)?;
    Ok(json!({
        "source": "steam-sdk",
        "items": items,
    }))
}

fn query_item(runtime: &mut BridgeRuntime, workshop_id: &str) -> Result<Value, String> {
    let items = query_details(runtime, &[workshop_id.to_string()])?;
    let item = items
        .as_array()
        .and_then(|items| items.first())
        .cloned()
        .ok_or_else(|| "Workshop item not found".to_string())?;

    Ok(json!({
        "source": "steam-sdk",
        "item": item,
    }))
}

fn query_collection(runtime: &mut BridgeRuntime, workshop_id: &str) -> Result<Value, String> {
    let ugc = runtime.client.ugc();
    let mut query = ugc
        .query_item(parse_published_file_id(workshop_id)?)
        .map_err(|e| e.to_string())?;
    query = query.include_children(true).include_long_desc(true);
    let (collection, children) = run_query_with_children(runtime, query)?;
    let collection = collection
        .into_iter()
        .next()
        .ok_or_else(|| "Workshop collection not found".to_string())?;
    let items = if children.is_empty() {
        Vec::new()
    } else {
        query_details_internal(runtime, &children)?
    };

    Ok(json!({
        "source": "steam-sdk",
        "collection": collection,
        "items": items,
    }))
}

fn query_details(runtime: &mut BridgeRuntime, workshop_ids: &[String]) -> Result<Value, String> {
    Ok(Value::Array(query_details_internal(runtime, workshop_ids)?))
}

fn query_details_internal(
    runtime: &mut BridgeRuntime,
    workshop_ids: &[String],
) -> Result<Vec<Value>, String> {
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
    let (items, _) = run_query_with_children(runtime, query)?;
    Ok(items)
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
) -> Result<Vec<Value>, String> {
    let (items, _) = run_query_with_children(runtime, query)?;
    Ok(items)
}

fn run_query_with_children(
    runtime: &mut BridgeRuntime,
    query: steamworks::QueryHandle,
) -> Result<(Vec<Value>, Vec<String>), String> {
    let (tx, rx) = mpsc::channel();
    query.fetch(move |result| {
        let payload = result.map_err(|err| err.to_string()).and_then(|results| {
            let mut items = Vec::new();
            let mut child_ids = Vec::new();
            for index in 0..results.returned_results() {
                if let Some(item) = results.get(index) {
                    if let Some(children) = results.get_children(index) {
                        child_ids.extend(children.into_iter().map(|child| child.0.to_string()));
                    }
                    items.push(query_result_to_json(&item, results.preview_url(index)));
                }
            }
            Ok((items, child_ids))
        });
        let _ = tx.send(payload);
    });

    let started_at = Instant::now();
    loop {
        runtime.client.run_callbacks();
        match rx.try_recv() {
            Ok(result) => return result,
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

fn query_result_to_json(item: &steamworks::QueryResult, preview_url: Option<String>) -> Value {
    let owner_id = item.owner.raw().to_string();
    let account_id = item.owner.account_id().raw().to_string();

    json!({
        "publishedfileid": item.published_file_id.0.to_string(),
        "title": item.title,
        "description": item.description,
        "short_description": item.description,
        "preview_url": preview_url.unwrap_or_default(),
        "creator": account_id,
        "creator_name": owner_id,
        "creator_steam_id": owner_id,
        "creator_account_id": account_id,
        "tags": item.tags.iter().map(|tag| json!({ "tag": tag })).collect::<Vec<_>>(),
        "file_size": item.file_size.to_string(),
        "time_created": item.time_created,
        "time_updated": item.time_updated,
        "num_children": item.num_children,
        "subscriptions": item.num_upvotes as u64 + item.num_downvotes as u64,
        "favorited": item.num_upvotes,
        "score": item.score,
        "file_type": match item.file_type {
            FileType::Collection => "collection",
            _ => "item",
        },
    })
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
