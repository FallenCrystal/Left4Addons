use crate::kv::{flatten_kv_strings, parse_key_values};
use crate::reader::{parse_vpk, read_vpk_entry, VpkEntry};
use l4a_protocol::MapEntry;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct MissionMapInfo {
    pub code: String,
    pub name: String,
    pub image_hint: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AddonMetadata {
    pub addon_info: BTreeMap<String, String>,
    pub raw_addon_info: Value,
    pub has_image: bool,
    pub image_path: Option<String>,
    pub files_count: usize,
    pub content_hash: String,
    pub maps: Vec<MapEntry>,
    pub error: Option<String>,
}

pub fn extract_addon_metadata<P: AsRef<Path>, Q: AsRef<Path>>(
    vpk_path: P,
    cache_dir: Q,
) -> AddonMetadata {
    let mut result = AddonMetadata::default();
    let mut archive = match parse_vpk(&vpk_path) {
        Ok(archive) => archive,
        Err(err) => {
            result.error = Some(err.to_string());
            return result;
        }
    };

    result.files_count = archive.entries.len();

    let mut addoninfo_content = String::new();
    let addoninfo_key = archive
        .entries
        .keys()
        .find(|k| {
            let lower = k.to_ascii_lowercase();
            lower == "addoninfo.txt"
                || lower.ends_with("/addoninfo.txt")
                || lower.ends_with("\\addoninfo.txt")
        })
        .cloned();

    if let Some(key) = addoninfo_key {
        if let Some(entry) = archive.entries.get(&key) {
            if let Ok(bytes) = read_vpk_entry(&mut archive.file, entry) {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                let kv = parse_key_values(&text);
                result.addon_info = flatten_kv_strings(&kv);
                result.raw_addon_info = kv;
                addoninfo_content = text;
            }
        }
    }

    result.content_hash = compute_content_fingerprint(&archive.entries, &addoninfo_content);

    let file_stem_key = normalize_vpk_cache_stem(vpk_path.as_ref());
    let cover_hash = hex::encode(Sha256::digest(
        format!("{}:{}", file_stem_key, result.content_hash).as_bytes(),
    ));
    let cover_filename = format!("{}_cover.jpg", &cover_hash[..16]);

    let addonimage_raw_key = archive
        .entries
        .keys()
        .find(|k| {
            let lower = k.to_ascii_lowercase();
            lower == "addonimage.jpg"
                || lower.ends_with("/addonimage.jpg")
                || lower.ends_with("\\addonimage.jpg")
                || lower == "addonimage.jpeg"
                || lower.ends_with("/addonimage.jpeg")
                || lower.ends_with("\\addonimage.jpeg")
                || lower == "addonimage.png"
                || lower.ends_with("/addonimage.png")
                || lower.ends_with("\\addonimage.png")
        })
        .cloned();
    let addonimage_vtf_key = archive
        .entries
        .keys()
        .find(|k| {
            let lower = k.to_ascii_lowercase();
            lower == "addonimage.vtf"
                || lower.ends_with("/addonimage.vtf")
                || lower.ends_with("\\addonimage.vtf")
        })
        .cloned();

    let mut image_saved = false;
    if let Some(key) = addonimage_raw_key {
        if save_archive_image(
            &mut archive.file,
            &archive.entries,
            &key,
            cache_dir.as_ref(),
            &cover_filename,
        ) {
            result.has_image = true;
            result.image_path = Some(cover_filename.clone());
            image_saved = true;
        }
    }
    if !image_saved {
        if let Some(key) = addonimage_vtf_key {
            if save_archive_image(
                &mut archive.file,
                &archive.entries,
                &key,
                cache_dir.as_ref(),
                &cover_filename,
            ) {
                result.has_image = true;
                result.image_path = Some(cover_filename.clone());
                image_saved = true;
            }
        }
    }

    let mission_keys: Vec<String> = archive
        .entries
        .keys()
        .filter(|key| {
            let lower = key.to_ascii_lowercase();
            (lower.starts_with("missions/")
                || lower.contains("/missions/")
                || lower.contains("\\missions\\"))
                && lower.ends_with(".txt")
        })
        .cloned()
        .collect();

    let files_lower: HashMap<String, String> = archive
        .entries
        .keys()
        .map(|k| (k.to_ascii_lowercase(), k.clone()))
        .collect();

    let mut mission_maps: Vec<MissionMapInfo> = Vec::new();
    let mut cover_hint: Option<String> = None;
    let mut map_names: HashMap<String, String> = HashMap::new();

    for key in &mission_keys {
        if let Some(entry) = archive.entries.get(key) {
            if let Ok(bytes) = read_vpk_entry(&mut archive.file, entry) {
                let text = String::from_utf8_lossy(&bytes);
                let kv = parse_key_values(&text);
                extract_map_names_from_kv(&kv, &mut map_names);
                let (parsed_maps, hint) = extract_mission_info_from_kv(&kv);
                if cover_hint.is_none() {
                    cover_hint = hint;
                }
                for m in parsed_maps {
                    if !mission_maps
                        .iter()
                        .any(|x| x.code.eq_ignore_ascii_case(&m.code))
                    {
                        mission_maps.push(m);
                    }
                }
            }
        }
    }

    if !image_saved {
        if let Some(ref hint) = cover_hint {
            if let Some(image_key) = find_vpk_image_key(hint, &files_lower, &archive.entries) {
                if save_archive_image(
                    &mut archive.file,
                    &archive.entries,
                    &image_key,
                    cache_dir.as_ref(),
                    &cover_filename,
                ) {
                    result.has_image = true;
                    result.image_path = Some(cover_filename.clone());
                }
            }
        }
    }

    let mut maps = Vec::new();
    let mut added_codes = HashSet::new();

    for mm in mission_maps {
        let code_lower = mm.code.to_ascii_lowercase();
        added_codes.insert(code_lower.clone());

        let search_term = mm.image_hint.as_deref().unwrap_or(&mm.code);
        let image_key = find_vpk_image_key(search_term, &files_lower, &archive.entries)
            .or_else(|| find_vpk_image_key(&mm.code, &files_lower, &archive.entries));

        let map_image_path = image_key.and_then(|key| {
            let map_hash = hex::encode(Sha256::digest(
                format!("{}:{}:{}", file_stem_key, result.content_hash, code_lower).as_bytes(),
            ));
            let cache_name = format!("{}_map.jpg", &map_hash[..16]);
            save_archive_image(
                &mut archive.file,
                &archive.entries,
                &key,
                cache_dir.as_ref(),
                &cache_name,
            )
            .then_some(cache_name)
        });

        maps.push(MapEntry {
            code: mm.code,
            name: mm.name,
            image_path: map_image_path,
            image_hint: mm.image_hint,
        });
    }

    let mut bsp_keys: Vec<String> = archive
        .entries
        .keys()
        .filter(|key| {
            let lower = key.to_ascii_lowercase();
            (lower.starts_with("maps/") || lower.contains("/maps/") || lower.contains("\\maps\\"))
                && lower.ends_with(".bsp")
        })
        .cloned()
        .collect();
    bsp_keys.sort();

    for key in bsp_keys {
        let stem = Path::new(&key)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if stem.is_empty() {
            continue;
        }
        let stem_lower = stem.to_ascii_lowercase();
        if added_codes.insert(stem_lower.clone()) {
            let display_name = map_names
                .get(&stem_lower)
                .cloned()
                .unwrap_or_else(|| stem.clone());
            let image_key = find_vpk_image_key(&stem_lower, &files_lower, &archive.entries);
            let map_image_path = image_key.and_then(|img_key| {
                let map_hash = hex::encode(Sha256::digest(
                    format!("{}:{}:{}", file_stem_key, result.content_hash, stem_lower).as_bytes(),
                ));
                let cache_name = format!("{}_map.jpg", &map_hash[..16]);
                save_archive_image(
                    &mut archive.file,
                    &archive.entries,
                    &img_key,
                    cache_dir.as_ref(),
                    &cache_name,
                )
                .then_some(cache_name)
            });

            maps.push(MapEntry {
                code: stem,
                name: display_name,
                image_path: map_image_path,
                image_hint: None,
            });
        }
    }

    result.maps = maps;
    result
}

/// 基于 VPK 内部文件清单（路径、CRC32、长度）与 `addoninfo.txt` 内容生成稳定 SHA-256 指纹。
///
/// 相比 v1 仅 hash 路径+addoninfo，引入每项 `crc` 与 `entry_length` 确保 VPK 内容更新时指纹必然改变。
pub fn compute_content_fingerprint(
    entries: &BTreeMap<String, VpkEntry>,
    addoninfo_text: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(addoninfo_text.as_bytes());
    hasher.update([0u8]);
    for (path, entry) in entries {
        hasher.update(path.as_bytes());
        hasher.update(entry.crc.to_le_bytes());
        hasher.update(entry.entry_length.to_le_bytes());
    }
    hex::encode(hasher.finalize())
}

fn normalize_vpk_cache_stem(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .trim_end_matches(".disabled")
        .trim_end_matches(".vpk")
        .to_string()
}

fn save_archive_image(
    file: &mut File,
    entries: &BTreeMap<String, VpkEntry>,
    entry_key: &str,
    cache_dir: &Path,
    cache_filename: &str,
) -> bool {
    let Some(entry) = entries.get(entry_key) else {
        return false;
    };
    let Ok(bytes) = read_vpk_entry(file, entry) else {
        return false;
    };

    if std::fs::create_dir_all(cache_dir).is_err() {
        return false;
    }
    let dest = cache_dir.join(cache_filename);

    if entry_key.to_ascii_lowercase().ends_with(".vtf") {
        let Ok(vtf_img) = vtf::from_bytes(&bytes) else {
            return false;
        };
        let Ok(decoded) = vtf_img.highres_image.decode(0) else {
            return false;
        };
        decoded
            .save_with_format(&dest, image::ImageFormat::Jpeg)
            .is_ok()
    } else {
        let Ok(mut out) = File::create(&dest) else {
            return false;
        };
        out.write_all(&bytes).is_ok()
    }
}

fn extract_map_names_from_kv(val: &Value, map_names: &mut HashMap<String, String>) {
    if let Some(obj) = val.as_object() {
        if let (Some(map_val), Some(name_val)) = (
            obj.get("map"),
            obj.get("displayname").or_else(|| obj.get("name")),
        ) {
            if let (Some(map_str), Some(name_str)) = (map_val.as_str(), name_val.as_str()) {
                map_names.insert(map_str.to_ascii_lowercase(), name_str.to_string());
            }
        }
        for sub_val in obj.values() {
            extract_map_names_from_kv(sub_val, map_names);
        }
    } else if let Some(arr) = val.as_array() {
        for sub_val in arr {
            extract_map_names_from_kv(sub_val, map_names);
        }
    }
}

pub fn extract_mission_info_from_kv(kv: &Value) -> (Vec<MissionMapInfo>, Option<String>) {
    let mut maps = Vec::new();
    let mut cover_hint = None;
    let mut seen_codes = HashSet::new();

    if let Some(obj) = kv.as_object() {
        if let Some(img) = obj.get("image").and_then(Value::as_str) {
            if !img.trim().is_empty() {
                cover_hint = Some(img.trim().to_string());
            }
        }
        if cover_hint.is_none() {
            if let Some(poster) = obj.get("poster").and_then(Value::as_object) {
                if let Some(pimg) = poster
                    .get("posterimage_widescreen")
                    .or_else(|| poster.get("posterimage"))
                    .and_then(Value::as_str)
                {
                    if !pimg.trim().is_empty() {
                        cover_hint = Some(pimg.trim().to_string());
                    }
                }
            }
        }
        if cover_hint.is_none() {
            if let Some(outro) = obj
                .get("outtroimage")
                .or_else(|| obj.get("outroimage"))
                .and_then(Value::as_str)
            {
                if !outro.trim().is_empty() {
                    cover_hint = Some(outro.trim().to_string());
                }
            }
        }

        if let Some(modes) = obj.get("modes").and_then(Value::as_object) {
            let mut mode_keys: Vec<&String> = modes.keys().collect();
            mode_keys.sort_by(|a, b| {
                let a_lower = a.to_ascii_lowercase();
                let b_lower = b.to_ascii_lowercase();
                match (a_lower == "coop", b_lower == "coop") {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.cmp(b),
                }
            });

            for mode_key in mode_keys {
                if let Some(mode_obj) = modes.get(mode_key).and_then(Value::as_object) {
                    let mut step_keys: Vec<&String> = mode_obj.keys().collect();
                    step_keys.sort_by(|a, b| {
                        match (a.parse::<u32>().ok(), b.parse::<u32>().ok()) {
                            (Some(na), Some(nb)) => na.cmp(&nb),
                            _ => a.cmp(b),
                        }
                    });

                    for step_key in step_keys {
                        if let Some(map_obj) = mode_obj.get(step_key).and_then(Value::as_object) {
                            if let Some(map_code) = map_obj
                                .get("map")
                                .or_else(|| map_obj.get("mapname"))
                                .and_then(Value::as_str)
                            {
                                let code = map_code.trim().to_string();
                                if !code.is_empty() && seen_codes.insert(code.to_ascii_lowercase()) {
                                    let name = map_obj
                                        .get("displayname")
                                        .or_else(|| map_obj.get("name"))
                                        .and_then(Value::as_str)
                                        .unwrap_or(&code)
                                        .trim()
                                        .to_string();
                                    let image_hint = map_obj
                                        .get("image")
                                        .and_then(Value::as_str)
                                        .map(|s| s.trim().to_string())
                                        .filter(|s| !s.is_empty());

                                    maps.push(MissionMapInfo {
                                        code: code.clone(),
                                        name: if name.is_empty() { code } else { name },
                                        image_hint,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if maps.is_empty() {
        extract_maps_recursive(kv, &mut maps, &mut seen_codes);
    }

    if cover_hint.is_none() {
        cover_hint = maps.first().and_then(|m| m.image_hint.clone());
    }

    (maps, cover_hint)
}

fn extract_maps_recursive(
    val: &Value,
    maps: &mut Vec<MissionMapInfo>,
    seen_codes: &mut HashSet<String>,
) {
    if let Some(obj) = val.as_object() {
        if let Some(map_code) = obj
            .get("map")
            .or_else(|| obj.get("mapname"))
            .and_then(Value::as_str)
        {
            let code = map_code.trim().to_string();
            if !code.is_empty() && seen_codes.insert(code.to_ascii_lowercase()) {
                let name = obj
                    .get("displayname")
                    .or_else(|| obj.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or(&code)
                    .trim()
                    .to_string();
                let image_hint = obj
                    .get("image")
                    .and_then(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());

                maps.push(MissionMapInfo {
                    code: code.clone(),
                    name: if name.is_empty() { code } else { name },
                    image_hint,
                });
            }
        }
        for sub_val in obj.values() {
            extract_maps_recursive(sub_val, maps, seen_codes);
        }
    } else if let Some(arr) = val.as_array() {
        for sub_val in arr {
            extract_maps_recursive(sub_val, maps, seen_codes);
        }
    }
}

pub fn find_vpk_image_key(
    hint_or_stem: &str,
    files_lower: &HashMap<String, String>,
    files: &BTreeMap<String, VpkEntry>,
) -> Option<String> {
    let clean = hint_or_stem
        .replace('\\', "/")
        .trim()
        .trim_matches('"')
        .to_ascii_lowercase();

    if clean.is_empty() {
        return None;
    }

    let mut candidates = Vec::new();
    let has_ext = clean.ends_with(".vtf")
        || clean.ends_with(".jpg")
        || clean.ends_with(".jpeg")
        || clean.ends_with(".png");

    if has_ext {
        candidates.push(clean.clone());
        candidates.push(format!("materials/{clean}"));
        candidates.push(format!("materials/vgui/{clean}"));
    } else {
        let exts = [".vtf", ".jpg", ".jpeg", ".png"];
        let mut bases = vec![
            clean.clone(),
            format!("materials/{clean}"),
            format!("materials/vgui/{clean}"),
        ];
        if !clean.starts_with("maps/") {
            bases.push(format!("materials/vgui/maps/{clean}"));
        }
        if !clean.starts_with("loading_screen_") && !clean.starts_with("maps/") {
            bases.push(format!("materials/vgui/loading_screen_{clean}"));
        }
        for base in bases {
            for ext in exts {
                candidates.push(format!("{base}{ext}"));
            }
        }
    }

    for c in &candidates {
        if let Some(k) = files_lower.get(c) {
            return Some(k.clone());
        }
        let win_c = c.replace('/', "\\");
        if let Some(k) = files_lower.get(&win_c) {
            return Some(k.clone());
        }
        let suffix_win = format!("\\{win_c}");
        let suffix_unix = format!("/{c}");
        if let Some((k, _)) = files.iter().find(|(k, _)| {
            let kl = k.to_ascii_lowercase();
            kl.ends_with(&suffix_win) || kl.ends_with(&suffix_unix)
        }) {
            return Some(k.clone());
        }
    }

    None
}
