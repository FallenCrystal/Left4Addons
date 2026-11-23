use serde_json::{json, Value};
use steamworks::{
    AccountId, FileType, ItemState, PublishedFileId, SteamId, UGCQueryType, UGCStatisticType,
    UGCType, UserListOrder,
};

pub(crate) fn query_result_to_json(
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

pub(crate) fn normalize_query_items(items: &mut [Value], sort: Option<&str>) {
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

pub(crate) fn parse_published_file_id(workshop_id: &str) -> Result<PublishedFileId, String> {
    workshop_id
        .trim()
        .parse::<u64>()
        .map(PublishedFileId)
        .map_err(|_| format!("Invalid workshop id: {}", workshop_id))
}

pub(crate) fn parse_account_id(creator_id: &str) -> Result<AccountId, String> {
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

pub(crate) fn map_item_type(section: Option<&str>) -> UGCType {
    match section.unwrap_or("readytouseitems") {
        "collections" => UGCType::Collections,
        _ => UGCType::ItemsReadyToUse,
    }
}

pub(crate) fn map_query_type(sort: Option<&str>) -> UGCQueryType {
    match sort.unwrap_or("trend") {
        "textsearch" => UGCQueryType::RankedByTextSearch,
        "totalprofiles" => UGCQueryType::RankedByTotalUniqueSubscriptions,
        "mostrecent" => UGCQueryType::RankedByPublicationDate,
        "toprated" => UGCQueryType::RankedByVote,
        "lastupdated" => UGCQueryType::RankedByLastUpdatedDate,
        _ => UGCQueryType::RankedByTrend,
    }
}

pub(crate) fn map_user_list_order(sort: Option<&str>) -> UserListOrder {
    match sort.unwrap_or("lastupdated") {
        "mostrecent" => UserListOrder::CreationOrderDesc,
        "lastupdated" => UserListOrder::LastUpdatedDesc,
        _ => UserListOrder::LastUpdatedDesc,
    }
}

pub(crate) fn item_state_strings(state: ItemState) -> Vec<&'static str> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ugc::{dedupe_warnings, is_usable_persona_name};

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

    #[test]
    fn parse_ids_trims_valid_input_and_rejects_invalid_values() {
        assert_eq!(parse_published_file_id(" 123 ").unwrap().0, 123);
        assert!(parse_published_file_id("not-an-id").is_err());
        assert!(parse_account_id("not-an-id").is_err());
    }

    #[test]
    fn warning_deduplication_trims_for_comparison_and_keeps_first_value() {
        assert_eq!(
            dedupe_warnings(vec![
                "warning".to_string(),
                "  ".to_string(),
                " warning ".to_string(),
                "second warning".to_string(),
            ]),
            vec!["warning", "second warning"]
        );
    }

    #[test]
    fn persona_names_reject_placeholders_and_accept_real_names() {
        assert!(!is_usable_persona_name(""));
        assert!(!is_usable_persona_name(" [unknown] "));
        assert!(!is_usable_persona_name("AUTHOR_NAME"));
        assert!(is_usable_persona_name("Bill"));
    }
}
