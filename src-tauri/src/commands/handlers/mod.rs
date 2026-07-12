use crate::steam::{
    fetch_collection_children_web, fetch_steam_details_web, BridgeDownloadStatus,
    WorkshopBrowseQuery, WorkshopCapabilities, WorkshopCollectionResponse, WorkshopHomeResponse,
    WorkshopItemResponse, WorkshopItemsResponse, WorkshopService,
};
use crate::vpk::{extract_addon_metadata, generate_dummy_vpk};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use super::types::{
    is_dummy_addon_info, Addon, Database, Group, KnownAddonEntry, MasterCollection, RenameItem,
    Settings, SettingsStore, WorkshopSeenItem, WorkshopSourceSettings,
};

pub mod addons;
pub mod cache;
pub mod db;
pub mod settings;
pub mod tasks;
pub mod workshop;
pub use addons::*;
pub use cache::*;
pub use db::*;
pub use settings::*;
pub use tasks::*;
pub use workshop::*;

const DOWNLOAD_CANCELLED_ERR: &str = "Download cancelled";
const WORKSHOP_HTML_FETCH_INTERVAL: Duration = Duration::from_secs(6);
const WORKSHOP_HTML_FETCH_PAUSE_DURATION: Duration = Duration::from_secs(10 * 60);
const DEFAULT_DOWNLOAD_CONCURRENCY: u32 = 2;
const DOWNLOAD_FINALIZE_SUPPRESS_MS: u64 = 30_000;

#[derive(Default)]
struct WorkshopHtmlFetchGate {
    next_allowed_at: Option<Instant>,
    pause_until: Option<Instant>,
    pause_reason: Option<String>,
}

static WORKSHOP_HTML_FETCH_GATE: OnceLock<Mutex<WorkshopHtmlFetchGate>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadResumeMetadata {
    workshop_id: String,
    target_filename: String,
    file_url: String,
    total_size: Option<u64>,
    etag: Option<String>,
    last_modified: Option<String>,
}

#[derive(Debug, Clone)]
struct SourcePolicy {
    allow_steamworks_sdk: bool,
    allow_steam_web_api: bool,
    allow_steam_community_html: bool,
    sdk_html_scope: String,
    source_order: Vec<String>,
}

impl SourcePolicy {
    fn from_settings(settings: &Settings) -> Self {
        let configured = &settings.workshop_source_settings;
        let preset = configured.preset.trim().to_string();
        let disable_sdk = settings.disable_steamworks_sdk;
        let sdk_html_scope = resolve_sdk_html_scope(
            &configured.sdk_html_scope,
            &preset,
            configured.allow_sdk_html_hybrid,
        );

        match preset.as_str() {
            "offline" => Self {
                allow_steamworks_sdk: false,
                allow_steam_web_api: false,
                allow_steam_community_html: false,
                sdk_html_scope: "disabled".to_string(),
                source_order: configured.source_order.clone(),
            },
            "sdk-only" | "sdkOnly" => Self {
                allow_steamworks_sdk: !disable_sdk && configured.allow_steamworks_sdk,
                allow_steam_web_api: false,
                allow_steam_community_html: false,
                sdk_html_scope: "disabled".to_string(),
                source_order: configured.source_order.clone(),
            },
            "hybrid" => Self {
                allow_steamworks_sdk: !disable_sdk && configured.allow_steamworks_sdk,
                allow_steam_web_api: configured.allow_steam_web_api,
                allow_steam_community_html: configured.allow_steam_community_html,
                sdk_html_scope: "all".to_string(),
                source_order: configured.source_order.clone(),
            },
            _ => Self {
                allow_steamworks_sdk: !disable_sdk && configured.allow_steamworks_sdk,
                allow_steam_web_api: configured.allow_steam_web_api,
                allow_steam_community_html: configured.allow_steam_community_html,
                sdk_html_scope,
                source_order: configured.source_order.clone(),
            },
        }
    }

    fn allow_bridge(&self) -> bool {
        self.allow_steamworks_sdk
    }

    fn allow_web_api(&self) -> bool {
        self.allow_steam_web_api
    }

    fn source_order(&self) -> &[String] {
        &self.source_order
    }

    fn allow_html(&self, source: &str, sdk_query_available: bool) -> bool {
        if !self.allow_steam_community_html {
            return false;
        }
        if !sdk_query_available {
            return true;
        }
        if is_manual_detail_workshop_fetch_source(source) {
            return true;
        }
        if is_search_workshop_fetch_source(source) {
            return matches!(
                self.sdk_html_scope.as_str(),
                "search" | "navigation" | "all"
            );
        }
        if is_navigation_workshop_fetch_source(source) {
            return matches!(self.sdk_html_scope.as_str(), "navigation" | "all");
        }
        if is_background_workshop_fetch_source(source) {
            return self.sdk_html_scope == "all";
        }
        false
    }
}

fn resolve_sdk_html_scope(
    configured_scope: &str,
    preset: &str,
    allow_sdk_html_hybrid: bool,
) -> String {
    match configured_scope.trim() {
        "disabled" | "search" | "navigation" | "all" => configured_scope.trim().to_string(),
        _ if preset == "hybrid" || allow_sdk_html_hybrid => "all".to_string(),
        _ => "search".to_string(),
    }
}

fn is_manual_detail_workshop_fetch_source(source: &str) -> bool {
    matches!(
        source,
        "addon-detail" | "workshop-detail" | "dependency-check"
    )
}

fn is_search_workshop_fetch_source(source: &str) -> bool {
    matches!(source, "workshop-search" | "workshop-creator")
}

fn is_navigation_workshop_fetch_source(source: &str) -> bool {
    matches!(source, "workshop-home" | "workshop-browse")
}

fn source_position(source_order: &[String], source: &str, fallback: usize) -> usize {
    source_order
        .iter()
        .position(|item| item == source)
        .unwrap_or(fallback)
}

fn normalize_master_collection_group_refs(db: &mut Database) -> bool {
    let valid_group_ids: HashSet<String> = db.groups.iter().map(|group| group.id.clone()).collect();
    let valid_master_collection_ids: HashSet<String> = db
        .master_collections
        .iter()
        .map(|collection| collection.id.clone())
        .collect();
    let mut changed = false;

    for collection in &mut db.master_collections {
        let before = collection.group_ids.len();
        let mut seen = HashSet::new();
        collection
            .group_ids
            .retain(|group_id| valid_group_ids.contains(group_id) && seen.insert(group_id.clone()));
        changed |= before != collection.group_ids.len();
    }

    for group in &mut db.groups {
        if let Some(master_collection_ids) = &mut group.master_collection_ids {
            let before = master_collection_ids.len();
            let mut seen = HashSet::new();
            master_collection_ids.retain(|collection_id| {
                valid_master_collection_ids.contains(collection_id)
                    && seen.insert(collection_id.clone())
            });
            changed |= before != master_collection_ids.len();
            if master_collection_ids.is_empty() {
                group.master_collection_ids = None;
                changed = true;
            }
        }
    }

    changed
}

fn find_sdk_installed_workshop_file(install_folder: &Path) -> Result<Option<PathBuf>, String> {
    if !install_folder.exists() {
        return Ok(None);
    }

    if install_folder.is_file() {
        let filename = install_folder
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if filename.ends_with(".vpk") || filename.ends_with("_legacy.bin") {
            return Ok(Some(install_folder.to_path_buf()));
        }
        return Ok(None);
    }

    if !install_folder.is_dir() {
        return Ok(None);
    }

    let mut vpk_candidates = Vec::new();
    let mut legacy_candidates = Vec::new();
    let mut file_candidates = Vec::new();

    for entry in fs::read_dir(install_folder).map_err(|e| {
        format!(
            "Failed to read SDK install folder {}: {}",
            install_folder.display(),
            e
        )
    })? {
        let entry =
            entry.map_err(|e| format!("Failed to inspect SDK install folder entry: {}", e))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to read SDK install folder entry type: {}", e))?;
        if !file_type.is_file() {
            continue;
        }

        let path = entry.path();
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if filename.ends_with(".vpk") {
            vpk_candidates.push(path);
        } else if filename.ends_with("_legacy.bin") {
            legacy_candidates.push(path);
        } else {
            file_candidates.push(path);
        }
    }

    vpk_candidates.sort();
    legacy_candidates.sort();
    file_candidates.sort();

    Ok(vpk_candidates
        .into_iter()
        .next()
        .or_else(|| legacy_candidates.into_iter().next())
        .or_else(|| {
            if file_candidates.len() == 1 {
                file_candidates.into_iter().next()
            } else {
                None
            }
        }))
}

fn import_sdk_workshop_file(
    source_path: &Path,
    workshop_id: &str,
    workshop_dir: &Path,
) -> Result<PathBuf, String> {
    if !workshop_dir.exists() {
        fs::create_dir_all(workshop_dir).map_err(|e| {
            format!(
                "Failed to create workshop directory {}: {}",
                workshop_dir.display(),
                e
            )
        })?;
    }

    let dest_path = workshop_dir.join(format!("{}.vpk", workshop_id));
    let temp_dest_path = workshop_dir.join(format!("{}.vpk.download", workshop_id));

    remove_dummy_workshop_targets(&dest_path)?;

    if temp_dest_path.exists() {
        fs::remove_file(&temp_dest_path).map_err(|e| {
            format!(
                "Failed to remove stale SDK import temp file {}: {}",
                temp_dest_path.display(),
                e
            )
        })?;
    }

    fs::copy(source_path, &temp_dest_path).map_err(|e| {
        format!(
            "Failed to copy SDK workshop file from {} to {}: {}",
            source_path.display(),
            temp_dest_path.display(),
            e
        )
    })?;
    move_or_copy_file(&temp_dest_path, &dest_path).map_err(|e| {
        format!(
            "Failed to finalize SDK workshop file from {} to {}: {}",
            temp_dest_path.display(),
            dest_path.display(),
            e
        )
    })?;

    Ok(dest_path)
}

fn resolve_cache_file(cache_dir: &Path, image_path: &str) -> Result<PathBuf, String> {
    let filename = image_path.strip_prefix("/cache/").unwrap_or(image_path);
    let rel_path = Path::new(filename);

    if filename.is_empty()
        || filename.contains("..")
        || filename.contains('/')
        || filename.contains('\\')
        || rel_path.components().count() != 1
    {
        return Err("Invalid cache image path".to_string());
    }

    let cache_root = fs::canonicalize(cache_dir)
        .map_err(|e| format!("Failed to resolve cache directory: {}", e))?;
    let file_path = fs::canonicalize(cache_root.join(filename))
        .map_err(|e| format!("Failed to resolve cache image: {}", e))?;

    if !file_path.starts_with(&cache_root) {
        return Err("Cache image path escapes cache directory".to_string());
    }

    Ok(file_path)
}

fn validate_open_url(url: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;
    match parsed.scheme() {
        "http" | "https" | "steam" => Ok(parsed),
        scheme => Err(format!("Unsupported URL scheme: {}", scheme)),
    }
}

fn validate_steamcommunity_url(url: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;
    if parsed.scheme() != "https" {
        return Err("Only HTTPS Steam Community URLs are allowed".to_string());
    }

    let host = parsed.host_str().unwrap_or_default();
    if host == "steamcommunity.com" || host.ends_with(".steamcommunity.com") {
        Ok(parsed)
    } else {
        Err("Only steamcommunity.com URLs are allowed".to_string())
    }
}

fn cache_image_extension(content_type: Option<&str>, url: &reqwest::Url) -> &'static str {
    if let Some(content_type) = content_type {
        if content_type.contains("png") {
            return "png";
        }
        if content_type.contains("webp") {
            return "webp";
        }
    }
    if url.path().to_lowercase().ends_with(".png") {
        return "png";
    }
    if url.path().to_lowercase().ends_with(".webp") {
        return "webp";
    }
    "jpg"
}

fn find_cached_remote_image(cache_dir: &Path, hash: &str) -> Option<String> {
    for extension in ["jpg", "png", "webp"] {
        let filename = format!("{}_remote_image.{}", hash, extension);
        if cache_dir.join(&filename).exists() {
            return Some(format!("/cache/{}", filename));
        }
    }
    None
}

fn workshop_html_fetch_gate() -> &'static Mutex<WorkshopHtmlFetchGate> {
    WORKSHOP_HTML_FETCH_GATE.get_or_init(|| Mutex::new(WorkshopHtmlFetchGate::default()))
}

fn reserve_workshop_html_fetch_slot() -> Result<Option<Duration>, String> {
    let now = Instant::now();
    let mut gate = workshop_html_fetch_gate()
        .lock()
        .map_err(|_| "Workshop HTML fetch gate mutex poisoned".to_string())?;

    if let Some(pause_until) = gate.pause_until {
        if pause_until > now {
            return Err(gate.pause_reason.clone().unwrap_or_else(|| {
                "Steam Workshop HTML fetching is temporarily paused".to_string()
            }));
        }
        gate.pause_until = None;
        gate.pause_reason = None;
    }

    let scheduled_at = gate.next_allowed_at.unwrap_or(now).max(now);
    gate.next_allowed_at = Some(scheduled_at + WORKSHOP_HTML_FETCH_INTERVAL);
    let wait = scheduled_at.saturating_duration_since(now);
    if wait.is_zero() {
        Ok(None)
    } else {
        Ok(Some(wait))
    }
}

fn pause_workshop_html_fetches(reason: &str) {
    if let Ok(mut gate) = workshop_html_fetch_gate().lock() {
        let until = Instant::now() + WORKSHOP_HTML_FETCH_PAUSE_DURATION;
        gate.pause_until = Some(until);
        gate.pause_reason = Some(reason.trim().to_string());
        gate.next_allowed_at = Some(until);
    }
}

fn strip_html_tags(input: &str) -> String {
    let tag_re = Regex::new(r"(?is)<[^>]+>").expect("valid tag regex");
    let without_tags = tag_re.replace_all(input, " ");
    without_tags
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_steamcommunity_error_message(html: &str) -> Option<String> {
    let re = Regex::new(
        r#"(?is)<div[^>]*class=["'][^"']*\berror_ctn\b[^"']*["'][^>]*>.*?<div[^>]*id=["'][^"']*bottom[^"']*["'][^>]*>.*?<div[^>]*>\s*<h3[^>]*>(.*?)</h3>"#,
    )
    .expect("valid workshop error regex");
    let captures = re.captures(html)?;
    let message = strip_html_tags(captures.get(1)?.as_str());
    if message.trim().is_empty() {
        None
    } else {
        Some(message)
    }
}

fn is_background_workshop_fetch_source(source: &str) -> bool {
    matches!(source, "startup-auto" | "background-refresh")
}

fn is_known_group_collection_id(groups_path: &Path, workshop_id: &str) -> bool {
    let workshop_id = workshop_id.trim();
    if workshop_id.is_empty() {
        return false;
    }

    load_groups(groups_path)
        .into_iter()
        .filter_map(|group| group.workshop_collection_id)
        .any(|collection_id| collection_id.trim() == workshop_id)
}

fn is_known_workshop_id(known_addons_path: &Path, groups_path: &Path, workshop_id: &str) -> bool {
    let workshop_id = workshop_id.trim();
    if workshop_id.is_empty() {
        return false;
    }

    let known_addons = load_known_addons(known_addons_path);
    if known_addons.contains_key(workshop_id) {
        return true;
    }

    if is_known_group_collection_id(groups_path, workshop_id) {
        return true;
    }

    known_addons
        .values()
        .any(|entry| entry.id == workshop_id || entry.workshop_id.as_deref() == Some(workshop_id))
}

fn ensure_background_workshop_fetch_allowed(
    source: &str,
    workshop_id: &str,
    known_addons_path: &Path,
    groups_path: &Path,
) -> Result<(), String> {
    if !is_background_workshop_fetch_source(source) {
        return Ok(());
    }

    if is_known_workshop_id(known_addons_path, groups_path, workshop_id) {
        return Ok(());
    }

    Err(format!(
        "Background workshop fetch blocked: workshop item {} is not present in known_addons",
        workshop_id
    ))
}

fn clear_persisted_workshop_media_and_content(
    obj: &mut serde_json::Map<String, serde_json::Value>,
) {
    for key in [
        "previewUrl",
        "imagePath",
        "shortDescription",
        "galleryPreviewUrls",
        "description",
        "descriptionHtml",
        "imageGallery",
        "galleryUrls",
        "backgroundImageUrl",
    ] {
        obj.remove(key);
    }
}

fn persist_seen_workshop_item_entry(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    item: &WorkshopSeenItem,
    source: &str,
    now: &str,
    allow_rich_content: bool,
) {
    insert_non_empty_string(obj, "workshopId", &item.workshop_id);
    insert_non_empty_string(obj, "title", &item.title);
    let author_ids = [
        item.author_id.clone(),
        item.author_steam_id.clone().unwrap_or_default(),
        item.author_account_id.clone().unwrap_or_default(),
        item.author_vanity_id.clone().unwrap_or_default(),
    ];
    insert_author_name_if_useful(obj, &item.author_name, &author_ids);
    insert_non_empty_string(obj, "creatorId", &item.author_id);
    insert_non_empty_string(obj, "authorId", &item.author_id);
    insert_non_empty_string(obj, "creatorProfileUrl", &item.author_url);
    insert_non_empty_string(obj, "authorUrl", &item.author_url);
    if let Some(vanity_id) = item.author_vanity_id.as_deref() {
        insert_non_empty_string(obj, "creatorVanityId", vanity_id);
    }
    if let Some(account_id) = item.author_account_id.as_deref() {
        insert_non_empty_string(obj, "creatorAccountId", account_id);
    }
    if let Some(steam_id) = item.author_steam_id.as_deref() {
        insert_non_empty_string(obj, "creatorSteamId", steam_id);
    }
    if item.author_id.chars().all(|c| c.is_ascii_digit()) && !item.author_id.is_empty() {
        insert_non_empty_string(obj, "creatorSteamId", &item.author_id);
    }
    if item.author_url.contains("/profiles/") {
        if let Some(id) = item
            .author_url
            .split("/profiles/")
            .nth(1)
            .and_then(|s| s.split('/').next())
        {
            if id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty() {
                insert_non_empty_string(obj, "creatorSteamId", id);
            }
        }
    }
    insert_optional_value(
        obj,
        "fileSizeDisplay",
        item.file_size.clone().map(serde_json::Value::String),
    );
    insert_optional_value(obj, "tags", item.tags.clone().map(|v| serde_json::json!(v)));
    insert_optional_u64(obj, "subscriptions", item.subscriptions);
    insert_optional_u64(obj, "favorites", item.favorites);
    insert_optional_u64(obj, "lifetimeSubscriptions", item.lifetime_subscriptions);
    insert_optional_u64(obj, "lifetimeFavorites", item.lifetime_favorites);
    insert_optional_u64(obj, "views", item.views);
    insert_optional_u64(obj, "comments", item.comments);
    insert_optional_u64(obj, "totalVotes", item.total_votes);
    insert_optional_u64(obj, "timeCreated", item.time_created);
    insert_optional_u64(obj, "timeUpdated", item.time_updated);
    insert_optional_u64(obj, "childCount", item.child_count);
    insert_optional_u64(obj, "previewCount", item.preview_count);
    insert_optional_vec_string(obj, "childItemIds", item.child_item_ids.clone());
    insert_non_empty_string(obj, "lastSeenSource", source);
    insert_non_empty_string(obj, "lastSeenAt", now);

    if allow_rich_content {
        insert_non_empty_string(obj, "previewUrl", &item.image_path);
        insert_non_empty_string(obj, "imagePath", &item.image_path);
        insert_optional_value(
            obj,
            "shortDescription",
            item.short_description
                .clone()
                .map(serde_json::Value::String),
        );
        insert_optional_vec_string(obj, "galleryPreviewUrls", item.gallery_preview_urls.clone());
    } else {
        clear_persisted_workshop_media_and_content(obj);
    }
}

fn persist_workshop_page_details_entry(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    workshop_id: &str,
    details: &serde_json::Value,
    source: &str,
    now: &str,
    allow_rich_content: bool,
) {
    insert_non_empty_string(obj, "workshopId", workshop_id);
    insert_non_empty_string(obj, "lastPageFetchedAt", now);
    insert_non_empty_string(obj, "lastPageSource", source);
    if let Some(title) = details.get("title").and_then(|v| v.as_str()) {
        insert_non_empty_string(obj, "title", title);
    }
    if let Some(creator_name) = details.get("creatorName").and_then(|v| v.as_str()) {
        let author_ids = [
            details
                .get("creatorSteamId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            details
                .get("creatorAccountId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            details
                .get("creatorVanityId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        ];
        insert_author_name_if_useful(obj, creator_name, &author_ids);
    }
    if let Some(profile_url) = details.get("creatorProfileUrl").and_then(|v| v.as_str()) {
        insert_non_empty_string(obj, "creatorProfileUrl", profile_url);
        insert_non_empty_string(obj, "authorUrl", profile_url);
    }
    if let Some(steam_id) = details.get("creatorSteamId").and_then(|v| v.as_str()) {
        insert_non_empty_string(obj, "creatorSteamId", steam_id);
        insert_non_empty_string(obj, "creatorId", steam_id);
    }
    if let Some(vanity_id) = details.get("creatorVanityId").and_then(|v| v.as_str()) {
        insert_non_empty_string(obj, "creatorVanityId", vanity_id);
        if obj
            .get("creatorId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            insert_non_empty_string(obj, "creatorId", vanity_id);
        }
    }
    if let Some(account_id) = details.get("creatorAccountId").and_then(|v| v.as_str()) {
        insert_non_empty_string(obj, "creatorAccountId", account_id);
    }
    if let Some(file_size) = details.get("fileSizeDisplay").and_then(|v| v.as_str()) {
        insert_non_empty_string(obj, "fileSizeDisplay", file_size);
    }
    if let Some(posted_text) = details.get("postedDateText").and_then(|v| v.as_str()) {
        insert_non_empty_string(obj, "postedDateText", posted_text);
    }
    if let Some(updated_text) = details.get("updatedDateText").and_then(|v| v.as_str()) {
        insert_non_empty_string(obj, "updatedDateText", updated_text);
    }
    insert_optional_value(
        obj,
        "changeNoteCount",
        details.get("changeNoteCount").cloned(),
    );
    insert_optional_value(obj, "ratingStars", details.get("ratingStars").cloned());
    insert_optional_value(obj, "ratingCount", details.get("ratingCount").cloned());
    insert_optional_value(
        obj,
        "uniqueVisitors",
        details.get("uniqueVisitors").cloned(),
    );
    insert_optional_value(
        obj,
        "currentSubscribers",
        details.get("currentSubscribers").cloned(),
    );
    insert_optional_value(
        obj,
        "currentFavorites",
        details.get("currentFavorites").cloned(),
    );

    if let Some(tags) = details.get("tags").cloned() {
        obj.insert("pageTags".to_string(), tags);
    }
    if let Some(required) = details.get("requiredItems").cloned() {
        obj.insert("requiredItems".to_string(), required);
    }
    if let Some(child_ids) = details.get("childItemIds").cloned() {
        obj.insert("childItemIds".to_string(), child_ids);
    } else if let Some(collection_items) = details.get("collectionItems").and_then(|v| v.as_array())
    {
        let child_ids: Vec<String> = collection_items
            .iter()
            .filter_map(|item| item.get("workshopId").and_then(|v| v.as_str()))
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect();
        insert_optional_vec_string(obj, "childItemIds", Some(child_ids));
    }
    if let Some(parent_collections) = details.get("parentCollections").cloned() {
        obj.insert("parentCollections".to_string(), parent_collections);
    }

    if allow_rich_content {
        if let Some(preview_url) = details.get("previewUrl").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "previewUrl", preview_url);
            insert_non_empty_string(obj, "imagePath", preview_url);
        }
        if let Some(description) = details.get("description").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "description", description);
        }
        if let Some(description_html) = details.get("descriptionHtml").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "descriptionHtml", description_html);
        }
        if let Some(gallery) = details.get("imageGallery").cloned() {
            obj.insert("imageGallery".to_string(), gallery.clone());
            obj.insert("galleryUrls".to_string(), gallery);
        }
        if let Some(background) = details.get("backgroundImageUrl").and_then(|v| v.as_str()) {
            insert_non_empty_string(obj, "backgroundImageUrl", background);
        }
    } else {
        clear_persisted_workshop_media_and_content(obj);
    }
}

fn request_download_cancellation(state: &crate::AppState, workshop_id: &str) -> Result<(), String> {
    let mut cancelled = state
        .cancelled_downloads
        .lock()
        .map_err(|_| "Failed to acquire cancelled downloads lock".to_string())?;
    cancelled.insert(workshop_id.to_string());
    Ok(())
}

fn clear_download_cancellation(state: &crate::AppState, workshop_id: &str) -> Result<(), String> {
    let mut cancelled = state
        .cancelled_downloads
        .lock()
        .map_err(|_| "Failed to acquire cancelled downloads lock".to_string())?;
    cancelled.remove(workshop_id);
    Ok(())
}

fn is_download_cancelled(state: &crate::AppState, workshop_id: &str) -> Result<bool, String> {
    let cancelled = state
        .cancelled_downloads
        .lock()
        .map_err(|_| "Failed to acquire cancelled downloads lock".to_string())?;
    Ok(cancelled.contains(workshop_id))
}

fn is_dummy_vpk(path: &Path) -> bool {
    if let Ok((files, mut file)) = crate::vpk::parse_vpk(path) {
        let addoninfo_key = files.keys().find(|k| {
            let lower = k.to_lowercase();
            lower == "addoninfo.txt"
                || lower.ends_with("/addoninfo.txt")
                || lower.ends_with("\\addoninfo.txt")
        });
        if let Some(key) = addoninfo_key {
            if let Some(entry) = files.get(key) {
                if let Ok(content_bytes) = crate::vpk::get_file_content(&mut file, entry) {
                    let text = String::from_utf8_lossy(&content_bytes);
                    let parsed = crate::vpk::parse_key_values(&text);
                    return is_dummy_addon_info(&parsed);
                }
            }
        }
    }
    false
}

fn remove_dummy_workshop_targets(dest_path: &Path) -> Result<(), String> {
    for candidate in [
        dest_path.to_path_buf(),
        dest_path.with_extension("vpk.disabled"),
    ] {
        if candidate.exists() && is_dummy_vpk(&candidate) {
            fs::remove_file(&candidate).map_err(|e| {
                format!(
                    "Failed to remove dummy workshop target {}: {}",
                    candidate.display(),
                    e
                )
            })?;
        }
    }

    Ok(())
}

fn load_known_addons(known_addons_path: &Path) -> HashMap<String, KnownAddonEntry> {
    if known_addons_path.exists() {
        if let Ok(content) = fs::read_to_string(known_addons_path) {
            if let Ok(parsed) = serde_json::from_str::<HashMap<String, KnownAddonEntry>>(&content) {
                let mut normalized = HashMap::new();
                for (key, mut entry) in parsed
                    .into_iter()
                    .filter(|(_, entry)| !is_dummy_addon_info(&entry.addon_info))
                {
                    let Some(workshop_id) = entry
                        .workshop_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                    else {
                        normalized.insert(key, entry);
                        continue;
                    };

                    entry.id = workshop_id.to_string();
                    let is_canonical_key = key == workshop_id;
                    match normalized.entry(workshop_id.to_string()) {
                        std::collections::hash_map::Entry::Vacant(slot) => {
                            slot.insert(entry);
                        }
                        std::collections::hash_map::Entry::Occupied(mut slot)
                            if is_canonical_key =>
                        {
                            slot.insert(entry);
                        }
                        std::collections::hash_map::Entry::Occupied(_) => {}
                    }
                }
                return normalized;
            }
        }
    }
    HashMap::new()
}

fn normalized_etag(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalized_last_modified(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn partial_download_path(download_cache_dir: &Path, workshop_id: &str) -> PathBuf {
    download_cache_dir.join(format!("{}.vpk.part", workshop_id))
}

fn partial_download_metadata_path(download_cache_dir: &Path, workshop_id: &str) -> PathBuf {
    download_cache_dir.join(format!("{}.json", workshop_id))
}

fn cleanup_partial_download(download_cache_dir: &Path, workshop_id: &str) -> Result<(), String> {
    for path in [
        partial_download_path(download_cache_dir, workshop_id),
        partial_download_metadata_path(download_cache_dir, workshop_id),
    ] {
        if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                format!(
                    "Failed to remove partial download artifact {}: {}",
                    path.display(),
                    e
                )
            })?;
        }
    }
    Ok(())
}

fn move_or_copy_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            fs::copy(src, dst)?;
            fs::remove_file(src)?;
            if dst.exists() {
                Ok(())
            } else {
                Err(rename_err)
            }
        }
    }
}

fn load_partial_download_metadata(metadata_path: &Path) -> Result<DownloadResumeMetadata, String> {
    let content = fs::read_to_string(metadata_path).map_err(|e| {
        format!(
            "Failed to read partial download metadata {}: {}",
            metadata_path.display(),
            e
        )
    })?;
    serde_json::from_str(&content).map_err(|e| {
        format!(
            "Failed to parse partial download metadata {}: {}",
            metadata_path.display(),
            e
        )
    })
}

fn save_partial_download_metadata(
    metadata_path: &Path,
    metadata: &DownloadResumeMetadata,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(metadata).map_err(|e| {
        format!(
            "Failed to serialize partial download metadata {}: {}",
            metadata_path.display(),
            e
        )
    })?;
    fs::write(metadata_path, json).map_err(|e| {
        format!(
            "Failed to write partial download metadata {}: {}",
            metadata_path.display(),
            e
        )
    })
}

fn parse_content_range_start(value: &str) -> Option<(u64, u64)> {
    let trimmed = value.trim();
    let bytes = trimmed.strip_prefix("bytes ")?;
    let (range, total) = bytes.split_once('/')?;
    let total = total.trim().parse::<u64>().ok()?;
    let (start, _) = range.split_once('-')?;
    let start = start.trim().parse::<u64>().ok()?;
    Some((start, total))
}

fn load_settings_store(settings_path: &Path) -> SettingsStore {
    if !settings_path.exists() {
        return SettingsStore::default();
    }

    let Ok(content) = fs::read_to_string(settings_path) else {
        return SettingsStore::default();
    };

    serde_json::from_str::<SettingsStore>(&content).unwrap_or_else(|_| {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
            SettingsStore {
                settings: serde_json::from_value(
                    value.get("settings").cloned().unwrap_or_default(),
                )
                .unwrap_or_default(),
                master_collections: serde_json::from_value(
                    value.get("masterCollections").cloned().unwrap_or_default(),
                )
                .unwrap_or_default(),
            }
        } else {
            SettingsStore::default()
        }
    })
}

fn load_groups(groups_path: &Path) -> Vec<Group> {
    if !groups_path.exists() {
        return Vec::new();
    }

    let Ok(content) = fs::read_to_string(groups_path) else {
        return Vec::new();
    };

    serde_json::from_str::<Vec<Group>>(&content).unwrap_or_else(|_| {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
            serde_json::from_value(value.get("groups").cloned().unwrap_or_default())
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    })
}

fn load_legacy_db_value(runtime_dir: &Path) -> Option<serde_json::Value> {
    let legacy_db_path = runtime_dir.join("db.json");
    if !legacy_db_path.exists() {
        return None;
    }

    let content = fs::read_to_string(legacy_db_path).ok()?;
    serde_json::from_str::<serde_json::Value>(&content).ok()
}

fn load_known_addons_from_legacy_db(
    legacy_db: &serde_json::Value,
) -> HashMap<String, KnownAddonEntry> {
    let mut migrated = HashMap::new();

    if let Some(addons_obj) = legacy_db.get("addons").and_then(|v| v.as_object()) {
        for value in addons_obj.values() {
            if let Ok(mut addon) = serde_json::from_value::<Addon>(value.clone()) {
                if addon.id.is_empty() {
                    addon.id = addon
                        .workshop_id
                        .clone()
                        .unwrap_or_else(|| addon.vpk_name.clone());
                }
                if addon.is_dummy || is_dummy_addon_info(&addon.addon_info) {
                    continue;
                }
                let id = addon
                    .workshop_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .unwrap_or(&addon.id)
                    .to_string();
                migrated.insert(
                    id.clone(),
                    KnownAddonEntry {
                        id,
                        vpk_name: addon.vpk_name.clone(),
                        workshop_id: addon.workshop_id.clone(),
                        workshop_id_candidate: addon.workshop_id_candidate,
                        workshop_id_source: addon.workshop_id_source,
                        workshop_id_validation_status: addon.workshop_id_validation_status,
                        workshop_id_last_attempt_at: addon.workshop_id_last_attempt_at,
                        addon_info: addon.addon_info,
                        has_image: addon.has_image,
                        image_path: addon.image_path,
                        steam_details: addon.steam_details,
                    },
                );
            }
        }
    }

    migrated
}

fn load_workshop_cache(cache_path: &Path) -> HashMap<String, serde_json::Value> {
    let (items, authors) = load_workshop_cache_document(cache_path);
    let mut items = items;
    expand_author_directory_into_cache(&mut items, &authors);
    expand_cached_item_compat_fields(&mut items);
    items
}

fn load_workshop_cache_document(
    cache_path: &Path,
) -> (
    HashMap<String, serde_json::Value>,
    HashMap<String, serde_json::Value>,
) {
    if !cache_path.exists() {
        return (HashMap::new(), HashMap::new());
    }

    let Ok(content) = fs::read_to_string(cache_path) else {
        return (HashMap::new(), HashMap::new());
    };

    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return (HashMap::new(), HashMap::new());
    };

    let authors = value
        .get("authors")
        .and_then(|v| v.as_object())
        .map(|authors| {
            authors
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();

    (workshop_cache_items_from_value(&value), authors)
}

fn workshop_cache_items_from_value(
    value: &serde_json::Value,
) -> HashMap<String, serde_json::Value> {
    let mut items = HashMap::new();
    if let Some(obj) = value.as_object() {
        collect_workshop_cache_items(obj, &mut items);
    }
    items
}

fn collect_workshop_cache_items(
    obj: &serde_json::Map<String, serde_json::Value>,
    items: &mut HashMap<String, serde_json::Value>,
) {
    for (key, value) in obj {
        if key == "schemaVersion" || key == "authors" {
            continue;
        }
        if key == "items" {
            if let Some(child_obj) = value.as_object() {
                collect_workshop_cache_items(child_obj, items);
            }
            continue;
        }

        let Some(value_obj) = value.as_object() else {
            continue;
        };
        let workshop_id = value_obj
            .get("workshopId")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .or_else(|| {
                key.chars()
                    .all(|c| c.is_ascii_digit())
                    .then(|| key.to_string())
            });

        if let Some(workshop_id) = workshop_id {
            items.insert(workshop_id, value.clone());
        }
    }
}

fn json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn account_id_to_steam_id(account_id: &str) -> Option<String> {
    let account = account_id.trim().parse::<u64>().ok()?;
    Some((76561197960265728u64 + account).to_string())
}

fn known_addon_to_workshop_cache_entry(
    key: &str,
    entry: &KnownAddonEntry,
) -> Option<(String, serde_json::Value)> {
    let workshop_id = entry
        .workshop_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .or_else(|| {
            let trimmed = key.trim();
            trimmed
                .chars()
                .all(|c| c.is_ascii_digit())
                .then_some(trimmed)
        })?;

    let mut obj = serde_json::Map::new();
    insert_non_empty_string(&mut obj, "workshopId", workshop_id);
    insert_non_empty_string(&mut obj, "title", &entry.vpk_name);
    if let Some(image_path) = entry.image_path.as_deref() {
        insert_non_empty_string(&mut obj, "imagePath", image_path);
        insert_non_empty_string(&mut obj, "previewUrl", image_path);
    }

    if let Some(details) = entry.steam_details.as_ref() {
        if let Some(title) = json_string(details, "title") {
            insert_non_empty_string(&mut obj, "title", &title);
        }
        if let Some(preview_url) = json_string(details, "preview_url") {
            insert_non_empty_string(&mut obj, "previewUrl", &preview_url);
            insert_non_empty_string(&mut obj, "imagePath", &preview_url);
        }
        if let Some(description) = json_string(details, "description") {
            insert_non_empty_string(&mut obj, "description", &description);
            insert_non_empty_string(&mut obj, "shortDescription", &description);
        }
        if let Some(creator) = json_string(details, "creator") {
            insert_non_empty_string(&mut obj, "creatorId", &creator);
            if creator.chars().all(|c| c.is_ascii_digit()) {
                insert_non_empty_string(&mut obj, "creatorSteamId", &creator);
            }
        }
        if let Some(account_id) = json_string(details, "creator_account_id") {
            insert_non_empty_string(&mut obj, "creatorAccountId", &account_id);
            if let Some(steam_id) = account_id_to_steam_id(&account_id) {
                insert_non_empty_string(&mut obj, "creatorSteamId", &steam_id);
            }
        }
        if let Some(vanity_id) = json_string(details, "creator_vanity_id") {
            insert_non_empty_string(&mut obj, "creatorVanityId", &vanity_id);
        }
        if let Some(creator_name) = json_string(details, "creator_name") {
            let ids = [
                json_string(&serde_json::Value::Object(obj.clone()), "creatorSteamId")
                    .unwrap_or_default(),
                json_string(&serde_json::Value::Object(obj.clone()), "creatorAccountId")
                    .unwrap_or_default(),
                json_string(&serde_json::Value::Object(obj.clone()), "creatorVanityId")
                    .unwrap_or_default(),
            ];
            insert_author_name_if_useful(&mut obj, &creator_name, &ids);
        }
        if let Some(file_size) = json_string(details, "file_size") {
            insert_non_empty_string(&mut obj, "fileSizeDisplay", &file_size);
        }
        if let Some(tags) = details.get("tags").cloned() {
            obj.insert("tags".to_string(), tags);
        }
        obj.insert("steamDetails".to_string(), details.clone());
    }

    Some((workshop_id.to_string(), serde_json::Value::Object(obj)))
}

fn merge_workshop_cache_values(base: &mut serde_json::Value, overlay: serde_json::Value) {
    let Some(base_obj) = base.as_object_mut() else {
        *base = overlay;
        return;
    };
    let Some(overlay_obj) = overlay.as_object() else {
        *base = overlay;
        return;
    };
    for (key, value) in overlay_obj {
        if !value.is_null() {
            base_obj.insert(key.clone(), value.clone());
        }
    }
}

fn load_known_addons_as_workshop_cache(
    known_addons_path: &Path,
) -> HashMap<String, serde_json::Value> {
    load_known_addons(known_addons_path)
        .iter()
        .filter_map(|(key, entry)| known_addon_to_workshop_cache_entry(key, entry))
        .collect()
}

fn author_identity_values(obj: &serde_json::Map<String, serde_json::Value>) -> HashSet<String> {
    let mut ids = HashSet::new();
    if let Some(value) = obj.get("authorKey").and_then(|v| v.as_str()) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            ids.insert(trimmed.to_lowercase());
        }
    }
    for key in [
        "creatorSteamId",
        "creatorAccountId",
        "creatorVanityId",
        "creatorId",
        "authorId",
        "creatorProfileUrl",
        "authorUrl",
    ] {
        if let Some(value) = obj.get(key).and_then(|v| v.as_str()) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                ids.insert(trimmed.to_lowercase());
                if key == "creatorAccountId" {
                    if let Some(steam_id) = account_id_to_steam_id(trimmed) {
                        ids.insert(steam_id);
                    }
                }
                if let Some(profile_id) = trimmed
                    .split("/profiles/")
                    .nth(1)
                    .and_then(|s| s.split('/').next())
                {
                    if !profile_id.is_empty() {
                        ids.insert(profile_id.to_lowercase());
                    }
                }
                if let Some(vanity_id) = trimmed
                    .split("/id/")
                    .nth(1)
                    .and_then(|s| s.split('/').next())
                {
                    if !vanity_id.is_empty() {
                        ids.insert(vanity_id.to_lowercase());
                    }
                }
            }
        }
    }
    ids
}

fn object_string(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn profile_steam_id(url: &str) -> Option<String> {
    url.split("/profiles/")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .map(str::trim)
        .filter(|id| id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty())
        .map(str::to_string)
}

fn profile_vanity_id(url: &str) -> Option<String> {
    url.split("/id/")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn author_directory_key(
    steam_id: Option<&str>,
    account_id: Option<&str>,
    vanity_id: Option<&str>,
    profile_url: Option<&str>,
) -> Option<String> {
    if let Some(steam_id) = steam_id
        .map(str::trim)
        .filter(|id| id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty())
    {
        return Some(steam_id.to_string());
    }
    if let Some(steam_id) = account_id.and_then(account_id_to_steam_id) {
        return Some(steam_id);
    }
    if let Some(profile_steam_id) = profile_url.and_then(profile_steam_id) {
        return Some(profile_steam_id);
    }
    if let Some(vanity_id) = vanity_id.map(str::trim).filter(|id| !id.is_empty()) {
        return Some(format!("vanity:{}", vanity_id.to_lowercase()));
    }
    if let Some(profile_vanity_id) = profile_url.and_then(profile_vanity_id) {
        return Some(format!("vanity:{}", profile_vanity_id.to_lowercase()));
    }
    profile_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(|url| format!("url:{}", url.to_lowercase()))
}

fn author_key_for_item(obj: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    object_string(obj, "authorKey").or_else(|| {
        let profile_url =
            object_string(obj, "creatorProfileUrl").or_else(|| object_string(obj, "authorUrl"));
        author_directory_key(
            object_string(obj, "creatorSteamId")
                .or_else(|| object_string(obj, "authorSteamId"))
                .or_else(|| {
                    object_string(obj, "creatorId")
                        .filter(|id| id.chars().all(|c| c.is_ascii_digit()) && id.len() >= 16)
                })
                .as_deref(),
            object_string(obj, "creatorAccountId")
                .or_else(|| object_string(obj, "authorAccountId"))
                .as_deref(),
            object_string(obj, "creatorVanityId")
                .or_else(|| object_string(obj, "authorVanityId"))
                .as_deref(),
            profile_url.as_deref(),
        )
    })
}

fn author_identity_values_from_entry(
    key: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> HashSet<String> {
    let mut ids = HashSet::from([key.to_lowercase()]);
    for value in [
        object_string(obj, "steamId"),
        object_string(obj, "accountId"),
        object_string(obj, "vanityId"),
        object_string(obj, "profileUrl"),
    ]
    .into_iter()
    .flatten()
    {
        ids.insert(value.to_lowercase());
        if let Some(steam_id) = account_id_to_steam_id(&value) {
            ids.insert(steam_id);
        }
        if let Some(steam_id) = profile_steam_id(&value) {
            ids.insert(steam_id.to_lowercase());
        }
        if let Some(vanity_id) = profile_vanity_id(&value) {
            ids.insert(vanity_id.to_lowercase());
            ids.insert(format!("vanity:{}", vanity_id.to_lowercase()));
        }
    }
    ids
}

fn collect_author_directory(
    cache: &HashMap<String, serde_json::Value>,
) -> HashMap<String, serde_json::Value> {
    let mut authors: HashMap<String, serde_json::Value> = HashMap::new();

    for value in cache.values() {
        let Some(obj) = value.as_object() else {
            continue;
        };
        let profile_url =
            object_string(obj, "creatorProfileUrl").or_else(|| object_string(obj, "authorUrl"));
        let steam_id = object_string(obj, "creatorSteamId")
            .or_else(|| object_string(obj, "authorSteamId"))
            .or_else(|| profile_url.as_deref().and_then(profile_steam_id))
            .or_else(|| {
                object_string(obj, "creatorId")
                    .filter(|id| id.chars().all(|c| c.is_ascii_digit()) && id.len() >= 16)
            });
        let account_id = object_string(obj, "creatorAccountId")
            .or_else(|| object_string(obj, "authorAccountId"));
        let vanity_id = object_string(obj, "creatorVanityId")
            .or_else(|| object_string(obj, "authorVanityId"))
            .or_else(|| profile_url.as_deref().and_then(profile_vanity_id));
        let Some(key) = author_directory_key(
            steam_id.as_deref(),
            account_id.as_deref(),
            vanity_id.as_deref(),
            profile_url.as_deref(),
        ) else {
            continue;
        };

        let mut entry = authors
            .remove(&key)
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        insert_non_empty_string(&mut entry, "key", &key);
        if let Some(steam_id) = steam_id.as_deref() {
            insert_non_empty_string(&mut entry, "steamId", steam_id);
        }
        if let Some(account_id) = account_id.as_deref() {
            insert_non_empty_string(&mut entry, "accountId", account_id);
            if !entry.contains_key("steamId") {
                if let Some(steam_id) = account_id_to_steam_id(account_id) {
                    insert_non_empty_string(&mut entry, "steamId", &steam_id);
                }
            }
        }
        if let Some(vanity_id) = vanity_id.as_deref() {
            insert_non_empty_string(&mut entry, "vanityId", vanity_id);
        }
        if let Some(profile_url) = profile_url.as_deref() {
            insert_non_empty_string(&mut entry, "profileUrl", profile_url);
        }

        let ids = author_identity_values(obj);
        let ids_vec = ids.iter().cloned().collect::<Vec<_>>();
        let name = object_string(obj, "creatorName").or_else(|| object_string(obj, "authorName"));
        if let Some(name) = name.filter(|name| !looks_like_placeholder_author_name(name, &ids_vec))
        {
            insert_non_empty_string(&mut entry, "name", &name);
        }
        authors.insert(key, serde_json::Value::Object(entry));
    }

    authors
}

fn expand_author_directory_into_cache(
    cache: &mut HashMap<String, serde_json::Value>,
    authors: &HashMap<String, serde_json::Value>,
) {
    if authors.is_empty() {
        return;
    }
    let author_entries: Vec<(
        String,
        serde_json::Map<String, serde_json::Value>,
        HashSet<String>,
    )> = authors
        .iter()
        .filter_map(|(key, value)| {
            let obj = value.as_object()?.clone();
            let ids = author_identity_values_from_entry(key, &obj);
            Some((key.clone(), obj, ids))
        })
        .collect();

    for value in cache.values_mut() {
        let Some(obj) = value.as_object_mut() else {
            continue;
        };
        let ids = author_identity_values(obj);
        let matched = author_key_for_item(obj)
            .and_then(|key| {
                author_entries
                    .iter()
                    .find(|(author_key, _, _)| author_key.eq_ignore_ascii_case(&key))
            })
            .or_else(|| {
                author_entries
                    .iter()
                    .find(|(_, _, author_ids)| !ids.is_disjoint(author_ids))
            });

        let Some((key, author, _)) = matched else {
            continue;
        };
        insert_non_empty_string(obj, "authorKey", key);
        if let Some(name) = object_string(author, "name") {
            insert_non_empty_string(obj, "creatorName", &name);
            insert_non_empty_string(obj, "authorName", &name);
        }
        if let Some(profile_url) = object_string(author, "profileUrl") {
            insert_non_empty_string(obj, "creatorProfileUrl", &profile_url);
            insert_non_empty_string(obj, "authorUrl", &profile_url);
        }
        if let Some(steam_id) = object_string(author, "steamId") {
            insert_non_empty_string(obj, "creatorSteamId", &steam_id);
            if obj
                .get("creatorId")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .is_none()
            {
                insert_non_empty_string(obj, "creatorId", &steam_id);
            }
        }
        if let Some(account_id) = object_string(author, "accountId") {
            insert_non_empty_string(obj, "creatorAccountId", &account_id);
        }
        if let Some(vanity_id) = object_string(author, "vanityId") {
            insert_non_empty_string(obj, "creatorVanityId", &vanity_id);
        }
    }
}

fn expand_cached_item_compat_fields(cache: &mut HashMap<String, serde_json::Value>) {
    for value in cache.values_mut() {
        let Some(obj) = value.as_object_mut() else {
            continue;
        };
        if !obj.contains_key("shortDescription") {
            if let Some(description) = object_string(obj, "description") {
                insert_non_empty_string(obj, "shortDescription", &description);
            }
        }
        if !obj.contains_key("galleryUrls") {
            if let Some(gallery) = obj.get("imageGallery").cloned() {
                obj.insert("galleryUrls".to_string(), gallery);
            }
        }
    }
}

fn compact_workshop_cache_item_for_save(
    value: &serde_json::Value,
    authors: &HashMap<String, serde_json::Value>,
) -> serde_json::Value {
    let Some(source_obj) = value.as_object() else {
        return value.clone();
    };
    let mut obj = source_obj.clone();
    if let Some(key) = author_key_for_item(&obj) {
        if authors.contains_key(&key) {
            insert_non_empty_string(&mut obj, "authorKey", &key);
            obj.remove("creatorName");
            obj.remove("authorName");
            obj.remove("creatorProfileUrl");
            obj.remove("authorUrl");
        }
    }

    let description = object_string(&obj, "description");
    let short_description = object_string(&obj, "shortDescription");
    if description.is_some() && description == short_description {
        obj.remove("shortDescription");
    }
    if obj.get("imageGallery") == obj.get("galleryUrls") {
        obj.remove("galleryUrls");
    }

    serde_json::Value::Object(obj)
}

fn propagate_author_names(cache: &mut HashMap<String, serde_json::Value>) {
    let learned: Vec<(String, Option<String>, HashSet<String>)> = cache
        .values()
        .filter_map(|value| {
            let obj = value.as_object()?;
            let ids = author_identity_values(obj);
            let name = obj
                .get("creatorName")
                .or_else(|| obj.get("authorName"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|name| {
                    !looks_like_placeholder_author_name(
                        name,
                        &ids.iter().cloned().collect::<Vec<_>>(),
                    )
                })?;
            let url = obj
                .get("creatorProfileUrl")
                .or_else(|| obj.get("authorUrl"))
                .and_then(|v| v.as_str())
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
            Some((name.to_string(), url, ids))
        })
        .collect();

    for value in cache.values_mut() {
        let Some(obj) = value.as_object_mut() else {
            continue;
        };
        let ids = author_identity_values(obj);
        let current_name = obj
            .get("creatorName")
            .or_else(|| obj.get("authorName"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let ids_vec = ids.iter().cloned().collect::<Vec<_>>();
        let has_placeholder_name = looks_like_placeholder_author_name(&current_name, &ids_vec);
        if has_placeholder_name {
            obj.remove("creatorName");
            obj.remove("authorName");
        }
        if ids.is_empty() {
            continue;
        }
        for (name, url, learned_ids) in &learned {
            if ids.is_disjoint(learned_ids) {
                continue;
            }
            if has_placeholder_name {
                insert_non_empty_string(obj, "creatorName", name);
                insert_non_empty_string(obj, "authorName", name);
            }
            if let Some(url) = url {
                insert_non_empty_string(obj, "creatorProfileUrl", url);
                insert_non_empty_string(obj, "authorUrl", url);
            }
            break;
        }
    }
}

fn merge_known_addon_snapshots_into_cache(
    cache: &mut HashMap<String, serde_json::Value>,
    known_addons_path: &Path,
) {
    for (workshop_id, mut known_value) in load_known_addons_as_workshop_cache(known_addons_path) {
        if let Some(existing) = cache.get(&workshop_id).cloned() {
            merge_workshop_cache_values(&mut known_value, existing);
        }
        cache.insert(workshop_id, known_value);
    }
}

fn workshop_cache_with_known_addons(
    cache_path: &Path,
    known_addons_path: &Path,
) -> HashMap<String, serde_json::Value> {
    let mut combined = load_known_addons_as_workshop_cache(known_addons_path);
    for (workshop_id, value) in load_workshop_cache(cache_path) {
        if let Some(existing) = combined.get_mut(&workshop_id) {
            merge_workshop_cache_values(existing, value);
        } else {
            combined.insert(workshop_id, value);
        }
    }
    propagate_author_names(&mut combined);
    combined
}

fn save_workshop_cache(
    cache_path: &Path,
    cache: &HashMap<String, serde_json::Value>,
) -> Result<(), String> {
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create cache directory: {}", e))?;
    }
    let authors = collect_author_directory(cache);
    let compact_items: HashMap<String, serde_json::Value> = cache
        .iter()
        .filter(|(key, value)| {
            key.chars().all(|c| c.is_ascii_digit())
                || value
                    .get("workshopId")
                    .and_then(|v| v.as_str())
                    .map(|id| !id.trim().is_empty())
                    .unwrap_or(false)
        })
        .map(|(key, value)| {
            (
                key.clone(),
                compact_workshop_cache_item_for_save(value, &authors),
            )
        })
        .collect();
    let json = serde_json::to_string_pretty(&serde_json::json!({
        "schemaVersion": 3,
        "items": compact_items,
        "authors": authors,
    }))
    .map_err(|e| format!("Failed to serialize workshop cache: {}", e))?;
    fs::write(cache_path, json).map_err(|e| format!("Failed to write workshop cache: {}", e))
}

fn insert_non_empty_string(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: &str,
) {
    let value = value.trim();
    if !value.is_empty() {
        obj.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
}

fn looks_like_placeholder_author_name(name: &str, ids: &[String]) -> bool {
    let name = name.trim();
    if name.is_empty()
        || name.chars().all(|c| c.is_ascii_digit())
        || name.eq_ignore_ascii_case("AUTHOR_NAME")
        || name.eq_ignore_ascii_case("[unknown]")
    {
        return true;
    }

    ids.iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .any(|id| name.eq_ignore_ascii_case(id))
}

fn insert_author_name_if_useful(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    name: &str,
    ids: &[String],
) {
    if looks_like_placeholder_author_name(name, ids) {
        return;
    }
    insert_non_empty_string(obj, "creatorName", name);
    insert_non_empty_string(obj, "authorName", name);
}

fn insert_optional_value(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<serde_json::Value>,
) {
    if let Some(value) = value {
        if !value.is_null() {
            obj.insert(key.to_string(), value);
        }
    }
}

fn insert_optional_u64(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<u64>,
) {
    if let Some(value) = value {
        obj.insert(key.to_string(), serde_json::json!(value));
    }
}

fn insert_optional_vec_string(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<Vec<String>>,
) {
    if let Some(values) = value {
        let cleaned: Vec<String> = values
            .into_iter()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect();
        if !cleaned.is_empty() {
            obj.insert(key.to_string(), serde_json::json!(cleaned));
        }
    }
}

fn cache_entry_object<'a>(
    cache: &'a mut HashMap<String, serde_json::Value>,
    workshop_id: &str,
) -> &'a mut serde_json::Map<String, serde_json::Value> {
    let entry = cache
        .entry(workshop_id.to_string())
        .or_insert_with(|| serde_json::json!({ "workshopId": workshop_id }));
    if !entry.is_object() {
        *entry = serde_json::json!({ "workshopId": workshop_id });
    }
    entry
        .as_object_mut()
        .expect("workshop cache entry must be an object")
}

fn append_workshop_crawl_log_internal(
    crawl_log_path: &Path,
    record: serde_json::Value,
) -> Result<(), String> {
    if let Some(parent) = crawl_log_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create crawl log directory: {}", e))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(crawl_log_path)
        .map_err(|e| format!("Failed to open crawl log: {}", e))?;
    let line = serde_json::to_string(&record)
        .map_err(|e| format!("Failed to serialize crawl log record: {}", e))?;
    writeln!(file, "{}", line).map_err(|e| format!("Failed to append crawl log: {}", e))
}

fn merge_workshop_details_into_addon(addon: &mut Addon, details: &serde_json::Value) {
    addon.workshop_details = Some(details.clone());

    if addon.image_path.is_none() {
        if let Some(preview_url) = details
            .get("previewUrl")
            .or_else(|| details.get("imagePath"))
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
        {
            addon.image_path = Some(preview_url.to_string());
            addon.has_image = true;
        }
    }

    let creator_name = details
        .get("creatorName")
        .or_else(|| details.get("authorName"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string());

    let creator_id = details
        .get("creatorSteamId")
        .or_else(|| details.get("creatorId"))
        .or_else(|| details.get("authorId"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string());

    let creator_vanity_id = details
        .get("creatorVanityId")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string());

    let creator_account_id = details
        .get("creatorAccountId")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string());

    if creator_name.is_some() || creator_id.is_some() {
        let mut steam_details = addon
            .steam_details
            .clone()
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
        if !steam_details.is_object() {
            steam_details = serde_json::Value::Object(serde_json::Map::new());
        }
        if let Some(obj) = steam_details.as_object_mut() {
            if let Some(name) = creator_name {
                let current = obj
                    .get("creator_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if current.trim().is_empty() || current == creator_id.as_deref().unwrap_or_default()
                {
                    obj.insert("creator_name".to_string(), serde_json::Value::String(name));
                }
            }
            if let Some(id) = creator_id {
                obj.entry("creator".to_string())
                    .or_insert_with(|| serde_json::Value::String(id));
            }
            if let Some(vanity_id) = creator_vanity_id {
                obj.entry("creator_vanity_id".to_string())
                    .or_insert_with(|| serde_json::Value::String(vanity_id));
            }
            if let Some(account_id) = creator_account_id {
                obj.entry("creator_account_id".to_string())
                    .or_insert_with(|| serde_json::Value::String(account_id));
            }
            if let Some(description) = details
                .get("description")
                .and_then(|v| v.as_str())
                .filter(|v| !v.trim().is_empty())
            {
                obj.entry("description".to_string())
                    .or_insert_with(|| serde_json::Value::String(description.trim().to_string()));
            }
        }
        addon.steam_details = Some(steam_details);
    }
}

fn database_with_workshop_cache(
    db: &Database,
    workshop_cache_path: &Path,
    known_addons_path: &Path,
) -> Database {
    let cache = workshop_cache_with_known_addons(workshop_cache_path, known_addons_path);
    if cache.is_empty() {
        return db.clone();
    }

    let mut response = db.clone();
    for addon in response.addons.values_mut() {
        if let Some(workshop_id) = addon.workshop_id.as_deref() {
            if let Some(details) = cache.get(workshop_id) {
                merge_workshop_details_into_addon(addon, details);
            }
        }
    }
    for addon in response.known_uninstalled_addons.values_mut() {
        if let Some(workshop_id) = addon.workshop_id.as_deref() {
            if let Some(details) = cache.get(workshop_id) {
                merge_workshop_details_into_addon(addon, details);
            }
        }
    }
    response
}

async fn fetch_steam_details(workshop_ids: &[String]) -> Result<Vec<serde_json::Value>, String> {
    fetch_steam_details_web(workshop_ids).await
}

async fn fetch_steam_details_hybrid(
    workshop_service: &WorkshopService,
    workshop_ids: &[String],
    allow_bridge: bool,
    allow_web_api: bool,
    source_order: &[String],
) -> Result<Vec<Value>, String> {
    if workshop_ids.is_empty() {
        return Ok(Vec::new());
    }

    if !allow_bridge && !allow_web_api {
        return Err("All remote workshop metadata sources are disabled".to_string());
    }

    if !allow_bridge {
        return fetch_steam_details(workshop_ids).await;
    }

    let web_first = source_position(source_order, "steam-web-api", 1)
        < source_position(source_order, "steamworks-sdk", 0);
    if web_first && allow_web_api {
        match fetch_steam_details(workshop_ids).await {
            Ok(details) if !details.is_empty() => return Ok(details),
            Ok(_) | Err(_) => {}
        }
    }

    match workshop_service.bridge_fetch_details(workshop_ids) {
        Ok(details) if !details.is_empty() => Ok(details),
        Ok(_) | Err(_) if allow_web_api => fetch_steam_details(workshop_ids).await,
        Ok(_) | Err(_) => Err(
            "Steamworks SDK did not return workshop details and Steam Web API is disabled"
                .to_string(),
        ),
    }
}

async fn fetch_collection_children_hybrid(
    workshop_service: &WorkshopService,
    collection_id: &str,
    allow_bridge: bool,
    allow_web_api: bool,
    source_order: &[String],
) -> Result<Vec<String>, String> {
    if !allow_bridge && !allow_web_api {
        return Err("All remote workshop collection sources are disabled".to_string());
    }

    if !allow_bridge {
        return fetch_collection_children_web(collection_id).await;
    }

    let web_first = source_position(source_order, "steam-web-api", 1)
        < source_position(source_order, "steamworks-sdk", 0);
    if web_first && allow_web_api {
        match fetch_collection_children_web(collection_id).await {
            Ok(ids) if !ids.is_empty() => return Ok(ids),
            Ok(_) | Err(_) => {}
        }
    }

    if let Ok(payload) = workshop_service.bridge_query_collection(collection_id) {
        let mut ids = Vec::new();
        for item in payload.items {
            if let Some(id) = item.get("publishedfileid").and_then(|v| v.as_str()) {
                ids.push(id.to_string());
            }
        }
        if !ids.is_empty() {
            return Ok(ids);
        }
    }

    if allow_web_api {
        fetch_collection_children_web(collection_id).await
    } else {
        Err(
            "Steamworks SDK did not return collection children and Steam Web API is disabled"
                .to_string(),
        )
    }
}

async fn attempt_bridge_download(
    state: &crate::AppState,
    workshop_service: &WorkshopService,
    workshop_id: &str,
    workshop_dir: &Path,
    app_handle: &AppHandle,
    allow_bridge: bool,
) -> Result<bool, String> {
    if !allow_bridge || !workshop_service.has_bridge() {
        return Ok(false);
    }

    workshop_service.bridge_request_download(workshop_id)?;

    #[derive(Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    struct DownloadProgress {
        workshop_id: String,
        percent: u32,
        downloaded: u64,
        total: u64,
        source: String,
        phase: String,
    }

    let expected_path = workshop_dir.join(format!("{}.vpk", workshop_id));
    let expected_disabled_path = workshop_dir.join(format!("{}.vpk.disabled", workshop_id));
    let started_at = Instant::now();
    let timeout = Duration::from_secs(120);
    let mut saw_download_activity = false;
    let mut saw_install_folder = false;
    let mut last_reported_percent = 0u32;
    let mut last_status_log = Instant::now() - Duration::from_secs(30);
    let mut last_status_signature = String::new();

    while started_at.elapsed() < timeout {
        if is_download_cancelled(state, workshop_id)? {
            return Err(DOWNLOAD_CANCELLED_ERR.to_string());
        }

        let status: BridgeDownloadStatus =
            match workshop_service.bridge_download_status(workshop_id) {
                Ok(status) => status,
                Err(err) => return Err(err),
            };

        let item_states = status.item_state.join(",");
        let install_folder = status.install_folder.as_deref().unwrap_or("");
        let installed_dir = if install_folder.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(install_folder))
        };
        let status_signature = format!(
            "installed={} downloaded={:?} total={:?} installFolder={} itemState={}",
            status.installed, status.downloaded, status.total, install_folder, item_states
        );
        if status_signature != last_status_signature
            || last_status_log.elapsed() >= Duration::from_secs(5)
        {
            println!(
                "Steam SDK download status for {}: {}",
                workshop_id, status_signature
            );
            last_status_signature = status_signature;
            last_status_log = Instant::now();
        }

        if let (Some(downloaded), Some(total)) = (status.downloaded, status.total) {
            if downloaded > 0 || total > 0 {
                saw_download_activity = true;
            }
            let raw_percent = if total > 0 {
                ((downloaded as f64 / total as f64) * 100.0) as u32
            } else {
                0
            }
            .min(99);
            let percent = raw_percent.max(last_reported_percent);
            last_reported_percent = percent;
            let _ = app_handle.emit(
                "download-progress",
                DownloadProgress {
                    workshop_id: workshop_id.to_string(),
                    percent,
                    downloaded,
                    total,
                    source: "steam-sdk".to_string(),
                    phase: if status.installed {
                        "install".to_string()
                    } else {
                        "download".to_string()
                    },
                },
            );
        }

        if (status.installed || installed_dir.is_some()) && last_reported_percent < 99 {
            last_reported_percent = 99;
            let _ = app_handle.emit(
                "download-progress",
                DownloadProgress {
                    workshop_id: workshop_id.to_string(),
                    percent: 99,
                    downloaded: status.downloaded.unwrap_or(0),
                    total: status.total.unwrap_or(0),
                    source: "steam-sdk".to_string(),
                    phase: "install".to_string(),
                },
            );
        }

        if expected_path.exists() || expected_disabled_path.exists() {
            let _ = app_handle.emit(
                "download-progress",
                DownloadProgress {
                    workshop_id: workshop_id.to_string(),
                    percent: 100,
                    downloaded: status.total.unwrap_or(0),
                    total: status.total.unwrap_or(0),
                    source: "steam-sdk".to_string(),
                    phase: "download".to_string(),
                },
            );
            return Ok(true);
        }

        if let Some(ref folder) = installed_dir {
            saw_install_folder = true;
            if let Some(source_path) = find_sdk_installed_workshop_file(folder)? {
                println!(
                    "Steam SDK install path resolved for {}: {} -> importing {}",
                    workshop_id,
                    folder.display(),
                    source_path.display()
                );
                crate::watcher::suppress_internal_refresh_for(
                    state,
                    Duration::from_millis(DOWNLOAD_FINALIZE_SUPPRESS_MS),
                );
                import_sdk_workshop_file(&source_path, workshop_id, workshop_dir)?;
                let final_total = status.total.unwrap_or_else(|| {
                    fs::metadata(&source_path)
                        .map(|meta| meta.len())
                        .unwrap_or(0)
                });
                let _ = app_handle.emit(
                    "download-progress",
                    DownloadProgress {
                        workshop_id: workshop_id.to_string(),
                        percent: 100,
                        downloaded: final_total,
                        total: final_total,
                        source: "steam-sdk".to_string(),
                        phase: "import".to_string(),
                    },
                );
                return Ok(true);
            } else {
                println!(
                    "Steam SDK install path present for {} but no importable file found yet: {}",
                    workshop_id,
                    folder.display()
                );
            }
        }

        if status.installed && !saw_install_folder {
            eprintln!(
                "Steam SDK marked workshop item {} installed without install folder; itemState={}, downloaded={:?}, total={:?}",
                workshop_id, item_states, status.downloaded, status.total
            );
        }

        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    eprintln!(
        "Steam SDK workshop download did not complete within timeout for {}: saw_download_activity={}, saw_install_folder={}, expected_path_exists={}, item_state_polling_fell_through",
        workshop_id,
        saw_download_activity,
        saw_install_folder,
        expected_path.exists() || expected_disabled_path.exists(),
    );

    Ok(false)
}

fn extract_workshop_id_from_url(url: &str) -> Option<String> {
    if let Some(pos) = url.find("id=") {
        let start = pos + 3;
        let end = url[start..]
            .find('&')
            .map(|idx| start + idx)
            .unwrap_or(url.len());
        let id_str = &url[start..end];
        if id_str.chars().all(|c| c.is_ascii_digit()) && !id_str.is_empty() {
            return Some(id_str.to_string());
        }
    }
    None
}

fn clean_group_name(name: &str) -> String {
    let mut s = name.trim().to_string();

    let re_ver = Regex::new(r"(?i)\s+v?\d+(?:\.\d+)*$").unwrap();
    let re_part_num = Regex::new(r"(?i)(?:[-#_/:,\s]+(?:part|pt|partie|pts|vol|volume|chapter|ch|act)?\s*(?:\d+|[ivxldcm]+)(?:\/\d+)?)$").unwrap();
    let re_part_word =
        Regex::new(r"(?i)(?:[-#_/:,\s]+(?:part|pt|partie|pts|vol|volume|chapter|ch|act))$")
            .unwrap();

    loop {
        let prev_len = s.len();

        s = re_ver.replace(&s, "").into_owned();
        s = re_part_num.replace(&s, "").into_owned();
        s = re_part_word.replace(&s, "").into_owned();

        s = s
            .trim_end_matches(|c: char| {
                c.is_whitespace()
                    || c == ':'
                    || c == '_'
                    || c == '-'
                    || c == '/'
                    || c == '\\'
                    || c == '#'
                    || c == '+'
                    || c == ','
                    || c == '.'
            })
            .to_string();

        if s.len() == prev_len {
            break;
        }
    }

    s
}

fn is_collection_detail(details: &Value) -> bool {
    details
        .get("file_type")
        .and_then(|v| v.as_str())
        .map(|v| v.eq_ignore_ascii_case("collection"))
        .unwrap_or_else(|| {
            details
                .get("file_type")
                .and_then(|v| v.as_u64())
                .map(|v| v == 2)
                .unwrap_or(false)
        })
}

fn upsert_known_addon_entry(
    known_addons: &mut HashMap<String, KnownAddonEntry>,
    workshop_id: &str,
    details: Option<&Value>,
) {
    let workshop_id = workshop_id.trim();
    if workshop_id.is_empty() {
        return;
    }

    let existing = known_addons.get(workshop_id).cloned();

    let preview_url = details
        .and_then(|value| value.get("preview_url"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_string())
        .or_else(|| existing.as_ref().and_then(|entry| entry.image_path.clone()));

    let addon_info = existing
        .as_ref()
        .map(|entry| entry.addon_info.clone())
        .unwrap_or(serde_json::Value::Null);
    let vpk_name = existing
        .as_ref()
        .map(|entry| entry.vpk_name.clone())
        .unwrap_or_else(|| format!("{}.vpk", workshop_id));

    known_addons.insert(
        workshop_id.to_string(),
        KnownAddonEntry {
            id: workshop_id.to_string(),
            vpk_name,
            workshop_id: Some(workshop_id.to_string()),
            workshop_id_candidate: None,
            workshop_id_source: None,
            workshop_id_validation_status: None,
            workshop_id_last_attempt_at: None,
            addon_info,
            has_image: preview_url.is_some()
                || existing
                    .as_ref()
                    .map(|entry| entry.has_image)
                    .unwrap_or(false),
            image_path: preview_url,
            steam_details: details
                .cloned()
                .or_else(|| existing.and_then(|entry| entry.steam_details)),
        },
    );
}

fn ensure_workshop_import_master_collection(db: &mut Database) -> String {
    if let Some(existing) = db
        .master_collections
        .iter()
        .find(|mc| mc.name_key.as_deref() == Some("masterCollections.systemWorkshopImport"))
    {
        return existing.id.clone();
    }

    let mc_id = format!(
        "mc_system_ws_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    db.master_collections.push(MasterCollection {
        id: mc_id.clone(),
        name: "从创意工坊导入".to_string(),
        name_key: Some("masterCollections.systemWorkshopImport".to_string()),
        group_ids: Vec::new(),
        is_system: true,
        icon: Some("Globe".to_string()),
    });
    mc_id
}

fn ensure_group_in_master_collection(group: &mut Group, mc_id: &str) {
    if let Some(ref mut ids) = group.master_collection_ids {
        if !ids.iter().any(|id| id == mc_id) {
            ids.push(mc_id.to_string());
        }
    } else {
        group.master_collection_ids = Some(vec![mc_id.to_string()]);
    }
}

fn clear_auto_groups(db: &mut Database) {
    let auto_group_ids: HashSet<String> = db
        .groups
        .iter()
        .filter(|group| group.source.as_deref() == Some("auto-group"))
        .map(|group| group.id.clone())
        .collect();
    if auto_group_ids.is_empty() {
        return;
    }

    db.groups
        .retain(|group| group.source.as_deref() != Some("auto-group"));
    for collection in &mut db.master_collections {
        collection
            .group_ids
            .retain(|group_id| !auto_group_ids.contains(group_id));
    }
}

fn addon_info_flag(addon: &Addon, key: &str) -> bool {
    addon
        .addon_info
        .as_object()
        .and_then(|fields| {
            fields
                .iter()
                .find(|(field, _)| field.eq_ignore_ascii_case(key))
                .map(|(_, value)| value)
        })
        .is_some_and(|value| {
            value
                .as_str()
                .map(|value| value.trim() == "1")
                .unwrap_or_else(|| {
                    value.as_i64() == Some(1)
                        || value.as_u64() == Some(1)
                        || value.as_bool() == Some(true)
                })
        })
}

fn tags_indicate_campaign_or_map(details: Option<&serde_json::Value>) -> bool {
    let Some(details) = details else {
        return false;
    };
    let Some(details) = details.as_object() else {
        return false;
    };

    ["tags", "pageTags"].iter().any(|key| {
        details
            .get(*key)
            .and_then(|value| value.as_array())
            .is_some_and(|tags| {
                tags.iter().any(|tag| {
                    let tag = tag
                        .as_str()
                        .or_else(|| tag.get("tag").and_then(|value| value.as_str()))
                        .or_else(|| tag.get("name").and_then(|value| value.as_str()))
                        .unwrap_or("")
                        .to_lowercase();
                    tag.contains("campaign") || tag.contains("map")
                })
            })
    })
}

fn addon_is_campaign_or_map(addon: &Addon) -> bool {
    addon_info_flag(addon, "addoncontent_campaign")
        || addon_info_flag(addon, "addoncontent_map")
        || tags_indicate_campaign_or_map(addon.steam_details.as_ref())
        || tags_indicate_campaign_or_map(addon.workshop_details.as_ref())
}

fn normalize_group_comparison_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn addon_author_identity(addon: &Addon) -> Option<String> {
    let addon_info_id = addon
        .addon_info
        .get("addonauthorsteamid")
        .or_else(|| addon.addon_info.get("addonAuthorSteamID"))
        .and_then(|value| value.as_str());
    let steam_id = addon
        .steam_details
        .as_ref()
        .and_then(|details| {
            details
                .get("creator_steam_id")
                .or_else(|| details.get("creatorSteamId"))
                .or_else(|| details.get("creator"))
        })
        .and_then(|value| value.as_str());
    let workshop_id = addon
        .workshop_details
        .as_ref()
        .and_then(|details| {
            details
                .get("creatorSteamId")
                .or_else(|| details.get("creatorId"))
                .or_else(|| details.get("authorId"))
        })
        .and_then(|value| value.as_str());

    addon_info_id
        .or(steam_id)
        .or(workshop_id)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_lowercase())
}

fn addon_group_description(addon: &Addon) -> Option<String> {
    let description = addon
        .addon_info
        .get("addondescription")
        .or_else(|| addon.addon_info.get("addontagline"))
        .or_else(|| {
            addon
                .steam_details
                .as_ref()
                .and_then(|details| details.get("description"))
        })
        .or_else(|| {
            addon
                .workshop_details
                .as_ref()
                .and_then(|details| details.get("description"))
        })
        .and_then(|value| value.as_str())?;
    let description = normalize_group_comparison_text(description);
    (description.len() > 10).then_some(description)
}

fn texture_group_root_title(title: &str) -> Option<String> {
    let texture_suffix = Regex::new(r"(?i)^(.*?)\s+textures?$").ok()?;
    let root = texture_suffix.captures(title)?.get(1)?.as_str().trim();
    (root.len() >= 3).then(|| root.to_string())
}

fn auto_group_internal(db: &mut Database) {
    clear_auto_groups(db);

    let mut grouped_vpks = HashSet::new();
    for g in &db.groups {
        for addon in &g.addons {
            grouped_vpks.insert(addon.clone());
        }
    }

    let mut ungrouped = Vec::new();
    for (name, addon) in &db.addons {
        if !grouped_vpks.contains(name) {
            ungrouped.push(addon.clone());
        }
    }

    struct Candidate {
        id: String,
        title: String,
        title_key: String,
        author_identity: Option<String>,
        description: Option<String>,
    }

    let mut candidates = Vec::new();
    for addon in ungrouped {
        if addon.is_dummy || !addon_is_campaign_or_map(&addon) {
            continue;
        }
        let title = addon
            .addon_info
            .get("addontitle")
            .and_then(|title| title.as_str())
            .filter(|title| !title.trim().is_empty())
            .or_else(|| {
                addon
                    .steam_details
                    .as_ref()
                    .and_then(|details| details.get("title").and_then(|title| title.as_str()))
                    .filter(|title| !title.trim().is_empty())
            })
            .unwrap_or(&addon.vpk_name)
            .to_string();

        candidates.push(Candidate {
            id: addon.id.clone(),
            title_key: normalize_group_comparison_text(&title),
            title,
            author_identity: addon_author_identity(&addon),
            description: addon_group_description(&addon),
        });
    }

    let re_part = Regex::new(r"(?i)^(.*?)\s*(?:[-#_]*\s*(?:part|pt|partie|pts|pack)\s*(\d+|[ivxldcm]+)(?:\/\d+)?|\s+v?\d+\.\d+|\s+v\d+)$").unwrap();
    let mut title_groups: HashMap<String, (String, Vec<String>)> = HashMap::new();
    for c in &candidates {
        if let Some(caps) = re_part.captures(&c.title) {
            let prefix = caps.get(1).unwrap().as_str().trim().to_string();
            if prefix.len() >= 3 {
                let cleaned_prefix = clean_group_name(&prefix);
                if cleaned_prefix.len() >= 3 {
                    let key = cleaned_prefix.to_lowercase();
                    let entry = title_groups
                        .entry(key)
                        .or_insert_with(|| (cleaned_prefix.clone(), Vec::new()));
                    if cleaned_prefix < entry.0 {
                        entry.0 = cleaned_prefix;
                    }
                    entry.1.push(c.id.clone());
                }
            }
        }
    }

    title_groups.retain(|_, (_, addons)| {
        let authors = addons
            .iter()
            .filter_map(|id| {
                candidates
                    .iter()
                    .find(|candidate| &candidate.id == id)
                    .and_then(|candidate| candidate.author_identity.as_ref())
            })
            .collect::<HashSet<_>>();
        authors.len() <= 1
    });

    for (prefix, addons) in title_groups.values_mut() {
        let prefix_key = normalize_group_comparison_text(prefix);
        let group_author = addons.iter().find_map(|id| {
            candidates
                .iter()
                .find(|candidate| &candidate.id == id)
                .and_then(|candidate| candidate.author_identity.as_ref())
        });
        for candidate in &candidates {
            let candidate_prefix_key =
                normalize_group_comparison_text(&clean_group_name(&candidate.title));
            let has_matching_author = group_author
                .map(|author| {
                    candidate
                        .author_identity
                        .as_ref()
                        .map(|candidate_author| candidate_author == author)
                        .unwrap_or(true)
                })
                .unwrap_or(true);
            if candidate_prefix_key == prefix_key
                && has_matching_author
                && !addons.contains(&candidate.id)
            {
                addons.push(candidate.id.clone());
            }
        }
    }

    let mut title_groups = title_groups.into_values().collect::<Vec<_>>();
    title_groups.sort_by(|left, right| left.0.cmp(&right.0));
    let mut grouped_ids = HashSet::new();
    for (prefix, mut addons) in title_groups {
        if addons.len() >= 2 {
            addons.sort();
            grouped_ids.extend(addons.iter().cloned());
            let group_id = format!(
                "group_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );

            db.groups.push(Group {
                id: group_id,
                name: prefix,
                addons,
                tags: None,
                workshop_collection_id: None,
                master_collection_ids: None,
                source: Some("auto-group".to_string()),
            });
        }
    }

    let mut identical_title_groups: HashMap<(String, String, String), (String, String, Vec<String>)> =
        HashMap::new();
    for candidate in &candidates {
        if grouped_ids.contains(&candidate.id) {
            continue;
        }
        let (Some(author_identity), Some(description)) =
            (candidate.author_identity.as_ref(), candidate.description.as_ref())
        else {
            continue;
        };
        let entry = identical_title_groups
            .entry((
                candidate.title_key.clone(),
                author_identity.clone(),
                description.clone(),
            ))
            .or_insert_with(|| {
                (
                    candidate.title.clone(),
                    author_identity.clone(),
                    Vec::new(),
                )
            });
        if candidate.title < entry.0 {
            entry.0 = candidate.title.clone();
        }
        entry.2.push(candidate.id.clone());
    }

    let mut identical_title_groups = identical_title_groups.into_values().collect::<Vec<_>>();
    identical_title_groups.sort_by(|left, right| left.0.cmp(&right.0));
    for (mut title, author_identity, mut addons) in identical_title_groups {
        if addons.len() < 2 {
            continue;
        }
        if let Some(root_title) = texture_group_root_title(&title) {
            let root_key = normalize_group_comparison_text(&root_title);
            for candidate in &candidates {
                let has_matching_author = candidate
                    .author_identity
                    .as_ref()
                    .map(|candidate_author| candidate_author == &author_identity)
                    .unwrap_or(true);
                if !grouped_ids.contains(&candidate.id)
                    && candidate.title_key == root_key
                    && has_matching_author
                    && !addons.contains(&candidate.id)
                {
                    addons.push(candidate.id.clone());
                }
            }
            if addons.len() >= 3 {
                title = root_title;
            }
        }
        addons.sort();
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
            addons,
            tags: None,
            workshop_collection_id: None,
            master_collection_ids: None,
            source: Some("auto-group".to_string()),
        });
    }
}

fn move_requires_dir_change(addon: &Addon, target_dir_type: &str) -> bool {
    addon.dir_type != target_dir_type
}

fn toggle_requires_state_change(addon: &Addon, enabled: bool) -> bool {
    addon.is_enabled != enabled
}

fn rename_requires_name_change(addon: &Addon, sanitized: &str) -> bool {
    addon.vpk_name != sanitized
}

#[cfg(test)]
mod tests {
    use super::{
        auto_group_internal, ensure_background_workshop_fetch_allowed,
        extract_steamcommunity_error_message, is_background_workshop_fetch_source,
        load_known_addons, load_workshop_cache, looks_like_placeholder_author_name,
        merge_known_addon_snapshots_into_cache, move_or_copy_file, move_requires_dir_change,
        normalize_master_collection_group_refs, parse_content_range_start,
        persist_seen_workshop_item_entry, persist_workshop_page_details_entry,
        propagate_author_names, remove_dummy_workshop_targets, rename_requires_name_change,
        save_workshop_cache, toggle_requires_state_change, workshop_cache_with_known_addons,
        SourcePolicy,
    };
    use crate::commands::types::{Addon, Database, Group, MasterCollection, WorkshopSeenItem};
    use crate::vpk::generate_dummy_vpk;
    use serde_json::json;
    use std::collections::HashMap;
    use std::fs;

    fn auto_group_addon(
        id: &str,
        title: &str,
        addon_info: serde_json::Value,
        tags: serde_json::Value,
    ) -> Addon {
        Addon {
            id: id.to_string(),
            vpk_name: format!("{}.vpk", id),
            workshop_id: None,
            workshop_id_candidate: None,
            workshop_id_source: None,
            workshop_id_validation_status: None,
            workshop_id_last_attempt_at: None,
            addon_info,
            has_image: false,
            image_path: None,
            files_count: 0,
            file_size: 0,
            parsed_at: String::new(),
            current_path: String::new(),
            dir_type: "loading".to_string(),
            is_enabled: true,
            steam_details: Some(json!({ "title": title, "tags": tags })),
            workshop_details: None,
            is_dummy: false,
        }
    }

    fn write_known_addons_file(name: &str, value: serde_json::Value) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "left4addons-{}-{}-known_addons.json",
            name,
            std::process::id()
        ));
        fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();
        path
    }

    fn write_groups_file(name: &str, value: serde_json::Value) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "left4addons-{}-{}-groups.json",
            name,
            std::process::id()
        ));
        fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();
        path
    }

    fn create_source_vpk(path: &std::path::Path) {
        use std::io::Write;

        let mut file = fs::File::create(path).unwrap();
        let content = b"\"addoninfo\"\n{\n\"addontitle\" \"Mock Addon\"\n}";

        let mut tree = Vec::new();
        tree.extend_from_slice(b"txt\0");
        tree.extend_from_slice(b"my_folder\0");
        tree.extend_from_slice(b"addoninfo\0");
        tree.extend_from_slice(&0u32.to_le_bytes());
        tree.extend_from_slice(&0u16.to_le_bytes());
        tree.extend_from_slice(&0x7fffu16.to_le_bytes());
        tree.extend_from_slice(&0u32.to_le_bytes());
        tree.extend_from_slice(&(content.len() as u32).to_le_bytes());
        tree.extend_from_slice(&0xffffu16.to_le_bytes());
        tree.extend_from_slice(b"\0");
        tree.extend_from_slice(b"\0");
        tree.extend_from_slice(b"\0");

        let tree_size = tree.len() as u32;

        file.write_all(&0x55aa1234u32.to_le_bytes()).unwrap();
        file.write_all(&1u32.to_le_bytes()).unwrap();
        file.write_all(&tree_size.to_le_bytes()).unwrap();
        file.write_all(&tree).unwrap();
        file.write_all(content).unwrap();
    }

    #[test]
    fn background_source_detection_is_limited_to_silent_refreshes() {
        assert!(is_background_workshop_fetch_source("startup-auto"));
        assert!(is_background_workshop_fetch_source("background-refresh"));
        assert!(!is_background_workshop_fetch_source("workshop-detail"));
        assert!(!is_background_workshop_fetch_source("workshop-home"));
    }

    #[test]
    fn normalizes_master_collection_refs_without_dropping_empty_groups() {
        let mut db = Database {
            groups: vec![
                Group {
                    id: "empty-group".to_string(),
                    name: "Empty Group".to_string(),
                    addons: vec![],
                    tags: None,
                    workshop_collection_id: None,
                    master_collection_ids: Some(vec![
                        "collection-1".to_string(),
                        "missing-collection".to_string(),
                    ]),
                    source: None,
                },
                Group {
                    id: "filled-group".to_string(),
                    name: "Filled Group".to_string(),
                    addons: vec!["addon-1".to_string()],
                    tags: None,
                    workshop_collection_id: None,
                    master_collection_ids: None,
                    source: None,
                },
            ],
            master_collections: vec![MasterCollection {
                id: "collection-1".to_string(),
                name: "Collection".to_string(),
                name_key: None,
                group_ids: vec![
                    "empty-group".to_string(),
                    "missing-group".to_string(),
                    "empty-group".to_string(),
                    "filled-group".to_string(),
                ],
                is_system: false,
                icon: None,
            }],
            ..Database::default()
        };

        assert!(normalize_master_collection_group_refs(&mut db));
        assert_eq!(db.groups.len(), 2);
        assert_eq!(
            db.master_collections[0].group_ids,
            vec!["empty-group".to_string(), "filled-group".to_string()]
        );
        assert_eq!(
            db.groups[0].master_collection_ids,
            Some(vec!["collection-1".to_string()])
        );
    }

    #[test]
    fn auto_group_rebuilds_campaign_parts_without_description_heuristics() {
        let mut db = Database {
            addons: HashMap::from([
                (
                    "base".to_string(),
                    auto_group_addon(
                        "base",
                        "City Escape",
                        json!({ "addoncontent_campaign": "1" }),
                        json!([]),
                    ),
                ),
                (
                    "part-1".to_string(),
                    auto_group_addon(
                        "part-1",
                        "City Escape Part 1/2",
                        json!({ "addoncontent_campaign": "1" }),
                        json!([]),
                    ),
                ),
                (
                    "part-2".to_string(),
                    auto_group_addon(
                        "part-2",
                        "city escape PART 2/2",
                        json!({}),
                        json!([{ "tag": "Campaigns" }]),
                    ),
                ),
                (
                    "script".to_string(),
                    auto_group_addon(
                        "script",
                        "City Escape Part 3/3",
                        json!({
                            "addoncontent_script": "1",
                            "addondescription": "shared campaign description"
                        }),
                        json!([{ "tag": "Scripts" }]),
                    ),
                ),
                (
                    "manual-part".to_string(),
                    auto_group_addon(
                        "manual-part",
                        "City Escape Part 4/4",
                        json!({ "addoncontent_campaign": "1" }),
                        json!([]),
                    ),
                ),
            ]),
            groups: vec![
                Group {
                    id: "old-auto".to_string(),
                    name: "Campaign Pack".to_string(),
                    addons: vec!["script".to_string()],
                    tags: None,
                    workshop_collection_id: None,
                    master_collection_ids: None,
                    source: Some("auto-group".to_string()),
                },
                Group {
                    id: "manual".to_string(),
                    name: "Manual Group".to_string(),
                    addons: vec!["manual-part".to_string()],
                    tags: None,
                    workshop_collection_id: None,
                    master_collection_ids: None,
                    source: Some("manual".to_string()),
                },
            ],
            master_collections: vec![MasterCollection {
                id: "campaigns".to_string(),
                name: "Campaigns".to_string(),
                name_key: Some("masterCollections.systemCampaignAuto".to_string()),
                group_ids: vec!["old-auto".to_string()],
                is_system: true,
                icon: None,
            }],
            ..Database::default()
        };

        auto_group_internal(&mut db);

        let auto_groups = db
            .groups
            .iter()
            .filter(|group| group.source.as_deref() == Some("auto-group"))
            .collect::<Vec<_>>();
        assert_eq!(auto_groups.len(), 1);
        assert_eq!(auto_groups[0].name, "City Escape");
        assert_eq!(auto_groups[0].addons, vec!["base", "part-1", "part-2"]);
        assert!(db.groups.iter().any(|group| group.id == "manual"));
        assert!(!db.groups.iter().any(|group| group.id == "old-auto"));
        assert!(db.master_collections[0].group_ids.is_empty());
    }

    #[test]
    fn auto_group_supports_pack_suffixes_without_including_similarly_named_content() {
        let mut db = Database {
            addons: HashMap::from([
                (
                    "base".to_string(),
                    auto_group_addon(
                        "base",
                        "Harbor",
                        json!({ "addoncontent_campaign": "1" }),
                        json!([]),
                    ),
                ),
                (
                    "pack-1".to_string(),
                    auto_group_addon(
                        "pack-1",
                        "Harbor Pack1",
                        json!({ "addoncontent_campaign": "1" }),
                        json!([]),
                    ),
                ),
                (
                    "pack-2".to_string(),
                    auto_group_addon(
                        "pack-2",
                        "Harbor Pack 2",
                        json!({ "addoncontent_map": "1" }),
                        json!([]),
                    ),
                ),
                (
                    "texture".to_string(),
                    auto_group_addon(
                        "texture",
                        "Harbor Texture",
                        json!({ "addoncontent_campaign": "1" }),
                        json!([]),
                    ),
                ),
            ]),
            ..Database::default()
        };

        auto_group_internal(&mut db);

        assert_eq!(db.groups.len(), 1);
        assert_eq!(db.groups[0].name, "Harbor");
        assert_eq!(db.groups[0].addons, vec!["base", "pack-1", "pack-2"]);
    }

    #[test]
    fn auto_group_prefers_addoninfo_title_over_workshop_title() {
        let mut first = auto_group_addon(
            "first",
            "Workshop Title One",
            json!({
                "addoncontent_campaign": "1",
                "addontitle": "Internal Campaign Part 1"
            }),
            json!([]),
        );
        let mut second = auto_group_addon(
            "second",
            "Workshop Title Two",
            json!({
                "addoncontent_campaign": "1",
                "addontitle": "Internal Campaign Part 2"
            }),
            json!([]),
        );
        first.steam_details = Some(json!({ "title": "Workshop Title One" }));
        second.steam_details = Some(json!({ "title": "Workshop Title Two" }));
        let mut db = Database {
            addons: HashMap::from([(first.id.clone(), first), (second.id.clone(), second)]),
            ..Database::default()
        };

        auto_group_internal(&mut db);

        assert_eq!(db.groups.len(), 1);
        assert_eq!(db.groups[0].name, "Internal Campaign");
        assert_eq!(db.groups[0].addons, vec!["first", "second"]);
    }

    #[test]
    fn auto_group_attaches_versioned_campaign_root_to_workshop_tagged_parts() {
        let root = auto_group_addon(
            "root",
            "Unrelated Workshop Title",
            json!({
                "addoncontent_campaign": "1",
                "addoncontent_map": "1",
                "addontitle": "广西-南宁 V1.15.1",
                "addonauthorsteamid": "HerobrineAce"
            }),
            json!([]),
        );
        let part_one = auto_group_addon(
            "part-one",
            "Another Workshop Title",
            json!({
                "addoncontent_campaign": "0",
                "addoncontent_map": "0",
                "addontitle": "广西-南宁 V1.15.1 Part 1",
                "addonauthorsteamid": "HerobrineAce"
            }),
            json!([{ "tag": "Campaigns" }]),
        );
        let part_two = auto_group_addon(
            "part-two",
            "Third Workshop Title",
            json!({
                "addoncontent_campaign": "0",
                "addoncontent_map": "0",
                "addontitle": "广西-南宁 V1.15.1 Part 2",
                "addonauthorsteamid": "HerobrineAce"
            }),
            json!([{ "tag": "Campaigns" }]),
        );
        let mut db = Database {
            addons: HashMap::from([
                (root.id.clone(), root),
                (part_one.id.clone(), part_one),
                (part_two.id.clone(), part_two),
            ]),
            ..Database::default()
        };

        auto_group_internal(&mut db);

        assert_eq!(db.groups.len(), 1);
        assert_eq!(db.groups[0].name, "广西-南宁");
        assert_eq!(db.groups[0].addons, vec!["part-one", "part-two", "root"]);
    }

    #[test]
    fn auto_group_rejects_part_groups_with_conflicting_known_authors() {
        let mut first = auto_group_addon(
            "first",
            "Conflict Part 1",
            json!({ "addoncontent_campaign": "1" }),
            json!([]),
        );
        first.steam_details = Some(json!({
            "title": "Conflict Part 1",
            "creator": "76561198000000001"
        }));
        let mut second = auto_group_addon(
            "second",
            "Conflict Part 2",
            json!({ "addoncontent_campaign": "1" }),
            json!([]),
        );
        second.steam_details = Some(json!({
            "title": "Conflict Part 2",
            "creator": "76561198000000002"
        }));
        let mut db = Database {
            addons: HashMap::from([(first.id.clone(), first), (second.id.clone(), second)]),
            ..Database::default()
        };

        auto_group_internal(&mut db);

        assert!(db.groups.is_empty());
    }

    #[test]
    fn auto_group_accepts_identical_titles_only_with_matching_author_and_description() {
        let mut first = auto_group_addon(
            "first",
            "Shared Campaign",
            json!({ "addoncontent_campaign": "1" }),
            json!([]),
        );
        first.steam_details = Some(json!({
            "title": "Shared Campaign",
            "creator": "76561198000000001",
            "description": "Two files for the same campaign"
        }));
        let mut second = first.clone();
        second.id = "second".to_string();
        second.vpk_name = "second.vpk".to_string();
        let mut unrelated = second.clone();
        unrelated.id = "unrelated".to_string();
        unrelated.vpk_name = "unrelated.vpk".to_string();
        unrelated.steam_details = Some(json!({
            "title": "Shared Campaign",
            "creator": "76561198000000002",
            "description": "Two files for the same campaign"
        }));
        let mut db = Database {
            addons: HashMap::from([
                (first.id.clone(), first),
                (second.id.clone(), second),
                (unrelated.id.clone(), unrelated),
            ]),
            ..Database::default()
        };

        auto_group_internal(&mut db);

        assert_eq!(db.groups.len(), 1);
        assert_eq!(db.groups[0].name, "Shared Campaign");
        assert_eq!(db.groups[0].addons, vec!["first", "second"]);
    }

    #[test]
    fn auto_group_attaches_campaign_root_to_identical_texture_parts() {
        let mut root = auto_group_addon(
            "root",
            "Workshop Root",
            json!({
                "addoncontent_campaign": "1",
                "addontitle": "Northshore"
            }),
            json!([]),
        );
        root.steam_details = Some(json!({
            "title": "Workshop Root",
            "creator": "76561198000000001",
            "description": "Campaign root package"
        }));

        let mut texture_one = auto_group_addon(
            "texture-one",
            "Workshop Texture One",
            json!({
                "addoncontent_campaign": "1",
                "addontitle": "Northshore Texture"
            }),
            json!([]),
        );
        texture_one.steam_details = Some(json!({
            "title": "Workshop Texture One",
            "creator": "76561198000000001",
            "description": "Shared texture package for Northshore"
        }));
        let mut texture_two = texture_one.clone();
        texture_two.id = "texture-two".to_string();
        texture_two.vpk_name = "texture-two.vpk".to_string();

        let mut foreign_root = root.clone();
        foreign_root.id = "foreign-root".to_string();
        foreign_root.vpk_name = "foreign-root.vpk".to_string();
        foreign_root.steam_details = Some(json!({
            "title": "Foreign Workshop Root",
            "creator": "76561198000000002",
            "description": "Another campaign with the same internal title"
        }));

        let mut db = Database {
            addons: HashMap::from([
                (root.id.clone(), root),
                (texture_one.id.clone(), texture_one),
                (texture_two.id.clone(), texture_two),
                (foreign_root.id.clone(), foreign_root),
            ]),
            ..Database::default()
        };

        auto_group_internal(&mut db);

        assert_eq!(db.groups.len(), 1);
        assert_eq!(db.groups[0].name, "Northshore");
        assert_eq!(
            db.groups[0].addons,
            vec!["root", "texture-one", "texture-two"]
        );
    }

    #[test]
    fn author_name_is_treated_as_a_placeholder() {
        assert!(looks_like_placeholder_author_name("AUTHOR_NAME", &[]));
        assert!(looks_like_placeholder_author_name("author_name", &[]));
        assert!(!looks_like_placeholder_author_name("Actual Author", &[]));
    }

    #[test]
    fn author_placeholder_is_removed_from_existing_cache_entries() {
        let mut cache = HashMap::from([(
            "123".to_string(),
            json!({
                "creatorName": "AUTHOR_NAME",
                "authorName": "AUTHOR_NAME",
                "creatorSteamId": "76561198000000001"
            }),
        )]);

        propagate_author_names(&mut cache);

        let entry = cache.get("123").unwrap();
        assert!(entry.get("creatorName").is_none());
        assert!(entry.get("authorName").is_none());
    }

    #[test]
    fn detail_sources_can_fetch_html_without_hybrid_mode() {
        let policy = SourcePolicy {
            allow_steamworks_sdk: true,
            allow_steam_web_api: true,
            allow_steam_community_html: true,
            sdk_html_scope: "disabled".to_string(),
            source_order: vec![
                "steamworks-sdk".to_string(),
                "steam-web-api".to_string(),
                "steamcommunity-html".to_string(),
            ],
        };

        assert!(policy.allow_html("addon-detail", true));
        assert!(policy.allow_html("workshop-detail", true));
        assert!(!policy.allow_html("workshop-home", true));
        assert!(!policy.allow_html("workshop-browse", true));
    }

    #[test]
    fn sdk_html_scope_controls_search_navigation_and_background_sources() {
        let search_policy = SourcePolicy {
            allow_steamworks_sdk: true,
            allow_steam_web_api: true,
            allow_steam_community_html: true,
            sdk_html_scope: "search".to_string(),
            source_order: vec![],
        };
        assert!(search_policy.allow_html("workshop-search", true));
        assert!(search_policy.allow_html("workshop-creator", true));
        assert!(!search_policy.allow_html("workshop-home", true));
        assert!(!search_policy.allow_html("background-refresh", true));

        let navigation_policy = SourcePolicy {
            sdk_html_scope: "navigation".to_string(),
            ..search_policy.clone()
        };
        assert!(navigation_policy.allow_html("workshop-home", true));
        assert!(navigation_policy.allow_html("workshop-browse", true));
        assert!(!navigation_policy.allow_html("startup-auto", true));

        let all_policy = SourcePolicy {
            sdk_html_scope: "all".to_string(),
            ..search_policy
        };
        assert!(all_policy.allow_html("startup-auto", true));
        assert!(all_policy.allow_html("background-refresh", true));
    }

    #[test]
    fn removes_only_dummy_workshop_targets() {
        let temp_dir = std::env::temp_dir().join(format!(
            "left4addons-{}-{}",
            "remove-dummy-workshop-targets",
            std::process::id()
        ));
        fs::remove_dir_all(&temp_dir).ok();
        fs::create_dir_all(&temp_dir).unwrap();

        let source_vpk = temp_dir.join("source.vpk");
        create_source_vpk(&source_vpk);

        let dummy_vpk = temp_dir.join("12345.vpk");
        let dummy_disabled_vpk = temp_dir.join("12345.vpk.disabled");
        let real_vpk = temp_dir.join("67890.vpk");

        generate_dummy_vpk(&source_vpk, &dummy_vpk).unwrap();
        generate_dummy_vpk(&source_vpk, &dummy_disabled_vpk).unwrap();
        fs::write(&real_vpk, b"real-addon").unwrap();

        remove_dummy_workshop_targets(&dummy_vpk).unwrap();

        assert!(!dummy_vpk.exists());
        assert!(!dummy_disabled_vpk.exists());
        assert!(real_vpk.exists());

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn background_fetch_only_allows_known_addons() {
        let path = write_known_addons_file(
            "allowed",
            json!({
                "12345": {
                    "id": "12345",
                    "vpkName": "12345.vpk",
                    "workshopId": "12345",
                    "addonInfo": {},
                    "hasImage": false,
                    "imagePath": null,
                    "steamDetails": null
                }
            }),
        );
        let groups_path = write_groups_file("allowed", json!([]));

        let allowed =
            ensure_background_workshop_fetch_allowed("startup-auto", "12345", &path, &groups_path);
        let blocked =
            ensure_background_workshop_fetch_allowed("startup-auto", "99999", &path, &groups_path);
        let manual = ensure_background_workshop_fetch_allowed(
            "workshop-detail",
            "99999",
            &path,
            &groups_path,
        );

        fs::remove_file(&path).ok();
        fs::remove_file(&groups_path).ok();

        assert!(allowed.is_ok());
        assert!(blocked.is_err());
        assert!(manual.is_ok());
    }

    #[test]
    fn normalizes_known_workshop_entries_to_raw_ids() {
        let path = write_known_addons_file(
            "normalize-workshop-id",
            json!({
                "12345.vpk": {
                    "id": "12345.vpk",
                    "vpkName": "12345.vpk",
                    "workshopId": "12345",
                    "addonInfo": {},
                    "hasImage": false,
                    "imagePath": null,
                    "steamDetails": null
                }
            }),
        );

        let entries = load_known_addons(&path);
        fs::remove_file(&path).ok();

        assert_eq!(entries.len(), 1);
        assert!(!entries.contains_key("12345.vpk"));
        assert_eq!(entries["12345"].id, "12345");
    }

    #[test]
    fn background_fetch_allows_known_collection_ids() {
        let known_addons_path = write_known_addons_file("collection-allowed", json!({}));
        let groups_path = write_groups_file(
            "collection-allowed",
            json!([
                {
                    "id": "group-1",
                    "name": "Known Collection",
                    "addons": [],
                    "workshopCollectionId": "54321",
                    "source": "workshop-import"
                }
            ]),
        );

        let allowed = ensure_background_workshop_fetch_allowed(
            "startup-auto",
            "54321",
            &known_addons_path,
            &groups_path,
        );

        fs::remove_file(&known_addons_path).ok();
        fs::remove_file(&groups_path).ok();

        assert!(allowed.is_ok());
    }

    #[test]
    fn extracts_friendly_workshop_error_message_from_html() {
        let html = r#"
            <div class="error_ctn">
              <div id="error_box_bottom">
                <div>
                  <h3>Too many requests, please try again later.</h3>
                </div>
              </div>
            </div>
        "#;

        let message = extract_steamcommunity_error_message(html);
        assert_eq!(
            message.as_deref(),
            Some("Too many requests, please try again later.")
        );
    }

    #[test]
    fn unknown_seen_items_do_not_persist_images_or_descriptions() {
        let mut obj = json!({
            "previewUrl": "https://example.com/old-preview.jpg",
            "imagePath": "https://example.com/old-preview.jpg",
            "shortDescription": "old description",
            "galleryPreviewUrls": ["https://example.com/old-gallery.jpg"],
            "description": "old page description"
        })
        .as_object()
        .cloned()
        .unwrap();

        persist_seen_workshop_item_entry(
            &mut obj,
            &WorkshopSeenItem {
                workshop_id: "99999".to_string(),
                title: "Unknown item".to_string(),
                image_path: "https://example.com/preview.jpg".to_string(),
                short_description: Some("new description".to_string()),
                gallery_preview_urls: Some(vec!["https://example.com/gallery.jpg".to_string()]),
                ..WorkshopSeenItem::default()
            },
            "workshop-browser",
            "2026-07-09T00:00:00Z",
            false,
        );

        assert_eq!(
            obj.get("title").and_then(|v| v.as_str()),
            Some("Unknown item")
        );
        assert!(obj.get("previewUrl").is_none());
        assert!(obj.get("imagePath").is_none());
        assert!(obj.get("shortDescription").is_none());
        assert!(obj.get("galleryPreviewUrls").is_none());
        assert!(obj.get("description").is_none());
    }

    #[test]
    fn known_seen_items_still_persist_preview_fields() {
        let mut obj = serde_json::Map::new();

        persist_seen_workshop_item_entry(
            &mut obj,
            &WorkshopSeenItem {
                workshop_id: "12345".to_string(),
                title: "Known item".to_string(),
                image_path: "https://example.com/preview.jpg".to_string(),
                short_description: Some("description".to_string()),
                gallery_preview_urls: Some(vec!["https://example.com/gallery.jpg".to_string()]),
                ..WorkshopSeenItem::default()
            },
            "workshop-browser",
            "2026-07-09T00:00:00Z",
            true,
        );

        assert_eq!(
            obj.get("previewUrl").and_then(|v| v.as_str()),
            Some("https://example.com/preview.jpg")
        );
        assert_eq!(
            obj.get("imagePath").and_then(|v| v.as_str()),
            Some("https://example.com/preview.jpg")
        );
        assert_eq!(
            obj.get("shortDescription").and_then(|v| v.as_str()),
            Some("description")
        );
        assert_eq!(
            obj.get("galleryPreviewUrls")
                .and_then(|v| v.as_array())
                .map(|v| v.len()),
            Some(1)
        );
    }

    #[test]
    fn unknown_page_details_do_not_persist_rich_content() {
        let mut obj = json!({
            "previewUrl": "https://example.com/old-preview.jpg",
            "imagePath": "https://example.com/old-preview.jpg",
            "description": "old description",
            "descriptionHtml": "<p>old</p>",
            "imageGallery": ["https://example.com/old-gallery.jpg"],
            "galleryUrls": ["https://example.com/old-gallery.jpg"],
            "backgroundImageUrl": "https://example.com/old-bg.jpg"
        })
        .as_object()
        .cloned()
        .unwrap();

        persist_workshop_page_details_entry(
            &mut obj,
            "99999",
            &json!({
                "title": "Unknown detail item",
                "previewUrl": "https://example.com/preview.jpg",
                "description": "new description",
                "descriptionHtml": "<p>new</p>",
                "imageGallery": ["https://example.com/gallery.jpg"],
                "backgroundImageUrl": "https://example.com/bg.jpg"
            }),
            "workshop-detail",
            "2026-07-09T00:00:00Z",
            false,
        );

        assert_eq!(
            obj.get("title").and_then(|v| v.as_str()),
            Some("Unknown detail item")
        );
        assert!(obj.get("previewUrl").is_none());
        assert!(obj.get("imagePath").is_none());
        assert!(obj.get("description").is_none());
        assert!(obj.get("descriptionHtml").is_none());
        assert!(obj.get("imageGallery").is_none());
        assert!(obj.get("galleryUrls").is_none());
        assert!(obj.get("backgroundImageUrl").is_none());
    }

    #[test]
    fn known_addons_are_exposed_as_workshop_snapshots() {
        let known_addons_path = write_known_addons_file(
            "snapshot",
            json!({
                "3560883926": {
                    "id": "3560883926",
                    "vpkName": "Early Days PART 1/6",
                    "workshopId": "3560883926",
                    "addonInfo": null,
                    "hasImage": true,
                    "imagePath": "https://example.com/preview.jpg",
                    "steamDetails": {
                        "publishedfileid": "3560883926",
                        "title": "Early Days PART 1/6",
                        "creator": "76561198012020581",
                        "description": "[b]UPDATE:[/b]\n\n[h2]Gameplay[/h2]",
                        "preview_url": "https://example.com/preview.jpg",
                        "tags": [{ "tag": "Campaigns" }]
                    }
                }
            }),
        );
        let cache_path = std::env::temp_dir().join(format!(
            "left4addons-{}-{}-workshop_cache.json",
            "snapshot",
            std::process::id()
        ));
        fs::remove_file(&cache_path).ok();

        let cache = workshop_cache_with_known_addons(&cache_path, &known_addons_path);
        let entry = cache.get("3560883926").unwrap();

        fs::remove_file(&known_addons_path).ok();
        fs::remove_file(&cache_path).ok();

        assert_eq!(
            entry.get("description").and_then(|v| v.as_str()),
            Some("[b]UPDATE:[/b]\n\n[h2]Gameplay[/h2]")
        );
        assert_eq!(
            entry.get("creatorSteamId").and_then(|v| v.as_str()),
            Some("76561198012020581")
        );
    }

    #[test]
    fn learned_author_name_updates_known_snapshots_with_matching_steam_id() {
        let known_addons_path = write_known_addons_file(
            "author-propagation",
            json!({
                "3560886114": {
                    "id": "3560886114",
                    "vpkName": "Early Days PART 2/6",
                    "workshopId": "3560886114",
                    "addonInfo": null,
                    "hasImage": false,
                    "imagePath": null,
                    "steamDetails": {
                        "publishedfileid": "3560886114",
                        "title": "Early Days PART 2/6",
                        "creator": "76561198012020581",
                        "creator_name": "76561198012020581"
                    }
                }
            }),
        );
        let mut cache = std::collections::HashMap::from([(
            "3560883926".to_string(),
            json!({
                "workshopId": "3560883926",
                "creatorName": "perfect_buddy",
                "creatorSteamId": "76561198012020581",
                "creatorAccountId": "51754853",
                "creatorProfileUrl": "https://steamcommunity.com/id/perfectbuddy"
            }),
        )]);

        merge_known_addon_snapshots_into_cache(&mut cache, &known_addons_path);
        propagate_author_names(&mut cache);

        fs::remove_file(&known_addons_path).ok();

        let related = cache.get("3560886114").unwrap();
        assert_eq!(
            related.get("creatorName").and_then(|v| v.as_str()),
            Some("perfect_buddy")
        );
        assert_eq!(
            related.get("creatorProfileUrl").and_then(|v| v.as_str()),
            Some("https://steamcommunity.com/id/perfectbuddy")
        );
    }

    #[test]
    fn workshop_cache_loader_skips_wrapper_keys_and_repairs_nested_items() {
        let cache_path = std::env::temp_dir().join(format!(
            "left4addons-{}-{}-workshop_cache.json",
            "nested-wrapper",
            std::process::id()
        ));
        fs::write(
            &cache_path,
            serde_json::to_string(&json!({
                "schemaVersion": 3,
                "authors": {
                    "76561198012020581": {
                        "name": "perfect_buddy",
                        "profileUrl": "https://steamcommunity.com/id/perfectbuddy",
                        "steamId": "76561198012020581",
                        "vanityId": "perfectbuddy"
                    }
                },
                "items": {
                    "3560883926": {
                        "workshopId": "3560883926",
                        "authorKey": "76561198012020581",
                        "description": "full description",
                        "imageGallery": ["https://example.com/1.jpg"]
                    },
                    "items": {
                        "3560886114": {
                            "workshopId": "3560886114",
                            "authorKey": "76561198012020581"
                        }
                    },
                    "schemaVersion": 2
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let cache = load_workshop_cache(&cache_path);
        fs::remove_file(&cache_path).ok();

        assert_eq!(cache.len(), 2);
        assert!(!cache.contains_key("items"));
        assert!(!cache.contains_key("schemaVersion"));
        let item = cache.get("3560883926").unwrap();
        assert_eq!(
            item.get("creatorName").and_then(|v| v.as_str()),
            Some("perfect_buddy")
        );
        assert_eq!(
            item.get("shortDescription").and_then(|v| v.as_str()),
            Some("full description")
        );
        assert_eq!(
            item.get("galleryUrls")
                .and_then(|v| v.as_array())
                .map(|v| v.len()),
            Some(1)
        );
    }

    #[test]
    fn workshop_cache_save_uses_author_directory_and_compacts_alias_fields() {
        let cache_path = std::env::temp_dir().join(format!(
            "left4addons-{}-{}-workshop_cache.json",
            "author-directory",
            std::process::id()
        ));
        fs::remove_file(&cache_path).ok();
        let cache = std::collections::HashMap::from([(
            "3560883926".to_string(),
            json!({
                "workshopId": "3560883926",
                "creatorName": "perfect_buddy",
                "authorName": "perfect_buddy",
                "creatorProfileUrl": "https://steamcommunity.com/id/perfectbuddy",
                "authorUrl": "https://steamcommunity.com/id/perfectbuddy",
                "creatorSteamId": "76561198012020581",
                "creatorVanityId": "perfectbuddy",
                "description": "full description",
                "shortDescription": "full description",
                "imageGallery": ["https://example.com/1.jpg"],
                "galleryUrls": ["https://example.com/1.jpg"]
            }),
        )]);

        save_workshop_cache(&cache_path, &cache).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&cache_path).unwrap()).unwrap();
        let item = saved
            .get("items")
            .and_then(|v| v.get("3560883926"))
            .and_then(|v| v.as_object())
            .unwrap();

        assert_eq!(saved.get("schemaVersion").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(
            saved
                .get("authors")
                .and_then(|v| v.get("76561198012020581"))
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str()),
            Some("perfect_buddy")
        );
        assert_eq!(
            item.get("authorKey").and_then(|v| v.as_str()),
            Some("76561198012020581")
        );
        assert!(item.get("creatorName").is_none());
        assert!(item.get("authorName").is_none());
        assert!(item.get("creatorProfileUrl").is_none());
        assert!(item.get("authorUrl").is_none());
        assert!(item.get("shortDescription").is_none());
        assert!(item.get("galleryUrls").is_none());

        let expanded = load_workshop_cache(&cache_path);
        fs::remove_file(&cache_path).ok();
        let expanded_item = expanded.get("3560883926").unwrap();
        assert_eq!(
            expanded_item.get("creatorName").and_then(|v| v.as_str()),
            Some("perfect_buddy")
        );
        assert_eq!(
            expanded_item
                .get("shortDescription")
                .and_then(|v| v.as_str()),
            Some("full description")
        );
        assert_eq!(
            expanded_item
                .get("galleryUrls")
                .and_then(|v| v.as_array())
                .map(|v| v.len()),
            Some(1)
        );
    }

    #[test]
    fn parse_content_range_start_extracts_start_and_total() {
        assert_eq!(
            parse_content_range_start("bytes 1024-2047/3492143"),
            Some((1024, 3492143))
        );
        assert_eq!(parse_content_range_start("invalid"), None);
    }

    #[test]
    fn settings_download_concurrency_defaults_to_two() {
        let settings: crate::commands::Settings =
            serde_json::from_value(json!({"workshopDir": "/w", "loadingDir": "/l"})).unwrap();
        assert_eq!(settings.download_concurrency, 2);
    }

    #[test]
    fn move_addons_skips_items_already_in_target_dir() {
        let addon = Addon {
            id: "local-addon".to_string(),
            vpk_name: "local-addon.vpk".to_string(),
            workshop_id: None,
            workshop_id_candidate: None,
            workshop_id_source: None,
            workshop_id_validation_status: None,
            workshop_id_last_attempt_at: None,
            addon_info: json!({}),
            has_image: false,
            image_path: None,
            files_count: 0,
            file_size: 0,
            parsed_at: String::new(),
            current_path: "/tmp/local-addon.vpk".to_string(),
            dir_type: "loading".to_string(),
            is_enabled: true,
            steam_details: None,
            workshop_details: None,
            is_dummy: false,
        };

        assert!(!move_requires_dir_change(&addon, "loading"));
        assert!(move_requires_dir_change(&addon, "workshop"));
    }

    #[test]
    fn toggle_addons_skips_items_already_in_requested_state() {
        let addon = Addon {
            id: "enabled-addon".to_string(),
            vpk_name: "enabled-addon.vpk".to_string(),
            workshop_id: None,
            workshop_id_candidate: None,
            workshop_id_source: None,
            workshop_id_validation_status: None,
            workshop_id_last_attempt_at: None,
            addon_info: json!({}),
            has_image: false,
            image_path: None,
            files_count: 0,
            file_size: 0,
            parsed_at: String::new(),
            current_path: "/tmp/enabled-addon.vpk".to_string(),
            dir_type: "workshop".to_string(),
            is_enabled: true,
            steam_details: None,
            workshop_details: None,
            is_dummy: false,
        };

        assert!(!toggle_requires_state_change(&addon, true));
        assert!(toggle_requires_state_change(&addon, false));
    }

    #[test]
    fn rename_addons_skips_items_already_using_requested_name() {
        let addon = Addon {
            id: "renamed-addon".to_string(),
            vpk_name: "[12345]Renamed Addon.vpk".to_string(),
            workshop_id: Some("12345".to_string()),
            workshop_id_candidate: None,
            workshop_id_source: None,
            workshop_id_validation_status: None,
            workshop_id_last_attempt_at: None,
            addon_info: json!({}),
            has_image: false,
            image_path: None,
            files_count: 0,
            file_size: 0,
            parsed_at: String::new(),
            current_path: "/tmp/[12345]Renamed Addon.vpk".to_string(),
            dir_type: "loading".to_string(),
            is_enabled: true,
            steam_details: None,
            workshop_details: None,
            is_dummy: false,
        };

        assert!(!rename_requires_name_change(
            &addon,
            "[12345]Renamed Addon.vpk"
        ));
        assert!(rename_requires_name_change(
            &addon,
            "[12345]Renamed Addon_1.vpk"
        ));
    }

    #[test]
    fn move_or_copy_file_falls_back_to_copy() {
        let temp_dir = std::env::temp_dir().join(format!(
            "left4addons-{}-{}-move-or-copy",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&temp_dir).unwrap();
        let src = temp_dir.join("source.tmp");
        let dst = temp_dir.join("nested").join("target.tmp");

        fs::write(&src, b"hello").unwrap();
        move_or_copy_file(&src, &dst).unwrap();

        assert!(!src.exists());
        assert_eq!(fs::read(&dst).unwrap(), b"hello");

        fs::remove_file(&dst).ok();
        fs::remove_dir_all(&temp_dir).ok();
    }
}
