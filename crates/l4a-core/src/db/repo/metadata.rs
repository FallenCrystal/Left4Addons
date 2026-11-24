use l4a_protocol::{AddonSummary, MapEntry};
use std::collections::BTreeMap;

pub fn serialize_enum<T: serde::Serialize>(val: &T) -> String {
    serde_json::to_string(val)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string()
}

pub fn parse_enum<T: serde::de::DeserializeOwned>(raw: &str, fallback: T) -> T {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).unwrap_or(fallback)
}

pub(super) fn strip_vpk_file_stem(raw_name: &str) -> String {
    let trimmed = raw_name.trim();
    let lower = trimmed.to_ascii_lowercase();
    let cut = if lower.ends_with(".vpk.disabled") {
        trimmed.len() - ".vpk.disabled".len()
    } else if lower.ends_with(".disabled") {
        trimmed.len() - ".disabled".len()
    } else if lower.ends_with(".vpk") {
        trimmed.len() - ".vpk".len()
    } else {
        trimmed.len()
    };
    trimmed[..cut].trim().to_string()
}

pub fn clean_addon_title(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "addon"
            | "addon_title"
            | "addontitle"
            | "my addon"
            | "my add-on"
            | "my first addon"
            | "my first add-on"
            | "untitled"
            | "none"
            | "n/a"
    ) {
        return None;
    }
    if let Some(rest) = lower.strip_prefix("workshop #") {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    Some(trimmed.to_string())
}

pub fn clean_workshop_title(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("workshop #") {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    Some(trimmed.to_string())
}

pub fn clean_author_name(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    if matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "author_name" | "addonauthor" | "unknown" | "unknown author" | "none" | "n/a"
    ) {
        return None;
    }
    Some(trimmed.to_string())
}

pub fn derive_categories_from_info(addon_info: &BTreeMap<String, String>) -> Vec<String> {
    let flag = |keys: &[&str]| -> bool {
        keys.iter().any(|k| {
            addon_info
                .get(&k.to_ascii_lowercase())
                .is_some_and(|v| v.trim() == "1" || v.eq_ignore_ascii_case("true"))
        })
    };
    let mut cats = Vec::new();
    if flag(&["addoncontent_campaign"]) {
        cats.push("Campaign".into());
    }
    if flag(&["addoncontent_map"]) {
        cats.push("Map".into());
    }
    if flag(&["addoncontent_survivor"]) {
        cats.push("Survivor".into());
    }
    if flag(&[
        "addoncontent_weaponmodel",
        "content_weaponmodel",
        "content_weapon",
    ]) {
        cats.push("Weapon".into());
    }
    if flag(&["addoncontent_skin"]) {
        cats.push("Skin".into());
    }
    if flag(&["addoncontent_script"]) {
        cats.push("Script".into());
    }
    if flag(&["addoncontent_music", "addoncontent_sound"]) {
        cats.push("Audio".into());
    }
    if flag(&["addoncontent_bossinfected", "addoncontent_commoninfected"]) {
        cats.push("Infected".into());
    }
    if flag(&["addoncontent_ui"]) {
        cats.push("UI".into());
    }
    cats
}

pub fn merge_categories(
    existing: &[String],
    addon_info: &BTreeMap<String, String>,
    maps: &[MapEntry],
    workshop_tags: &[String],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push_unique = |cat: &str| {
        if !out.iter().any(|c| c == cat) {
            out.push(cat.to_string());
        }
    };

    for cat in existing {
        if !cat.trim().is_empty() {
            push_unique(cat.trim());
        }
    }
    for cat in derive_categories_from_info(addon_info) {
        push_unique(&cat);
    }
    if !maps.is_empty() {
        if maps.len() >= 2 {
            push_unique("Campaign");
        }
        push_unique("Map");
    }
    for raw_tag in workshop_tags {
        let lower = raw_tag.trim().to_ascii_lowercase();
        match lower.as_str() {
            "campaigns" | "campaign" => push_unique("Campaign"),
            "maps" | "map" => push_unique("Map"),
            "survivors" | "survivor" => push_unique("Survivor"),
            "weapons" | "weapon" => push_unique("Weapon"),
            "skins" | "skin" => push_unique("Skin"),
            "scripts" | "script" => push_unique("Script"),
            "sounds" | "sound" | "music" | "audio" => push_unique("Audio"),
            "infected" | "common infected" | "special infected" | "tank" | "witch" | "hunter"
            | "boomer" | "smoker" | "charger" | "jockey" | "spitter" => push_unique("Infected"),
            "ui" | "hud" => push_unique("UI"),
            _ => {}
        }
    }
    out
}

pub fn collect_installed_addon_maps(addon: &AddonSummary) -> Vec<MapEntry> {
    addon.maps.clone()
}

pub fn is_remote_image_url(raw: &str) -> bool {
    let bytes = raw.trim().as_bytes();
    bytes
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case(b"http://"))
        || bytes
            .get(..8)
            .is_some_and(|p| p.eq_ignore_ascii_case(b"https://"))
        || bytes.starts_with(b"//")
}

pub fn normalize_valid_remote_cover_url(raw: &str) -> Option<String> {
    let normalized = l4a_workshop::normalize_workshop_url(raw)?;
    let parsed = url::Url::parse(&normalized).ok()?;
    if matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
    {
        Some(parsed.into())
    } else {
        None
    }
}

pub fn is_valid_local_cover_key(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with('.') || trimmed.contains("..") {
        return false;
    }
    if !trimmed
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return false;
    }
    matches!(
        trimmed.rsplit_once('.').map(|(_, ext)| ext),
        Some("jpg" | "jpeg" | "png" | "webp")
    )
}

pub(super) fn local_cover_for_db(addon: &AddonSummary) -> Option<&str> {
    addon
        .local_cover_image
        .as_deref()
        .map(str::trim)
        .filter(|c| is_valid_local_cover_key(c))
        .or_else(|| {
            addon
                .cover_image
                .as_deref()
                .map(str::trim)
                .filter(|c| is_valid_local_cover_key(c))
        })
}
