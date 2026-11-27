mod support;

use axum::{
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use l4a_core::db::LocalDb;
use l4a_core::{AppPaths, CoreError, EventSink, L4aEngine, NullEventSink};
use l4a_mirror::MirrorRouter;
use l4a_protocol::{
    AddonDirType, AddonId, AddonOperationOutcome, AppConfig, BatchItemsRequest, BatchItemsResponse,
    CoreEvent, DesktopError, DesktopErrorCode, GameUpdateDummyPolicy, GroupId, InstallState,
    IssueId, LoadoutProfile, MetadataSource, ProfileId, ReconcileAction, ReconcileBatchItem,
    ReconcileIssueKind, RenameRuleConfig, SearchQuery, SearchResponse, UserGroup,
    WorkshopCollectionRecord, WorkshopFetchOutcome, WorkshopId, WorkshopItemRecord,
    WorkshopItemStatus,
};
use l4a_steam::{MockSteamBridge, SubscribedItemInfo};
use l4a_workshop::{
    SourceRoutingPolicy, SteamBridgeProvider, SteamWebApiProvider, WorkshopError, WorkshopResolver,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;
use tempfile::tempdir;
use tokio::{net::TcpListener, sync::Notify};

use support::{create_test_vpk, item_record, make_engine, make_engine_with_events, RecordedEvents};

#[test]
fn concurrent_opens_migrate_legacy_task_titles() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("legacy.db");
    drop(LocalDb::open(&db_path).unwrap());
    rusqlite::Connection::open(&db_path)
        .unwrap()
        .execute_batch("ALTER TABLE tasks DROP COLUMN target_title")
        .unwrap();

    let workers = 8;
    let start = Barrier::new(workers);
    std::thread::scope(|scope| {
        let openers: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    start.wait();
                    let db = LocalDb::open(&db_path).unwrap();
                    assert!(db.list_tasks().unwrap().is_empty());
                })
            })
            .collect();
        for opener in openers {
            opener.join().unwrap();
        }
    });
}

#[test]
fn metadata_cache_migration_reparses_an_unchanged_vpk_once() {
    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir(&loading_dir).unwrap();
    create_test_vpk(
        &loading_dir.join("local.vpk"),
        "Correct title",
        "Author",
        false,
        None,
        b"v1",
    );
    let cfg = AppConfig {
        loading_dir: loading_dir.to_str().unwrap().into(),
        ..Default::default()
    };
    let open = || {
        make_engine(
            dir.path(),
            cfg.clone(),
            Arc::new(MockSteamBridge::new(false)),
        )
    };
    let engine = open();
    let id = engine.scan().unwrap().addons[0].id.clone();
    let group = engine
        .save_user_group(UserGroup {
            id: GroupId::new("migration-test").unwrap(),
            name: "My group".into(),
            tags: vec![],
            color: None,
            addon_ids: vec![id.clone()],
        })
        .unwrap();
    drop(engine);

    let db_path = AppPaths::from_runtime_dir(dir.path().join("runtime")).db_path;
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute_batch(
        r#"DELETE FROM schema_migrations WHERE version = 2;
           UPDATE addons SET title = 'Cached title', addon_info_json = '{"vltava":"bad field"}';"#,
    )
    .unwrap();
    drop(conn);

    let engine = open();
    assert!(engine.db().list_physical_files().unwrap().is_empty());
    let before_scan = engine.snapshot().unwrap();
    assert_eq!(before_scan.addons[0].title, "Cached title");
    assert_eq!(before_scan.user_groups, vec![group.clone()]);
    let after_scan = engine.scan().unwrap();
    assert_eq!(after_scan.addons[0].id, id);
    assert_eq!(after_scan.addons[0].title, "Correct title");
    assert_eq!(after_scan.user_groups, vec![group]);
    drop(engine);

    assert_eq!(open().db().list_physical_files().unwrap().len(), 1);
}

#[test]
fn reconcile_batch_reports_partial_failure_and_emits_one_update() {
    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    let workshop_dir = loading_dir.join("workshop");
    std::fs::create_dir_all(&workshop_dir).unwrap();
    for file in ["123456.vpk", "234567.vpk"] {
        create_test_vpk(&workshop_dir.join(file), file, "Author", false, None, b"v1");
    }
    let events = Arc::new(RecordedEvents::default());
    let engine = make_engine_with_events(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            workshop_dir: workshop_dir.to_str().unwrap().into(),
            enable_dummy_bypass: true,
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
        events.clone(),
    );
    let snapshot = engine.scan().unwrap();
    assert_eq!(snapshot.open_issues.len(), 2);
    events.drain();
    let items = [
        ReconcileBatchItem {
            issue_id: snapshot.open_issues[0].id.clone(),
            action: ReconcileAction::Ignore,
        },
        ReconcileBatchItem {
            issue_id: IssueId("stale".into()),
            action: ReconcileAction::Ignore,
        },
        ReconcileBatchItem {
            issue_id: snapshot.open_issues[1].id.clone(),
            action: ReconcileAction::LeaveInWorkshop,
        },
    ];
    let report = engine.resolve_issues_batch(&items).unwrap();
    assert_eq!(report.items.len(), items.len());
    for (result, item) in report.items.iter().zip(&items) {
        assert_eq!(result.issue_id, item.issue_id);
        assert_eq!(result.action, item.action);
    }
    for i in [0, 2] {
        assert!(report.items[i].ok);
        assert!(report.items[i].error.is_none());
    }
    assert!(!report.items[1].ok);
    assert_eq!(
        report.items[1].error.as_ref().unwrap().code,
        DesktopErrorCode::Conflict
    );
    assert!(report.snapshot.open_issues.is_empty());
    assert_eq!(
        events.drain(),
        vec![CoreEvent::SnapshotUpdated {
            external_change: false
        }]
    );
    assert!(engine.scan().unwrap().open_issues.is_empty());
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
    let denied = engine
        .apply_renames(&[(aid.clone(), "Illegal Workshop Rename.vpk".into())])
        .unwrap();
    assert!(matches!(
        denied.items[0].outcome,
        AddonOperationOutcome::Failed {
            error: DesktopError {
                code: DesktopErrorCode::Conflict,
                ..
            },
        }
    ));

    // Move to loading/ then rename without workshop ID prefix -> rescan keeps workshop_id!
    engine
        .move_addons(std::slice::from_ref(&aid), AddonDirType::Loading)
        .unwrap();
    let rename = engine
        .apply_renames(&[(aid.clone(), "本地模组.vpk".into())])
        .unwrap();
    assert_eq!(rename.items[0].outcome, AddonOperationOutcome::Applied);
    let snap = engine.snapshot().unwrap();
    assert_eq!(
        snap.addons[0].current_file_name.as_deref(),
        Some("本地模组.vpk")
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
    create_test_vpk(
        &loading_dir.join("s1.vpk"),
        "Zoey Model Part 1",
        "AuthorS",
        false,
        None,
        b"s1",
    );
    create_test_vpk(
        &loading_dir.join("s2.vpk"),
        "Zoey Model Part 2",
        "AuthorS",
        false,
        None,
        b"s2",
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

    // scan() automatically builds the campaign bundle for Yama (clean name "Yama", ordered parts)
    // and ignores non-campaign Zoey Model Part 1/2!
    assert_eq!(snap.bundles.len(), 1);
    assert_eq!(snap.bundles[0].name, "Yama");
    assert_eq!(
        snap.bundles[0].member_addon_ids,
        vec![p1_id.clone(), p2_id.clone()]
    );
    assert_eq!(snap.bundles[0].maps.len(), 2);

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
    engine.track_collection(col_id, true).await.unwrap();
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
    assert_eq!(apply_report.disabled_count, 3);
    assert!(apply_report
        .batch
        .items
        .iter()
        .any(|i| i.addon_id == w1_id && i.outcome == AddonOperationOutcome::Applied));
    assert_eq!(apply_report.batch.refresh_error, None);
    assert!(loading_dir.join("w1.vpk.disabled").exists());
    assert!(loading_dir.join("s1.vpk.disabled").exists());
    assert!(loading_dir.join("s2.vpk.disabled").exists());
    assert!(loading_dir.join("p1.vpk").exists());
}

#[tokio::test]
async fn existing_settings_default_to_https_for_catalog_and_covers_until_disabled() {
    let dir = tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/api/v1/items/search",
            get({
                let requests = requests.clone();
                move || async move {
                    requests.fetch_add(1, Ordering::Relaxed);
                    Json(SearchResponse {
                        items: vec![],
                        total: 0,
                        page: 1,
                        page_size: 30,
                        source: MetadataSource::CatalogServer,
                    })
                }
            }),
        )
        .route(
            "/cover.jpg",
            get({
                let requests = requests.clone();
                move || async move {
                    requests.fetch_add(1, Ordering::Relaxed);
                    ([("content-type", "image/jpeg")], vec![0xff, 0xd8, 0xff])
                }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let paths = AppPaths::from_runtime_dir(dir.path());
    paths.ensure_dirs().unwrap();
    std::fs::write(
        &paths.settings_path,
        format!(
            "loadingDir = 'addons'\nworkshopDir = 'addons/workshop'\n\
             catalogServerUrl = 'http://{addr}'\nsourceOrder = ['catalog-server']\n"
        ),
    )
    .unwrap();
    let engine = L4aEngine::open_default(dir.path(), Arc::new(NullEventSink)).unwrap();
    let query = SearchQuery::default();
    let cover_url = format!("http://{addr}/cover.jpg");
    assert!(engine.config().https_only);
    assert!(engine
        .search_workshop(&query)
        .await
        .unwrap_err()
        .to_string()
        .contains("HTTPS"));
    assert!(engine
        .fetch_workshop_cover(&cover_url)
        .await
        .unwrap_err()
        .to_string()
        .contains("HTTPS"));
    assert_eq!(requests.load(Ordering::Relaxed), 0);

    let config = engine
        .save_config(AppConfig {
            https_only: false,
            ..engine.config()
        })
        .unwrap();
    assert!(
        !l4a_core::load_config(&paths.settings_path)
            .unwrap()
            .https_only
    );
    assert_eq!(
        engine.search_workshop(&query).await.unwrap().source,
        MetadataSource::CatalogServer
    );
    assert_eq!(
        engine.fetch_workshop_cover(&cover_url).await.unwrap(),
        ("image/jpeg".to_string(), vec![0xff, 0xd8, 0xff])
    );

    engine
        .save_config(AppConfig {
            https_only: true,
            ..config
        })
        .unwrap();
    assert!(engine.search_workshop(&query).await.is_err());
    assert!(engine.fetch_workshop_cover(&cover_url).await.is_err());
    assert_eq!(requests.load(Ordering::Relaxed), 2);
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
        max_download_retries: 0,
        enable_dummy_bypass: true,
        source_order: vec!["steamworks-sdk".into()],
        ..Default::default()
    };
    let engine = make_engine(dir.path(), cfg, bridge);

    let blocked = engine.download_workshop_item(wid).await.unwrap_err();
    assert!(
        matches!(blocked, CoreError::Workshop(WorkshopError::Network(message)) if message.contains("HTTPS"))
    );
    engine
        .save_config(AppConfig {
            https_only: false,
            ..engine.config()
        })
        .unwrap();

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

    let report = engine
        .apply_renames(&[
            (snap.addons[0].id.clone(), "Shared.vpk".into()),
            (snap.addons[1].id.clone(), "Shared.vpk".into()),
        ])
        .unwrap();
    assert_eq!(report.items[0].outcome, AddonOperationOutcome::NotAttempted);
    assert!(matches!(
        report.items[1].outcome,
        AddonOperationOutcome::Failed {
            error: DesktopError {
                code: DesktopErrorCode::Conflict,
                ..
            },
        }
    ));
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

    let returned = engine
        .move_addons(std::slice::from_ref(&aid_500001), AddonDirType::Workshop)
        .unwrap();
    assert_eq!(returned.items[0].outcome, AddonOperationOutcome::Applied);
    let back_snap = engine.snapshot().unwrap();
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
        https_only: false,
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
            https_only: false,
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
    let c = AddonId::from_workshop_id(WorkshopId(791003));
    let report = engine
        .move_addons(
            &[a.clone(), a.clone(), b.clone(), c.clone()],
            AddonDirType::Loading,
        )
        .unwrap();
    assert_eq!(report.refresh_error, None);
    let wire = serde_json::to_value(&report).unwrap();
    assert_eq!(wire["items"].as_array().unwrap().len(), 3);
    assert_eq!(
        wire["items"][0],
        serde_json::json!({ "addonId": a, "status": "applied" })
    );
    assert_eq!(wire["items"][1]["addonId"], serde_json::json!(b));
    assert_eq!(wire["items"][1]["status"], "failed");
    assert_eq!(wire["items"][1]["error"]["code"], "conflict");
    assert_eq!(
        wire["items"][2],
        serde_json::json!({ "addonId": c, "status": "not_attempted" })
    );
    assert!(wire.get("refreshError").is_none());
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

#[test]
fn batch_toggle_reports_a_rolled_back_item_and_an_inventory_refresh_error() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    std::fs::create_dir_all(&loading).unwrap();
    for id in 810001..=810003 {
        create_test_vpk(
            &loading.join(format!("{id}.vpk")),
            &id.to_string(),
            "Author",
            false,
            None,
            b"mod",
        );
    }
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_string_lossy().into_owned(),
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );
    engine.scan().unwrap();
    let sql = rusqlite::Connection::open(dir.path().join("runtime/l4a_v2.db")).unwrap();
    sql.execute_batch(
        "CREATE TRIGGER fail_toggle BEFORE UPDATE OF enabled ON addons
         WHEN OLD.workshop_id = 810002 AND NEW.enabled != OLD.enabled BEGIN
             SELECT RAISE(ABORT, 'toggle failed');
         END;",
    )
    .unwrap();

    let first = AddonId::from_workshop_id(WorkshopId(810001));
    let second = AddonId::from_workshop_id(WorkshopId(810002));
    let third = AddonId::from_workshop_id(WorkshopId(810003));
    let report = engine
        .toggle_addons(&[first.clone(), second.clone(), third], false, false)
        .unwrap();
    assert_eq!(report.refresh_error, None);
    assert_eq!(report.items[0].outcome, AddonOperationOutcome::Applied);
    assert!(matches!(
        report.items[1].outcome,
        AddonOperationOutcome::Failed {
            error: DesktopError {
                code: DesktopErrorCode::Database,
                ..
            },
        }
    ));
    assert_eq!(report.items[2].outcome, AddonOperationOutcome::NotAttempted);
    assert!(loading.join("810001.vpk.disabled").is_file());
    assert!(loading.join("810002.vpk").is_file());
    assert!(!loading.join("810002.vpk.disabled").exists());
    assert!(loading.join("810003.vpk").is_file());

    sql.execute_batch(
        "DROP TRIGGER fail_toggle;
         CREATE TRIGGER fail_refresh BEFORE DELETE ON physical_files BEGIN
             SELECT RAISE(ABORT, 'refresh failed');
         END;",
    )
    .unwrap();
    let report = engine
        .toggle_addons(&[first, second.clone()], false, false)
        .unwrap();
    assert_eq!(report.items[0].outcome, AddonOperationOutcome::Unchanged);
    assert_eq!(report.items[1].outcome, AddonOperationOutcome::Applied);
    assert_eq!(
        serde_json::to_value(&report).unwrap()["refreshError"]["code"],
        "database"
    );
    assert!(loading.join("810002.vpk.disabled").is_file());
    assert!(!engine.db().get_addon(&second).unwrap().unwrap().enabled);

    let new_name = "[810009] 本地模组.vpk";
    let renamed = engine
        .apply_renames(&[(second.clone(), new_name.into())])
        .unwrap();
    assert_eq!(renamed.items[0].outcome, AddonOperationOutcome::Applied);
    assert!(renamed.refresh_error.is_some());

    sql.execute_batch("DROP TRIGGER fail_refresh;").unwrap();
    let scanned = engine.scan().unwrap();
    let renamed_addon = scanned
        .addons
        .iter()
        .find(|addon| addon.id == second)
        .unwrap();
    assert_eq!(renamed_addon.current_file_name.as_deref(), Some(new_name));
    assert_eq!(renamed_addon.workshop_id, Some(WorkshopId(810002)));
    let retried = engine.toggle_addons(&[second], false, false).unwrap();
    assert_eq!(retried.refresh_error, None);
    assert_eq!(retried.items[0].outcome, AddonOperationOutcome::Unchanged);
}

#[test]
fn delete_reports_restore_files_if_the_record_cannot_be_removed() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    let workshop = loading.join("workshop");
    std::fs::create_dir_all(&workshop).unwrap();
    let workshop_file = workshop.join("820001.vpk");
    create_test_vpk(&workshop_file, "Delete", "Author", false, None, b"original");
    let original_bytes = std::fs::read(&workshop_file).unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_string_lossy().into_owned(),
            workshop_dir: workshop.to_string_lossy().into_owned(),
            enable_dummy_bypass: true,
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );
    engine.scan().unwrap();
    let id = AddonId::from_workshop_id(WorkshopId(820001));
    let ids = std::slice::from_ref(&id);
    engine.move_addons(ids, AddonDirType::Loading).unwrap();
    let local_file = loading.join("820001.vpk");
    let dummy_bytes = std::fs::read(&workshop_file).unwrap();

    let report = engine.delete_addons(ids, false, true).unwrap();
    assert!(matches!(
        report.items[0].outcome,
        AddonOperationOutcome::Failed {
            error: DesktopError {
                code: DesktopErrorCode::Conflict,
                ..
            },
        }
    ));
    let sql = rusqlite::Connection::open(dir.path().join("runtime/l4a_v2.db")).unwrap();
    sql.execute_batch(
        "CREATE TRIGGER fail_delete BEFORE DELETE ON addons BEGIN
             SELECT RAISE(ABORT, 'delete failed');
         END;",
    )
    .unwrap();
    let report = engine.delete_addons(ids, true, true).unwrap();
    assert!(matches!(
        report.items[0].outcome,
        AddonOperationOutcome::Failed {
            error: DesktopError {
                code: DesktopErrorCode::Database,
                ..
            },
        }
    ));
    assert_eq!(report.refresh_error, None);
    assert_eq!(std::fs::read(&local_file).unwrap(), original_bytes);
    assert_eq!(std::fs::read(&workshop_file).unwrap(), dummy_bytes);

    sql.execute_batch("DROP TRIGGER fail_delete;").unwrap();
    let report = engine.delete_addons(ids, true, false).unwrap();
    assert_eq!(report.items[0].outcome, AddonOperationOutcome::Applied);
    assert!(!local_file.exists());
    assert!(!workshop_file.exists());
    assert_eq!(
        engine.db().get_addon(&id).unwrap().unwrap().install_state,
        InstallState::UninstalledKnown
    );
    let report = engine.delete_addons(ids, false, true).unwrap();
    assert_eq!(report.items[0].outcome, AddonOperationOutcome::Applied);
    assert!(engine.db().get_addon(&id).unwrap().is_none());
    assert_eq!(
        engine.delete_addons(ids, false, true).unwrap().items[0].outcome,
        AddonOperationOutcome::Unchanged
    );
}

#[test]
fn cached_images_require_a_regular_file_and_a_valid_cache_key() {
    let dir = tempdir().unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig::default(),
        Arc::new(MockSteamBridge::new(false)),
    );
    let cache = AppPaths::from_runtime_dir(dir.path().join("runtime")).image_cache_dir;
    for key in ["cover.jpg", "cover.jpeg", "cover.png", "cover.webp"] {
        std::fs::write(cache.join(key), b"cached image bytes").unwrap();
        assert_eq!(
            engine.read_cached_image(key).unwrap(),
            b"cached image bytes"
        );
    }
    std::fs::create_dir(cache.join("directory.png")).unwrap();
    for invalid in [
        "",
        ".",
        "..",
        "../cover.jpg",
        "sub/cover.jpg",
        "sub\\cover.jpg",
        "a..b.jpg",
        "cover\0.png",
        "cover.JPG",
        "cover.png.exe",
        "cover",
        "directory.png",
    ] {
        assert!(
            matches!(
                engine.read_cached_image(invalid),
                Err(CoreError::InvalidInput(_))
            ),
            "accepted {invalid:?}"
        );
    }
    let outside = dir.path().join("outside.png");
    std::fs::write(&outside, b"outside cache").unwrap();
    assert!(matches!(
        engine.read_cached_image(outside.to_str().unwrap()),
        Err(CoreError::InvalidInput(_))
    ));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside, cache.join("alias.png")).unwrap();
        assert!(matches!(
            engine.read_cached_image("alias.png"),
            Err(CoreError::InvalidInput(_))
        ));
    }
    assert!(matches!(
        engine.read_cached_image("missing.webp"),
        Err(CoreError::NotFound(_))
    ));
}

#[test]
fn obsolete_dependency_deletion_checks_all_installed_parents() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    std::fs::create_dir(&loading).unwrap();
    let (parent, disabled_parent, dependency) =
        (WorkshopId(830001), WorkshopId(830002), WorkshopId(830003));
    for (wid, suffix) in [
        (parent, ""),
        (disabled_parent, ".disabled"),
        (dependency, ""),
    ] {
        create_test_vpk(
            &loading.join(format!("{wid}.vpk{suffix}")),
            &format!("Addon {wid}"),
            "Author",
            false,
            None,
            b"payload",
        );
    }
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_str().unwrap().into(),
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );
    engine.scan().unwrap();
    for id in [parent, disabled_parent] {
        engine
            .db()
            .upsert_workshop_snapshot(
                &item_record(id.0, "Parent", 1700000000, vec![dependency.0]),
                true,
            )
            .unwrap();
    }
    let error = engine
        .safe_delete_obsolete_dependency(dependency, true, true)
        .unwrap_err();
    assert!(matches!(error, CoreError::Conflict(message) if message.contains("830001, 830002")));
    let dependency_path = loading.join(format!("{dependency}.vpk"));
    assert!(dependency_path.is_file());
    assert!(engine
        .db()
        .get_addon_by_workshop_id(dependency)
        .unwrap()
        .is_some());

    engine
        .db()
        .upsert_workshop_snapshot(&item_record(parent.0, "Parent", 1700000001, vec![]), true)
        .unwrap();
    assert!(matches!(
        engine.safe_delete_obsolete_dependency(dependency, true, true),
        Err(CoreError::Conflict(message)) if message.contains("830002") && !message.contains("830001")
    ));
    let removed_parent = engine
        .delete_addons(&[AddonId::from_workshop_id(disabled_parent)], true, false)
        .unwrap();
    assert_eq!(
        removed_parent.items[0].outcome,
        AddonOperationOutcome::Applied
    );
    assert_eq!(
        engine.db().list_all_active_dependency_edges().unwrap(),
        vec![(disabled_parent, dependency)]
    );

    let report = engine
        .safe_delete_obsolete_dependency(dependency, true, true)
        .unwrap();
    assert_eq!(report.items[0].outcome, AddonOperationOutcome::Applied);
    assert_eq!(report.refresh_error, None);
    assert!(!dependency_path.exists());
    assert!(engine
        .db()
        .get_addon_by_workshop_id(dependency)
        .unwrap()
        .is_none());
    assert!(matches!(
        engine.safe_delete_obsolete_dependency(dependency, true, true),
        Err(CoreError::NotFound(_))
    ));
}

#[test]
fn overwritten_dummy_issues_stay_open_with_bypass_disabled() {
    for policy in [
        GameUpdateDummyPolicy::AutoKeepLocalRegenerateDummy,
        GameUpdateDummyPolicy::AutoAdoptKeepCustomizations,
    ] {
        let dir = tempdir().unwrap();
        let loading = dir.path().join("addons");
        let workshop = loading.join("workshop");
        std::fs::create_dir_all(&workshop).unwrap();
        let local = loading.join("830004.vpk");
        let upstream = workshop.join("830004.vpk");
        create_test_vpk(&local, "Local", "Author", false, None, b"local");
        create_test_vpk(&upstream, "Upstream", "Author", false, None, b"new update");
        let local_bytes = std::fs::read(&local).unwrap();
        let upstream_bytes = std::fs::read(&upstream).unwrap();
        let engine = make_engine(
            dir.path(),
            AppConfig {
                loading_dir: loading.to_str().unwrap().into(),
                workshop_dir: workshop.to_str().unwrap().into(),
                game_update_dummy_policy: policy,
                enable_dummy_bypass: false,
                auto_adopt_new_workshop_downloads: true,
                ..Default::default()
            },
            Arc::new(MockSteamBridge::new(false)),
        );
        let snapshot = engine.scan().unwrap();
        assert_eq!(snapshot.open_issues.len(), 1);
        assert_eq!(
            snapshot.open_issues[0].kind,
            ReconcileIssueKind::DummyOverwrittenByGameUpdate
        );
        assert_eq!(std::fs::read(local).unwrap(), local_bytes);
        assert_eq!(std::fs::read(upstream).unwrap(), upstream_bytes);
    }
}

#[tokio::test]
async fn retracking_keeps_removed_members_and_sync_skips_manual_collections() {
    let dir = tempdir().unwrap();
    let bridge = Arc::new(MockSteamBridge::new(true));
    let engine = make_engine(dir.path(), AppConfig::default(), bridge.clone());
    let (collection, absent, returns, kept) = (
        WorkshopId(840000),
        WorkshopId(840001),
        WorkshopId(840002),
        WorkshopId(840003),
    );
    let set_children = |child_ids| {
        bridge.insert_collection(WorkshopCollectionRecord {
            collection_id: collection,
            status: WorkshopItemStatus::Active,
            title: Some("Campaign collection".into()),
            description: None,
            preview_url: None,
            child_ids,
            items: vec![],
            source: MetadataSource::SteamworksSdk,
            fetched_at: Utc::now(),
        });
    };
    set_children(vec![absent, returns, kept]);
    engine.track_collection(collection, true).await.unwrap();
    set_children(vec![kept]);
    assert_eq!(
        engine.sync_workshop().await.unwrap().collection_diffs[0].removed_items,
        vec![absent, returns]
    );

    set_children(vec![kept, returns]);
    let tracked = engine.track_collection(collection, false).await.unwrap();
    assert_eq!(tracked.member_workshop_ids, vec![kept, returns]);
    assert_eq!(tracked.removed_upstream_ids, vec![absent]);
    assert!(!tracked.auto_sync);
    let retracked = engine.track_collection(collection, false).await.unwrap();
    assert_eq!(retracked.removed_upstream_ids, tracked.removed_upstream_ids);

    set_children(vec![absent]);
    let report = engine.sync_workshop().await.unwrap();
    assert_eq!(report.checked_items, 0);
    assert!(report.collection_diffs.is_empty());
    assert_eq!(engine.snapshot().unwrap().collections, vec![retracked]);
}

#[test]
fn deleting_a_bundle_cascades_its_members_and_emits_a_snapshot_update() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    std::fs::create_dir(&loading).unwrap();
    create_test_vpk(
        &loading.join("850001.vpk"),
        "Addon",
        "Author",
        false,
        None,
        b"payload",
    );
    let events = Arc::new(RecordedEvents::default());
    let engine = make_engine_with_events(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_str().unwrap().into(),
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
        events.clone(),
    );
    let aid = engine.scan().unwrap().addons[0].id.clone();
    let bundle = engine
        .create_manual_bundle("Delete me".into(), vec![aid.clone()])
        .unwrap();
    let kept = engine
        .create_manual_bundle("Keep me".into(), vec![aid.clone()])
        .unwrap();
    events.drain();
    engine.delete_bundle(&bundle.id).unwrap();
    assert_eq!(
        events.drain(),
        vec![CoreEvent::SnapshotUpdated {
            external_change: false
        }]
    );
    let snapshot = engine.snapshot().unwrap();
    assert_eq!(snapshot.bundles, vec![kept]);
    assert_eq!(snapshot.addons[0].id, aid);
    let sql = rusqlite::Connection::open(dir.path().join("runtime/l4a_v2.db")).unwrap();
    let members: i64 = sql
        .query_row(
            "SELECT COUNT(*) FROM bundle_members WHERE bundle_id = ?1",
            [bundle.id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(members, 0);
    assert!(matches!(
        engine.delete_bundle(&bundle.id),
        Err(CoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn workshop_item_inspection_reports_removed_private_and_missing_items() {
    let dir = tempdir().unwrap();
    let bridge = Arc::new(MockSteamBridge::new(true));
    let engine = make_engine(dir.path(), AppConfig::default(), bridge.clone());
    let active = item_record(860001, "Available", 1700000000, vec![860002]);
    bridge.insert_item(active.clone());
    bridge.insert_outcome(WorkshopFetchOutcome::Removed {
        workshop_id: WorkshopId(860002),
        source: MetadataSource::SteamworksSdk,
        observed_at: Utc::now(),
        reason: None,
    });
    bridge.insert_outcome(WorkshopFetchOutcome::PrivateOrUnavailable {
        workshop_id: WorkshopId(860003),
        source: MetadataSource::SteamworksSdk,
        observed_at: Utc::now(),
        reason: None,
    });
    assert_eq!(
        engine
            .fetch_workshop_item(active.workshop_id)
            .await
            .unwrap(),
        active
    );
    for id in [860002, 860003, 860004] {
        assert!(matches!(
            engine.fetch_workshop_item(WorkshopId(id)).await,
            Err(CoreError::NotFound(_))
        ));
    }
}

#[tokio::test]
async fn subscribed_workshop_ids_come_from_the_steam_bridge() {
    let dir = tempdir().unwrap();
    let bridge = Arc::new(MockSteamBridge::new(true));
    let ids = [WorkshopId(870002), WorkshopId(870001)];
    let engine = make_engine(dir.path(), AppConfig::default(), bridge.clone());
    assert!(engine.subscribed_workshop_ids().await.unwrap().is_empty());
    bridge.set_subscribed(
        ids.iter()
            .map(|id| SubscribedItemInfo {
                workshop_id: *id,
                item_state: vec!["subscribed".into()],
                install_folder: None,
                size_on_disk: None,
            })
            .collect(),
    );
    assert_eq!(engine.subscribed_workshop_ids().await.unwrap(), ids);

    let disabled = tempdir().unwrap();
    let engine = make_engine(
        disabled.path(),
        AppConfig::default(),
        Arc::new(MockSteamBridge::new(false)),
    );
    assert!(matches!(
        engine.subscribed_workshop_ids().await,
        Err(CoreError::Workshop(WorkshopError::Unsupported(_)))
    ));
}

#[test]
fn deleting_part_clears_stale_auto_bundle_and_config_roundtrips_preferences() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    std::fs::create_dir_all(&loading).unwrap();
    create_test_vpk(
        &loading.join("sakura_1.vpk"),
        "樱花战役",
        "Mapper",
        true,
        None,
        b"part-1",
    );
    create_test_vpk(
        &loading.join("sakura_2.vpk"),
        "樱花战役",
        "Mapper",
        false,
        None,
        b"part-2",
    );

    let cfg = AppConfig {
        loading_dir: loading.to_string_lossy().into_owned(),
        addon_name_preference: l4a_protocol::AddonNamePreference::FileName,
        collapse_campaign_bundles: false,
        ..Default::default()
    };
    let engine = make_engine(dir.path(), cfg, Arc::new(MockSteamBridge::new(false)));
    let snap = engine.scan().unwrap();
    assert_eq!(snap.bundles.len(), 1);
    assert_eq!(snap.bundles[0].member_addon_ids.len(), 2);

    // Deleting one part immediately drops the 1-member auto-bundle without needing a manual rescan.
    let first_id = snap.addons[0].id.clone();
    engine.delete_addons(&[first_id], true, true).unwrap();
    let after_delete = engine.snapshot().unwrap();
    assert_eq!(after_delete.addons.len(), 1);
    assert!(after_delete.bundles.is_empty());

    // Saving config writes valid TOML with scalar preferences preserved across Engine restarts.
    let updated_cfg = AppConfig {
        addon_name_preference: l4a_protocol::AddonNamePreference::FileName,
        collapse_campaign_bundles: false,
        ..engine.config()
    };
    engine.save_config(updated_cfg).unwrap();
    drop(engine);

    let reopened =
        L4aEngine::open_default(dir.path().join("runtime"), Arc::new(NullEventSink)).unwrap();
    assert_eq!(
        reopened.config().addon_name_preference,
        l4a_protocol::AddonNamePreference::FileName
    );
    assert!(!reopened.config().collapse_campaign_bundles);
}

async fn serve_workshop_fixture(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn serve_catalog_items(items: Arc<Mutex<Vec<WorkshopItemRecord>>>) -> String {
    serve_workshop_fixture(Router::new().route(
        "/api/v1/items/batch",
        post(move |Json(batch): Json<BatchItemsRequest>| async move {
            Json(BatchItemsResponse {
                items: items
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|item| batch.ids.contains(&item.workshop_id))
                    .cloned()
                    .collect(),
                missing_ids: Vec::new(),
            })
        }),
    ))
    .await
}

#[tokio::test]
async fn hydration_uses_web_api_without_sdk_and_legacy_urls_work_through_http_mirror() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    std::fs::create_dir_all(&loading).unwrap();
    let fixture = dir.path().join("fixture.vpk");
    create_test_vpk(&fixture, "Remote Mod", "Mapper", false, None, b"remote");
    create_test_vpk(
        &loading.join("909901.vpk"),
        "Local Mod",
        "Mapper",
        false,
        None,
        b"local",
    );

    let cover = "http://cloud-3.steamusercontent.com/ugc/909901/cover.jpg";
    let file = "http://cloud-3.steamusercontent.com/ugc/909902/mod.vpk";
    let body = std::fs::read(&fixture).unwrap();
    let app = Router::new()
        .route(
            "/ISteamRemoteStorage/GetPublishedFileDetails/v1/",
            post(move || async move {
                Json(serde_json::json!({"response": {"publishedfiledetails": [
                    {"publishedfileid": "909901", "result": 1, "title": "Web API Mod", "preview_url": cover, "file_url": file},
                    {"publishedfileid": "909902", "result": 1, "title": "Remote Mod", "file_url": file}
                ]}}))
            }),
        )
        .route(
            "/ugc/909901/cover.jpg",
            get(|| async { ([("content-type", "image/jpeg")], vec![0xff, 0xd8, 0xff]) }),
        )
        .route("/ugc/909902/mod.vpk", get(move || async move { body }));
    let base_url = serve_workshop_fixture(app).await;
    let mirror = Arc::new(MirrorRouter::from_rules(vec![serde_json::from_value(
        serde_json::json!({
            "rules": "cloud-3.steamusercontent.com",
            "target": base_url.strip_prefix("http://").unwrap(),
        }),
    )
    .unwrap()]));

    let bridge = Arc::new(MockSteamBridge::new(true));
    let mut steam_item = item_record(909901, "SDK Mod", 1_710_000_000, vec![]);
    steam_item.file_url = Some(file.into());
    bridge.insert_item(steam_item);
    let resolver = Arc::new(WorkshopResolver::new(
        vec![
            Arc::new(SteamBridgeProvider::new(bridge.clone())),
            Arc::new(SteamWebApiProvider::with_base_url(base_url, mirror.clone())),
        ],
        SourceRoutingPolicy::default(),
    ));
    let paths = AppPaths::from_runtime_dir(dir.path().join("runtime"));
    let engine = L4aEngine::with_dependencies(
        paths,
        Arc::new(LocalDb::open_in_memory().unwrap()),
        AppConfig {
            loading_dir: loading.to_string_lossy().into_owned(),
            https_only: false,
            max_download_retries: 0,
            ..Default::default()
        },
        mirror,
        bridge,
        resolver,
        Arc::new(NullEventSink),
    )
    .unwrap();
    engine.scan().unwrap();

    let wid = WorkshopId(909901);
    assert_eq!(engine.hydrate_missing_workshop_metadata().await.unwrap(), 1);
    let snapshot = engine.db().get_workshop_snapshot(wid).unwrap().unwrap();
    assert_eq!(snapshot.source, MetadataSource::SteamWebApi);
    assert_eq!(snapshot.file_url.as_deref(), Some(file));
    let cover_url = engine.snapshot().unwrap().addons[0]
        .cover_image
        .clone()
        .unwrap();
    assert_eq!(cover_url, cover);
    assert_eq!(
        engine.fetch_workshop_cover(&cover_url).await.unwrap(),
        ("image/jpeg".into(), vec![0xff, 0xd8, 0xff])
    );
    let downloaded = engine
        .download_workshop_item(WorkshopId(909902))
        .await
        .unwrap();
    assert_eq!(downloaded.install_state, InstallState::Installed);
    assert_eq!(
        engine.fetch_workshop_item(wid).await.unwrap().title,
        "SDK Mod"
    );
}

#[tokio::test]
async fn hydration_retries_web_api_item_timeouts_after_backoff() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    std::fs::create_dir_all(&loading).unwrap();
    let wid = WorkshopId(909905);
    create_test_vpk(
        &loading.join(format!("{wid}.vpk")),
        "Local Mod",
        "Mapper",
        false,
        None,
        b"local",
    );

    let requests = Arc::new(AtomicUsize::new(0));
    let base_url = serve_workshop_fixture(Router::new().route(
        "/ISteamRemoteStorage/GetPublishedFileDetails/v1/",
        post({
            let requests = requests.clone();
            move || async move {
                let details = if requests.fetch_add(1, Ordering::SeqCst) == 0 {
                    serde_json::json!({"publishedfileid": wid.to_string(), "result": 16})
                } else {
                    serde_json::json!({
                        "publishedfileid": wid.to_string(),
                        "result": 1,
                        "title": "Workshop Mod"
                    })
                };
                Json(serde_json::json!({"response": {"publishedfiledetails": [details]}}))
            }
        }),
    ))
    .await;
    let mirror = Arc::new(MirrorRouter::empty());
    let resolver = Arc::new(WorkshopResolver::new(
        vec![Arc::new(SteamWebApiProvider::with_base_url(
            base_url,
            mirror.clone(),
        ))],
        SourceRoutingPolicy::default(),
    ));
    let engine = L4aEngine::with_dependencies(
        AppPaths::from_runtime_dir(dir.path().join("runtime")),
        Arc::new(LocalDb::open_in_memory().unwrap()),
        AppConfig {
            loading_dir: loading.to_string_lossy().into_owned(),
            https_only: false,
            ..Default::default()
        },
        mirror,
        Arc::new(MockSteamBridge::new(false)),
        resolver,
        Arc::new(NullEventSink),
    )
    .unwrap();
    engine.scan().unwrap();

    assert_eq!(engine.hydrate_missing_workshop_metadata().await.unwrap(), 0);
    assert!(engine.db().get_workshop_snapshot(wid).unwrap().is_none());
    assert_eq!(engine.hydrate_missing_workshop_metadata().await.unwrap(), 0);
    assert_eq!(requests.load(Ordering::SeqCst), 1);

    engine.expire_hydration_backoff_for_test();
    engine.scan().unwrap();
    assert_eq!(engine.hydrate_missing_workshop_metadata().await.unwrap(), 1);
    let addon = engine.db().get_addon_by_workshop_id(wid).unwrap().unwrap();
    assert_eq!(addon.remote_status, WorkshopItemStatus::Active);
    assert_eq!(addon.workshop_title.as_deref(), Some("Workshop Mod"));
}

#[tokio::test]
async fn hydration_discards_stale_tombstones_and_backoff_after_catalog_change() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    std::fs::create_dir_all(&loading).unwrap();
    for (wid, title) in [(909903, "First Mod"), (909904, "Second Mod")] {
        create_test_vpk(
            &loading.join(format!("{wid}.vpk")),
            title,
            "Mapper",
            false,
            None,
            b"local",
        );
    }

    let old_request = Arc::new(Notify::new());
    let release_old = Arc::new(Notify::new());
    let old_url = serve_workshop_fixture(Router::new().route(
        "/api/v1/items/batch",
        post({
            let old_request = old_request.clone();
            let release_old = release_old.clone();
            move || async move {
                old_request.notify_one();
                release_old.notified().await;
                let mut removed = item_record(909903, "Old Catalog", 1_710_000_000, vec![]);
                removed.status = WorkshopItemStatus::Removed;
                Json(BatchItemsResponse {
                    items: vec![removed],
                    missing_ids: vec![WorkshopId(909904)],
                })
            }
        }),
    ))
    .await;
    let new_url = serve_catalog_items(Arc::new(Mutex::new(vec![
        item_record(909903, "First From New Catalog", 1_720_000_000, vec![]),
        item_record(909904, "Second From New Catalog", 1_720_000_000, vec![]),
    ])))
    .await;
    let config = AppConfig {
        loading_dir: loading.to_string_lossy().into_owned(),
        https_only: false,
        catalog_server_url: Some(old_url),
        source_order: vec!["catalog-server".into()],
        ..Default::default()
    };
    let engine = Arc::new(make_engine(
        dir.path(),
        config.clone(),
        Arc::new(MockSteamBridge::new(false)),
    ));
    engine.save_config(config).unwrap();
    engine.scan().unwrap();

    let hydration = tokio::spawn({
        let engine = engine.clone();
        async move { engine.hydrate_missing_workshop_metadata().await.unwrap() }
    });
    tokio::time::timeout(Duration::from_secs(5), old_request.notified())
        .await
        .unwrap();
    engine
        .save_config(AppConfig {
            catalog_server_url: Some(new_url),
            ..engine.config()
        })
        .unwrap();
    assert_eq!(engine.hydrate_missing_workshop_metadata().await.unwrap(), 0);
    release_old.notify_one();
    assert_eq!(hydration.await.unwrap(), 2);
    let snapshots = engine.db().list_workshop_snapshots().unwrap();
    assert_eq!(
        snapshots[&WorkshopId(909903)].status,
        WorkshopItemStatus::Active
    );
    assert_eq!(
        snapshots[&WorkshopId(909903)].title,
        "First From New Catalog"
    );
    assert_eq!(
        snapshots[&WorkshopId(909904)].title,
        "Second From New Catalog"
    );
}

#[tokio::test]
async fn hydrates_missing_workshop_metadata_prefers_workshop_cover_and_caches_remote_image() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    let workshop = loading.join("workshop");
    std::fs::create_dir_all(&workshop).unwrap();

    // Create two campaign parts in loading_dir with Workshop IDs in filename, plus a Steam-style sidecar JPG.
    create_test_vpk(
        &loading.join("[910001] yama_main.vpk"),
        "Yama",
        "Mapper",
        true,
        Some("l4d_yama_1"),
        b"yama-part-1",
    );
    create_test_vpk(
        &loading.join("[910002] yama_assets.vpk"),
        "Yama Assets",
        "Mapper",
        false,
        None,
        b"yama-part-2",
    );
    // Minimal JPEG magic header + EOI so image::guess_format detects JPEG for the sidecar.
    let minimal_jpeg = vec![
        0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00,
        0x01, 0x00, 0x01, 0x00, 0x00, 0xff, 0xd9,
    ];
    std::fs::write(workshop.join("910001.jpg"), &minimal_jpeg).unwrap();

    let mut main_item = item_record(
        910001,
        "Yama Campaign Remastered",
        1_710_000_000,
        vec![910002],
    );
    // Preserve the source URL so mirror routing can choose the connection protocol.
    main_item.preview_url =
        Some("http://cloud-3.steamusercontent.com/ugc/910001/preview.jpg".into());
    let asset_item = item_record(910002, "Yama Assets Pack", 1_710_000_000, vec![]);
    let catalog_url = serve_catalog_items(Arc::new(Mutex::new(vec![main_item, asset_item]))).await;

    let cfg = AppConfig {
        loading_dir: loading.to_string_lossy().into_owned(),
        workshop_dir: workshop.to_string_lossy().into_owned(),
        https_only: false,
        catalog_server_url: Some(catalog_url),
        source_order: vec!["catalog-server".into()],
        ..Default::default()
    };
    let engine = make_engine(
        dir.path(),
        cfg.clone(),
        Arc::new(MockSteamBridge::new(false)),
    );
    engine.save_config(cfg).unwrap();
    let initial = engine.scan().unwrap();
    let before = initial
        .addons
        .iter()
        .find(|a| a.workshop_id == Some(WorkshopId(910001)))
        .unwrap();
    assert_eq!(before.title, "Yama");
    assert_eq!(before.workshop_title, None);
    assert!(before
        .local_cover_image
        .as_deref()
        .is_some_and(|name| name.ends_with("_sidecar.jpg")));
    assert_eq!(before.cover_image, before.local_cover_image);

    let hydrated = engine.hydrate_missing_workshop_metadata().await.unwrap();
    assert_eq!(hydrated, 2);
    // Second pass is a no-op once snapshots are hydrated.
    assert_eq!(engine.hydrate_missing_workshop_metadata().await.unwrap(), 0);

    let after_snap = engine.snapshot().unwrap();
    let after = after_snap
        .addons
        .iter()
        .find(|a| a.workshop_id == Some(WorkshopId(910001)))
        .unwrap();
    assert_eq!(
        after.workshop_title.as_deref(),
        Some("Yama Campaign Remastered")
    );
    assert_eq!(after.addon_title.as_deref(), Some("Yama"));
    assert_eq!(after.title, "Yama Campaign Remastered");
    assert_eq!(
        after.cover_image.as_deref(),
        Some("http://cloud-3.steamusercontent.com/ugc/910001/preview.jpg")
    );
    assert_eq!(after.local_cover_image, before.local_cover_image);
    assert_eq!(after_snap.bundles.len(), 1);
    assert_eq!(
        after_snap.bundles[0].kind,
        l4a_protocol::BundleKind::DependencyGraph
    );

    // Batch rename honors WorkshopTitle (default) vs AddonTitle preference.
    let rules = engine.config().rename_rules;
    let preview_ws = engine
        .preview_rename(&rules, std::slice::from_ref(&after.id))
        .unwrap();
    assert_eq!(
        preview_ws[0].target_file_name,
        "[910001] Yama Campaign Remastered.vpk"
    );
    engine
        .save_config(AppConfig {
            addon_name_preference: l4a_protocol::AddonNamePreference::AddonTitle,
            ..engine.config()
        })
        .unwrap();
    let preview_addon = engine
        .preview_rename(&rules, std::slice::from_ref(&after.id))
        .unwrap();
    assert_eq!(preview_addon[0].target_file_name, "[910001] Yama.vpk");

    // Remote cover fetching persists valid images to disk cache, while HTML error pages disguised as image/jpeg are never cached.
    let hits = Arc::new(AtomicUsize::new(0));
    let poison_hits = Arc::new(AtomicUsize::new(0));
    let hits_route = hits.clone();
    let poison_route = poison_hits.clone();
    let jpeg_payload = minimal_jpeg.clone();
    let app = Router::new()
        .route(
            "/ws-cover.jpg",
            get(move || {
                let hits_route = hits_route.clone();
                let body = jpeg_payload.clone();
                async move {
                    hits_route.fetch_add(1, Ordering::SeqCst);
                    ([("content-type", "image/jpeg")], body)
                }
            }),
        )
        .route(
            "/captive-portal.jpg",
            get(move || {
                let poison_route = poison_route.clone();
                async move {
                    poison_route.fetch_add(1, Ordering::SeqCst);
                    (
                        [("content-type", "image/jpeg")],
                        b"<html>Gateway Error</html>".to_vec(),
                    )
                }
            }),
        );
    let cover_server_url = serve_workshop_fixture(app).await;
    let cover_url = format!("{cover_server_url}/ws-cover.jpg");
    let first_fetch = engine.fetch_workshop_cover(&cover_url).await.unwrap();
    let second_fetch = engine.fetch_workshop_cover(&cover_url).await.unwrap();
    assert_eq!(first_fetch.0, "image/jpeg");
    assert_eq!(first_fetch, second_fetch);
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    let poison_url = format!("{cover_server_url}/captive-portal.jpg");
    let _ = engine.fetch_workshop_cover(&poison_url).await.unwrap();
    let _ = engine.fetch_workshop_cover(&poison_url).await.unwrap();
    // Must re-fetch because non-image HTML payload was not written to ws_cover_* disk cache.
    assert_eq!(poison_hits.load(Ordering::SeqCst), 2);
}

#[test]
fn scan_refreshes_legacy_sidecar_cover_without_changing_the_vpk() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    let workshop = loading.join("workshop");
    std::fs::create_dir_all(&workshop).unwrap();
    create_test_vpk(
        &loading.join("920001.vpk"),
        "Outpost",
        "Mapper",
        false,
        None,
        b"vpk",
    );
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_string_lossy().into_owned(),
            workshop_dir: workshop.to_string_lossy().into_owned(),
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );

    let jpeg = |pixel: [u8; 3]| {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode(&pixel, 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    };
    let original = jpeg([255, 0, 0]);
    let sidecar = workshop.join("920001.jpg");
    std::fs::write(&sidecar, &original).unwrap();

    let mut addon = engine.scan().unwrap().addons.pop().unwrap();
    let vpk_hash = addon.content_hash.as_deref().unwrap();
    let legacy_key = format!("{}_sidecar.jpg", &vpk_hash[..16]);
    let paths = AppPaths::from_runtime_dir(dir.path().join("runtime"));
    std::fs::write(paths.image_cache_dir.join(&legacy_key), original).unwrap();
    addon.cover_image = Some(legacy_key.clone());
    addon.local_cover_image = Some(legacy_key.clone());
    engine.db().upsert_addon(&addon).unwrap();

    let replacement = jpeg([0, 0, 255]);
    std::fs::write(&sidecar, &replacement).unwrap();
    let refreshed = engine.scan().unwrap().addons.pop().unwrap();
    let new_key = refreshed.local_cover_image.as_deref().unwrap();
    assert_eq!(refreshed.content_hash, addon.content_hash);
    assert_ne!(new_key, legacy_key);
    assert_eq!(refreshed.cover_image.as_deref(), Some(new_key));
    assert_eq!(engine.read_cached_image(new_key).unwrap(), replacement);
    assert_eq!(
        engine.scan().unwrap().addons[0]
            .local_cover_image
            .as_deref(),
        Some(new_key)
    );
}

#[tokio::test]
async fn hydration_sidecar_cover_utf8_category_and_ttl_regressions() {
    use std::sync::atomic::AtomicBool;
    use std::time::SystemTime;

    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    let workshop = loading.join("workshop");
    std::fs::create_dir_all(&workshop).unwrap();

    // 1. VPK scanned BEFORE Steam writes workshop/<wid>.jpg picks up the sidecar on an unchanged rescan.
    create_test_vpk(
        &loading.join("[920001] outpost.vpk"),
        "Outpost",
        "Mapper",
        false,
        None,
        b"outpost-vpk",
    );
    create_test_vpk(
        &loading.join("[920002] transient.vpk"),
        "Transient",
        "Mapper",
        false,
        None,
        b"transient-vpk",
    );

    let catalog_items = Arc::new(Mutex::new(Vec::new()));
    let catalog_url = serve_catalog_items(catalog_items.clone()).await;
    let cfg = AppConfig {
        loading_dir: loading.to_string_lossy().into_owned(),
        workshop_dir: workshop.to_string_lossy().into_owned(),
        https_only: false,
        catalog_server_url: Some(catalog_url),
        source_order: vec!["catalog-server".into()],
        ..Default::default()
    };
    let engine = make_engine(
        dir.path(),
        cfg.clone(),
        Arc::new(MockSteamBridge::new(false)),
    );
    engine.save_config(cfg).unwrap();

    let snap1 = engine.scan().unwrap();
    let a1 = snap1
        .addons
        .iter()
        .find(|a| a.workshop_id == Some(WorkshopId(920001)))
        .unwrap();
    assert_eq!(a1.local_cover_image, None);

    let minimal_jpeg = vec![
        0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00,
        0x01, 0x00, 0x01, 0x00, 0x00, 0xff, 0xd9,
    ];
    std::fs::write(workshop.join("920001.jpg"), &minimal_jpeg).unwrap();

    // Rescan without modifying outpost.vpk: must discover 920001.jpg sidecar!
    let snap2 = engine.scan().unwrap();
    let a1_after_sidecar = snap2
        .addons
        .iter()
        .find(|a| a.workshop_id == Some(WorkshopId(920001)))
        .unwrap();
    let sidecar_key = a1_after_sidecar
        .local_cover_image
        .clone()
        .expect("sidecar discovered on unchanged scan");
    assert!(sidecar_key.ends_with("_sidecar.jpg"));

    // 2. Transient missing provider outcome backs off briefly, then retries once backoff expires.
    assert_eq!(engine.hydrate_missing_workshop_metadata().await.unwrap(), 0);
    let mut item_920002 = item_record(920002, "Transient Hydrated", 1_720_000_000, vec![]);
    item_920002.tags = vec![];
    catalog_items.lock().unwrap().push(item_920002);
    // Still inside backoff window -> skipped
    assert_eq!(engine.hydrate_missing_workshop_metadata().await.unwrap(), 0);
    // Backoff expires -> retried and hydrated!
    engine.expire_hydration_backoff_for_test();
    assert_eq!(engine.hydrate_missing_workshop_metadata().await.unwrap(), 1);

    // 3. Non-ASCII invalid relative cover URL (where byte 7/8 fall inside multi-byte UTF-8 chars) never panics
    // and falls back to local_cover_image; also Workshop Sync snapshot is not overwritten by hydration.
    let mut item_920001 = item_record(920001, "Outpost Synced", 1_720_000_500, vec![]);
    item_920001.preview_url = Some("封面图片路径/preview.jpg".into());
    item_920001.tags = vec!["Weapons".into()];
    engine
        .db()
        .upsert_workshop_snapshot(&item_920001, true)
        .unwrap();

    // Even if the provider has an older title, hydration keeps the synced snapshot.
    let stale_item = item_record(920001, "Stale Hydration Title", 1_700_000_000, vec![]);
    catalog_items.lock().unwrap().push(stale_item);
    engine.expire_hydration_backoff_for_test();
    assert_eq!(engine.hydrate_missing_workshop_metadata().await.unwrap(), 0);

    let snap3 = engine.snapshot().unwrap();
    let a1_utf8 = snap3
        .addons
        .iter()
        .find(|a| a.workshop_id == Some(WorkshopId(920001)))
        .unwrap();
    assert_eq!(a1_utf8.workshop_title.as_deref(), Some("Outpost Synced"));
    assert_eq!(a1_utf8.cover_image.as_deref(), Some(sidecar_key.as_str()));
    assert_eq!(a1_utf8.categories, vec!["Weapon".to_string()]);

    // 4. Unchanged scan + toggle must NOT persist Workshop-derived "Weapon" into local categories_json.
    let _ = engine.scan().unwrap();
    engine
        .toggle_addons(std::slice::from_ref(&a1_utf8.id), false, false)
        .unwrap();
    item_920001.tags = vec!["Survivors".into()];
    engine
        .db()
        .upsert_workshop_snapshot(&item_920001, true)
        .unwrap();
    let snap4 = engine.snapshot().unwrap();
    let a1_retagged = snap4
        .addons
        .iter()
        .find(|a| a.workshop_id == Some(WorkshopId(920001)))
        .unwrap();
    assert_eq!(a1_retagged.categories, vec!["Survivor".to_string()]);

    // 5. Expired cached Workshop covers revalidate with origin, and fall back to stale bytes when upstream is 503.
    let minimal_png: Vec<u8> = vec![
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00,
    ];
    let version_two = Arc::new(AtomicBool::new(false));
    let fail_upstream = Arc::new(AtomicBool::new(false));
    let redirect_loop = Arc::new(AtomicBool::new(false));
    let hits = Arc::new(AtomicUsize::new(0));
    let v2_route = version_two.clone();
    let fail_route = fail_upstream.clone();
    let redirect_route = redirect_loop.clone();
    let hits_route = hits.clone();
    let jpg_body = minimal_jpeg.clone();
    let png_body = minimal_png.clone();
    let app = Router::new().route(
        "/rotating.img",
        get(move || {
            let v2 = v2_route.load(Ordering::SeqCst);
            let fail = fail_route.load(Ordering::SeqCst);
            let redirect = redirect_route.load(Ordering::SeqCst);
            let hits_route = hits_route.clone();
            let jpg = jpg_body.clone();
            let png = png_body.clone();
            async move {
                hits_route.fetch_add(1, Ordering::SeqCst);
                if redirect {
                    return (
                        axum::http::StatusCode::FOUND,
                        [("location", "/rotating.img")],
                        Vec::new(),
                    );
                }
                if fail {
                    return (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        [("content-type", "text/plain")],
                        b"upstream down".to_vec(),
                    );
                }
                if v2 {
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "image/png")],
                        png,
                    )
                } else {
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "image/jpeg")],
                        jpg,
                    )
                }
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let rotating_url = format!("http://{addr}/rotating.img");
    let (mime1, bytes1) = engine.fetch_workshop_cover(&rotating_url).await.unwrap();
    assert_eq!(mime1, "image/jpeg");
    assert_eq!(bytes1, minimal_jpeg);
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    let expire_cache = |cache_dir: &std::path::Path| {
        let expired_time = SystemTime::now() - Duration::from_secs(90_000);
        for entry in std::fs::read_dir(cache_dir).unwrap().flatten() {
            if entry.file_name().to_string_lossy().starts_with("ws_cover_") {
                let f = std::fs::File::options()
                    .write(true)
                    .open(entry.path())
                    .unwrap();
                f.set_modified(expired_time).unwrap();
            }
        }
    };

    // Expire cache + switch origin to v2 PNG -> revalidates and updates cached format!
    let cache_dir = AppPaths::from_runtime_dir(dir.path().join("runtime")).image_cache_dir;
    expire_cache(&cache_dir);
    version_two.store(true, Ordering::SeqCst);
    let (mime2, bytes2) = engine.fetch_workshop_cover(&rotating_url).await.unwrap();
    assert_eq!(mime2, "image/png");
    assert_eq!(bytes2, minimal_png);
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    // Expire cache again + origin returns 503 -> falls back to stale cached PNG!
    expire_cache(&cache_dir);
    fail_upstream.store(true, Ordering::SeqCst);
    let (mime3, bytes3) = engine.fetch_workshop_cover(&rotating_url).await.unwrap();
    assert_eq!(mime3, "image/png");
    assert_eq!(bytes3, minimal_png);
    assert_eq!(hits.load(Ordering::SeqCst), 3);

    redirect_loop.store(true, Ordering::SeqCst);
    let (mime, bytes) = engine.fetch_workshop_cover(&rotating_url).await.unwrap();
    assert_eq!(mime, "image/png");
    assert_eq!(bytes, minimal_png);
    assert!(hits.load(Ordering::SeqCst) > 4);
}
