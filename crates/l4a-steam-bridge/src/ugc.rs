use crate::convert::{
    map_item_type, map_query_type, map_user_list_order, parse_account_id, query_result_to_json,
};
use crate::types::{BridgeRuntime, QueryExecution, QueryItemsPayload, QueryItemsWithChildren};
use crate::{APP_ID, PERSONA_LOOKUP_TIMEOUT, QUERY_TIMEOUT};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use steamworks::{AppIDs, AppId, SteamId, UserList};

pub(crate) fn build_query_handle(
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

pub(crate) fn run_query(
    runtime: &mut BridgeRuntime,
    query: steamworks::QueryHandle,
) -> Result<(Vec<Value>, Vec<String>), String> {
    let execution = run_query_with_children_page(runtime, query)?;
    Ok((execution.items, execution.warnings))
}

pub(crate) fn run_query_with_children(
    runtime: &mut BridgeRuntime,
    query: steamworks::QueryHandle,
) -> Result<QueryItemsWithChildren, String> {
    let execution = run_query_with_children_page(runtime, query)?;
    Ok((execution.items, execution.child_ids, execution.warnings))
}

pub(crate) fn run_query_with_children_page(
    runtime: &mut BridgeRuntime,
    query: steamworks::QueryHandle,
) -> Result<QueryExecution, String> {
    let (tx, rx) = mpsc::channel();
    query.fetch(move |result| {
        let payload = result.map_err(|err| err.to_string()).map(|results| {
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
            (items, child_ids, results.total_results())
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

pub(crate) fn is_usable_persona_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty()
        && !trimmed.eq_ignore_ascii_case("[unknown]")
        && !trimmed.eq_ignore_ascii_case("AUTHOR_NAME")
}

pub(crate) fn dedupe_warnings(warnings: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    warnings
        .into_iter()
        .filter(|warning| {
            let trimmed = warning.trim();
            !trimmed.is_empty() && seen.insert(trimmed.to_string())
        })
        .collect()
}
