use super::{
    SteamSearchRequest, SteamSearchTarget, WorkshopObservation, PROTO_FIELD_SEARCH_TEXT_TARGET,
    WORKSHOP_FILE_TYPE_COLLECTION,
};
use chrono::{DateTime, Utc};
use l4a_protocol::{
    MetadataSource, WorkshopDependency, WorkshopFetchOutcome, WorkshopFileType, WorkshopId,
    WorkshopItemRecord, WorkshopItemStatus, WorkshopPreview,
};
use steam_vent::EResult;
use steam_vent_proto_steam::steammessages_publishedfile_steamclient::{
    CPublishedFile_QueryFiles_Request, PublishedFileDetails,
};

pub(super) fn map_detail(
    d: &PublishedFileDetails,
    creator_name: Option<String>,
    observed_at: DateTime<Utc>,
) -> Option<WorkshopObservation> {
    let workshop_id = WorkshopId::new(d.publishedfileid?).ok()?;
    let source = MetadataSource::SteamKit;
    let result = d
        .result
        .and_then(|code| EResult::try_from(code as i32).ok());
    let outcome = match result {
        Some(EResult::OK) if d.banned.unwrap_or(false) => WorkshopFetchOutcome::Removed {
            workshop_id,
            source,
            observed_at,
            reason: d
                .ban_reason
                .clone()
                .or_else(|| Some("Banned on Steam Workshop".into())),
        },
        Some(EResult::OK) if can_resolve_by_id(d) => WorkshopFetchOutcome::Found {
            item: details_to_record(workshop_id, d, creator_name, observed_at),
        },
        Some(EResult::FileNotFound) => WorkshopFetchOutcome::Removed {
            workshop_id,
            source,
            observed_at,
            reason: Some("Steam CM result=9 (FileNotFound)".into()),
        },
        Some(EResult::OK | EResult::AccessDenied) => WorkshopFetchOutcome::PrivateOrUnavailable {
            workshop_id,
            source,
            observed_at,
            reason: Some(if matches!(result, Some(EResult::OK)) {
                "Steam CM visibility is restricted".into()
            } else {
                "Steam CM result=15 (AccessDenied)".into()
            }),
        },
        _ => {
            tracing::warn!(%workshop_id, result = ?d.result, "Steam CM 未返回可核验的条目状态");
            return None;
        }
    };
    Some(WorkshopObservation {
        outcome,
        listed: d.visibility() == 0,
        consumer_appid: d.consumer_appid.filter(|id| *id > 0),
    })
}

pub fn build_search_files_proto(
    app_id: u32,
    request: &SteamSearchRequest,
) -> CPublishedFile_QueryFiles_Request {
    let mut proto = CPublishedFile_QueryFiles_Request {
        query_type: Some(request.query_type),
        page: Some(request.page.max(1)),
        numperpage: Some(request.num_per_page.clamp(1, 100)),
        appid: Some(app_id),
        // Never restrict creator_appid to 550: L4D2 Authoring Tools maps use creator_appid=563.
        creator_appid: None,
        requiredtags: request.required_tags.clone(),
        match_all_tags: Some(true),
        search_text: request.search_text.clone(),
        filetype: Some(request.file_type),
        days: request.days,
        cache_max_age_seconds: Some(0),
        language: Some(request.language),
        return_vote_data: Some(true),
        return_tags: Some(true),
        return_previews: Some(true),
        return_children: Some(true),
        return_short_description: Some(false),
        return_details: Some(true),
        ..Default::default()
    };
    if let Some(target) = request
        .search_text_target
        .filter(|t| *t != SteamSearchTarget::All)
    {
        proto
            .special_fields
            .mut_unknown_fields()
            .add_varint(PROTO_FIELD_SEARCH_TEXT_TARGET, target as u64);
    }
    proto
}

pub(super) fn can_resolve_by_id(d: &PublishedFileDetails) -> bool {
    matches!(d.visibility(), 0 | 3)
}

pub(super) fn details_to_record(
    wid: WorkshopId,
    d: &PublishedFileDetails,
    creator_name: Option<String>,
    fetched_at: DateTime<Utc>,
) -> WorkshopItemRecord {
    let is_collection = d.file_type == Some(WORKSHOP_FILE_TYPE_COLLECTION);
    let file_type = if is_collection {
        WorkshopFileType::Collection
    } else {
        WorkshopFileType::Item
    };

    let mut dependencies = Vec::new();
    let mut collection_children = Vec::new();
    for child in &d.children {
        if let Some(cid) = child
            .publishedfileid
            .and_then(|raw| WorkshopId::new(raw).ok())
        {
            if is_collection {
                collection_children.push(cid);
            } else {
                dependencies.push(WorkshopDependency {
                    workshop_id: cid,
                    title: None,
                    required: true,
                });
            }
        }
    }

    let tags: Vec<String> = d
        .tags
        .iter()
        .filter_map(|t| {
            t.tag
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    t.display_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                })
                .map(ToString::to_string)
        })
        .collect();

    let previews: Vec<WorkshopPreview> = d
        .previews
        .iter()
        .map(|p| WorkshopPreview {
            preview_id: p.previewid.filter(|id| *id > 0).map(|id| id.to_string()),
            url: p
                .url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string),
            youtube_video_id: p
                .youtubevideoid
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string),
            preview_type: p.preview_type.unwrap_or(0),
        })
        .collect();

    let title = d
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("Workshop #{wid}"));

    let description = d
        .file_description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            d.short_description
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .map(ToString::to_string);

    let preview_url = d
        .preview_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let file_url = d
        .file_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    WorkshopItemRecord {
        workshop_id: wid,
        file_type,
        status: WorkshopItemStatus::Active,
        title,
        description,
        creator_steam_id: d.creator.filter(|c| *c > 0).map(|c| c.to_string()),
        creator_name,
        preview_url,
        previews,
        file_url,
        file_size: d.file_size.filter(|s| *s > 0),
        time_created: d.time_created.filter(|t| *t > 0).map(i64::from),
        time_updated: d.time_updated.filter(|t| *t > 0).map(i64::from),
        tags,
        dependencies,
        collection_children,
        subscriptions: d.subscriptions.map(u64::from),
        favorites: d.favorited.map(u64::from),
        source: MetadataSource::SteamKit,
        fetched_at,
    }
}
