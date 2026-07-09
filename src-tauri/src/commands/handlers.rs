use crate::steam::{
    fetch_collection_children_web, fetch_steam_details_web, BridgeDownloadStatus,
    WorkshopBrowseQuery, WorkshopCapabilities, WorkshopCollectionResponse, WorkshopHomeResponse,
    WorkshopItemResponse, WorkshopItemsResponse, WorkshopService,
};
use crate::vpk::{extract_addon_metadata, generate_dummy_vpk};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

use super::types::{
    is_dummy_addon_info, Addon, Database, Group, KnownAddonEntry, MasterCollection, RenameItem,
    Settings, SettingsStore, WorkshopSeenItem, WorkshopSourceSettings,
};

const DOWNLOAD_CANCELLED_ERR: &str = "Download cancelled";
const WORKSHOP_HTML_FETCH_INTERVAL: Duration = Duration::from_secs(6);
const WORKSHOP_HTML_FETCH_PAUSE_DURATION: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
struct WorkshopHtmlFetchGate {
    next_allowed_at: Option<Instant>,
    pause_until: Option<Instant>,
    pause_reason: Option<String>,
}

static WORKSHOP_HTML_FETCH_GATE: OnceLock<Mutex<WorkshopHtmlFetchGate>> = OnceLock::new();

#[derive(Debug, Clone)]
struct SourcePolicy {
    preset: String,
    allow_steamworks_sdk: bool,
    allow_steam_web_api: bool,
    allow_steam_community_html: bool,
    allow_sdk_html_hybrid: bool,
    source_order: Vec<String>,
}

impl SourcePolicy {
    fn from_settings(settings: &Settings) -> Self {
        let configured = &settings.workshop_source_settings;
        let preset = configured.preset.trim().to_string();
        let disable_sdk = settings.disable_steamworks_sdk;

        match preset.as_str() {
            "offline" => Self {
                preset,
                allow_steamworks_sdk: false,
                allow_steam_web_api: false,
                allow_steam_community_html: false,
                allow_sdk_html_hybrid: false,
                source_order: configured.source_order.clone(),
            },
            "sdk-only" | "sdkOnly" => Self {
                preset: "sdk-only".to_string(),
                allow_steamworks_sdk: !disable_sdk && configured.allow_steamworks_sdk,
                allow_steam_web_api: false,
                allow_steam_community_html: false,
                allow_sdk_html_hybrid: false,
                source_order: configured.source_order.clone(),
            },
            "hybrid" => Self {
                preset,
                allow_steamworks_sdk: !disable_sdk && configured.allow_steamworks_sdk,
                allow_steam_web_api: configured.allow_steam_web_api,
                allow_steam_community_html: configured.allow_steam_community_html,
                allow_sdk_html_hybrid: true,
                source_order: configured.source_order.clone(),
            },
            _ => Self {
                preset: "conservative".to_string(),
                allow_steamworks_sdk: !disable_sdk && configured.allow_steamworks_sdk,
                allow_steam_web_api: configured.allow_steam_web_api,
                allow_steam_community_html: configured.allow_steam_community_html,
                allow_sdk_html_hybrid: configured.allow_sdk_html_hybrid,
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

    fn allow_html(&self, sdk_query_available: bool) -> bool {
        if !self.allow_steam_community_html {
            return false;
        }
        if self.preset == "hybrid" || self.allow_sdk_html_hybrid {
            return true;
        }
        !sdk_query_available
    }
}

fn source_position(source_order: &[String], source: &str, fallback: usize) -> usize {
    source_order
        .iter()
        .position(|item| item == source)
        .unwrap_or(fallback)
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
    fs::rename(&temp_dest_path, &dest_path).map_err(|e| {
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
    if known_addons.contains_key(workshop_id)
        || known_addons.contains_key(&format!("{}.vpk", workshop_id))
    {
        return true;
    }

    if is_known_group_collection_id(groups_path, workshop_id) {
        return true;
    }

    known_addons.values().any(|entry| {
        entry.id == workshop_id
            || entry.vpk_name == format!("{}.vpk", workshop_id)
            || entry.workshop_id.as_deref() == Some(workshop_id)
    })
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
                return parsed
                    .into_iter()
                    .filter(|(_, entry)| !is_dummy_addon_info(&entry.addon_info))
                    .collect();
            }
        }
    }
    HashMap::new()
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
                let id = addon.id.clone();
                migrated.insert(
                    id.clone(),
                    KnownAddonEntry {
                        id,
                        vpk_name: addon.vpk_name.clone(),
                        workshop_id: addon.workshop_id.clone(),
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
            enable_dummy_bypass: false,
            suppress_sdk_unavailable_warning: false,
            disable_steamworks_sdk: false,
            force_steamworks_sdk_download: false,
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

    let db = Database {
        settings: settings_store.settings,
        addons: merged_addons,
        groups,
        known_uninstalled_addons,
        master_collections: settings_store.master_collections,
    };

    if !settings_existed || !groups_existed || !known_addons_existed {
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

    if learned.is_empty() {
        return;
    }

    for value in cache.values_mut() {
        let Some(obj) = value.as_object_mut() else {
            continue;
        };
        let ids = author_identity_values(obj);
        if ids.is_empty() {
            continue;
        }
        let current_name = obj
            .get("creatorName")
            .or_else(|| obj.get("authorName"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        for (name, url, learned_ids) in &learned {
            if ids.is_disjoint(learned_ids) {
                continue;
            }
            let ids_vec = ids.iter().cloned().collect::<Vec<_>>();
            if looks_like_placeholder_author_name(current_name, &ids_vec) {
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
    if name.is_empty() || name.chars().all(|c| c.is_ascii_digit()) {
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
            };
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

pub async fn scan_addons_internal(
    db: &mut Database,
    settings_path: &Path,
    groups_path: &Path,
    known_addons_path: &Path,
    cache_dir: &Path,
    _workshop_service: &WorkshopService,
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
    let mut new_workshop_ids = HashSet::new();

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

            let mut workshop_id = None;
            let base_name = Path::new(&vpk_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if base_name.chars().all(|c| c.is_ascii_digit()) {
                workshop_id = Some(base_name.to_string());
            }

            let meta = extract_addon_metadata(&file_info.full_path, cache_dir);

            // Fallback: extract workshop ID from url keys inside addon_info (all keys are lowercase now)
            if workshop_id.is_none() {
                if let Some(url_val) = meta
                    .addon_info
                    .get("addonurl0")
                    .or_else(|| meta.addon_info.get("addonurl"))
                {
                    if let Some(url_str) = url_val.as_str() {
                        workshop_id = extract_workshop_id_from_url(url_str);
                    }
                }
            }

            let is_dummy = meta
                .addon_info
                .get("addondescription")
                .and_then(|v| v.as_str())
                .map(|v| v == "A dummy addon generated by Left 4 Addons")
                .unwrap_or(false);

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
                is_dummy,
            };

            if let Some(ref w_id) = workshop_id {
                if !is_dummy {
                    new_workshop_ids.insert(w_id.clone());
                }
            }

            if !is_dummy {
                known_addons.insert(
                    id.clone(),
                    KnownAddonEntry {
                        id: id.clone(),
                        vpk_name: vpk_name.clone(),
                        workshop_id: workshop_id.clone(),
                        addon_info: addon.addon_info.clone(),
                        has_image: addon.has_image,
                        image_path: addon.image_path.clone(),
                        steam_details: addon.steam_details.clone(),
                    },
                );
            }

            active_addons.insert(id.clone(), addon);
        } else if let Some(mut addon) = cached {
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
                                addon_info: addon.addon_info.clone(),
                                has_image: addon.has_image,
                                image_path: addon.image_path.clone(),
                                steam_details: addon.steam_details.clone(),
                            },
                        );
                    }
                }
            }

            addon.file_size = file_info.size;
            addon.current_path = file_info.full_path.to_string_lossy().to_string();
            addon.dir_type = file_info.dir_type;
            addon.is_enabled = file_info.is_enabled;
            addon.is_dummy = addon
                .addon_info
                .get("addondescription")
                .and_then(|v| v.as_str())
                .map(|v| v == "A dummy addon generated by Left 4 Addons")
                .unwrap_or(false);

            if addon.workshop_id.is_none() {
                if let Some(url_val) = addon
                    .addon_info
                    .get("addonurl0")
                    .or_else(|| addon.addon_info.get("addonurl"))
                {
                    if let Some(url_str) = url_val.as_str() {
                        addon.workshop_id = extract_workshop_id_from_url(url_str);
                        if let Some(ref w_id) = addon.workshop_id {
                            if !addon.is_dummy {
                                new_workshop_ids.insert(w_id.clone());
                            }
                        }
                    }
                }
            }

            let id = addon.id.clone();
            active_addons.insert(id, addon);
        }
    }

    drop(new_workshop_ids);

    let mut uninstalled = HashMap::new();
    for (id, entry) in &known_addons {
        if !active_addons.contains_key(id) {
            uninstalled.insert(
                id.clone(),
                Addon {
                    id: id.clone(),
                    vpk_name: entry.vpk_name.clone(),
                    workshop_id: entry.workshop_id.clone(),
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
        if !active_addons.contains_key(id) && !uninstalled.contains_key(id) {
            uninstalled.insert(id.clone(), addon.clone());
        }
    }

    db.addons = active_addons;
    db.known_uninstalled_addons = uninstalled;

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
    db.groups.retain(|g| !g.addons.is_empty());

    save_db_internal(settings_path, groups_path, known_addons_path, db);
    Ok(())
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

    let legacy_key = format!("{}.vpk", workshop_id);
    let existing = known_addons
        .get(workshop_id)
        .cloned()
        .or_else(|| known_addons.get(&legacy_key).cloned());

    known_addons.remove(&legacy_key);

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

fn auto_group_internal(db: &mut Database) {
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
        description: String,
    }

    let mut candidates = Vec::new();
    for addon in ungrouped {
        let title = addon
            .steam_details
            .as_ref()
            .and_then(|d| d.get("title").and_then(|t| t.as_str()))
            .or_else(|| addon.addon_info.get("addontitle").and_then(|t| t.as_str()))
            .unwrap_or(&addon.vpk_name)
            .to_string();

        let description = addon
            .addon_info
            .get("addondescription")
            .and_then(|t| t.as_str())
            .or_else(|| {
                addon
                    .addon_info
                    .get("addontagline")
                    .and_then(|t| t.as_str())
            })
            .or_else(|| {
                addon
                    .steam_details
                    .as_ref()
                    .and_then(|d| d.get("description").and_then(|t| t.as_str()))
            })
            .unwrap_or("")
            .to_string();

        candidates.push(Candidate {
            id: addon.id.clone(),
            title,
            description,
        });
    }

    let mut desc_groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, c) in candidates.iter().enumerate() {
        let desc_trim = c.description.trim();
        if desc_trim.len() > 10 {
            let key = desc_trim.to_lowercase();
            desc_groups.entry(key).or_default().push(i);
        }
    }

    let mut indices_to_remove = HashSet::new();

    for (_desc, idxs) in desc_groups {
        if idxs.len() >= 2 {
            let mut common_prefix = candidates[idxs[0]].title.clone();
            for &idx in &idxs[1..] {
                let current_title = &candidates[idx].title;
                let mut common_len = 0;
                for (c1, c2) in common_prefix.chars().zip(current_title.chars()) {
                    if c1 == c2 {
                        common_len += c1.len_utf8();
                    } else {
                        break;
                    }
                }
                common_prefix.truncate(common_len);
            }

            let mut group_name = clean_group_name(&common_prefix);
            if group_name.is_empty() {
                group_name = "Campaign Pack".to_string();
            }

            let group_id = format!(
                "group_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );

            let mut addons = Vec::new();
            for &idx in &idxs {
                addons.push(candidates[idx].id.clone());
                indices_to_remove.insert(idx);
            }

            db.groups.push(Group {
                id: group_id,
                name: group_name,
                addons,
                tags: None,
                workshop_collection_id: None,
                master_collection_ids: None,
                source: Some("auto-group".to_string()),
            });
        }
    }

    let mut remaining_candidates = Vec::new();
    for (i, c) in candidates.into_iter().enumerate() {
        if !indices_to_remove.contains(&i) {
            remaining_candidates.push(c);
        }
    }

    let re_part = Regex::new(r"(?i)^(.*?)\s*(?:[-#_]*\s*(?:part|pt|partie|pts)\s*(\d+|[ivxldcm]+)(?:\/\d+)?|\s+v?\d+\.\d+|\s+v\d+)$").unwrap();
    let mut title_groups: HashMap<String, Vec<String>> = HashMap::new();
    for c in remaining_candidates {
        if let Some(caps) = re_part.captures(&c.title) {
            let prefix = caps.get(1).unwrap().as_str().trim().to_string();
            if prefix.len() >= 3 {
                let cleaned_prefix = clean_group_name(&prefix);
                if cleaned_prefix.len() >= 3 {
                    title_groups.entry(cleaned_prefix).or_default().push(c.id);
                }
            }
        }
    }

    for (prefix, addons) in title_groups {
        if addons.len() >= 2 {
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
}

#[tauri::command]
pub async fn get_settings(state: State<'_, crate::AppState>) -> Result<Settings, String> {
    let db = state.db.lock().await;
    Ok(db.settings.clone())
}

#[tauri::command]
pub async fn save_settings(
    loading_dir: String,
    enable_dummy_bypass: bool,
    suppress_sdk_unavailable_warning: bool,
    disable_steamworks_sdk: bool,
    force_steamworks_sdk_download: bool,
    workshop_source_settings: Option<WorkshopSourceSettings>,
    state: State<'_, crate::AppState>,
    app_handle: AppHandle,
) -> Result<Database, String> {
    let mut db = state.db.lock().await;
    let loading_path = PathBuf::from(&loading_dir);
    let workshop_dir = loading_path.join("workshop").to_string_lossy().to_string();

    db.settings.workshop_dir = workshop_dir;
    db.settings.loading_dir = loading_dir;
    db.settings.enable_dummy_bypass = enable_dummy_bypass;
    db.settings.suppress_sdk_unavailable_warning = suppress_sdk_unavailable_warning;
    db.settings.disable_steamworks_sdk = disable_steamworks_sdk;
    db.settings.force_steamworks_sdk_download = force_steamworks_sdk_download;
    if let Some(workshop_source_settings) = workshop_source_settings {
        db.settings.workshop_source_settings = workshop_source_settings;
    }
    if disable_steamworks_sdk {
        state.workshop_service.shutdown();
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
    if let Err(err) = crate::watcher::rebind_addon_watcher(&app_handle, &loading_path) {
        crate::watcher::emit_watch_error(&app_handle, &err);
    }
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
        &state.known_addons_path,
    ))
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

#[tauri::command]
pub async fn get_cache_image(
    image_path: String,
    state: State<'_, crate::AppState>,
) -> Result<Vec<u8>, String> {
    let file_path = resolve_cache_file(&state.cache_dir, &image_path)?;
    fs::read(&file_path).map_err(|e| format!("Failed to read cache image: {}", e))
}

#[tauri::command]
pub async fn cache_remote_image(
    url: String,
    state: State<'_, crate::AppState>,
) -> Result<String, String> {
    let parsed = validate_open_url(&url)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Only HTTP(S) images can be cached".to_string());
    }

    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(parsed.as_str().as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    if let Some(cached_path) = find_cached_remote_image(&state.cache_dir, &hash) {
        return Ok(cached_path);
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let response = client
        .get(parsed.clone())
        .send()
        .await
        .map_err(|e| format!("Failed to fetch remote image: {}", e))?;
    if !response.status().is_success() {
        return Err(format!(
            "Remote image responded with status {}",
            response.status()
        ));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
    let extension = cache_image_extension(content_type.as_deref(), &parsed);
    let filename = format!("{}_remote_image.{}", hash, extension);
    let file_path = state.cache_dir.join(&filename);
    if file_path.exists() {
        return Ok(format!("/cache/{}", filename));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read remote image: {}", e))?;
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create image cache directory: {}", e))?;
    }
    fs::write(&file_path, &bytes).map_err(|e| format!("Failed to cache remote image: {}", e))?;
    Ok(format!("/cache/{}", filename))
}

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
            .is_none_or(|details| !is_collection_detail(details))
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
            source_policy.allow_html(sdk_query_available),
            sdk_query_available,
        )
    };
    if !allow_html {
        let err = if sdk_query_available {
            "Steam Community HTML fetching is disabled while Steamworks SDK is available. Enable SDK + Steam Community hybrid crawling in settings to allow it.".to_string()
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
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
        .map_err(|e| e.to_string())?;

    let res = match client.get(parsed).send().await {
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
        let dest_filename = format!("{}.vpk", workshop_id);
        let dest_path = workshop_dir.join(&dest_filename);
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
        let tmp_path = workshop_dir.join(format!("{}.download", dest_filename));
        if tmp_path.exists() {
            fs::remove_file(&tmp_path)
                .map_err(|e| format!("Failed to remove stale download file: {}", e))?;
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
        let mut response = client
            .get(&file_url)
            .send()
            .await
            .map_err(|e| format!("Download request failed: {}", e))?;
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

        let total_size = response.content_length().unwrap_or(0);
        let mut file = fs::File::create(&tmp_path)
            .map_err(|e| format!("Failed to create local file: {}", e))?;
        let mut downloaded = 0;

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
                fs::remove_file(&tmp_path).ok();
                return Err(DOWNLOAD_CANCELLED_ERR.to_string());
            }

            use std::io::Write;
            file.write_all(&chunk)
                .map_err(|e| format!("Failed to write chunk: {}", e))?;
            downloaded += chunk.len() as u64;

            let percent = if total_size > 0 {
                (downloaded as f64 / total_size as f64 * 100.0) as u32
            } else {
                0
            };

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
            fs::remove_file(&tmp_path).ok();
            return Err(DOWNLOAD_CANCELLED_ERR.to_string());
        }

        crate::watcher::suppress_internal_refresh(&state);
        fs::rename(&tmp_path, &dest_path)
            .map_err(|e| format!("Failed to finalize downloaded file: {}", e))?;

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
    let db = state.db.lock().await;
    Ok(database_with_workshop_cache(
        &db,
        &state.workshop_cache_path,
        &state.known_addons_path,
    ))
}

#[tauri::command]
pub async fn get_background_tasks(
    state: State<'_, crate::AppState>,
) -> Result<serde_json::Value, String> {
    if !state.background_tasks_path.exists() {
        return Ok(serde_json::json!([]));
    }
    let content = fs::read_to_string(&state.background_tasks_path)
        .map_err(|e| format!("Failed to read background tasks: {}", e))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse background tasks: {}", e))
}

#[tauri::command]
pub async fn save_background_task_snapshot(
    tasks: serde_json::Value,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    if let Some(parent) = state.background_tasks_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create task directory: {}", e))?;
    }
    let json = serde_json::to_string_pretty(&tasks)
        .map_err(|e| format!("Failed to serialize background tasks: {}", e))?;
    fs::write(&state.background_tasks_path, json)
        .map_err(|e| format!("Failed to write background tasks: {}", e))
}

#[tauri::command]
pub async fn cancel_download(
    workshop_id: String,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    request_download_cancellation(&state, &workshop_id)
}

#[tauri::command]
pub async fn append_workshop_crawl_log(
    record: serde_json::Value,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    append_workshop_crawl_log_internal(&state.workshop_crawl_log_path, record)
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_background_workshop_fetch_allowed, extract_steamcommunity_error_message,
        is_background_workshop_fetch_source, load_workshop_cache,
        merge_known_addon_snapshots_into_cache, persist_seen_workshop_item_entry,
        persist_workshop_page_details_entry, propagate_author_names, remove_dummy_workshop_targets,
        save_workshop_cache, workshop_cache_with_known_addons,
    };
    use crate::commands::types::WorkshopSeenItem;
    use crate::vpk::generate_dummy_vpk;
    use serde_json::json;
    use std::fs;

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
        assert!(cache.get("items").is_none());
        assert!(cache.get("schemaVersion").is_none());
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
}
