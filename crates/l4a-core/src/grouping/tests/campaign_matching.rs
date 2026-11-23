use super::{make_addon, make_ws_snapshot};
use crate::db::LocalDb;
use crate::grouping::{rebuild_auto_bundles, set_bundle_locked};
use l4a_protocol::{AddonId, BundleKind};
use std::collections::BTreeSet;

#[test]
fn excludes_yama_missing_texture_fix_and_survivor_expression_dependencies() {
    let db = LocalDb::open_in_memory().unwrap();

    // Yama Part 1 + Part 2 (same author) + third-party "地图缺失材质修复" & "Yama 缺失材质修复"
    db.upsert_addon(&make_addon(
        "ws:101",
        Some(101),
        "[101] Yama_Part1.vpk",
        Some("Yama"),
        Some("Kuroda"),
        &["Campaign"],
        &["l4d_yama_1"],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "ws:102",
        Some(102),
        "[102] Yama_Part2.vpk",
        None, // Part 2 VPK missing addoninfo.txt!
        None,
        &[],
        &[],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "ws:103",
        Some(103),
        "[103] Map_Missing_Texture_Fix.vpk",
        Some("地图缺失材质修复"),
        Some("Fixer"),
        &["Campaign"],
        &[],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "ws:104",
        Some(104),
        "[104] Yama_Texture_Fix.vpk",
        Some("Yama 缺失材质修复"),
        Some("Kuroda"),
        &["Campaign"],
        &[],
    ))
    .unwrap();

    // Survivor facial expression depending on survivor base model -> must NEVER bundle
    db.upsert_addon(&make_addon(
        "ws:201",
        Some(201),
        "201.vpk",
        Some("Zoey Facial Expressions"),
        Some("ModelerA"),
        &["Survivor"],
        &[],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "ws:202",
        Some(202),
        "202.vpk",
        Some("Zoey Base Model"),
        Some("ModelerA"),
        &["Survivor"],
        &[],
    ))
    .unwrap();

    db.upsert_workshop_snapshot(
        &make_ws_snapshot(
            101,
            "Yama (Part 1 - Maps)",
            "76561198000000001",
            &["Campaigns"],
            &[102],
        ),
        true,
    )
    .unwrap();
    db.upsert_workshop_snapshot(
        &make_ws_snapshot(
            102,
            "Yama (Part 2 - Materials)",
            "76561198000000001",
            &["Campaigns"],
            &[],
        ),
        true,
    )
    .unwrap();
    // Fix mods declare Yama (101) as required dependency on Workshop
    db.upsert_workshop_snapshot(
        &make_ws_snapshot(
            103,
            "地图缺失材质修复",
            "76561198000000099",
            &["Campaigns"],
            &[101],
        ),
        true,
    )
    .unwrap();
    db.upsert_workshop_snapshot(
        &make_ws_snapshot(
            104,
            "Yama 缺失材质修复",
            "76561198000000001",
            &["Campaigns"],
            &[101],
        ),
        true,
    )
    .unwrap();
    db.upsert_workshop_snapshot(
        &make_ws_snapshot(
            201,
            "Zoey Facial Expressions",
            "76561198000000055",
            &["Survivors"],
            &[202],
        ),
        true,
    )
    .unwrap();
    db.upsert_workshop_snapshot(
        &make_ws_snapshot(
            202,
            "Zoey Base Model",
            "76561198000000055",
            &["Survivors"],
            &[],
        ),
        true,
    )
    .unwrap();

    let bundles = rebuild_auto_bundles(&db).unwrap();
    assert_eq!(bundles.len(), 1);
    assert_eq!(bundles[0].name, "Yama");
    assert_eq!(bundles[0].kind, BundleKind::DependencyGraph);
    assert_eq!(
        bundles[0].member_addon_ids,
        vec![AddonId("ws:101".into()), AddonId("ws:102".into())]
    );
}

#[test]
fn groups_local_attachment_campaign_parts_and_ignores_sdk_boilerplate_templates() {
    let db = LocalDb::open_in_memory().unwrap();

    // Case 1: Local 3-part campaign where all VPKs copied identical addoninfo.txt ("寂静岭")
    let mut sh1 = make_addon(
        "hash:sh1",
        None,
        "silenthill_part1.vpk",
        Some("寂静岭"),
        Some("MapperSH"),
        &["Campaign"],
        &["sh_m1", "sh_m2"],
    );
    sh1.addon_info.insert(
        "addondescription".into(),
        "Classic Silent Hill custom campaign".into(),
    );
    let mut sh2 = make_addon(
        "hash:sh2",
        None,
        "silenthill_part2.vpk",
        Some("寂静岭"),
        Some("MapperSH"),
        &[], // Part 2 only has models/materials!
        &[],
    );
    sh2.addon_info.insert(
        "addondescription".into(),
        "Classic Silent Hill custom campaign".into(),
    );
    let sh3 = make_addon(
        "hash:sh3",
        None,
        "silenthill_part3.vpk",
        None, // Part 3 has NO addoninfo.txt at all!
        None,
        &[],
        &[],
    );
    db.upsert_addon(&sh1).unwrap();
    db.upsert_addon(&sh2).unwrap();
    db.upsert_addon(&sh3).unwrap();

    // Case 2: Versioned root + Part 1 + Part 2 (v1 广西-南宁 regression test)
    db.upsert_addon(&make_addon(
        "hash:nn_root",
        None,
        "nn_main.vpk",
        Some("广西-南宁 V1.15.1"),
        Some("HerobrineAce"),
        &["Campaign", "Map"],
        &["nn_m1"],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "hash:nn_p1",
        None,
        "nn_1.vpk",
        Some("广西-南宁 V1.15.1 Part 1"),
        Some("HerobrineAce"),
        &[],
        &[],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "hash:nn_p2",
        None,
        "nn_2.vpk",
        Some("广西-南宁 V1.15.1 Part 2"),
        Some("HerobrineAce"),
        &[],
        &[],
    ))
    .unwrap();

    // Case 3: Two unrelated maps leaving Valve SDK sample "Dead Line" / "AUTHOR_NAME" untouched -> must NOT group!
    let mut sdk_a = make_addon(
        "hash:sdk_a",
        None,
        "alpha_custom.vpk",
        Some("Dead Line"),
        Some("AUTHOR_NAME"),
        &["Campaign"],
        &["alpha_m1"],
    );
    sdk_a.addon_info.insert(
        "addondescription".into(),
        "Same copied SDK description text".into(),
    );
    let mut sdk_b = make_addon(
        "hash:sdk_b",
        None,
        "omega_custom.vpk",
        Some("Dead Line"),
        Some("AUTHOR_NAME"),
        &["Campaign"],
        &["omega_m1"],
    );
    sdk_b.addon_info.insert(
        "addondescription".into(),
        "Same copied SDK description text".into(),
    );
    db.upsert_addon(&sdk_a).unwrap();
    db.upsert_addon(&sdk_b).unwrap();

    let bundles = rebuild_auto_bundles(&db).unwrap();
    assert_eq!(bundles.len(), 2);

    let nn = bundles
        .iter()
        .find(|b| b.name == "广西-南宁")
        .expect("广西-南宁 bundle");
    assert_eq!(
        nn.member_addon_ids,
        vec![
            AddonId("hash:nn_root".into()),
            AddonId("hash:nn_p1".into()),
            AddonId("hash:nn_p2".into()),
        ]
    );

    let sh = bundles
        .iter()
        .find(|b| b.name == "寂静岭")
        .expect("寂静岭 bundle");
    assert_eq!(
        sh.member_addon_ids,
        vec![
            AddonId("hash:sh1".into()),
            AddonId("hash:sh2".into()),
            AddonId("hash:sh3".into()),
        ]
    );
}

#[test]
fn groups_identical_local_campaign_packages_with_different_filenames() {
    let db = LocalDb::open_in_memory().unwrap();

    // Local campaign split into arbitrary filenames (a.vpk, b.vpk) with identical addoninfo
    let mut a = make_addon(
        "hash:loc_a",
        None,
        "archive_main.vpk",
        Some("Bloody Moors"),
        Some(" MoorsTeam "),
        &["Campaign"],
        &["bm_m1", "bm_m2"],
    );
    a.addon_info.insert(
        "addondescription".into(),
        "Five-chapter gothic moorland survival campaign.".into(),
    );
    let mut b = make_addon(
        "hash:loc_b",
        None,
        "extra_pack.vpk",
        Some("Bloody Moors"),
        Some("MoorsTeam"),
        &[],
        &[],
    );
    b.addon_info.insert(
        "addondescription".into(),
        "Five-chapter gothic moorland survival campaign.".into(),
    );
    db.upsert_addon(&a).unwrap();
    db.upsert_addon(&b).unwrap();

    let bundles = rebuild_auto_bundles(&db).unwrap();
    assert_eq!(bundles.len(), 1);
    assert_eq!(bundles[0].name, "Bloody Moors");
    assert_eq!(
        bundles[0].member_addon_ids,
        vec![AddonId("hash:loc_a".into()), AddonId("hash:loc_b".into())]
    );
}

#[test]
fn covers_compound_assets_trailing_versions_cjk_roots_and_fix_annotated_campaigns() {
    let db = LocalDb::open_in_memory().unwrap();

    // 1. Compound asset qualifier `(Models & Materials)` + `Weapon`/`Infected` content flags on Part 2
    db.upsert_addon(&make_addon(
        "ws:5001",
        Some(5001),
        "[5001] Yama_Maps.vpk",
        Some("Yama (Maps)"),
        Some("Kuroda"),
        &["Campaign", "Map"],
        &["l4d_yama_1"],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "ws:5002",
        Some(5002),
        "[5002] Yama_Assets.vpk",
        Some("Yama (Models & Materials)"),
        Some("Kuroda"),
        &["Infected", "Weapon"],
        &[],
    ))
    .unwrap();

    // 2. Trailing version after `Part N` + English number words `Chapter One` / `Chapter Two`
    db.upsert_addon(&make_addon(
        "hash:ch1",
        None,
        "pripyat_ch1.vpk",
        Some("Pripyat: Chapter One v2.1"),
        Some("ZoneTeam"),
        &["Campaign", "Map"],
        &["pr_m1"],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "hash:ch2",
        None,
        "pripyat_ch2.vpk",
        Some("Pripyat: Chapter Two v2.1"),
        Some("ZoneTeam"),
        &["Campaign", "Map"],
        &["pr_m2"],
    ))
    .unwrap();

    // 3. 2-char CJK campaign (`樱花_1.vpk` + `樱花_2.vpk`) with `汉化修复版` in shared campaign title + hotfix version split
    let mut yh1 = make_addon(
        "hash:yh1",
        None,
        "樱花_1.vpk",
        Some("樱花 (汉化修复版)"),
        Some("SakuraDev"),
        &["Campaign", "Map"],
        &["yh_m1"],
    );
    yh1.version = Some("1.2".into());
    let mut yh2 = make_addon(
        "hash:yh2",
        None,
        "樱花_2.vpk",
        Some("樱花 (汉化修复版)"),
        Some("SakuraDev"),
        &[],
        &[],
    );
    yh2.version = Some("1.0".into());
    db.upsert_addon(&yh1).unwrap();
    db.upsert_addon(&yh2).unwrap();

    // 4. Generic standalone fix titles (`地图缺失材质修复`) must still NEVER merge!
    let mut fix_a = make_addon(
        "hash:fix_a",
        None,
        "missing_tex_a.vpk",
        Some("地图缺失材质修复"),
        Some("Fixer"),
        &["Campaign"],
        &["fix_m1"],
    );
    fix_a
        .addon_info
        .insert("addondescription".into(), "Same generic fix desc".into());
    let mut fix_b = make_addon(
        "hash:fix_b",
        None,
        "missing_tex_b.vpk",
        Some("地图缺失材质修复"),
        Some("Fixer"),
        &[],
        &[],
    );
    fix_b
        .addon_info
        .insert("addondescription".into(), "Same generic fix desc".into());
    db.upsert_addon(&fix_a).unwrap();
    db.upsert_addon(&fix_b).unwrap();

    let bundles = rebuild_auto_bundles(&db).unwrap();
    assert_eq!(bundles.len(), 3);
    let names: BTreeSet<String> = bundles.iter().map(|b| b.name.clone()).collect();
    assert!(names.contains("Yama"));
    assert!(names.contains("Pripyat"));
    assert!(names.contains("樱花 (汉化修复版)"));
}

#[test]
fn preserves_locked_bundle_ids_and_members_when_later_parts_share_root_and_author() {
    let db = LocalDb::open_in_memory().unwrap();
    db.upsert_addon(&make_addon(
        "hash:ag1",
        None,
        "afterglow_1.vpk",
        None,
        Some("Mapper"),
        &["Campaign", "Map"],
        &["ag_m1"],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "hash:ag2",
        None,
        "afterglow_2.vpk",
        None,
        Some("Mapper"),
        &[],
        &[],
    ))
    .unwrap();

    let initial = rebuild_auto_bundles(&db).unwrap();
    assert_eq!(initial.len(), 1);
    let locked_id = initial[0].id.clone();
    set_bundle_locked(&db, &locked_id, true).unwrap();

    // Scan two new parts with the exact same root ("afterglow") and author ("Mapper")
    db.upsert_addon(&make_addon(
        "hash:ag3",
        None,
        "afterglow_3.vpk",
        None,
        Some("Mapper"),
        &["Campaign", "Map"],
        &["ag_m3"],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "hash:ag4",
        None,
        "afterglow_4.vpk",
        None,
        Some("Mapper"),
        &[],
        &[],
    ))
    .unwrap();

    let rebuilt = rebuild_auto_bundles(&db).unwrap();
    assert_eq!(rebuilt.len(), 2);
    let original = rebuilt
        .iter()
        .find(|b| b.id == locked_id)
        .expect("locked bundle preserved");
    assert!(original.locked);
    assert_eq!(
        original.member_addon_ids,
        vec![AddonId("hash:ag1".into()), AddonId("hash:ag2".into())]
    );

    let extra = rebuilt
        .iter()
        .find(|b| b.id != locked_id)
        .expect("new unlocked bundle gets non-colliding ID");
    assert!(!extra.locked);
    assert_eq!(
        extra.member_addon_ids,
        vec![AddonId("hash:ag3".into()), AddonId("hash:ag4".into())]
    );
}
