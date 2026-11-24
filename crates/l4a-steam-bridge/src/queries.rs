use crate::convert::{item_state_strings, normalize_query_items, parse_published_file_id};
use crate::types::{
    BridgeRuntime, BrowseParams, HomeSection, HomeSectionDef, QueryItemsPayload,
    SubscribedItemPayload,
};
use crate::ugc::{
    build_query_handle, dedupe_warnings, run_query, run_query_with_children,
    run_query_with_children_page,
};
use crate::{APP_ID, QUERY_TIMEOUT};
use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use steamworks::{AppIDs, AppId, ItemState, UGCType, UserList, UserListOrder};

pub(crate) fn query_home(runtime: &mut BridgeRuntime) -> Result<Value, String> {
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

pub(crate) fn query_items(
    runtime: &mut BridgeRuntime,
    payload: QueryItemsPayload,
) -> Result<Value, String> {
    let query = build_query_handle(runtime, &payload, None)?;
    let (mut items, warnings) = run_query(runtime, query)?;
    normalize_query_items(&mut items, payload.sort.as_deref());
    Ok(json!({
        "source": "steam-sdk",
        "items": items,
        "warnings": dedupe_warnings(warnings),
    }))
}

pub(crate) fn query_item(runtime: &mut BridgeRuntime, workshop_id: &str) -> Result<Value, String> {
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

pub(crate) fn query_collection(
    runtime: &mut BridgeRuntime,
    workshop_id: &str,
) -> Result<Value, String> {
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

pub(crate) fn query_details(
    runtime: &mut BridgeRuntime,
    workshop_ids: &[String],
) -> Result<Value, String> {
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

pub(crate) fn request_download(
    runtime: &mut BridgeRuntime,
    workshop_id: &str,
) -> Result<Value, String> {
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

pub(crate) fn get_download_status(
    runtime: &mut BridgeRuntime,
    workshop_id: &str,
) -> Result<Value, String> {
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

pub(crate) fn get_subscribed_items(runtime: &mut BridgeRuntime) -> Result<Value, String> {
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

pub(crate) fn get_favorited_collections(runtime: &mut BridgeRuntime) -> Result<Value, String> {
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
