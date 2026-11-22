use l4a_protocol::WorkshopId;
use l4a_vpk::{
    extract_addon_metadata, extract_mission_info_from_kv, generate_dummy_vpk, inspect_dummy_vpk,
    parse_key_values, parse_vpk, read_vpk_entry, write_vpk, DummyInspection, DummySpec,
    VpkFileToWrite,
};
use std::fs;
use tempfile::tempdir;

#[test]
fn extracts_non_utf8_metadata_and_nested_missions() {
    let dir = tempdir().unwrap();
    let vpk_path = dir.path().join("encoded.vpk");
    let cache_dir = dir.path().join("cache");
    let addoninfo = r#""AddonInfo" { "addonTitle" "Café" }"#;
    let mission =
        r#""mission" { "modes" { "coop" { "1" { "map" "c1" "displayname" "Entrée" } } } }"#;

    let le = |text: &str| -> Vec<u8> { text.encode_utf16().flat_map(u16::to_le_bytes).collect() };
    let be = |text: &str| -> Vec<u8> { text.encode_utf16().flat_map(u16::to_be_bytes).collect() };
    let latin1 = addoninfo.chars().map(|c| c as u8).chain([0]).collect();
    for encoded in [
        [vec![0xef, 0xbb, 0xbf], addoninfo.as_bytes().to_vec()].concat(),
        [vec![0xff, 0xfe], le(addoninfo)].concat(),
        [vec![0xfe, 0xff], be(addoninfo)].concat(),
        le(addoninfo),
        be(addoninfo),
        latin1,
    ] {
        write_vpk(
            &vpk_path,
            &[
                VpkFileToWrite {
                    ext: "txt".into(),
                    path: "".into(),
                    filename: "addoninfo".into(),
                    content: encoded,
                },
                VpkFileToWrite {
                    ext: "txt".into(),
                    path: "missions".into(),
                    filename: "encoded".into(),
                    content: [vec![0xfe, 0xff], be(mission)].concat(),
                },
            ],
        )
        .unwrap();
        let metadata = extract_addon_metadata(&vpk_path, &cache_dir);
        assert_eq!(metadata.addon_info["addontitle"], "Café");
        assert_eq!(metadata.maps.len(), 1);
        assert_eq!(metadata.maps[0].code, "c1");
        assert_eq!(metadata.maps[0].name, "Entrée");
        let DummyInspection::RealVpk { content_hash, .. } = inspect_dummy_vpk(&vpk_path) else {
            panic!("expected a real VPK");
        };
        assert_eq!(metadata.content_hash, content_hash);
    }
}

#[test]
fn vpk_write_and_read_roundtrip_extracts_metadata_and_maps() {
    let dir = tempdir().unwrap();
    let vpk_path = dir.path().join("campaign_part1.vpk");
    let cache_dir = dir.path().join("cache");

    let addoninfo = br#""AddonInfo"
{
    "addonSteamAppID" "550"
    "addonversion" "2.4"
    "addontitle" "City 17 Redux"
    "addonauthor" "Tester"
    "addonContent_Campaign" "1"
    "addonURL0" "https://steamcommunity.com/sharedfiles/filedetails/?id=123456789"
}"#;

    let mission = br#""mission"
{
    "Name" "city17"
    "poster"
    {
        "posterImage" "vgui/city17_poster"
    }
    "modes"
    {
        "versus"
        {
            "1" { "Map" "c17_vs1" "DisplayName" "VS Stage" }
        }
        "coop"
        {
            "2" { "Map" "c17_m2" "DisplayName" "Subway" }
            "1" { "Map" "c17_m1" "DisplayName" "Plaza" }
        }
    }
}"#;

    let fake_jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];

    write_vpk(
        &vpk_path,
        &[
            VpkFileToWrite {
                ext: "txt".into(),
                path: "".into(),
                filename: "addoninfo".into(),
                content: addoninfo.to_vec(),
            },
            VpkFileToWrite {
                ext: "txt".into(),
                path: "missions".into(),
                filename: "city17".into(),
                content: mission.to_vec(),
            },
            VpkFileToWrite {
                ext: "bsp".into(),
                path: "maps".into(),
                filename: "c17_extra".into(),
                content: b"VBSP_FAKE".to_vec(),
            },
            VpkFileToWrite {
                ext: "jpg".into(),
                path: "materials/vgui".into(),
                filename: "city17_poster".into(),
                content: fake_jpg.clone(),
            },
        ],
    )
    .unwrap();

    let mut archive = parse_vpk(&vpk_path).unwrap();
    assert_eq!(archive.entries.len(), 4);
    let read_back = read_vpk_entry(&mut archive.file, &archive.entries["addoninfo.txt"]).unwrap();
    assert_eq!(read_back, addoninfo);

    let meta = extract_addon_metadata(&vpk_path, &cache_dir);
    assert!(meta.error.is_none());
    assert_eq!(meta.files_count, 4);
    assert_eq!(
        meta.addon_info.get("addontitle").map(String::as_str),
        Some("City 17 Redux")
    );
    assert!(meta.has_image);
    assert!(cache_dir.join(meta.image_path.as_ref().unwrap()).exists());

    // Coop maps should appear before versus and be numerically ordered.
    assert_eq!(meta.maps.len(), 4);
    assert_eq!(meta.maps[0].code, "c17_m1");
    assert_eq!(meta.maps[1].code, "c17_m2");
    assert_eq!(meta.maps[2].code, "c17_vs1");
    assert_eq!(meta.maps[3].code, "c17_extra");
}

#[test]
fn content_fingerprint_changes_when_entry_bytes_change() {
    let dir = tempdir().unwrap();
    let p1 = dir.path().join("a.vpk");
    let p2 = dir.path().join("b.vpk");
    let cache = dir.path().join("cache");

    write_vpk(
        &p1,
        &[VpkFileToWrite {
            ext: "bsp".into(),
            path: "maps".into(),
            filename: "m1".into(),
            content: b"version-1".to_vec(),
        }],
    )
    .unwrap();

    write_vpk(
        &p2,
        &[VpkFileToWrite {
            ext: "bsp".into(),
            path: "maps".into(),
            filename: "m1".into(),
            content: b"version-2".to_vec(),
        }],
    )
    .unwrap();

    let m1 = extract_addon_metadata(&p1, &cache);
    let m2 = extract_addon_metadata(&p2, &cache);
    assert_ne!(m1.content_hash, m2.content_hash);
}

#[test]
fn dummy_v2_roundtrip_and_legacy_v1_detection() {
    let dir = tempdir().unwrap();
    let real_path = dir.path().join("real.vpk");
    let dummy_v2_path = dir.path().join("dummy_v2.vpk");
    let dummy_v1_path = dir.path().join("dummy_v1.vpk");
    let cache = dir.path().join("cache");

    write_vpk(
        &real_path,
        &[
            VpkFileToWrite {
                ext: "txt".into(),
                path: "".into(),
                filename: "addoninfo".into(),
                content: br#""AddonInfo" { "addontitle" "Real Mod" "addonversion" "1.2" }"#
                    .to_vec(),
            },
            VpkFileToWrite {
                ext: "jpg".into(),
                path: "".into(),
                filename: "addonimage".into(),
                content: vec![1, 2, 3, 4],
            },
        ],
    )
    .unwrap();

    let real_meta = extract_addon_metadata(&real_path, &cache);
    let real_size = fs::metadata(&real_path).unwrap().len();
    let wid = WorkshopId::new(987654321).unwrap();

    generate_dummy_vpk(
        &real_path,
        &dummy_v2_path,
        &DummySpec {
            workshop_id: wid,
            original_hash: real_meta.content_hash.clone(),
            original_size: real_size,
            original_time_updated: Some(1_700_000_000),
        },
    )
    .unwrap();

    match inspect_dummy_vpk(&real_path) {
        DummyInspection::RealVpk {
            content_hash,
            file_size,
            title,
        } => {
            assert_eq!(content_hash, real_meta.content_hash);
            assert_eq!(file_size, real_size);
            assert_eq!(title.as_deref(), Some("Real Mod"));
        }
        other => panic!("expected RealVpk, got {other:?}"),
    }

    match inspect_dummy_vpk(&dummy_v2_path) {
        DummyInspection::Dummy(meta) => {
            assert_eq!(meta.schema_version, 2);
            assert_eq!(meta.workshop_id, Some(wid));
            assert_eq!(meta.original_hash, Some(real_meta.content_hash));
            assert_eq!(meta.original_size, Some(real_size));
            assert_eq!(meta.original_time_updated, Some(1_700_000_000));
        }
        other => panic!("expected Dummy v2, got {other:?}"),
    }

    // Legacy v1 dummy detection
    write_vpk(
        &dummy_v1_path,
        &[VpkFileToWrite {
            ext: "txt".into(),
            path: "".into(),
            filename: "addoninfo".into(),
            content: br#""AddonInfo" { "addonDescription" "A dummy addon generated by Left 4 Addons" "l4aDummy" "1" }"#.to_vec(),
        }],
    )
    .unwrap();

    match inspect_dummy_vpk(&dummy_v1_path) {
        DummyInspection::Dummy(meta) => {
            assert_eq!(meta.schema_version, 1);
            assert_eq!(meta.original_hash, None);
        }
        other => panic!("expected Dummy v1, got {other:?}"),
    }
}

#[test]
fn recursive_mission_fallback_extracts_maps() {
    let kv = parse_key_values(
        r#"
        "mission" {
            "chapters" {
                "chapter1" { "map" "custom_01" "displayname" "Chapter One" }
            }
        }
    "#,
    );
    let (maps, _) = extract_mission_info_from_kv(&kv);
    assert_eq!(maps.len(), 1);
    assert_eq!(maps[0].code, "custom_01");
    assert_eq!(maps[0].name, "Chapter One");
}

#[test]
fn corrupted_huge_tree_size_is_rejected_without_oom_and_dummy_escapes_quotes() {
    let dir = tempdir().unwrap();

    // 1. Crafted VPK header claiming huge 2 GiB tree_size inside a 16-byte file
    let bad_path = dir.path().join("huge_tree.vpk");
    let mut bad_bytes = Vec::new();
    bad_bytes.extend_from_slice(&0x55aa1234u32.to_le_bytes());
    bad_bytes.extend_from_slice(&1u32.to_le_bytes());
    bad_bytes.extend_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
    bad_bytes.extend_from_slice(&[0u8; 4]);
    fs::write(&bad_path, &bad_bytes).unwrap();

    let err = match parse_vpk(&bad_path) {
        Ok(_) => panic!("expected CorruptedTree error for oversized VPK tree"),
        Err(e) => e,
    };
    assert!(matches!(err, l4a_vpk::VpkReadError::CorruptedTree));

    // 2. Title with quotes, backslashes, and newlines produces valid Dummy v2 KeyValues
    let real_path = dir.path().join("special.vpk");
    let dummy_path = dir.path().join("special_dummy.vpk");
    let cache = dir.path().join("cache");
    write_vpk(
        &real_path,
        &[VpkFileToWrite {
            ext: "txt".into(),
            path: "".into(),
            filename: "addoninfo".into(),
            content: b"\"AddonInfo\"\n{\n    \"addontitle\" \"Map \\\"Quoted\\\" \\\\ Path\\nNext\"\n    \"addonversion\" \"1.0\"\n}\n".to_vec(),
        }],
    )
    .unwrap();
    let meta = extract_addon_metadata(&real_path, &cache);
    let size = fs::metadata(&real_path).unwrap().len();
    let wid = WorkshopId::new(777888).unwrap();
    generate_dummy_vpk(
        &real_path,
        &dummy_path,
        &DummySpec {
            workshop_id: wid,
            original_hash: meta.content_hash.clone(),
            original_size: size,
            original_time_updated: Some(1234),
        },
    )
    .unwrap();

    match inspect_dummy_vpk(&dummy_path) {
        DummyInspection::Dummy(d) => {
            assert_eq!(d.schema_version, 2);
            assert_eq!(d.workshop_id, Some(wid));
            assert_eq!(d.original_hash, Some(meta.content_hash));
        }
        other => panic!("expected valid Dummy v2, got {other:?}"),
    }
}
