use super::*;
use l4a_protocol::AddonOperationOutcome;
use l4a_steam::MockSteamBridge;
use std::time::Duration;
use tempfile::tempdir;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, timeout};

#[path = "../../tests/support/mod.rs"]
mod support;

use support::{create_test_vpk, item_record, make_engine, make_engine_with_events, RecordedEvents};

fn stage_vpk(
    request: DownloadRequest,
    progress: ProgressSink,
    bytes: &[u8],
) -> Result<DownloadResult, WorkshopError> {
    fs::write(&request.dest_path, bytes)?;
    let size = bytes.len() as u64;
    progress(DownloadProgress {
        workshop_id: request.workshop_id,
        downloaded: size,
        total: Some(size),
        percent: 100,
        speed_bps: Some(size),
    });
    Ok(DownloadResult {
        dest_path: request.dest_path,
        bytes_written: size,
        resumed_from: 0,
    })
}

async fn update_from_fixture(
    engine: &L4aEngine,
    wid: WorkshopId,
    bytes: &[u8],
) -> Result<AddonSummary, CoreError> {
    engine
        .transfer_workshop_item_with(wid, true, |request, _, progress| async move {
            stage_vpk(request, progress, bytes)
        })
        .await
}

#[tokio::test]
async fn workshop_update_preserves_customization_and_links_and_rolls_back_on_error() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    let workshop = loading.join("workshop");
    std::fs::create_dir_all(&workshop).unwrap();
    let wid = WorkshopId(880001);
    let dummy = workshop.join(format!("{wid}.vpk"));
    create_test_vpk(
        &dummy,
        "Old title",
        "Author",
        true,
        Some("old_map"),
        b"old contents",
    );
    let original_bytes = std::fs::read(&dummy).unwrap();
    let bridge = Arc::new(MockSteamBridge::new(true));
    let events = Arc::new(RecordedEvents::default());
    let engine = make_engine_with_events(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_str().unwrap().into(),
            workshop_dir: workshop.to_str().unwrap().into(),
            enable_dummy_bypass: true,
            ..Default::default()
        },
        bridge.clone(),
        events.clone(),
    );
    let aid = engine.scan().unwrap().addons[0].id.clone();
    let ids = std::slice::from_ref(&aid);
    let moved = engine.move_addons(ids, AddonDirType::Loading).unwrap();
    assert_eq!(moved.items[0].outcome, AddonOperationOutcome::Applied);
    let name = "Custom addon.vpk";
    let renamed = engine.apply_renames(&[(aid.clone(), name.into())]).unwrap();
    assert_eq!(renamed.items[0].outcome, AddonOperationOutcome::Applied);
    let toggled = engine.toggle_addons(ids, false, false).unwrap();
    assert_eq!(toggled.items[0].outcome, AddonOperationOutcome::Applied);
    let destination = loading.join(format!("{name}.disabled"));
    let dummy_before = std::fs::read(&dummy).unwrap();

    let mut legacy = engine.db().get_addon(&aid).unwrap().unwrap();
    engine.db().delete_addon_record(&aid).unwrap();
    legacy.id = AddonId("legacy-addon-id".into());
    legacy.local_time_updated = Some(1700000000);
    engine.db().upsert_addon(&legacy).unwrap();
    let aid = legacy.id;
    let old_snapshot = item_record(wid.0, "Old remote metadata", 1700000001, vec![880002]);
    engine
        .db()
        .upsert_workshop_snapshot(&old_snapshot, true)
        .unwrap();
    let original = engine.db().get_addon(&aid).unwrap().unwrap();
    assert!(original.update_available);
    let bundle = engine
        .create_manual_bundle("Manual bundle".into(), vec![aid.clone()])
        .unwrap();
    let group = engine
        .save_user_group(UserGroup {
            id: GroupId("update-group".into()),
            name: "Custom group".into(),
            tags: vec![],
            color: None,
            addon_ids: vec![aid.clone()],
        })
        .unwrap();
    let profile = engine
        .save_profile(LoadoutProfile {
            id: ProfileId("update-profile".into()),
            name: "Local profile".into(),
            description: String::new(),
            enabled_addon_ids: vec![aid.clone()],
        })
        .unwrap();

    let vpk = dir.path().join("updated.vpk");
    create_test_vpk(
        &vpk,
        "New title",
        "Author",
        true,
        Some("new_map"),
        b"updated contents",
    );
    let updated_bytes = std::fs::read(&vpk).unwrap();
    let mut new_snapshot = item_record(wid.0, "Updated remote metadata", 1700000002, vec![]);
    new_snapshot.file_url = Some("https://example.invalid/updated.vpk".into());
    new_snapshot.preview_url = Some("https://example.com/updated.jpg".into());
    bridge.insert_item(new_snapshot.clone());
    assert!(matches!(
        update_from_fixture(&engine, wid, &dummy_before).await,
        Err(CoreError::Vpk(_))
    ));
    assert_eq!(std::fs::read(&destination).unwrap(), original_bytes);

    let sql = rusqlite::Connection::open(dir.path().join("runtime/l4a_v2.db")).unwrap();
    sql.execute_batch(
        "CREATE TRIGGER fail_completion BEFORE UPDATE ON tasks
         WHEN NEW.status = 'completed' BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
    )
    .unwrap();
    assert!(matches!(
        update_from_fixture(&engine, wid, &updated_bytes).await,
        Err(CoreError::Db(_))
    ));
    assert_eq!(std::fs::read(&destination).unwrap(), original_bytes);
    assert_eq!(std::fs::read(&dummy).unwrap(), dummy_before);
    assert_eq!(engine.db().get_addon(&aid).unwrap().unwrap(), original);
    assert_eq!(
        engine
            .db()
            .get_workshop_snapshot(wid)
            .unwrap()
            .unwrap()
            .time_updated,
        old_snapshot.time_updated
    );
    assert!(engine
        .db()
        .list_tasks()
        .unwrap()
        .iter()
        .all(|task| task.status == TaskStatus::Failed));
    assert!(!events
        .drain()
        .iter()
        .any(|event| matches!(event, CoreEvent::DownloadProgress { percent: 100, .. })));
    sql.execute_batch("DROP TRIGGER fail_completion").unwrap();

    let updated = update_from_fixture(&engine, wid, &updated_bytes)
        .await
        .unwrap();
    let DummyInspection::RealVpk {
        content_hash,
        file_size,
        ..
    } = inspect_dummy_vpk(vpk)
    else {
        panic!("invalid update fixture");
    };
    assert_eq!(updated.id, aid);
    assert_eq!(updated.current_path.as_deref(), destination.to_str());
    assert_eq!(updated.current_file_name.as_deref(), Some(name));
    assert_eq!(updated.primary_dir, Some(AddonDirType::Loading));
    assert!(!updated.enabled);
    assert_eq!(std::fs::read(&destination).unwrap(), updated_bytes);
    assert!(!loading.join(name).exists());
    assert_eq!(updated.content_hash.as_deref(), Some(content_hash.as_str()));
    assert_ne!(updated.content_hash, original.content_hash);
    assert_eq!(updated.file_size, file_size);
    assert_eq!(updated.files_count, 3);
    assert_eq!(updated.maps[0].code, "new_map");
    assert_eq!(updated.addon_info["addontitle"], "New title");
    assert_eq!(updated.cover_image, new_snapshot.preview_url);
    assert_eq!(updated.local_time_updated, new_snapshot.time_updated);
    assert_eq!(updated.remote_time_updated, new_snapshot.time_updated);
    assert!(!updated.update_available);
    assert!(updated.has_dummy_bypass);
    let DummyInspection::Dummy(meta) = inspect_dummy_vpk(&dummy) else {
        panic!("expected regenerated Dummy");
    };
    assert_eq!(meta.schema_version, 2);
    assert_eq!(meta.workshop_id, Some(wid));
    assert_eq!(meta.original_hash, Some(content_hash));
    assert_eq!(meta.original_size, Some(file_size));
    assert_eq!(meta.original_time_updated, new_snapshot.time_updated);
    let snapshot = engine.snapshot().unwrap();
    assert_eq!(snapshot.addons, vec![updated]);
    assert_eq!(snapshot.bundles, vec![bundle]);
    assert_eq!(snapshot.user_groups, vec![group]);
    assert_eq!(snapshot.profiles, vec![profile]);
    assert_eq!(
        engine.db().get_workshop_snapshot(wid).unwrap().unwrap(),
        new_snapshot
    );
    let task = &snapshot.tasks[0];
    assert_eq!(task.kind, TaskKind::Download);
    assert_eq!(task.status, TaskStatus::Completed);
    assert_eq!(
        task.target_title.as_deref(),
        Some("Updated remote metadata")
    );
    assert_eq!(task.progress_percent, 100);
    assert!(matches!(events.drain().as_slice(), [..,
        CoreEvent::DownloadProgress {
            task_id,
            workshop_id,
            title,
            downloaded,
            total: Some(total),
            percent: 100,
            ..
        },
        CoreEvent::SnapshotUpdated { external_change: false },
    ] if task_id == &task.id
        && *workshop_id == wid
        && title.as_deref() == Some("Updated remote metadata")
        && *downloaded == file_size
        && *total == file_size));
}

#[tokio::test]
async fn workshop_update_can_replace_an_addon_in_the_workshop_directory() {
    let dir = tempdir().unwrap();
    let workshop = dir.path().join("workshop");
    std::fs::create_dir(&workshop).unwrap();
    let wid = WorkshopId(880003);
    let destination = workshop.join(format!("{wid}.vpk.disabled"));
    create_test_vpk(
        &destination,
        "Old title",
        "Author",
        false,
        None,
        b"old contents",
    );
    let vpk = dir.path().join("replacement.vpk");
    create_test_vpk(&vpk, "New title", "Author", false, None, b"new contents");
    let bytes = std::fs::read(vpk).unwrap();
    let bridge = Arc::new(MockSteamBridge::new(true));
    let engine = make_engine(
        dir.path(),
        AppConfig {
            workshop_dir: workshop.to_str().unwrap().into(),
            enable_dummy_bypass: true,
            ..Default::default()
        },
        bridge.clone(),
    );
    let mut addon = engine.scan().unwrap().addons.remove(0);
    addon.cover_image = Some("old-cover.jpg".into());
    addon.local_time_updated = Some(1700000000);
    engine.db().upsert_addon(&addon).unwrap();
    let mut item = item_record(wid.0, "Replacement", 1700000001, vec![]);
    item.preview_url = Some("https://example.com/old-cover.jpg".into());
    engine.db().upsert_workshop_snapshot(&item, false).unwrap();
    item.time_updated = None;
    item.preview_url = None;
    item.file_url = Some("https://example.invalid/replacement.vpk".into());
    bridge.insert_item(item);
    assert!(matches!(
        engine.update_workshop_item(WorkshopId(880004)).await,
        Err(CoreError::NotFound(_))
    ));

    let updated = update_from_fixture(&engine, wid, &bytes).await.unwrap();
    assert_eq!(updated.id, addon.id);
    assert_eq!(updated.primary_dir, Some(AddonDirType::Workshop));
    assert_eq!(updated.current_path.as_deref(), destination.to_str());
    assert!(!updated.enabled);
    assert!(!updated.has_dummy_bypass);
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);
    assert!(!workshop.join(format!("{wid}.vpk")).exists());
    assert_eq!(updated.local_time_updated, None);
    assert_eq!(updated.remote_time_updated, None);
    assert_eq!(updated.cover_image, None);
    assert_eq!(engine.snapshot().unwrap().addons, vec![updated]);

    std::fs::remove_file(destination).unwrap();
    assert!(matches!(
        engine.update_workshop_item(wid).await,
        Err(CoreError::NotFound(_))
    ));
}

type PendingVpk = (WorkshopId, oneshot::Sender<()>);

#[derive(Clone)]
struct GatedVpk {
    bytes: Arc<[u8]>,
    requested: mpsc::UnboundedSender<PendingVpk>,
}

impl GatedVpk {
    fn new(bytes: Vec<u8>) -> (Self, mpsc::UnboundedReceiver<PendingVpk>) {
        let (requested, receiver) = mpsc::unbounded_channel();
        (
            Self {
                bytes: bytes.into(),
                requested,
            },
            receiver,
        )
    }

    async fn transfer(
        self,
        engine: &L4aEngine,
        wid: WorkshopId,
        update: bool,
    ) -> Result<AddonSummary, CoreError> {
        engine
            .transfer_workshop_item_with(wid, update, |request, cancel, progress| async move {
                let (release, gated) = oneshot::channel();
                self.requested.send((request.workshop_id, release)).unwrap();
                tokio::select! {
                    _ = cancel.cancelled() => return Err(WorkshopError::Cancelled),
                    result = gated => result.unwrap(),
                }
                stage_vpk(request, progress, &self.bytes)
            })
            .await
    }
}

fn download_status(engine: &L4aEngine, wid: WorkshopId) -> Option<TaskStatus> {
    engine
        .db()
        .list_tasks()
        .unwrap()
        .into_iter()
        .find(|task| task.kind == TaskKind::Download && task.target_id == wid.to_string())
        .map(|task| task.status)
}

async fn wait_for_queued_download(engine: &L4aEngine, wid: WorkshopId) {
    timeout(Duration::from_secs(5), async {
        while download_status(engine, wid) != Some(TaskStatus::Queued) {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("download should enter the queue");
}

async fn expect_vpk_request(
    requests: &mut mpsc::UnboundedReceiver<PendingVpk>,
    expected: WorkshopId,
) -> oneshot::Sender<()> {
    let (wid, release) = timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wid, expected);
    release
}

#[tokio::test]
async fn installs_and_updates_share_a_resizable_queue_with_queued_cancellation() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    let workshop = loading.join("workshop");
    std::fs::create_dir_all(&workshop).unwrap();
    let (install_id, update_id, cancelled_id, next_id) = (
        WorkshopId(890001),
        WorkshopId(890002),
        WorkshopId(890003),
        WorkshopId(890004),
    );
    create_test_vpk(
        &loading.join(format!("{update_id}.vpk")),
        "Installed",
        "Author",
        false,
        None,
        b"old",
    );
    let fixture = dir.path().join("download.vpk");
    create_test_vpk(&fixture, "Download", "Author", false, None, b"new");
    let (fixture, mut requests) = GatedVpk::new(std::fs::read(fixture).unwrap());
    let bridge = Arc::new(MockSteamBridge::new(true));
    for wid in [install_id, update_id, cancelled_id, next_id] {
        let mut item = item_record(wid.0, "Download", 1700000000, vec![]);
        item.file_url = Some(format!("https://example.invalid/{wid}.vpk"));
        bridge.insert_item(item);
    }
    let engine = Arc::new(make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_str().unwrap().into(),
            workshop_dir: workshop.to_str().unwrap().into(),
            download_concurrency: 1,
            source_order: vec!["steamworks-sdk".into()],
            ..Default::default()
        },
        bridge,
    ));
    engine.scan().unwrap();
    let install = |wid| {
        let engine = engine.clone();
        let fixture = fixture.clone();
        tokio::spawn(async move { fixture.transfer(&engine, wid, false).await })
    };
    let first = install(install_id);
    let release_first = expect_vpk_request(&mut requests, install_id).await;
    assert_eq!(
        download_status(&engine, install_id),
        Some(TaskStatus::Running)
    );

    let update = tokio::spawn({
        let engine = engine.clone();
        let fixture = fixture.clone();
        async move { fixture.transfer(&engine, update_id, true).await }
    });
    wait_for_queued_download(&engine, update_id).await;
    let cancelled = install(cancelled_id);
    wait_for_queued_download(&engine, cancelled_id).await;
    assert!(engine.cancel_download(cancelled_id));
    assert!(matches!(
        timeout(Duration::from_secs(5), cancelled)
            .await
            .unwrap()
            .unwrap(),
        Err(CoreError::Workshop(WorkshopError::Cancelled))
    ));
    assert_eq!(
        download_status(&engine, cancelled_id),
        Some(TaskStatus::Cancelled)
    );
    assert!(!engine.cancel_download(cancelled_id));
    assert!(requests.try_recv().is_err());

    let mut config = engine.config();
    config.download_concurrency = 2;
    engine.save_config(config.clone()).unwrap();
    let release_update = expect_vpk_request(&mut requests, update_id).await;
    assert_eq!(
        download_status(&engine, update_id),
        Some(TaskStatus::Running)
    );

    config.download_concurrency = 1;
    engine.save_config(config).unwrap();
    let next = install(next_id);
    wait_for_queued_download(&engine, next_id).await;
    release_first.send(()).unwrap();
    timeout(Duration::from_secs(5), first)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(timeout(Duration::from_millis(50), requests.recv())
        .await
        .is_err());
    assert_eq!(download_status(&engine, next_id), Some(TaskStatus::Queued));

    release_update.send(()).unwrap();
    timeout(Duration::from_secs(5), update)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    expect_vpk_request(&mut requests, next_id)
        .await
        .send(())
        .unwrap();
    timeout(Duration::from_secs(5), next)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    for wid in [install_id, update_id, next_id] {
        assert_eq!(download_status(&engine, wid), Some(TaskStatus::Completed));
    }
}

#[tokio::test]
async fn workshop_update_detects_a_rename_during_transfer() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    std::fs::create_dir(&loading).unwrap();
    let wid = WorkshopId(890005);
    let path = loading.join(format!("{wid}.vpk"));
    create_test_vpk(&path, "Installed", "Author", false, None, b"old");
    let original = std::fs::read(&path).unwrap();
    let fixture = dir.path().join("download.vpk");
    create_test_vpk(&fixture, "Update", "Author", false, None, b"new");
    let (fixture, mut requests) = GatedVpk::new(std::fs::read(fixture).unwrap());
    let bridge = Arc::new(MockSteamBridge::new(true));
    let mut item = item_record(wid.0, "Update", 1700000000, vec![]);
    item.file_url = Some(format!("https://example.invalid/{wid}.vpk"));
    bridge.insert_item(item);
    let engine = Arc::new(make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_str().unwrap().into(),
            ..Default::default()
        },
        bridge,
    ));
    let aid = engine.scan().unwrap().addons[0].id.clone();

    let update = tokio::spawn({
        let engine = engine.clone();
        async move { fixture.transfer(&engine, wid, true).await }
    });
    let release = expect_vpk_request(&mut requests, wid).await;
    let renamed = engine
        .apply_renames(&[(aid, "New local name.vpk".into())])
        .unwrap();
    assert_eq!(renamed.items[0].outcome, AddonOperationOutcome::Applied);
    release.send(()).unwrap();

    assert!(matches!(
        timeout(Duration::from_secs(5), update)
            .await
            .unwrap()
            .unwrap(),
        Err(CoreError::Conflict(_))
    ));
    assert!(!path.exists());
    assert_eq!(
        std::fs::read(loading.join("New local name.vpk")).unwrap(),
        original
    );
    assert!(engine.db().get_workshop_snapshot(wid).unwrap().is_none());
    assert_eq!(download_status(&engine, wid), Some(TaskStatus::Failed));
}

#[tokio::test]
async fn download_migrates_legacy_tasks_table_and_emits_title_in_progress() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    std::fs::create_dir(&loading).unwrap();

    // Seed an older SQLite database whose `tasks` table lacks `target_title`.
    let db_path = AppPaths::from_runtime_dir(dir.path().join("runtime")).db_path;
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                status TEXT NOT NULL,
                target_id TEXT NOT NULL,
                progress_percent INTEGER NOT NULL DEFAULT 0,
                message TEXT,
                error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            INSERT INTO tasks (id, kind, status, target_id, progress_percent, message, error, created_at, updated_at)
            VALUES ('legacy-1', 'download', 'completed', '111222', 100, '下载完成', NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z');
            "#,
        )
        .unwrap();
    }

    let wid = WorkshopId(890006);
    let fixture = dir.path().join("campaign.vpk");
    create_test_vpk(
        &fixture,
        "Dark Carnival Remix",
        "Author",
        true,
        None,
        b"vpk",
    );
    let bytes = std::fs::read(&fixture).unwrap();

    let bridge = Arc::new(MockSteamBridge::new(true));
    let mut item = item_record(wid.0, "Dark Carnival Remix", 1700000100, vec![]);
    item.file_url = Some(format!("https://example.invalid/{wid}.vpk"));
    item.file_size = Some(bytes.len() as u64);
    bridge.insert_item(item);

    let events = Arc::new(RecordedEvents::default());
    let engine = make_engine_with_events(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_str().unwrap().into(),
            ..Default::default()
        },
        bridge,
        events.clone(),
    );

    engine
        .transfer_workshop_item_with(wid, false, |request, _, progress| async move {
            stage_vpk(request, progress, &bytes)
        })
        .await
        .unwrap();

    let tasks = engine.db().list_tasks().unwrap();
    let newest = tasks
        .iter()
        .find(|t| t.target_id == wid.to_string())
        .unwrap();
    assert_eq!(newest.target_title.as_deref(), Some("Dark Carnival Remix"));
    let legacy = tasks.iter().find(|t| t.id.as_str() == "legacy-1").unwrap();
    assert_eq!(legacy.target_title, None);

    let progress_events: Vec<_> = events
        .drain()
        .into_iter()
        .filter(|ev| matches!(ev, CoreEvent::DownloadProgress { .. }))
        .collect();
    assert!(progress_events.len() >= 2);
    assert!(progress_events.iter().all(|ev| matches!(
        ev,
        CoreEvent::DownloadProgress {
            title: Some(title),
            ..
        } if title == "Dark Carnival Remix"
    )));
}
