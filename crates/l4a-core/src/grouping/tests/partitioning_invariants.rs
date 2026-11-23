use super::{make_addon, make_ws_snapshot};
use crate::db::LocalDb;
use crate::grouping::rebuild_auto_bundles;
use l4a_protocol::AddonId;

#[test]
fn enforces_author_compatibility_with_single_steam_id_and_disjoint_bundle_membership() {
    let db = LocalDb::open_in_memory().unwrap();

    // Workshop Outpost Part 1 by Mapper (with Steam ID) + Local Outpost Part 2 by Dana (no Steam ID) -> must NOT bundle!
    db.upsert_addon(&make_addon(
        "ws:701",
        Some(701),
        "[701] Outpost_Part1.vpk",
        Some("Outpost Part 1"),
        Some("Mapper"),
        &["Campaign", "Map"],
        &["op_m1"],
    ))
    .unwrap();
    let mut ws_701 = make_ws_snapshot(
        701,
        "Outpost Part 1",
        "76561198000000111",
        &["Campaigns"],
        &[],
    );
    ws_701.creator_name = Some("Mapper".into());
    db.upsert_workshop_snapshot(&ws_701, true).unwrap();

    db.upsert_addon(&make_addon(
        "hash:702",
        None,
        "outpost_part2_dana.vpk",
        Some("Outpost Part 2"),
        Some("Dana"),
        &[],
        &[],
    ))
    .unwrap();
    assert!(rebuild_auto_bundles(&db).unwrap().is_empty());

    // Adding Mapper's local Part 2 groups ONLY Mapper's parts while keeping Dana's separate
    db.upsert_addon(&make_addon(
        "hash:703",
        None,
        "outpost_part2_mapper.vpk",
        Some("Outpost Part 2"),
        Some("Mapper"),
        &[],
        &[],
    ))
    .unwrap();
    let bundles = rebuild_auto_bundles(&db).unwrap();
    assert_eq!(bundles.len(), 1);
    assert_eq!(
        bundles[0].member_addon_ids,
        vec![AddonId("ws:701".into()), AddonId("hash:703".into())]
    );

    // VPK with distinct AddonInfo ("Alpha Valley") and Workshop ("Beta Ridge") roots must never appear in two bundles
    let db2 = LocalDb::open_in_memory().unwrap();
    db2.upsert_addon(&make_addon(
        "ws:801",
        Some(801),
        "801.vpk",
        Some("Alpha Valley"),
        Some("DualAuthor"),
        &["Campaign", "Map"],
        &["av_m1"],
    ))
    .unwrap();
    let mut ws_801 = make_ws_snapshot(801, "Beta Ridge", "76561198000000222", &["Campaigns"], &[]);
    ws_801.creator_name = Some("DualAuthor".into());
    db2.upsert_workshop_snapshot(&ws_801, true).unwrap();

    db2.upsert_addon(&make_addon(
        "hash:802",
        None,
        "alpha_p2.vpk",
        Some("Alpha Valley Part 2"),
        Some("DualAuthor"),
        &[],
        &[],
    ))
    .unwrap();
    db2.upsert_addon(&make_addon(
        "hash:803",
        None,
        "beta_p1.vpk",
        Some("Beta Ridge Part 1"),
        Some("DualAuthor"),
        &["Campaign", "Map"],
        &["br_m1"],
    ))
    .unwrap();
    db2.upsert_addon(&make_addon(
        "hash:804",
        None,
        "beta_p2.vpk",
        Some("Beta Ridge Part 2"),
        Some("DualAuthor"),
        &[],
        &[],
    ))
    .unwrap();

    let bundles2 = rebuild_auto_bundles(&db2).unwrap();
    let owners = bundles2
        .iter()
        .filter(|b| b.member_addon_ids.contains(&AddonId("ws:801".into())))
        .count();
    assert_eq!(owners, 1);
}

#[test]
fn separates_underscore_fix_patches_campaign_releases_chinese_bufen_and_huge_asset_ranks() {
    let db = LocalDb::open_in_memory().unwrap();

    // 1. Underscore-delimited standalone fix (`river_run_texture_fix.vpk`) copying campaign AddonInfo title
    db.upsert_addon(&make_addon(
        "hash:rr1",
        None,
        "river_run_part1.vpk",
        Some("River Run Part 1"),
        Some("RiverDev"),
        &["Campaign", "Map"],
        &["rr_m1"],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "hash:rr2",
        None,
        "river_run_part2.vpk",
        Some("River Run Part 2"),
        Some("RiverDev"),
        &[],
        &[],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "hash:rr_fix",
        None,
        "river_run_texture_fix.vpk",
        Some("River Run"),
        Some("RiverDev"),
        &["Campaign"],
        &[],
    ))
    .unwrap();

    // 2. Separately installed v1 and v2 multi-part campaign releases stay in separate bundles
    for (id, fname, title, ver) in [
        (
            "hash:ct_v1_p1",
            "citadel_v1_p1.vpk",
            "Citadel v1 Part 1",
            "1.0",
        ),
        (
            "hash:ct_v1_p2",
            "citadel_v1_p2.vpk",
            "Citadel v1 Part 2",
            "1.0",
        ),
        (
            "hash:ct_v2_p1",
            "citadel_v2_p1.vpk",
            "Citadel v2 Part 1",
            "2.0",
        ),
        (
            "hash:ct_v2_p2",
            "citadel_v2_p2.vpk",
            "Citadel v2 Part 2",
            "2.0",
        ),
    ] {
        let mut addon = make_addon(
            id,
            None,
            fname,
            Some(title),
            Some("CitadelTeam"),
            if id.ends_with("p1") {
                &["Campaign", "Map"]
            } else {
                &[]
            },
            if id.ends_with("p1") { &["ct_m1"] } else { &[] },
        );
        addon.version = Some(ver.into());
        db.upsert_addon(&addon).unwrap();
    }

    // 3. Full two-character Chinese suffix `第1部分` / `第2部分` + huge u32::MAX asset suffix
    db.upsert_addon(&make_addon(
        "hash:yz1",
        None,
        "alpha_pack.vpk",
        Some("远征 第1部分"),
        Some("CNMapper"),
        &["Campaign", "Map"],
        &["yz_m1"],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "hash:yz2",
        None,
        "beta_pack.vpk",
        Some("远征 第2部分"),
        Some("CNMapper"),
        &[],
        &[],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "hash:yz_tex",
        None,
        "gamma_pack.vpk",
        Some("远征 Textures 4294967295"),
        Some("CNMapper"),
        &[],
        &[],
    ))
    .unwrap();

    let bundles = rebuild_auto_bundles(&db).unwrap();
    assert_eq!(bundles.len(), 4);

    let rr = bundles
        .iter()
        .find(|b| b.name == "River Run")
        .expect("River Run bundle");
    assert_eq!(
        rr.member_addon_ids,
        vec![AddonId("hash:rr1".into()), AddonId("hash:rr2".into())]
    );

    let citadel_bundles: Vec<_> = bundles.iter().filter(|b| b.name == "Citadel").collect();
    assert_eq!(citadel_bundles.len(), 2);
    assert!(citadel_bundles
        .iter()
        .all(|b| b.member_addon_ids.len() == 2));

    let yz = bundles
        .iter()
        .find(|b| b.name == "远征")
        .expect("远征 bundle");
    assert_eq!(
        yz.member_addon_ids,
        vec![
            AddonId("hash:yz1".into()),
            AddonId("hash:yz2".into()),
            AddonId("hash:yz_tex".into()),
        ]
    );
}

#[test]
fn wildcard_clustering_is_order_independent_and_cjk_versions_stay_separate() {
    let db = LocalDb::open_in_memory().unwrap();

    // hash:111 (authorless Part 2) sorts BEFORE hash:222 (Dana Part 2) and ws:701 (Mapper Part 1):
    // must still attach hash:111 to Mapper's Part 1 instead of being stolen by Dana!
    db.upsert_addon(&make_addon(
        "hash:111",
        None,
        "outpost_part2.vpk",
        Some("Outpost Part 2"),
        None,
        &[],
        &[],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "hash:222",
        None,
        "outpost_part2_dana.vpk",
        Some("Outpost Part 2"),
        Some("Dana"),
        &[],
        &[],
    ))
    .unwrap();
    db.upsert_addon(&make_addon(
        "ws:701",
        Some(701),
        "[701] Outpost_Part1.vpk",
        Some("Outpost Part 1"),
        Some("Mapper"),
        &["Campaign", "Map"],
        &["op_m1"],
    ))
    .unwrap();
    let mut ws_701 = make_ws_snapshot(
        701,
        "Outpost Part 1",
        "76561198000000111",
        &["Campaigns"],
        &[],
    );
    ws_701.creator_name = Some("Mapper".into());
    db.upsert_workshop_snapshot(&ws_701, true).unwrap();

    // CJK-adjacent V1/V2 releases ("南宁V1" vs "南宁V2") with identical addonversion stay separate
    for (id, fname, title) in [
        ("hash:nn_v1_p1", "nnv1_1.vpk", "南宁V1 Part 1"),
        ("hash:nn_v1_p2", "nnv1_2.vpk", "南宁V1 Part 2"),
        ("hash:nn_v2_p1", "nnv2_1.vpk", "南宁V2 Part 1"),
        ("hash:nn_v2_p2", "nnv2_2.vpk", "南宁V2 Part 2"),
    ] {
        db.upsert_addon(&make_addon(
            id,
            None,
            fname,
            Some(title),
            Some("NNMapper"),
            if id.ends_with("p1") {
                &["Campaign", "Map"]
            } else {
                &[]
            },
            if id.ends_with("p1") { &["nn_m1"] } else { &[] },
        ))
        .unwrap();
    }

    let bundles = rebuild_auto_bundles(&db).unwrap();
    assert_eq!(bundles.len(), 3);
    let outpost = bundles
        .iter()
        .find(|b| b.name == "Outpost")
        .expect("Outpost bundle");
    assert_eq!(
        outpost.member_addon_ids,
        vec![AddonId("ws:701".into()), AddonId("hash:111".into())]
    );
    let nn_bundles: Vec<_> = bundles.iter().filter(|b| b.name == "南宁").collect();
    assert_eq!(nn_bundles.len(), 2);
}

#[test]
fn cross_dimension_and_steam_wildcards_are_strictly_permutation_invariant() {
    for swap_ids in [false, true] {
        let db = LocalDb::open_in_memory().unwrap();
        let (id_a, id_b) = if swap_ids {
            ("zhash:900", "ahash:100")
        } else {
            ("ahash:100", "zhash:900")
        };

        // 8A: Cross-dimension partial wildcards (author-only Part 2 vs two authorless explicit releases)
        db.upsert_addon(&make_addon(
            "hash:8a_auth_p2",
            None,
            "alpha_p2.vpk",
            Some("AlphaBase Part 2"),
            Some("Mapper"),
            &[],
            &[],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            id_a,
            None,
            "alpha_v1_p1.vpk",
            Some("AlphaBase v1 Part 1"),
            None,
            &["Campaign", "Map"],
            &["ab_m1"],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            id_b,
            None,
            "alpha_v2_p1.vpk",
            Some("AlphaBase v2 Part 1"),
            None,
            &["Campaign", "Map"],
            &["ab_m2"],
        ))
        .unwrap();

        // Symmetric 8A: Release-only Part 2 vs two release-less distinct authors
        db.upsert_addon(&make_addon(
            "hash:sym_rel_p2",
            None,
            "beta_v1_p2.vpk",
            Some("BetaRidge v1 Part 2"),
            None,
            &[],
            &[],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            if swap_ids {
                "zhash:beta_alice"
            } else {
                "ahash:beta_alice"
            },
            None,
            "beta_alice_p1.vpk",
            Some("BetaRidge Part 1"),
            Some("Alice"),
            &["Campaign", "Map"],
            &["br_m1"],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            if swap_ids {
                "ahash:beta_bob"
            } else {
                "zhash:beta_bob"
            },
            None,
            "beta_bob_p1.vpk",
            Some("BetaRidge Part 1"),
            Some("Bob"),
            &["Campaign", "Map"],
            &["br_m2"],
        ))
        .unwrap();

        // 8B-1: Nameless SteamID-only Workshop Part 1 vs two distinct named local parts
        db.upsert_addon(&make_addon(
            "ws:801",
            Some(801),
            "[801] gamma_p1.vpk",
            Some("GammaPort Part 1"),
            None,
            &["Campaign", "Map"],
            &["gp_m1"],
        ))
        .unwrap();
        db.upsert_workshop_snapshot(
            &make_ws_snapshot(
                801,
                "GammaPort Part 1",
                "76561198000000801",
                &["Campaigns"],
                &[],
            ),
            true,
        )
        .unwrap();
        db.upsert_addon(&make_addon(
            if swap_ids {
                "zhash:gamma_alice"
            } else {
                "ahash:gamma_alice"
            },
            None,
            "gamma_alice_p2.vpk",
            Some("GammaPort Part 2"),
            Some("Alice"),
            &[],
            &[],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            if swap_ids {
                "ahash:gamma_bob"
            } else {
                "zhash:gamma_bob"
            },
            None,
            "gamma_bob_p3.vpk",
            Some("GammaPort Part 3"),
            Some("Bob"),
            &[],
            &[],
        ))
        .unwrap();

        // 8B-2: Local named Part 1 ("ahash:..." < "ws:...") must NEVER steal ws_902_p2 from its SteamID sibling ws_901_p1
        db.upsert_addon(&make_addon(
            if swap_ids {
                "zhash:delta_alice_p1"
            } else {
                "ahash:delta_alice_p1"
            },
            None,
            "delta_alice_p1.vpk",
            Some("DeltaKeep Part 1"),
            Some("Alice"),
            &["Campaign", "Map"],
            &["dk_m1"],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            "ws:901",
            Some(901),
            "[901] delta_p1.vpk",
            Some("DeltaKeep Part 1"),
            None,
            &["Campaign", "Map"],
            &["dk_m1"],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            "ws:902",
            Some(902),
            "[902] delta_p2.vpk",
            Some("DeltaKeep Part 2"),
            None,
            &[],
            &[],
        ))
        .unwrap();
        db.upsert_workshop_snapshot(
            &make_ws_snapshot(
                901,
                "DeltaKeep Part 1",
                "76561198000000900",
                &["Campaigns"],
                &[],
            ),
            true,
        )
        .unwrap();
        db.upsert_workshop_snapshot(
            &make_ws_snapshot(902, "DeltaKeep Part 2", "76561198000000900", &[], &[]),
            true,
        )
        .unwrap();

        // 8C: Authorless Part 1 wildcard never steals Part 2 from Mapper Part 1, and competing Part 2 wildcards do not arbitrarily tie-break
        db.upsert_addon(&make_addon(
            "ws:951",
            Some(951),
            "[951] echo_p1.vpk",
            Some("EchoVale Part 1"),
            Some("Mapper"),
            &["Campaign", "Map"],
            &["ev_m1"],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            if swap_ids {
                "zhash:echo_wild_p1"
            } else {
                "ahash:echo_wild_p1"
            },
            None,
            "echo_wild_p1.vpk",
            Some("EchoVale Part 1"),
            None,
            &["Campaign", "Map"],
            &["ev_m1"],
        ))
        .unwrap();
        let echo_p2_id = if swap_ids {
            "ahash:echo_wild_p2"
        } else {
            "zhash:echo_wild_p2"
        };
        db.upsert_addon(&make_addon(
            echo_p2_id,
            None,
            "echo_wild_p2.vpk",
            Some("EchoVale Part 2"),
            None,
            &[],
            &[],
        ))
        .unwrap();

        db.upsert_addon(&make_addon(
            "ws:961",
            Some(961),
            "[961] foxtrot_p1.vpk",
            Some("FoxtrotRun Part 1"),
            Some("Mapper"),
            &["Campaign", "Map"],
            &["fr_m1"],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            if swap_ids {
                "zhash:foxtrot_p2_a"
            } else {
                "ahash:foxtrot_p2_a"
            },
            None,
            "foxtrot_p2_a.vpk",
            Some("FoxtrotRun Part 2"),
            None,
            &[],
            &[],
        ))
        .unwrap();
        db.upsert_addon(&make_addon(
            if swap_ids {
                "ahash:foxtrot_p2_b"
            } else {
                "zhash:foxtrot_p2_b"
            },
            None,
            "foxtrot_p2_b.vpk",
            Some("FoxtrotRun Part 2"),
            None,
            &[],
            &[],
        ))
        .unwrap();

        let bundles = rebuild_auto_bundles(&db).unwrap();
        // Only DeltaKeep ([ws:901, ws:902]) and EchoVale ([ws:951, echo_p2_id]) should bundle!
        assert_eq!(bundles.len(), 2, "swap_ids={swap_ids}: {bundles:?}");
        let delta = bundles
            .iter()
            .find(|b| b.name == "DeltaKeep")
            .expect("DeltaKeep bundle");
        assert_eq!(
            delta.member_addon_ids,
            vec![AddonId("ws:901".into()), AddonId("ws:902".into())]
        );
        let echo = bundles
            .iter()
            .find(|b| b.name == "EchoVale")
            .expect("EchoVale bundle");
        assert_eq!(
            echo.member_addon_ids,
            vec![AddonId("ws:951".into()), AddonId(echo_p2_id.into())]
        );
    }
}
