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

pub fn parse_vpk<P: AsRef<Path>>(file_path: P) -> Result<(HashMap<String, VpkEntry>, File), String> {
    let mut file = File::open(&file_path).map_err(|e| e.to_string())?;
    let mut header_buf = vec![0; 28];
    file.read_exact(&mut header_buf).map_err(|e| e.to_string())?;

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

    file.seek(SeekFrom::Start(header_size as u64)).map_err(|e| e.to_string())?;
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
                if preload_bytes > 0 {
                    if offset + preload_bytes as usize <= tree_buf.len() {
                        preload_data.extend_from_slice(&tree_buf[offset..offset + preload_bytes as usize]);
                        offset += preload_bytes as usize;
                    }
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
        let file_offset = entry.header_size as u64 + entry.tree_size as u64 + entry.entry_offset as u64;
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
            while let Some(nc) = chars.next() {
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
            return serde_json::to_value(parse_object(&tokens, &mut index)).unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        } else if index < tokens.len() && tokens[index] == "{" {
            index += 1;
            return serde_json::to_value(parse_object(&tokens, &mut index)).unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
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
        lower == "addoninfo.txt" || lower.ends_with("/addoninfo.txt") || lower.ends_with("\\addoninfo.txt")
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
        lower == "addonimage.jpg" || lower.ends_with("/addonimage.jpg") || lower.ends_with("\\addonimage.jpg") ||
        lower == "addonimage.jpeg" || lower.ends_with("/addonimage.jpeg") || lower.ends_with("\\addonimage.jpeg") ||
        lower == "addonimage.png" || lower.ends_with("/addonimage.png") || lower.ends_with("\\addonimage.png")
    });
    let addonimage_vtf_key = files.keys().find(|k| {
        let lower = k.to_lowercase();
        lower == "addonimage.vtf" || lower.ends_with("/addonimage.vtf") || lower.ends_with("\\addonimage.vtf")
    });

    let vpk_name = vpk_path
        .as_ref()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let clean_vpk_name = vpk_name
        .replace(".disabled", "")
        .replace(".vpk", "");
    
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
                            if decoded.save_with_format(&full_cache_path, image::ImageFormat::Jpeg).is_ok() {
                                result.has_image = true;
                                result.image_path = Some(format!("/cache/{}", cache_filename));
                            }
                        }
                    }
                }
            }
        }
    }

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
        ext_map.entry(f.ext.clone())
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
                let filename_str = if f.filename.is_empty() { " " } else { &f.filename };
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

    file.write_all(&0x55aa1234u32.to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(&1u32.to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(&tree_size.to_le_bytes()).map_err(|e| e.to_string())?;

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
        lower == "addoninfo.txt" || lower.ends_with("/addoninfo.txt") || lower.ends_with("\\addoninfo.txt")
    });

    let mut steam_app_id = "550".to_string();
    let mut addon_version = "1.0".to_string();
    let mut addon_title = original_vpk_path.as_ref()
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
        lower == "addonimage.jpg" || lower.ends_with("/addonimage.jpg") || lower.ends_with("\\addonimage.jpg")
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
        lower == "addonimage.vtf" || lower.ends_with("/addonimage.vtf") || lower.ends_with("\\addonimage.vtf")
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
