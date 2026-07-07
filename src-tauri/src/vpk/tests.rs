use std::fs::File;
use std::path::PathBuf;

use super::{extract_addon_metadata, parse_key_values};
use super::core::read_string;

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
