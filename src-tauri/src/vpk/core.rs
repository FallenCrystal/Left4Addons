use md5::{Digest, Md5};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::types::{AddonMetadata, VpkEntry, VpkFileToWrite};

pub(crate) fn read_string(buf: &[u8], offset: &mut usize) -> String {
    let start = *offset;
    while *offset < buf.len() && buf[*offset] != 0 {
        *offset += 1;
    }
    let s = String::from_utf8_lossy(&buf[start..*offset]).into_owned();
    *offset += 1; // skip null terminator
    s
}

pub fn parse_vpk<P: AsRef<Path>>(
    file_path: P,
) -> Result<(HashMap<String, VpkEntry>, File), String> {
    let mut file = File::open(&file_path).map_err(|e| e.to_string())?;
    let mut header_buf = vec![0; 28];
    file.read_exact(&mut header_buf)
        .map_err(|e| e.to_string())?;

    let signature = u32::from_le_bytes(header_buf[0..4].try_into().unwrap());
    if signature != 0x55aa1234 {
        return Err("Not a VPK file".to_string());
    }

    let version = u32::from_le_bytes(header_buf[4..8].try_into().unwrap());
    let tree_size: u32;
    let header_size: u32;

    if version == 1 {
        tree_size = u32::from_le_bytes(header_buf[8..12].try_into().unwrap());
        header_size = 12;
    } else if version == 2 {
        tree_size = u32::from_le_bytes(header_buf[8..12].try_into().unwrap());
        header_size = 28;
    } else {
        return Err(format!("Unsupported VPK version: {}", version));
    }

    file.seek(SeekFrom::Start(header_size as u64))
        .map_err(|e| e.to_string())?;
    let mut tree_buf = vec![0; tree_size as usize];
    file.read_exact(&mut tree_buf).map_err(|e| e.to_string())?;

    let mut offset = 0;
    let mut files = HashMap::new();

    while offset < tree_buf.len() {
        let ext = read_string(&tree_buf, &mut offset);
        if ext.is_empty() {
            break;
        }

        while offset < tree_buf.len() {
            let path_str = read_string(&tree_buf, &mut offset);
            if path_str.is_empty() {
                break;
            }

            while offset < tree_buf.len() {
                let filename = read_string(&tree_buf, &mut offset);
                if filename.is_empty() {
                    break;
                }

                if offset + 18 > tree_buf.len() {
                    break;
                }

                let entry_slice = &tree_buf[offset..offset + 18];
                offset += 18;

                let crc = u32::from_le_bytes(entry_slice[0..4].try_into().unwrap());
                let preload_bytes = u16::from_le_bytes(entry_slice[4..6].try_into().unwrap());
                let archive_index = u16::from_le_bytes(entry_slice[6..8].try_into().unwrap());
                let entry_offset = u32::from_le_bytes(entry_slice[8..12].try_into().unwrap());
                let entry_length = u32::from_le_bytes(entry_slice[12..16].try_into().unwrap());
                let _terminator = u16::from_le_bytes(entry_slice[16..18].try_into().unwrap());

                let mut preload_data = Vec::new();
                if preload_bytes > 0 && offset + preload_bytes as usize <= tree_buf.len() {
                    preload_data
                        .extend_from_slice(&tree_buf[offset..offset + preload_bytes as usize]);
                    offset += preload_bytes as usize;
                }

                let norm_path = path_str.trim();
                let full_path = if norm_path.is_empty() {
                    format!("{}.{}", filename, ext)
                } else {
                    format!("{}/{}.{}", norm_path, filename, ext)
                };

                files.insert(
                    full_path,
                    VpkEntry {
                        crc,
                        preload_bytes,
                        archive_index,
                        entry_offset,
                        entry_length,
                        preload_data,
                        header_size,
                        tree_size,
                    },
                );
            }
        }
    }

    Ok((files, file))
}

pub fn get_file_content(file: &mut File, entry: &VpkEntry) -> std::io::Result<Vec<u8>> {
    let mut data = entry.preload_data.clone();
    if entry.entry_length > 0 {
        let file_offset =
            entry.header_size as u64 + entry.tree_size as u64 + entry.entry_offset as u64;
        file.seek(SeekFrom::Start(file_offset))?;
        let mut file_buf = vec![0; entry.entry_length as usize];
        file.read_exact(&mut file_buf)?;
        data.extend(file_buf);
    }
    Ok(data)
}

pub fn parse_key_values(text: &str) -> serde_json::Value {
    let mut clean_text = String::new();
    for line in text.lines() {
        let mut clean_line = String::new();
        let mut chars = line.chars().peekable();
        let mut in_quote = false;
        let mut escaped = false;

        while let Some(c) = chars.next() {
            if escaped {
                clean_line.push(c);
                escaped = false;
                continue;
            }
            if c == '\\' {
                clean_line.push(c);
                escaped = true;
                continue;
            }
            if c == '"' {
                in_quote = !in_quote;
                clean_line.push(c);
                continue;
            }
            if !in_quote && c == '/' && chars.peek() == Some(&'/') {
                let preceded = clean_line.ends_with(':') || clean_line.ends_with('"');
                if !preceded {
                    break;
                }
            }
            clean_line.push(c);
        }
        clean_text.push_str(&clean_line);
        clean_text.push('\n');
    }

    let mut tokens = Vec::new();
    let mut chars = clean_text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '{' || c == '}' {
            tokens.push(c.to_string());
            chars.next();
            continue;
        }
        if c == '"' {
            let mut s = String::new();
            s.push(chars.next().unwrap());
            let mut escaped = false;
            for nc in chars.by_ref() {
                s.push(nc);
                if escaped {
                    escaped = false;
                } else if nc == '\\' {
                    escaped = true;
                } else if nc == '"' {
                    break;
                }
            }
            tokens.push(s);
        } else {
            let mut s = String::new();
            while let Some(&nc) = chars.peek() {
                if nc.is_whitespace() || nc == '{' || nc == '}' || nc == '"' {
                    break;
                }
                s.push(chars.next().unwrap());
            }
            if !s.is_empty() {
                tokens.push(s);
            }
        }
    }

    fn clean_token(t: &str) -> String {
        if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
            let inner = &t[1..t.len() - 1];
            inner.replace("\\\"", "\"").replace("\\\\", "\\")
        } else {
            t.to_string()
        }
    }

    let mut index = 0;

    fn parse_object(tokens: &[String], index: &mut usize) -> HashMap<String, serde_json::Value> {
        let mut obj = HashMap::new();
        while *index < tokens.len() {
            let tok = &tokens[*index];
            if tok == "}" {
                *index += 1;
                return obj;
            }
            if tok == "{" {
                *index += 1;
                continue;
            }

            let key = clean_token(tok).to_lowercase();
            *index += 1;

            if *index >= tokens.len() {
                break;
            }

            let next_tok = &tokens[*index];
            if next_tok == "{" {
                *index += 1;
                let sub_obj = parse_object(tokens, index);
                obj.insert(key, serde_json::to_value(sub_obj).unwrap());
            } else if next_tok == "}" {
                obj.insert(key, serde_json::Value::String(String::new()));
            } else {
                let val = clean_token(next_tok);
                obj.insert(key, serde_json::Value::String(val));
                *index += 1;
            }
        }
        obj
    }

    while index < tokens.len() {
        let tok = &tokens[index];
        index += 1;
        if tok == "{" {
            return serde_json::to_value(parse_object(&tokens, &mut index))
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        } else if index < tokens.len() && tokens[index] == "{" {
            index += 1;
            return serde_json::to_value(parse_object(&tokens, &mut index))
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        }
    }

    serde_json::Value::Object(serde_json::Map::new())
}

pub fn extract_addon_metadata<P: AsRef<Path>, Q: AsRef<Path>>(
    vpk_path: P,
    cache_dir: Q,
) -> AddonMetadata {
    let mut result = AddonMetadata::default();

    let (files, mut file) = match parse_vpk(&vpk_path) {
        Ok(val) => val,
        Err(err) => {
            result.error = Some(err);
            return result;
        }
    };

    result.files_count = files.len();

    // Find addoninfo.txt
    let mut addoninfo_content = String::new();
    let addoninfo_key = files.keys().find(|k| {
        let lower = k.to_lowercase();
        lower == "addoninfo.txt"
            || lower.ends_with("/addoninfo.txt")
            || lower.ends_with("\\addoninfo.txt")
    });
    if let Some(key) = addoninfo_key {
        if let Some(entry) = files.get(key) {
            if let Ok(content_bytes) = get_file_content(&mut file, entry) {
                let text = String::from_utf8_lossy(&content_bytes);
                result.addon_info = parse_key_values(&text);
                addoninfo_content = text.to_string();
            }
        }
    }

    // Fast hash based on addoninfo and directory structure
    let mut hasher = Md5::new();
    hasher.update(addoninfo_content.as_bytes());
    let mut paths: Vec<&String> = files.keys().collect();
    paths.sort();
    for p in paths {
        hasher.update(p.as_bytes());
    }
    result.hash = format!("{:x}", hasher.finalize());

    // Find addonimage.jpg or addonimage.vtf
    let addonimage_jpg_key = files.keys().find(|k| {
        let lower = k.to_lowercase();
        lower == "addonimage.jpg"
            || lower.ends_with("/addonimage.jpg")
            || lower.ends_with("\\addonimage.jpg")
            || lower == "addonimage.jpeg"
            || lower.ends_with("/addonimage.jpeg")
            || lower.ends_with("\\addonimage.jpeg")
            || lower == "addonimage.png"
            || lower.ends_with("/addonimage.png")
            || lower.ends_with("\\addonimage.png")
    });
    let addonimage_vtf_key = files.keys().find(|k| {
        let lower = k.to_lowercase();
        lower == "addonimage.vtf"
            || lower.ends_with("/addonimage.vtf")
            || lower.ends_with("\\addonimage.vtf")
    });

    let vpk_name = vpk_path
        .as_ref()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let clean_vpk_name = vpk_name.replace(".disabled", "").replace(".vpk", "");

    let mut hasher = Md5::new();
    hasher.update(clean_vpk_name.as_bytes());
    let hash_result = hasher.finalize();

    let cache_filename = format!("{:x}_image.jpg", hash_result);
    let full_cache_path = cache_dir.as_ref().join(&cache_filename);
    let mut image_saved = false;

    if let Some(key) = addonimage_jpg_key {
        if let Some(entry) = files.get(key) {
            if let Ok(image_buf) = get_file_content(&mut file, entry) {
                if !cache_dir.as_ref().exists() {
                    let _ = std::fs::create_dir_all(&cache_dir);
                }

                if let Ok(mut cache_file) = File::create(&full_cache_path) {
                    if cache_file.write_all(&image_buf).is_ok() {
                        result.has_image = true;
                        result.image_path = Some(format!("/cache/{}", cache_filename));
                        image_saved = true;
                    }
                }
            }
        }
    }

    if !image_saved {
        if let Some(key) = addonimage_vtf_key {
            if let Some(entry) = files.get(key) {
                if let Ok(vtf_bytes) = get_file_content(&mut file, entry) {
                    if !cache_dir.as_ref().exists() {
                        let _ = std::fs::create_dir_all(&cache_dir);
                    }
                    // Try to decode VTF to JPG
                    if let Ok(vtf) = vtf::from_bytes(&vtf_bytes) {
                        if let Ok(decoded) = vtf.highres_image.decode(0) {
                            if decoded
                                .save_with_format(&full_cache_path, image::ImageFormat::Jpeg)
                                .is_ok()
                            {
                                result.has_image = true;
                                result.image_path = Some(format!("/cache/{}", cache_filename));
                                image_saved = true;
                            }
                        }
                    }
                }
            }
        }
    }

    // Find all mission files under missions/
    let mut mission_keys = Vec::new();
    for key in files.keys() {
        let lower = key.to_lowercase();
        if (lower.starts_with("missions/") || lower.contains("/missions/") || lower.contains("\\missions\\"))
            && lower.ends_with(".txt")
        {
            mission_keys.push(key);
        }
    }

    let files_lower: HashMap<String, &String> = files.keys().map(|k| (k.to_lowercase(), k)).collect();

    // Parse missions to get structured map entries and cover image hint
    let mut mission_maps: Vec<MissionMapInfo> = Vec::new();
    let mut cover_hint: Option<String> = None;
    let mut map_names: HashMap<String, String> = HashMap::new();

    for key in &mission_keys {
        if let Some(entry) = files.get(*key) {
            if let Ok(content_bytes) = get_file_content(&mut file, entry) {
                let text = String::from_utf8_lossy(&content_bytes);
                let kv = parse_key_values(&text);
                extract_map_names_from_kv(&kv, &mut map_names);
                let (parsed_maps, hint) = extract_mission_info_from_kv(&kv);
                if cover_hint.is_none() {
                    cover_hint = hint;
                }
                for m in parsed_maps {
                    if !mission_maps.iter().any(|x| x.code.eq_ignore_ascii_case(&m.code)) {
                        mission_maps.push(m);
                    }
                }
            }
        }
    }

    // Try resolving cover image from cover_hint if addonimage was not found
    if !image_saved {
        if let Some(ref hint) = cover_hint {
            if let Some(k) = find_vpk_image_key(hint, &files_lower, &files) {
                if let Some(entry) = files.get(k) {
                    if let Ok(img_bytes) = get_file_content(&mut file, entry) {
                        if !cache_dir.as_ref().exists() {
                            let _ = std::fs::create_dir_all(&cache_dir);
                        }
                        if k.to_lowercase().ends_with(".vtf") {
                            if let Ok(vtf) = vtf::from_bytes(&img_bytes) {
                                if let Ok(decoded) = vtf.highres_image.decode(0) {
                                    if decoded
                                        .save_with_format(&full_cache_path, image::ImageFormat::Jpeg)
                                        .is_ok()
                                    {
                                        result.has_image = true;
                                        result.image_path = Some(format!("/cache/{}", cache_filename));
                                    }
                                }
                            }
                        } else {
                            if let Ok(mut cache_file) = File::create(&full_cache_path) {
                                if cache_file.write_all(&img_bytes).is_ok() {
                                    result.has_image = true;
                                    result.image_path = Some(format!("/cache/{}", cache_filename));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Build map list
    let mut maps = Vec::new();
    let mut added_codes = std::collections::HashSet::new();

    // 1. Add maps defined in missions/*.txt
    for mm in mission_maps {
        let code_lower = mm.code.to_lowercase();
        added_codes.insert(code_lower.clone());

        let search_term = mm.image_hint.as_deref().unwrap_or(&mm.code);
        let image_key = find_vpk_image_key(search_term, &files_lower, &files)
            .or_else(|| find_vpk_image_key(&mm.code, &files_lower, &files));

        let mut map_image_path = None;
        if let Some(k) = image_key {
            if let Some(entry) = files.get(k) {
                if let Ok(img_bytes) = get_file_content(&mut file, entry) {
                    let mut hasher = Md5::new();
                    hasher.update(clean_vpk_name.as_bytes());
                    hasher.update(code_lower.as_bytes());
                    let map_hash = format!("{:x}", hasher.finalize());

                    let cache_filename = format!("{}_map.jpg", map_hash);
                    let full_cache_path = cache_dir.as_ref().join(&cache_filename);

                    if !cache_dir.as_ref().exists() {
                        let _ = std::fs::create_dir_all(&cache_dir);
                    }

                    if k.to_lowercase().ends_with(".vtf") {
                        if let Ok(vtf) = vtf::from_bytes(&img_bytes) {
                            if let Ok(decoded) = vtf.highres_image.decode(0) {
                                if decoded
                                    .save_with_format(&full_cache_path, image::ImageFormat::Jpeg)
                                    .is_ok()
                                {
                                    map_image_path = Some(format!("/cache/{}", cache_filename));
                                }
                            }
                        }
                    } else {
                        if let Ok(mut cache_file) = File::create(&full_cache_path) {
                            if cache_file.write_all(&img_bytes).is_ok() {
                                map_image_path = Some(format!("/cache/{}", cache_filename));
                            }
                        }
                    }
                }
            }
        }

        maps.push(super::types::MapEntry {
            code: mm.code,
            name: mm.name,
            image: map_image_path,
            image_hint: mm.image_hint,
        });
    }

    // 2. Append any extra .bsp files under maps/ that were not in missions/*.txt
    let mut bsp_keys = Vec::new();
    for key in files.keys() {
        let lower = key.to_lowercase();
        if (lower.starts_with("maps/") || lower.contains("/maps/") || lower.contains("\\maps\\"))
            && lower.ends_with(".bsp")
        {
            bsp_keys.push(key);
        }
    }
    bsp_keys.sort();

    for key in bsp_keys {
        let stem = Path::new(key)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        let stem_lower = stem.to_lowercase();
        if !added_codes.contains(&stem_lower) {
            added_codes.insert(stem_lower.clone());
            let display_name = map_names.get(&stem_lower).cloned().unwrap_or_else(|| stem.clone());

            let image_key = find_vpk_image_key(&stem_lower, &files_lower, &files);
            let mut map_image_path = None;

            if let Some(k) = image_key {
                if let Some(entry) = files.get(k) {
                    if let Ok(img_bytes) = get_file_content(&mut file, entry) {
                        let mut hasher = Md5::new();
                        hasher.update(clean_vpk_name.as_bytes());
                        hasher.update(stem_lower.as_bytes());
                        let map_hash = format!("{:x}", hasher.finalize());

                        let cache_filename = format!("{}_map.jpg", map_hash);
                        let full_cache_path = cache_dir.as_ref().join(&cache_filename);

                        if !cache_dir.as_ref().exists() {
                            let _ = std::fs::create_dir_all(&cache_dir);
                        }

                        if k.to_lowercase().ends_with(".vtf") {
                            if let Ok(vtf) = vtf::from_bytes(&img_bytes) {
                                if let Ok(decoded) = vtf.highres_image.decode(0) {
                                    if decoded
                                        .save_with_format(&full_cache_path, image::ImageFormat::Jpeg)
                                        .is_ok()
                                    {
                                        map_image_path = Some(format!("/cache/{}", cache_filename));
                                    }
                                }
                            }
                        } else {
                            if let Ok(mut cache_file) = File::create(&full_cache_path) {
                                if cache_file.write_all(&img_bytes).is_ok() {
                                    map_image_path = Some(format!("/cache/{}", cache_filename));
                                }
                            }
                        }
                    }
                }
            }

            maps.push(super::types::MapEntry {
                code: stem,
                name: display_name,
                image: map_image_path,
                image_hint: None,
            });
        }
    }

    result.maps = maps;
    result.maps_scanned = Some(true);

    result
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffffffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            if (c & 1) != 0 {
                c = (c >> 1) ^ 0xedb88320;
            } else {
                c >>= 1;
            }
        }
    }
    !c
}

pub fn write_vpk<P: AsRef<Path>>(file_path: P, files: &[VpkFileToWrite]) -> Result<(), String> {
    use std::io::Write;
    let mut file = File::create(file_path).map_err(|e| e.to_string())?;

    let mut ext_map: HashMap<String, HashMap<String, Vec<&VpkFileToWrite>>> = HashMap::new();
    for f in files {
        ext_map
            .entry(f.ext.clone())
            .or_default()
            .entry(f.path.clone())
            .or_default()
            .push(f);
    }

    let mut tree_buf = Vec::new();
    let mut data_offset = 0u32;
    let mut files_to_write_data = Vec::new();

    for (ext, paths) in &ext_map {
        let ext_str = if ext.is_empty() { " " } else { ext };
        tree_buf.extend_from_slice(ext_str.as_bytes());
        tree_buf.push(0);

        for (path, file_list) in paths {
            let path_str = if path.is_empty() { " " } else { path };
            tree_buf.extend_from_slice(path_str.as_bytes());
            tree_buf.push(0);

            for f in file_list {
                let filename_str = if f.filename.is_empty() {
                    " "
                } else {
                    &f.filename
                };
                tree_buf.extend_from_slice(filename_str.as_bytes());
                tree_buf.push(0);

                let crc_val = crc32(&f.content);
                let entry_length = f.content.len() as u32;

                tree_buf.extend_from_slice(&crc_val.to_le_bytes());
                tree_buf.extend_from_slice(&0u16.to_le_bytes());
                tree_buf.extend_from_slice(&0x7fffu16.to_le_bytes());
                tree_buf.extend_from_slice(&data_offset.to_le_bytes());
                tree_buf.extend_from_slice(&entry_length.to_le_bytes());
                tree_buf.extend_from_slice(&0xffffu16.to_le_bytes());

                data_offset += entry_length;
                files_to_write_data.push(&f.content);
            }
            tree_buf.push(0);
        }
        tree_buf.push(0);
    }
    tree_buf.push(0);

    let tree_size = tree_buf.len() as u32;

    file.write_all(&0x55aa1234u32.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&1u32.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&tree_size.to_le_bytes())
        .map_err(|e| e.to_string())?;

    file.write_all(&tree_buf).map_err(|e| e.to_string())?;

    for content in files_to_write_data {
        file.write_all(content).map_err(|e| e.to_string())?;
    }

    Ok(())
}

pub fn generate_dummy_vpk<P: AsRef<Path>, Q: AsRef<Path>>(
    original_vpk_path: P,
    dummy_vpk_path: Q,
) -> Result<(), String> {
    let (files, mut file) = parse_vpk(&original_vpk_path)?;

    let addoninfo_key = files.keys().find(|k| {
        let lower = k.to_lowercase();
        lower == "addoninfo.txt"
            || lower.ends_with("/addoninfo.txt")
            || lower.ends_with("\\addoninfo.txt")
    });

    let mut steam_app_id = "550".to_string();
    let mut addon_version = "1.0".to_string();
    let mut addon_title = original_vpk_path
        .as_ref()
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .replace(".disabled", "")
        .replace(".vpk", "");

    if let Some(key) = addoninfo_key {
        if let Some(entry) = files.get(key) {
            if let Ok(content_bytes) = get_file_content(&mut file, entry) {
                let text = String::from_utf8_lossy(&content_bytes);
                let parsed = parse_key_values(&text);
                if let serde_json::Value::Object(map) = parsed {
                    if let Some(v) = map.get("addonsteamappid").and_then(|v| v.as_str()) {
                        steam_app_id = v.to_string();
                    } else if let Some(v) = map.get("addonsteamappid").and_then(|v| v.as_number()) {
                        steam_app_id = v.to_string();
                    }
                    if let Some(v) = map.get("addonversion").and_then(|v| v.as_str()) {
                        addon_version = v.to_string();
                    } else if let Some(v) = map.get("addonversion").and_then(|v| v.as_number()) {
                        addon_version = v.to_string();
                    }
                    if let Some(v) = map.get("addontitle").and_then(|v| v.as_str()) {
                        addon_title = v.to_string();
                    }
                }
            }
        }
    }

    let mut files_to_write = Vec::new();

    // Find and extract addonimage.jpg
    let addonimage_jpg_key = files.keys().find(|k| {
        let lower = k.to_lowercase();
        lower == "addonimage.jpg"
            || lower.ends_with("/addonimage.jpg")
            || lower.ends_with("\\addonimage.jpg")
    });
    if let Some(key) = addonimage_jpg_key {
        if let Some(entry) = files.get(key) {
            if let Ok(content_bytes) = get_file_content(&mut file, entry) {
                files_to_write.push(VpkFileToWrite {
                    ext: "jpg".to_string(),
                    path: "".to_string(),
                    filename: "addonimage".to_string(),
                    content: content_bytes,
                });
            }
        }
    }

    // Find and extract addonimage.vtf
    let addonimage_vtf_key = files.keys().find(|k| {
        let lower = k.to_lowercase();
        lower == "addonimage.vtf"
            || lower.ends_with("/addonimage.vtf")
            || lower.ends_with("\\addonimage.vtf")
    });
    if let Some(key) = addonimage_vtf_key {
        if let Some(entry) = files.get(key) {
            if let Ok(content_bytes) = get_file_content(&mut file, entry) {
                files_to_write.push(VpkFileToWrite {
                    ext: "vtf".to_string(),
                    path: "".to_string(),
                    filename: "addonimage".to_string(),
                    content: content_bytes,
                });
            }
        }
    }

    let addoninfo_content = format!(
        r#""AddonInfo"
{{
    "addonSteamAppID" "{}"
    "addonversion" "{}"
    "addontitle" "{} (L4A Dummy)"
    "addonDescription" "A dummy addon generated by Left 4 Addons"
    "l4aDummy" "1"
}}
"#,
        steam_app_id, addon_version, addon_title
    );

    files_to_write.push(VpkFileToWrite {
        ext: "txt".to_string(),
        path: "".to_string(),
        filename: "addoninfo".to_string(),
        content: addoninfo_content.into_bytes(),
    });

    write_vpk(dummy_vpk_path, &files_to_write)?;

    Ok(())
}

fn extract_map_names_from_kv(val: &serde_json::Value, map_names: &mut HashMap<String, String>) {
    if let Some(obj) = val.as_object() {
        if let (Some(map_val), Some(name_val)) = (
            obj.get("map"),
            obj.get("displayname").or_else(|| obj.get("name")),
        ) {
            if let (Some(map_str), Some(name_str)) = (map_val.as_str(), name_val.as_str()) {
                map_names.insert(map_str.to_lowercase(), name_str.to_string());
            }
        }
        for (_, sub_val) in obj {
            extract_map_names_from_kv(sub_val, map_names);
        }
    } else if let Some(arr) = val.as_array() {
        for sub_val in arr {
            extract_map_names_from_kv(sub_val, map_names);
        }
    }
}

#[derive(Debug, Clone)]
pub struct MissionMapInfo {
    pub code: String,
    pub name: String,
    pub image_hint: Option<String>,
}

pub fn extract_mission_info_from_kv(kv: &serde_json::Value) -> (Vec<MissionMapInfo>, Option<String>) {
    let mut maps = Vec::new();
    let mut cover_hint = None;
    let mut seen_codes = std::collections::HashSet::new();

    if let Some(obj) = kv.as_object() {
        // Extract root cover image / poster hints
        if let Some(img) = obj.get("image").and_then(|v| v.as_str()) {
            if !img.trim().is_empty() {
                cover_hint = Some(img.trim().to_string());
            }
        }
        if cover_hint.is_none() {
            if let Some(poster) = obj.get("poster").and_then(|v| v.as_object()) {
                if let Some(pimg) = poster
                    .get("posterimage_widescreen")
                    .or_else(|| poster.get("posterimage"))
                    .and_then(|v| v.as_str())
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
                .and_then(|v| v.as_str())
            {
                if !outro.trim().is_empty() {
                    cover_hint = Some(outro.trim().to_string());
                }
            }
        }

        // Extract maps from "modes"
        if let Some(modes) = obj.get("modes").and_then(|v| v.as_object()) {
            let mut mode_keys: Vec<&String> = modes.keys().collect();
            mode_keys.sort_by(|a, b| {
                let a_lower = a.to_lowercase();
                let b_lower = b.to_lowercase();
                if a_lower == "coop" {
                    std::cmp::Ordering::Less
                } else if b_lower == "coop" {
                    std::cmp::Ordering::Greater
                } else {
                    a.cmp(b)
                }
            });

            for mode_key in mode_keys {
                if let Some(mode_obj) = modes.get(mode_key).and_then(|v| v.as_object()) {
                    let mut step_keys: Vec<&String> = mode_obj.keys().collect();
                    step_keys.sort_by(|a, b| {
                        let num_a = a.parse::<u32>().ok();
                        let num_b = b.parse::<u32>().ok();
                        match (num_a, num_b) {
                            (Some(na), Some(nb)) => na.cmp(&nb),
                            _ => a.cmp(b),
                        }
                    });

                    for step_key in step_keys {
                        if let Some(map_obj) = mode_obj.get(step_key).and_then(|v| v.as_object()) {
                            if let Some(map_code) = map_obj
                                .get("map")
                                .or_else(|| map_obj.get("mapname"))
                                .and_then(|v| v.as_str())
                            {
                                let code = map_code.trim().to_string();
                                if !code.is_empty() {
                                    let code_lower = code.to_lowercase();
                                    if !seen_codes.contains(&code_lower) {
                                        seen_codes.insert(code_lower);

                                        let name = map_obj
                                            .get("displayname")
                                            .or_else(|| map_obj.get("name"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or(&code)
                                            .trim()
                                            .to_string();

                                        let image_hint = map_obj
                                            .get("image")
                                            .and_then(|v| v.as_str())
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
    }

    if maps.is_empty() {
        extract_maps_recursive(kv, &mut maps, &mut seen_codes);
    }

    if cover_hint.is_none() {
        if let Some(first_map) = maps.first() {
            if let Some(hint) = &first_map.image_hint {
                cover_hint = Some(hint.clone());
            }
        }
    }

    (maps, cover_hint)
}

fn extract_maps_recursive(
    val: &serde_json::Value,
    maps: &mut Vec<MissionMapInfo>,
    seen_codes: &mut std::collections::HashSet<String>,
) {
    if let Some(obj) = val.as_object() {
        if let Some(map_code) = obj
            .get("map")
            .or_else(|| obj.get("mapname"))
            .and_then(|v| v.as_str())
        {
            let code = map_code.trim().to_string();
            if !code.is_empty() {
                let code_lower = code.to_lowercase();
                if !seen_codes.contains(&code_lower) {
                    seen_codes.insert(code_lower);

                    let name = obj
                        .get("displayname")
                        .or_else(|| obj.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&code)
                        .trim()
                        .to_string();

                    let image_hint = obj
                        .get("image")
                        .and_then(|v| v.as_str())
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
        for sub_val in obj.values() {
            extract_maps_recursive(sub_val, maps, seen_codes);
        }
    } else if let Some(arr) = val.as_array() {
        for sub_val in arr {
            extract_maps_recursive(sub_val, maps, seen_codes);
        }
    }
}

pub fn find_vpk_image_key<'a>(
    hint_or_stem: &str,
    files_lower: &HashMap<String, &'a String>,
    files: &'a HashMap<String, VpkEntry>,
) -> Option<&'a String> {
    let clean = hint_or_stem
        .replace('\\', "/")
        .trim()
        .trim_matches('"')
        .to_lowercase();

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
        candidates.push(format!("materials/{}", clean));
        candidates.push(format!("materials/vgui/{}", clean));
    } else {
        let exts = [".vtf", ".jpg", ".jpeg", ".png"];
        let mut bases = vec![
            clean.clone(),
            format!("materials/{}", clean),
            format!("materials/vgui/{}", clean),
        ];

        if !clean.starts_with("maps/") {
            bases.push(format!("materials/vgui/maps/{}", clean));
        }

        if !clean.starts_with("loading_screen_") && !clean.starts_with("maps/") {
            bases.push(format!("materials/vgui/loading_screen_{}", clean));
        }

        for base in bases {
            for ext in exts {
                candidates.push(format!("{}{}", base, ext));
            }
        }
    }

    for c in &candidates {
        if let Some(k) = files_lower.get(c) {
            return Some(*k);
        }
        let win_c = c.replace('/', "\\");
        if let Some(k) = files_lower.get(&win_c) {
            return Some(*k);
        }
        let suffix_win = format!("\\{}", win_c);
        let suffix_unix = format!("/{}", c);
        if let Some((k, _)) = files.iter().find(|(k, _)| {
            let kl = k.to_lowercase();
            kl.ends_with(&suffix_win) || kl.ends_with(&suffix_unix)
        }) {
            return Some(k);
        }
    }

    None
}


