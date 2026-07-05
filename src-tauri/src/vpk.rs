use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use serde::{Serialize, Deserialize};
use md5::{Md5, Digest};

#[derive(Debug, Clone)]
pub struct VpkEntry {
    pub crc: u32,
    pub preload_bytes: u16,
    pub archive_index: u16,
    pub entry_offset: u32,
    pub entry_length: u32,
    pub preload_data: Vec<u8>,
    pub header_size: u32,
    pub tree_size: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct AddonMetadata {
    #[serde(rename = "addonInfo")]
    pub addon_info: serde_json::Value,
    #[serde(rename = "hasImage")]
    pub has_image: bool,
    #[serde(rename = "imagePath")]
    pub image_path: Option<String>,
    #[serde(rename = "filesCount")]
    pub files_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn read_string(buf: &[u8], offset: &mut usize) -> String {
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
    let addoninfo_key = files.keys().find(|k| {
        let lower = k.to_lowercase();
        lower == "addoninfo.txt" || lower.ends_with("/addoninfo.txt") || lower.ends_with("\\addoninfo.txt")
    });
    if let Some(key) = addoninfo_key {
        if let Some(entry) = files.get(key) {
            if let Ok(content_bytes) = get_file_content(&mut file, entry) {
                let text = String::from_utf8_lossy(&content_bytes);
                result.addon_info = parse_key_values(&text);
            }
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_read_string() {
        let buf = b"hello\0world\0";
        let mut offset = 0;
        let s1 = read_string(buf, &mut offset);
        assert_eq!(s1, "hello");
        assert_eq!(offset, 6);

        let s2 = read_string(buf, &mut offset);
        assert_eq!(s2, "world");
        assert_eq!(offset, 12);
    }

    #[test]
    fn test_parse_key_values_simple() {
        let kv = r#"
            "AddonInfo"
            {
                "addonTitle" "Test Addon"
                "addonVersion" "1.0"
                "addonAuthor" "Test Author"
            }
        "#;
        let parsed = parse_key_values(kv);
        assert!(parsed.is_object());
        let obj = parsed.as_object().unwrap();
        
        assert_eq!(obj.get("addontitle").unwrap().as_str().unwrap(), "Test Addon");
        assert_eq!(obj.get("addonversion").unwrap().as_str().unwrap(), "1.0");
        assert_eq!(obj.get("addonauthor").unwrap().as_str().unwrap(), "Test Author");
    }

    #[test]
    fn test_parse_key_values_nested() {
        let kv = r#"
            "AddonInfo"
            {
                "addonTitle" "Nested Addon"
                "addonContent_Campaign" "1"
                "NestedObject"
                {
                    "Key1" "Value1"
                }
            }
        "#;
        let parsed = parse_key_values(kv);
        assert!(parsed.is_object());
        let obj = parsed.as_object().unwrap();
        assert_eq!(obj.get("addontitle").unwrap().as_str().unwrap(), "Nested Addon");
        assert_eq!(obj.get("addoncontent_campaign").unwrap().as_str().unwrap(), "1");
        
        let nested = obj.get("nestedobject").unwrap().as_object().unwrap();
        assert_eq!(nested.get("key1").unwrap().as_str().unwrap(), "Value1");
    }

    #[test]
    fn test_parse_key_values_comments() {
        let kv = r#"
            // This is a comment
            "AddonInfo"
            {
                "addonTitle" "Commented Addon" // inline comment
                "addonVersion" "2.0"
            }
        "#;
        let parsed = parse_key_values(kv);
        assert!(parsed.is_object());
        let obj = parsed.as_object().unwrap();
        assert_eq!(obj.get("addontitle").unwrap().as_str().unwrap(), "Commented Addon");
        assert_eq!(obj.get("addonversion").unwrap().as_str().unwrap(), "2.0");
    }

    #[test]
    fn test_extract_addon_metadata_mock_vpk() {
        let temp_dir = PathBuf::from("../target/test_temp_dir");
        if !temp_dir.exists() {
            let _ = std::fs::create_dir_all(&temp_dir);
        }
        
        let vpk_path = temp_dir.join("mock_addon.vpk");
        let temp_cache_dir = temp_dir.join("cache");
        
        {
            use std::io::Write;
            let mut file = File::create(&vpk_path).unwrap();
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

        let metadata = extract_addon_metadata(&vpk_path, &temp_cache_dir);
        assert!(metadata.error.is_none());
        assert_eq!(metadata.files_count, 1);
        
        let addon_title = metadata.addon_info.get("addontitle").and_then(|t| t.as_str());
        assert_eq!(addon_title, Some("Mock Addon"));
        
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

