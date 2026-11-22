use super::{AppPaths, EventSink, L4aEngine, LocalDb, NullEventSink};
use chrono::Utc;
use l4a_mirror::MirrorRouter;
use l4a_protocol::{
    AppConfig, CoreEvent, MetadataSource, WorkshopDependency, WorkshopFileType, WorkshopId,
    WorkshopItemRecord, WorkshopItemStatus,
};
use l4a_steam::MockSteamBridge;
use l4a_vpk::{write_vpk, VpkFileToWrite};
use l4a_workshop::{
    SourceRoutingPolicy, SteamBridgeProvider, WebScraperPlaceholderProvider, WorkshopResolver,
};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub(super) fn create_test_vpk(
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

pub(super) fn make_engine(
    root: &Path,
    config: AppConfig,
    bridge: Arc<MockSteamBridge>,
) -> L4aEngine {
    make_engine_with_events(root, config, bridge, Arc::new(NullEventSink))
}

pub(super) fn make_engine_with_events(
    root: &Path,
    config: AppConfig,
    bridge: Arc<MockSteamBridge>,
    events: Arc<dyn EventSink>,
) -> L4aEngine {
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
    L4aEngine::with_dependencies(paths, db, config, mirror, bridge, resolver, events).unwrap()
}

#[derive(Default)]
pub(super) struct RecordedEvents(Mutex<Vec<CoreEvent>>);

impl RecordedEvents {
    pub(super) fn drain(&self) -> Vec<CoreEvent> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

impl EventSink for RecordedEvents {
    fn emit(&self, event: CoreEvent) {
        self.0.lock().unwrap().push(event);
    }
}

pub(super) fn item_record(id: u64, title: &str, ts: i64, deps: Vec<u64>) -> WorkshopItemRecord {
    WorkshopItemRecord {
        workshop_id: WorkshopId::new(id).unwrap(),
        file_type: WorkshopFileType::Item,
        status: WorkshopItemStatus::Active,
        title: title.to_string(),
        description: Some("desc".into()),
        creator_steam_id: Some("76561198000000001".into()),
        creator_name: Some("Mapper".into()),
        preview_url: None,
        previews: Vec::new(),
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
