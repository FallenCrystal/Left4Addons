use std::fs::File;
use std::path::PathBuf;

use super::core::read_string;
use super::{extract_addon_metadata, parse_key_values};

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

    // Call past EOF should safely return empty string without panic
    let s3 = read_string(buf, &mut offset);
    assert_eq!(s3, "");
    assert_eq!(offset, 12);
}

#[test]
fn test_read_string_truncated() {
    let buf = b"no_null_terminator";
    let mut offset = 0;
    let s = read_string(buf, &mut offset);
    assert_eq!(s, "no_null_terminator");
    assert_eq!(offset, buf.len());

    // Subsequent read past end returns empty string safely
    let s_eof = read_string(buf, &mut offset);
    assert_eq!(s_eof, "");
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

    assert_eq!(
        obj.get("addontitle").unwrap().as_str().unwrap(),
        "Test Addon"
    );
    assert_eq!(obj.get("addonversion").unwrap().as_str().unwrap(), "1.0");
    assert_eq!(
        obj.get("addonauthor").unwrap().as_str().unwrap(),
        "Test Author"
    );
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
    assert_eq!(
        obj.get("addontitle").unwrap().as_str().unwrap(),
        "Nested Addon"
    );
    assert_eq!(
        obj.get("addoncontent_campaign").unwrap().as_str().unwrap(),
        "1"
    );

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
    assert_eq!(
        obj.get("addontitle").unwrap().as_str().unwrap(),
        "Commented Addon"
    );
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

    let addon_title = metadata
        .addon_info
        .get("addontitle")
        .and_then(|t| t.as_str());
    assert_eq!(addon_title, Some("Mock Addon"));
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_extract_mission_info_from_kv() {
    let kv = r#"
        "mission"
        {
            "Name" "Test Campaign"
            "Image" "maps/test_campaign_cover"
            "poster"
            {
                "posterImage" "test_poster"
            }
            "modes"
            {
                "coop"
                {
                    "1"
                    {
                        "Map" "m1_test"
                        "DisplayName" "Map 1 Test"
                        "Image" "maps/m1_thumb"
                    }
                    "2"
                    {
                        "Map" "m2_test"
                        "DisplayName" "Map 2 Test"
                        "Image" "maps/m2_thumb"
                    }
                }
            }
        }
    "#;
    let parsed = parse_key_values(kv);
    let (maps, cover_hint) = super::extract_mission_info_from_kv(&parsed);

    assert_eq!(cover_hint, Some("maps/test_campaign_cover".to_string()));
    assert_eq!(maps.len(), 2);
    assert_eq!(maps[0].code, "m1_test");
    assert_eq!(maps[0].name, "Map 1 Test");
    assert_eq!(maps[0].image_hint, Some("maps/m1_thumb".to_string()));
    assert_eq!(maps[1].code, "m2_test");
    assert_eq!(maps[1].name, "Map 2 Test");
    assert_eq!(maps[1].image_hint, Some("maps/m2_thumb".to_string()));
}

#[test]
fn test_find_vpk_image_key() {
    use std::collections::HashMap;

    let mut files = HashMap::new();
    let vpk_entry = super::VpkEntry {
        crc: 0,
        preload_bytes: 0,
        archive_index: 0,
        entry_offset: 0,
        entry_length: 0,
        preload_data: vec![],
        header_size: 12,
        tree_size: 0,
    };
    files.insert("materials/vgui/maps/ls_fsl.vtf".to_string(), vpk_entry);

    let files_lower: HashMap<String, &String> =
        files.keys().map(|k| (k.to_lowercase(), k)).collect();

    let found = super::find_vpk_image_key("maps/LS_FSL", &files_lower, &files);
    assert_eq!(found, Some(&"materials/vgui/maps/ls_fsl.vtf".to_string()));
}

#[test]
fn test_real_l4d2server_vpks() {
    use std::path::Path;
    let lingshan_path =
        Path::new("/home/akkariin/桌面/L4D2Server/l4d2/left4dead2/addons/lingshan_main.vpk");
    if lingshan_path.exists() {
        let temp_cache_dir = std::env::temp_dir().join("l4a_test_lingshan_cache");
        let meta = extract_addon_metadata(lingshan_path, &temp_cache_dir);
        assert!(meta.error.is_none());
        assert_eq!(meta.maps.len(), 8);
        assert_eq!(meta.maps[0].code, "M1_LS_FSL_ND");
        assert_eq!(meta.maps[0].name, "灵城-丰收路");
        assert_eq!(meta.maps[0].image_hint, Some("maps/LS_FSL".to_string()));
    }
}
