use axum::{routing::get, Router};
use chrono::Utc;
use l4a_core::db::LocalDb;
use l4a_core::{AppPaths, L4aEngine, NullEventSink};
use l4a_mirror::MirrorRouter;
use l4a_protocol::{
    AddonDirType, AddonId, AppConfig, GameUpdateDummyPolicy, GroupId, InstallState, LoadoutProfile,
    MetadataSource, ProfileId, ReconcileAction, ReconcileIssueKind, RenameRuleConfig, UserGroup,
    WorkshopCollectionRecord, WorkshopDependency, WorkshopFetchOutcome, WorkshopFileType,
    WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use l4a_steam::MockSteamBridge;
use l4a_vpk::{write_vpk, VpkFileToWrite};
use l4a_workshop::{
    SourceRoutingPolicy, SteamBridgeProvider, WebScraperPlaceholderProvider, WorkshopResolver,
};
use std::path::Path;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::net::TcpListener;

fn create_test_vpk(
    path: &Path,
    title: &str,
    author: &str,
    campaign: bool,
    map_code: Option<&str>,
    extra_bytes: &[u8],
) {
    let addoninfo = format!(
        "\"AddonInfo\"\n{{\n    \"addonSteamAppID\" \"550\"\n    \"addonversion\" \"1.0\"\n    \"addontitle\" \"{title}\"\n    \"addonauthor\" \"{author}\"\n    \"addonContent_Campaign\" \"{}\"\n}}\n",
        if campaign { "1" } else { "0" }
    );
    let mut entries = vec![
        VpkFileToWrite {
            ext: "txt".into(),
            path: "".into(),
            filename: "addoninfo".into(),
            content: addoninfo.into_bytes(),
        },
        VpkFileToWrite {
            ext: "bin".into(),
            path: "data".into(),
            filename: "payload".into(),
            content: extra_bytes.to_vec(),
        },
    ];
    if let Some(code) = map_code {
        entries.push(VpkFileToWrite {
            ext: "bsp".into(),
            path: "maps".into(),
            filename: code.into(),
            content: format!("BSP_{code}").into_bytes(),
        });
    }
    write_vpk(path, &entries).unwrap();
}

fn make_engine(root: &Path, config: AppConfig, bridge: Arc<MockSteamBridge>) -> L4aEngine {
    let paths = AppPaths::from_runtime_dir(root.join("runtime"));
    let db = Arc::new(LocalDb::open(&paths.db_path).unwrap());
    let mirror = Arc::new(MirrorRouter::empty());
    let resolver = Arc::new(WorkshopResolver::new(
        vec![
            Arc::new(WebScraperPlaceholderProvider::placeholder()),
            Arc::new(SteamBridgeProvider::new(bridge.clone())),
        ],
        SourceRoutingPolicy::default(),
    ));
    L4aEngine::with_dependencies(
        paths,
        db,
        config,
        mirror,
        bridge,
        resolver,
        None,
        Arc::new(NullEventSink),
    )
    .unwrap()
}

fn item_record(id: u64, title: &str, ts: i64, deps: Vec<u64>) -> WorkshopItemRecord {
    WorkshopItemRecord {
        workshop_id: WorkshopId::new(id).unwrap(),
        file_type: WorkshopFileType::Item,
        status: WorkshopItemStatus::Active,
        title: title.to_string(),
        description: Some("desc".into()),
        creator_steam_id: Some("76561198000000001".into()),
        creator_name: Some("Mapper".into()),
        preview_url: None,
        file_url: None,
        file_size: Some(1024),
        time_created: Some(1_700_000_000),
        time_updated: Some(ts),
        tags: vec!["Campaign".into()],
        dependencies: deps
            .into_iter()
            .map(|d| WorkshopDependency {
                workshop_id: WorkshopId::new(d).unwrap(),
                title: None,
                required: true,
            })
            .collect(),
        collection_children: Vec::new(),
        subscriptions: None,
        favorites: None,
        source: MetadataSource::SteamworksSdk,
        fetched_at: Utc::now(),
    }
}

#[tokio::test]
async fn scan_rename_preserves_workshop_identity_and_map_search_works() {
    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    let workshop_dir = loading_dir.join("workshop");
    std::fs::create_dir_all(&workshop_dir).unwrap();

    let vpk = workshop_dir.join("123456789.vpk");
    create_test_vpk(
        &vpk,
        "Urban Flight",
        "MapperA",
        true,
        Some("uf_m1_airport"),
        b"v1",
    );

    let cfg = AppConfig {
        loading_dir: loading_dir.to_string_lossy().into_owned(),
        workshop_dir: workshop_dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let engine = make_engine(dir.path(), cfg, Arc::new(MockSteamBridge::new(false)));

    let snap = engine.scan().unwrap();
    assert_eq!(snap.addons.len(), 1);
    let aid = snap.addons[0].id.clone();
    assert_eq!(
        snap.addons[0].workshop_id,
        Some(WorkshopId::new(123456789).unwrap())
    );

    // Direct rename inside workshop/ is refused to avoid Steam redownload
    let ws_err = engine
        .apply_renames(&[(aid.clone(), "Illegal Workshop Rename.vpk".into())])
        .unwrap_err();
    assert!(matches!(ws_err, l4a_core::CoreError::Conflict(_)));

    // Move to loading/ then rename without workshop ID prefix -> rescan keeps workshop_id!
    engine
        .move_addons(std::slice::from_ref(&aid), AddonDirType::Loading)
        .unwrap();
    let snap = engine
        .apply_renames(&[(aid.clone(), "Custom Urban Name.vpk".into())])
        .unwrap();
    assert_eq!(
        snap.addons[0].current_file_name.as_deref(),
        Some("Custom Urban Name.vpk")
    );

    let rescanned = engine.scan().unwrap();
    assert_eq!(rescanned.addons.len(), 1);
    assert_eq!(rescanned.addons[0].id, aid);
    assert_eq!(
        rescanned.addons[0].workshop_id,
        Some(WorkshopId::new(123456789).unwrap())
    );

    let map_hits = engine.search_by_map_code("uf_m1").unwrap();
    assert_eq!(map_hits.len(), 1);
}

#[tokio::test]
async fn dummy_bypass_v2_reconciles_game_update_overwrite_and_auto_downloads() {
    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    let workshop_dir = loading_dir.join("workshop");
    std::fs::create_dir_all(&workshop_dir).unwrap();

    let wid = WorkshopId::new(888001).unwrap();
    create_test_vpk(
        &workshop_dir.join("888001.vpk"),
        "Dead Center Mod",
        "Author1",
        true,
        Some("dc_mod_1"),
        b"original-bytes-v1",
    );

    let cfg = AppConfig {
        loading_dir: loading_dir.to_string_lossy().into_owned(),
        workshop_dir: workshop_dir.to_string_lossy().into_owned(),
        enable_dummy_bypass: true,
        game_update_dummy_policy: GameUpdateDummyPolicy::Ask,
        ..Default::default()
    };
    let engine = make_engine(dir.path(), cfg, Arc::new(MockSteamBridge::new(false)));
    engine.scan().unwrap();

    let aid = AddonId::from_workshop_id(wid);
    // User moves addon to loading/, renames it, disables it, and places it in a custom group.
    engine
        .move_addons(std::slice::from_ref(&aid), AddonDirType::Loading)
        .unwrap();
    engine
        .apply_renames(&[(aid.clone(), "My Favorite Campaign.vpk".into())])
        .unwrap();
    engine
        .toggle_addons(std::slice::from_ref(&aid), false, false)
        .unwrap();
    engine
        .save_user_group(UserGroup {
            id: GroupId("grp_coop".into()),
            name: "Coop Nights".into(),
            tags: vec!["fav".into()],
            color: None,
            addon_ids: vec![aid.clone()],
        })
        .unwrap();

    let snap = engine.scan().unwrap();
    assert!(snap.open_issues.is_empty());
    assert!(snap.addons[0].has_dummy_bypass);

    // 1. Simulate L4D2 overwriting workshop/888001.vpk with a newer real VPK update!
    create_test_vpk(
        &workshop_dir.join("888001.vpk"),
        "Dead Center Mod",
        "Author1",
        true,
        Some("dc_mod_1"),
        b"updated-bytes-v2-from-steam",
    );

    let snap = engine.scan().unwrap();
    let issue = snap
        .open_issues
        .iter()
        .find(|i| i.kind == ReconcileIssueKind::DummyOverwrittenByGameUpdate)
        .expect("should detect overwritten dummy");

    // Resolve via AdoptWorkshopUpdate -> keeps custom filename & disabled state, updates content, regenerates v2 dummy!
    let resolved_snap = engine
        .resolve_issue(&issue.id, ReconcileAction::AdoptWorkshopUpdate)
        .unwrap();
    assert!(
        resolved_snap.open_issues.is_empty(),
        "unexpected open issues: {:?}",
        resolved_snap.open_issues
    );
    let updated_addon = &resolved_snap.addons[0];
    assert_eq!(
        updated_addon.current_file_name.as_deref(),
        Some("My Favorite Campaign.vpk")
    );
    assert!(!updated_addon.enabled);
    assert!(updated_addon.has_dummy_bypass);
    assert_eq!(resolved_snap.user_groups[0].addon_ids, vec![aid.clone()]);
    assert!(loading_dir
        .join("My Favorite Campaign.vpk.disabled")
        .exists());

    // 2. Simulate L4D2 auto-downloading a brand-new workshop item 999002.vpk
    create_test_vpk(
        &workshop_dir.join("999002.vpk"),
        "Auto Downloaded Skin",
        "Author2",
        false,
        None,
        b"skin-bytes",
    );
    let snap2 = engine.scan().unwrap();
    let auto_issue = snap2
        .open_issues
        .iter()
        .find(|i| i.kind == ReconcileIssueKind::NewWorkshopAutoDownload)
        .expect("should detect newly auto-downloaded workshop item");

    let snap3 = engine
        .resolve_issue(
            &auto_issue.id,
            ReconcileAction::ImportToLoadingAndCreateDummy,
        )
        .unwrap();
    assert!(snap3.open_issues.is_empty());
    let imported = snap3
        .addons
        .iter()
        .find(|a| a.workshop_id == Some(WorkshopId::new(999002).unwrap()))
        .unwrap();
    assert_eq!(imported.primary_dir, Some(AddonDirType::Loading));
    assert!(imported.has_dummy_bypass);

    // 3. Delete loading/ file for 999002 externally -> OrphanDummy detected & cleaned up
    std::fs::remove_file(imported.current_path.as_ref().unwrap()).unwrap();
    let snap4 = engine.scan().unwrap();
    let orphan_issue = snap4
        .open_issues
        .iter()
        .find(|i| i.kind == ReconcileIssueKind::OrphanDummy)
        .expect("should detect orphan dummy");
    let snap5 = engine
        .resolve_issue(&orphan_issue.id, ReconcileAction::RemoveOrphanDummy)
        .unwrap();
    assert!(!workshop_dir.join("999002.vpk").exists());
    assert!(snap5.open_issues.is_empty());
}

#[tokio::test]
async fn lifecycle_sync_detects_updates_upstream_removal_and_multi_parent_obsolete_deps() {
    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    let workshop_dir = loading_dir.join("workshop");
    std::fs::create_dir_all(&loading_dir).unwrap();

    // Installed addons: ParentA (100), ParentB (101), DepX (200), DepY (201), RemovedMap (300)
    create_test_vpk(
        &loading_dir.join("100.vpk"),
        "Parent A",
        "Auth",
        true,
        None,
        b"a",
    );
    create_test_vpk(
        &loading_dir.join("101.vpk"),
        "Parent B",
        "Auth",
        true,
        None,
        b"b",
    );
    create_test_vpk(
        &loading_dir.join("200.vpk"),
        "Shared Texture X",
        "Auth",
        false,
        None,
        b"x",
    );
    create_test_vpk(
        &loading_dir.join("201.vpk"),
        "Shared Model Y",
        "Auth",
        false,
        None,
        b"y",
    );
    create_test_vpk(
        &loading_dir.join("300.vpk"),
        "Soon Removed",
        "Auth",
        true,
        None,
        b"r",
    );

    let bridge = Arc::new(MockSteamBridge::new(true));
    let cfg = AppConfig {
        loading_dir: loading_dir.to_string_lossy().into_owned(),
        workshop_dir: workshop_dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let engine = make_engine(dir.path(), cfg, bridge.clone());
    engine.scan().unwrap();

    // Sync #1: ParentA depends on X(200) & Y(201); ParentB depends on Y(201); RemovedMap is Active.
    bridge.insert_item(item_record(100, "Parent A", 1000, vec![200, 201]));
    bridge.insert_item(item_record(101, "Parent B", 1000, vec![201]));
    bridge.insert_item(item_record(200, "Shared Texture X", 1000, vec![]));
    bridge.insert_item(item_record(201, "Shared Model Y", 1000, vec![]));
    bridge.insert_item(item_record(300, "Soon Removed", 1000, vec![]));

    let r1 = engine.sync_workshop().await.unwrap();
    assert_eq!(r1.checked_items, 5);
    // Align local timestamps to simulate user already up-to-date on v1000
    for wid in [100u64, 101, 200, 201, 300] {
        engine
            .db()
            .update_addon_local_timestamp(
                &AddonId::from_workshop_id(WorkshopId::new(wid).unwrap()),
                Some(1000),
            )
            .unwrap();
    }

    // Sync #2:
    // - ParentA updates to ts=2000, removes BOTH DepX(200) & DepY(201), adds DepZ(202)!
    // - ParentB still depends on DepY(201)!
    // - Item 300 is removed from Workshop!
    bridge.insert_item(item_record(100, "Parent A", 2000, vec![202]));
    bridge.insert_outcome(WorkshopFetchOutcome::Removed {
        workshop_id: WorkshopId::new(300).unwrap(),
        source: MetadataSource::SteamWebApi,
        observed_at: Utc::now(),
        reason: Some("deleted by author".into()),
    });

    let r2 = engine.sync_workshop().await.unwrap();
    assert_eq!(r2.updates_available.len(), 1);
    assert_eq!(
        r2.updates_available[0].workshop_id,
        WorkshopId::new(100).unwrap()
    );

    assert_eq!(r2.removed_upstream.len(), 1);
    assert_eq!(
        r2.removed_upstream[0].workshop_id,
        WorkshopId::new(300).unwrap()
    );
    assert!(r2.removed_upstream[0].local_file_preserved);
    assert!(loading_dir.join("300.vpk").exists());

    assert_eq!(r2.dependency_diffs.len(), 1);
    assert_eq!(
        r2.dependency_diffs[0].added_dependencies,
        vec![WorkshopId::new(202).unwrap()]
    );
    assert_eq!(
        r2.dependency_diffs[0].removed_dependencies,
        vec![WorkshopId::new(200).unwrap(), WorkshopId::new(201).unwrap()]
    );

    // DepX(200) has 0 remaining parents -> can_safely_remove = true!
    // DepY(201) is STILL required by ParentB(101) -> can_safely_remove = false!
    let x_hint = r2
        .obsolete_dependencies
        .iter()
        .find(|h| h.dependency_workshop_id == WorkshopId::new(200).unwrap())
        .unwrap();
    assert!(x_hint.can_safely_remove);

    let y_hint = r2
        .obsolete_dependencies
        .iter()
        .find(|h| h.dependency_workshop_id == WorkshopId::new(201).unwrap())
        .unwrap();
    assert!(!y_hint.can_safely_remove);
    assert_eq!(
        y_hint.remaining_active_parents,
        vec![WorkshopId::new(101).unwrap()]
    );
}

#[tokio::test]
async fn grouping_bundles_locked_collections_and_profiles_work_cleanly() {
    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    let workshop_dir = loading_dir.join("workshop");
    std::fs::create_dir_all(&loading_dir).unwrap();

    create_test_vpk(
        &loading_dir.join("p1.vpk"),
        "Yama Part 1",
        "AuthorZ",
        true,
        Some("yama_m1"),
        b"p1",
    );
    create_test_vpk(
        &loading_dir.join("p2.vpk"),
        "Yama Part 2",
        "AuthorZ",
        true,
        Some("yama_m2"),
        b"p2",
    );
    create_test_vpk(
        &loading_dir.join("w1.vpk"),
        "AK47 Skin",
        "AuthorW",
        false,
        None,
        b"w1",
    );

    let bridge = Arc::new(MockSteamBridge::new(true));
    let col_id = WorkshopId::new(55555).unwrap();
    bridge.insert_collection(WorkshopCollectionRecord {
        collection_id: col_id,
        status: WorkshopItemStatus::Active,
        title: Some("Friday Pack".into()),
        description: None,
        preview_url: None,
        child_ids: vec![
            WorkshopId::new(7001).unwrap(),
            WorkshopId::new(7002).unwrap(),
        ],
        items: vec![],
        source: MetadataSource::SteamworksSdk,
        fetched_at: Utc::now(),
    });

    let cfg = AppConfig {
        loading_dir: loading_dir.to_string_lossy().into_owned(),
        workshop_dir: workshop_dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let engine = make_engine(dir.path(), cfg, bridge.clone());
    let snap = engine.scan().unwrap();

    let p1_id = snap
        .addons
        .iter()
        .find(|a| a.title == "Yama Part 1")
        .unwrap()
        .id
        .clone();
    let p2_id = snap
        .addons
        .iter()
        .find(|a| a.title == "Yama Part 2")
        .unwrap()
        .id
        .clone();
    let w1_id = snap
        .addons
        .iter()
        .find(|a| a.title == "AK47 Skin")
        .unwrap()
        .id
        .clone();

    // Auto bundle aggregates Yama Part 1 + Part 2 and combines their maps!
    let bundles = engine.rebuild_bundles().unwrap();
    assert_eq!(bundles.len(), 1);
    assert_eq!(bundles[0].maps.len(), 2);

    // Lock the bundle and verify rebuilding does not overwrite it
    engine.set_bundle_locked(&bundles[0].id, true).unwrap();
    let rebuilt_again = engine.rebuild_bundles().unwrap();
    assert_eq!(rebuilt_again.len(), 1);
    assert!(rebuilt_again[0].locked);

    // Many-to-many user groups allow the same addon in multiple groups simultaneously
    engine
        .save_user_group(UserGroup {
            id: GroupId("g_maps".into()),
            name: "Campaigns".into(),
            tags: vec![],
            color: None,
            addon_ids: vec![p1_id.clone(), p2_id.clone()],
        })
        .unwrap();
    engine
        .save_user_group(UserGroup {
            id: GroupId("g_friday".into()),
            name: "Friday Lobby".into(),
            tags: vec![],
            color: None,
            addon_ids: vec![p1_id.clone(), w1_id.clone()],
        })
        .unwrap();
    assert_eq!(engine.snapshot().unwrap().user_groups.len(), 2);

    // Tracked collection detects upstream removal diff on sync
    engine.track_collection(col_id, false).await.unwrap();
    bridge.insert_collection(WorkshopCollectionRecord {
        collection_id: col_id,
        status: WorkshopItemStatus::Active,
        title: Some("Friday Pack".into()),
        description: None,
        preview_url: None,
        child_ids: vec![
            WorkshopId::new(7002).unwrap(),
            WorkshopId::new(7003).unwrap(),
        ],
        items: vec![],
        source: MetadataSource::SteamworksSdk,
        fetched_at: Utc::now(),
    });
    let sync_report = engine.sync_workshop().await.unwrap();
    assert_eq!(sync_report.collection_diffs.len(), 1);
    assert_eq!(
        sync_report.collection_diffs[0].added_items,
        vec![WorkshopId::new(7003).unwrap()]
    );
    assert_eq!(
        sync_report.collection_diffs[0].removed_items,
        vec![WorkshopId::new(7001).unwrap()]
    );

    // Loadout profile toggles only loading/ addons cleanly
    engine
        .save_profile(LoadoutProfile {
            id: ProfileId("prof_clean".into()),
            name: "Vanilla+ Campaign".into(),
            description: "Only Yama".into(),
            enabled_addon_ids: vec![p1_id.clone(), p2_id.clone()],
        })
        .unwrap();
    let apply_report = engine
        .apply_profile(&ProfileId("prof_clean".into()))
        .unwrap();
    assert_eq!(apply_report.disabled_count, 1);
    assert!(loading_dir.join("w1.vpk.disabled").exists());
    assert!(loading_dir.join("p1.vpk").exists());
}

#[tokio::test]
async fn download_workflow_fetches_vpk_and_registers_addon_with_dummy() {
    let dir = tempdir().unwrap();
    let vpk_fixture = dir.path().join("fixture.vpk");
    create_test_vpk(
        &vpk_fixture,
        "Remote Mod",
        "RemoteAuthor",
        false,
        None,
        b"remote-vpk-data",
    );
    let vpk_bytes = Arc::new(std::fs::read(&vpk_fixture).unwrap());

    let app = Router::new().route(
        "/mod.vpk",
        get({
            let bytes = vpk_bytes.clone();
            move || {
                let bytes = bytes.clone();
                async move { (*bytes).clone() }
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let loading_dir = dir.path().join("addons");
    let workshop_dir = loading_dir.join("workshop");
    let wid = WorkshopId::new(654321).unwrap();
    let bridge = Arc::new(MockSteamBridge::new(true));
    let mut rec = item_record(654321, "Remote Mod", 1_700_009_999, vec![]);
    rec.file_url = Some(format!("http://{addr}/mod.vpk"));
    bridge.insert_item(rec);

    let cfg = AppConfig {
        loading_dir: loading_dir.to_string_lossy().into_owned(),
        workshop_dir: workshop_dir.to_string_lossy().into_owned(),
        enable_dummy_bypass: true,
        ..Default::default()
    };
    let engine = make_engine(dir.path(), cfg, bridge);

    let summary = engine.download_workshop_item(wid).await.unwrap();
    assert_eq!(summary.install_state, InstallState::Installed);
    assert!(summary.has_dummy_bypass);
    assert!(loading_dir.join("[654321] Remote Mod.vpk").exists());
    assert!(workshop_dir.join("654321.vpk").exists());
}

#[tokio::test]
async fn preview_rename_and_apply_reject_conflicts_without_clobbering_files() {
    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    let workshop_dir = loading_dir.join("workshop");
    std::fs::create_dir_all(&loading_dir).unwrap();

    create_test_vpk(
        &loading_dir.join("a.vpk"),
        "Same Name",
        "Auth",
        false,
        None,
        b"a",
    );
    create_test_vpk(
        &loading_dir.join("b.vpk"),
        "Same Name",
        "Auth",
        false,
        None,
        b"b",
    );

    let cfg = AppConfig {
        loading_dir: loading_dir.to_string_lossy().into_owned(),
        workshop_dir: workshop_dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let engine = make_engine(dir.path(), cfg, Arc::new(MockSteamBridge::new(false)));
    let snap = engine.scan().unwrap();

    let rules = RenameRuleConfig {
        enable_workshop_id_prefix: false,
        enable_group_prefix: false,
        ..Default::default()
    };

    let preview = engine.preview_rename(&rules, &[]).unwrap();
    assert_eq!(preview.len(), 2);
    assert!(preview.iter().any(|p| p.conflict.is_some()));

    let err = engine
        .apply_renames(&[
            (snap.addons[0].id.clone(), "Shared.vpk".into()),
            (snap.addons[1].id.clone(), "Shared.vpk".into()),
        ])
        .unwrap_err();
    assert!(matches!(err, l4a_core::CoreError::Conflict(_)));
    assert!(loading_dir.join("a.vpk").exists());
    assert!(loading_dir.join("b.vpk").exists());
}

#[tokio::test]
async fn web_api_sync_without_dependencies_does_not_erase_existing_deps() {
    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    let workshop_dir = loading_dir.join("workshop");
    std::fs::create_dir_all(&loading_dir).unwrap();

    create_test_vpk(
        &loading_dir.join("110.vpk"),
        "Parent",
        "Auth",
        true,
        None,
        b"p",
    );
    create_test_vpk(
        &loading_dir.join("210.vpk"),
        "Dep",
        "Auth",
        false,
        None,
        b"d",
    );

    let bridge = Arc::new(MockSteamBridge::new(true));
    let wid_p = WorkshopId::new(110).unwrap();
    let wid_d = WorkshopId::new(210).unwrap();
    bridge.insert_item(item_record(110, "Parent", 1000, vec![210]));
    bridge.insert_item(item_record(210, "Dep", 1000, vec![]));

    let cfg = AppConfig {
        loading_dir: loading_dir.to_string_lossy().into_owned(),
        workshop_dir: workshop_dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let engine = make_engine(dir.path(), cfg, bridge.clone());
    engine.scan().unwrap();
    engine.sync_workshop().await.unwrap();
    assert_eq!(
        engine.db().list_active_dependencies(wid_p).unwrap().len(),
        1
    );

    // Next sync returns SteamWebApi snapshot (which does not carry dependencies)
    let mut web_item = item_record(110, "Parent", 1500, vec![]);
    web_item.source = MetadataSource::SteamWebApi;
    bridge.insert_item(web_item);

    let report = engine.sync_workshop().await.unwrap();
    assert!(report.obsolete_dependencies.is_empty());
    assert_eq!(
        engine
            .db()
            .list_active_dependencies(wid_p)
            .unwrap()
            .into_iter()
            .map(|d| d.workshop_id)
            .collect::<Vec<_>>(),
        vec![wid_d]
    );
}

#[tokio::test]
async fn reconcile_ignore_and_leave_persist_corrupted_skipped_and_move_back_cleans_dummy() {
    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    let workshop_dir = loading_dir.join("workshop");
    std::fs::create_dir_all(&workshop_dir).unwrap();

    create_test_vpk(
        &workshop_dir.join("500001.vpk"),
        "Ignored Mod",
        "Auth",
        false,
        None,
        b"v1",
    );
    create_test_vpk(
        &workshop_dir.join("500002.vpk"),
        "Kept Mod",
        "Auth",
        false,
        None,
        b"k1",
    );

    let cfg = AppConfig {
        loading_dir: loading_dir.to_string_lossy().into_owned(),
        workshop_dir: workshop_dir.to_string_lossy().into_owned(),
        enable_dummy_bypass: true,
        ..Default::default()
    };
    let engine = make_engine(dir.path(), cfg, Arc::new(MockSteamBridge::new(false)));

    let snap = engine.scan().unwrap();
    assert_eq!(snap.open_issues.len(), 2);
    let i1 = snap
        .open_issues
        .iter()
        .find(|i| i.workshop_id == Some(WorkshopId::new(500001).unwrap()))
        .unwrap()
        .id
        .clone();
    let i2 = snap
        .open_issues
        .iter()
        .find(|i| i.workshop_id == Some(WorkshopId::new(500002).unwrap()))
        .unwrap()
        .id
        .clone();

    engine.resolve_issue(&i1, ReconcileAction::Ignore).unwrap();
    engine
        .resolve_issue(&i2, ReconcileAction::LeaveInWorkshop)
        .unwrap();

    // Repeated scans keep both issues suppressed while disk files are unchanged
    assert!(engine.scan().unwrap().open_issues.is_empty());
    assert!(engine.scan().unwrap().open_issues.is_empty());

    // Changing 500001 on disk resurfaces the issue automatically
    create_test_vpk(
        &workshop_dir.join("500001.vpk"),
        "Ignored Mod",
        "Auth",
        false,
        None,
        b"v2-modified",
    );
    let resurfaced = engine.scan().unwrap();
    assert_eq!(resurfaced.open_issues.len(), 1);
    assert_eq!(
        resurfaced.open_issues[0].workshop_id,
        Some(WorkshopId::new(500001).unwrap())
    );

    // Corrupted VPKs are recorded in physical_files only and never collide in addons table
    std::fs::write(loading_dir.join("broken1.vpk"), b"not-a-vpk").unwrap();
    std::fs::write(loading_dir.join("broken2.vpk"), b"also-broken").unwrap();
    let snap_corr = engine.scan().unwrap();
    assert_eq!(snap_corr.addons.len(), 2);
    assert!(!snap_corr.addons.iter().any(|a| a.id.0 == "hash:"));

    // URL extraction ignores appid=550 and extracts id=987654321
    assert_eq!(
        l4a_core::scanner::extract_workshop_id_from_url(
            "https://steamcommunity.com/sharedfiles/filedetails/?appid=550&id=987654321"
        ),
        Some(WorkshopId::new(987654321).unwrap())
    );
    assert_eq!(
        l4a_core::scanner::extract_workshop_id_from_url(
            "https://steamcommunity.com/sharedfiles/filedetails/?appid=550"
        ),
        None
    );

    // Move back from loading/ to workshop/ restores {wid}.vpk name & enabled state and cleans dummy
    let aid_500001 = AddonId::from_workshop_id(WorkshopId::new(500001).unwrap());
    engine
        .resolve_issue(
            &resurfaced.open_issues[0].id,
            ReconcileAction::ImportToLoadingAndCreateDummy,
        )
        .unwrap();
    engine
        .apply_renames(&[(aid_500001.clone(), "Custom500001.vpk".into())])
        .unwrap();
    engine
        .toggle_addons(std::slice::from_ref(&aid_500001), false, false)
        .unwrap();

    let back_snap = engine
        .move_addons(std::slice::from_ref(&aid_500001), AddonDirType::Workshop)
        .unwrap();
    let moved_back = back_snap
        .addons
        .iter()
        .find(|a| a.id == aid_500001)
        .unwrap();
    assert_eq!(moved_back.primary_dir, Some(AddonDirType::Workshop));
    assert!(moved_back.enabled);
    assert_eq!(moved_back.current_file_name.as_deref(), Some("500001.vpk"));
    assert!(workshop_dir.join("500001.vpk").exists());
    assert!(!loading_dir.join("Custom500001.vpk.disabled").exists());
    assert!(back_snap.open_issues.is_empty());
}

#[tokio::test]
async fn downloads_can_retry_after_invalid_data_and_an_install_transaction_error() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    let workshop = loading.join("workshop");
    let fixture = dir.path().join("fixture.vpk");
    create_test_vpk(&fixture, "Remote Mod", "Author", false, None, b"download");
    let body = Arc::new(std::sync::Mutex::new(b"invalid VPK".to_vec()));
    let app = Router::new().route(
        "/mod.vpk",
        get({
            let body = body.clone();
            move || {
                let bytes = body.lock().unwrap().clone();
                async move { bytes }
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let wid = WorkshopId(654322);
    let bridge = Arc::new(MockSteamBridge::new(true));
    let mut item = item_record(wid.0, "Remote Mod", 1700000000, vec![]);
    item.file_url = Some(format!("http://{addr}/mod.vpk"));
    bridge.insert_item(item);
    let config = AppConfig {
        loading_dir: loading.to_str().unwrap().into(),
        workshop_dir: workshop.to_str().unwrap().into(),
        enable_dummy_bypass: true,
        ..Default::default()
    };
    let engine = make_engine(dir.path(), config, bridge);
    let dest = loading.join("[654322] Remote Mod.vpk");
    let dummy = workshop.join("654322.vpk");

    assert!(matches!(
        engine.download_workshop_item(wid).await,
        Err(l4a_core::CoreError::Vpk(_))
    ));
    assert!(!dest.exists());
    assert_eq!(
        engine.db().list_tasks().unwrap()[0].status,
        l4a_protocol::TaskStatus::Failed
    );

    *body.lock().unwrap() = std::fs::read(&fixture).unwrap();
    let sql = rusqlite::Connection::open(dir.path().join("runtime/l4a_v2.db")).unwrap();
    sql.execute_batch(
        "CREATE TRIGGER fail_completion BEFORE UPDATE ON tasks
         WHEN NEW.status = 'completed' BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
    )
    .unwrap();
    assert!(engine.download_workshop_item(wid).await.is_err());
    assert!(!dest.exists());
    assert!(!dummy.exists());
    assert!(engine.db().get_workshop_snapshot(wid).unwrap().is_none());
    assert!(engine.db().list_addons().unwrap().is_empty());
    assert!(engine.db().list_physical_files().unwrap().is_empty());
    assert!(engine
        .db()
        .list_tasks()
        .unwrap()
        .iter()
        .all(|task| task.status == l4a_protocol::TaskStatus::Failed));

    sql.execute_batch("DROP TRIGGER fail_completion").unwrap();
    let installed = engine.download_workshop_item(wid).await.unwrap();
    assert!(installed.has_dummy_bypass);
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        std::fs::read(&fixture).unwrap()
    );
    assert!(dummy.is_file());
    assert_eq!(
        engine.db().list_tasks().unwrap()[0].status,
        l4a_protocol::TaskStatus::Completed
    );

    assert!(matches!(
        engine.download_workshop_item(wid).await,
        Err(l4a_core::CoreError::Conflict(_))
    ));
    std::fs::remove_file(&dest).unwrap();
    assert_eq!(
        engine
            .db()
            .get_addon_by_workshop_id(wid)
            .unwrap()
            .unwrap()
            .install_state,
        InstallState::Installed
    );
    engine.download_workshop_item(wid).await.unwrap();
    assert!(dest.is_file());

    let new_loading = dir.path().join("new-addons");
    let mut config = engine.config();
    config.loading_dir = new_loading.to_str().unwrap().into();
    config.workshop_dir = new_loading.join("workshop").to_str().unwrap().into();
    config.source_order = vec!["steamworks-sdk".into()];
    engine.save_config(config).unwrap();
    let reinstalled = engine.download_workshop_item(wid).await.unwrap();
    assert_eq!(
        reinstalled.current_path.as_deref(),
        new_loading.join("[654322] Remote Mod.vpk").to_str()
    );
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        std::fs::read(fixture).unwrap()
    );
}

#[tokio::test]
async fn downloads_respect_cancellation_and_files_created_during_transfer() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    std::fs::create_dir(&loading).unwrap();
    let fixture = dir.path().join("fixture.vpk");
    create_test_vpk(&fixture, "Remote Mod", "Author", false, None, b"download");
    let requested = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let app = Router::new().route(
        "/mod.vpk",
        get({
            let requested = requested.clone();
            let release = release.clone();
            let bytes = std::fs::read(&fixture).unwrap();
            move || {
                let (requested, release, bytes) =
                    (requested.clone(), release.clone(), bytes.clone());
                async move {
                    requested.notify_one();
                    release.notified().await;
                    bytes
                }
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let wid = WorkshopId(654323);
    let bridge = Arc::new(MockSteamBridge::new(true));
    let mut item = item_record(wid.0, "Remote Mod", 1700000000, vec![]);
    item.file_url = Some(format!("http://{addr}/mod.vpk"));
    bridge.insert_item(item);
    let engine = Arc::new(make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_str().unwrap().into(),
            ..Default::default()
        },
        bridge,
    ));
    let dest = loading.join("[654323] Remote Mod.vpk");
    let download = |engine: Arc<L4aEngine>| {
        tokio::spawn(async move { engine.download_workshop_item(wid).await })
    };

    let first = download(engine.clone());
    tokio::time::timeout(std::time::Duration::from_secs(5), requested.notified())
        .await
        .unwrap();
    std::fs::write(&dest, b"outside writer").unwrap();
    release.notify_one();
    assert!(matches!(
        first.await.unwrap(),
        Err(l4a_core::CoreError::Conflict(_))
    ));
    assert_eq!(std::fs::read(&dest).unwrap(), b"outside writer");
    std::fs::remove_file(&dest).unwrap();

    let second = download(engine.clone());
    tokio::time::timeout(std::time::Duration::from_secs(5), requested.notified())
        .await
        .unwrap();
    assert!(engine.cancel_download(wid));
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), second)
            .await
            .unwrap()
            .unwrap(),
        Err(l4a_core::CoreError::Workshop(
            l4a_workshop::WorkshopError::Cancelled
        ))
    ));
    release.notify_one();
    assert!(!dest.exists());
    assert_eq!(
        engine.db().list_tasks().unwrap()[0].status,
        l4a_protocol::TaskStatus::Cancelled
    );
}

#[test]
fn batches_keep_successful_moves_and_reconcile_rejects_stale_issues() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    let workshop = loading.join("workshop");
    std::fs::create_dir_all(&workshop).unwrap();
    let first = workshop.join("791001.vpk");
    let second = workshop.join("791002.vpk.disabled");
    let conflict = workshop.join("791002.vpk");
    create_test_vpk(&first, "First", "Author", false, None, b"first");
    create_test_vpk(&second, "Second", "Author", false, None, b"second");
    let first_bytes = std::fs::read(&first).unwrap();
    let second_bytes = std::fs::read(&second).unwrap();
    // A real file at the canonical Dummy path forces the second move to roll back.
    std::fs::write(&conflict, b"external occupant").unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_str().unwrap().into(),
            workshop_dir: workshop.to_str().unwrap().into(),
            enable_dummy_bypass: true,
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );
    engine.scan().unwrap();
    let sql = rusqlite::Connection::open(dir.path().join("runtime/l4a_v2.db")).unwrap();
    sql.execute_batch(
        "CREATE TABLE test_inventory_refresh (path TEXT);
         CREATE TRIGGER count_scan_deletes AFTER DELETE ON physical_files BEGIN
             INSERT INTO test_inventory_refresh VALUES (OLD.path);
         END;",
    )
    .unwrap();
    let a = AddonId::from_workshop_id(WorkshopId(791001));
    let b = AddonId::from_workshop_id(WorkshopId(791002));
    assert!(engine
        .move_addons(&[a.clone(), b.clone()], AddonDirType::Loading)
        .is_err());
    assert_eq!(
        std::fs::read(loading.join("791001.vpk")).unwrap(),
        first_bytes
    );
    assert_eq!(std::fs::read(&second).unwrap(), second_bytes);
    assert_eq!(std::fs::read(&conflict).unwrap(), b"external occupant");
    assert!(!loading.join("791002.vpk.disabled").exists());
    assert_eq!(
        engine.db().get_addon(&a).unwrap().unwrap().primary_dir,
        Some(AddonDirType::Loading)
    );
    assert_eq!(
        engine.db().get_addon(&b).unwrap().unwrap().primary_dir,
        Some(AddonDirType::Workshop)
    );

    let replaced_rows: i64 = sql
        .query_row("SELECT COUNT(*) FROM test_inventory_refresh", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        replaced_rows, 3,
        "the initial inventory is replaced once for the batch"
    );

    create_test_vpk(&first, "First", "Author", false, None, b"updated");
    let older = engine
        .scan()
        .unwrap()
        .open_issues
        .into_iter()
        .find(|issue| issue.kind == ReconcileIssueKind::DummyOverwrittenByGameUpdate)
        .unwrap();
    create_test_vpk(&first, "First", "Author", false, None, b"updated again");
    assert!(engine
        .resolve_issue(&older.id, ReconcileAction::AdoptWorkshopUpdate)
        .is_err());
    assert_eq!(
        std::fs::read(loading.join("791001.vpk")).unwrap(),
        first_bytes
    );
    let newer = engine
        .snapshot()
        .unwrap()
        .open_issues
        .into_iter()
        .find(|issue| issue.kind == ReconcileIssueKind::DummyOverwrittenByGameUpdate)
        .unwrap();
    assert_ne!(newer.id, older.id);
    assert!(engine
        .resolve_issue(&newer.id, ReconcileAction::RemoveOrphanDummy)
        .is_err());
    let new_bytes = std::fs::read(&first).unwrap();
    engine
        .resolve_issue(&newer.id, ReconcileAction::AdoptWorkshopUpdate)
        .unwrap();
    assert_eq!(
        std::fs::read(loading.join("791001.vpk")).unwrap(),
        new_bytes
    );
}
