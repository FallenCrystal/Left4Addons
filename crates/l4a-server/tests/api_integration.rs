use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use chrono::Utc;
use http_body_util::BodyExt;
use l4a_mirror::MirrorRouter;
use l4a_protocol::{
    AdminItemDetailResponse, AdminLoginResponse, AdminSessionInfo, BatchItemsResponse,
    CatalogOverviewResponse, CatalogSyncConfigDto, CatalogSyncStatus, MetadataSource,
    SearchResponse, SteamAuthFlowState, SteamAuthFlowStatus, SteamAuthMode, WorkshopDependency,
    WorkshopFetchOutcome, WorkshopFileType, WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
    WorkshopPreview,
};
use l4a_server::{
    build_router,
    config::{CatalogConfig, CatalogConfigStore},
    db::ServerRepo,
    upstream::{
        MockWorkshopUpstream, WorkshopQueryPage, MATCHING_FILE_TYPE_ITEMS,
        QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE, QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
    },
    ServerState,
};
use l4a_workshop::{CatalogBackendProvider, WorkshopProvider};
use serde_json::json;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower::ServiceExt;

async fn raw_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    headers: Vec<(&str, &str)>,
    body: Option<serde_json::Value>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        builder = builder.header(k, v);
    }
    let req = if let Some(val) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&val).unwrap()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };

    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let resp_headers = resp.headers().clone();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, resp_headers, bytes)
}

async fn json_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    headers: Vec<(&str, &str)>,
    body: Option<serde_json::Value>,
) -> (StatusCode, Vec<u8>) {
    let (status, _, bytes) = raw_request(app, method, uri, headers, body).await;
    (status, bytes)
}

fn sample_item(id: u64, title: &str, updated: i64, file_url: Option<&str>) -> WorkshopItemRecord {
    WorkshopItemRecord {
        workshop_id: WorkshopId(id),
        file_type: WorkshopFileType::Item,
        status: WorkshopItemStatus::Active,
        title: title.to_string(),
        description: Some(format!("Description for {title} 100%")),
        creator_steam_id: Some("76561198000000123".to_string()),
        creator_name: Some("Mapper".to_string()),
        preview_url: Some(format!(
            "https://cdn.steamusercontent.com/ugc/{id}_cover.jpg"
        )),
        previews: vec![WorkshopPreview {
            preview_id: Some(format!("p_{id}")),
            url: Some(format!(
                "https://cdn.steamusercontent.com/ugc/{id}_shot1.jpg"
            )),
            youtube_video_id: None,
            preview_type: 0,
        }],
        file_url: file_url.map(str::to_owned),
        file_size: Some(1024 * 1024),
        time_created: Some(1_700_000_000),
        time_updated: Some(updated),
        tags: vec!["Campaigns".to_string(), "Maps".to_string()],
        dependencies: Vec::new(),
        collection_children: Vec::new(),
        subscriptions: Some(500),
        favorites: Some(50),
        source: MetadataSource::SteamKit,
        fetched_at: Utc::now(),
    }
}

#[tokio::test]
async fn search_items_supports_catalog_sorts_and_title_relevance() {
    let state = ServerState::in_memory("test-admin-password").unwrap();
    for (id, title, description, created, updated, subscriptions) in [
        (9001, "MISSION_", None, Some(6), Some(20), None),
        (9002, "mission_ Zulu", None, Some(60), Some(50), Some(8)),
        (9003, "Mission_ alpha", None, Some(60), Some(10), Some(8)),
        (
            9004,
            "An earlier Mission_ text",
            None,
            Some(30),
            Some(50),
            Some(100),
        ),
        (
            9005,
            "Beta",
            Some("A mission_ in the description"),
            None,
            Some(100),
            Some(100),
        ),
        (
            9006,
            "beta",
            Some("A mission_ in the description"),
            Some(30),
            None,
            Some(100),
        ),
        (9007, "MissionX", None, Some(99), Some(75), Some(1)),
    ] {
        state
            .repo
            .upsert_item(
                &WorkshopItemRecord {
                    workshop_id: WorkshopId(id),
                    file_type: WorkshopFileType::Item,
                    status: WorkshopItemStatus::Active,
                    title: title.into(),
                    description: description.map(str::to_owned),
                    creator_steam_id: None,
                    creator_name: None,
                    preview_url: None,
                    previews: vec![],
                    file_url: None,
                    file_size: None,
                    time_created: created,
                    time_updated: updated,
                    tags: vec!["Campaign".into()],
                    dependencies: vec![],
                    collection_children: vec![],
                    subscriptions,
                    favorites: None,
                    source: MetadataSource::AdminOverride,
                    fetched_at: Utc::now(),
                },
                true,
                false,
            )
            .unwrap();
    }
    let app = build_router(state);
    let search = async |params: &str| {
        let (status, bytes) = json_request(
            &app,
            "GET",
            &format!("/api/v1/items/search{params}"),
            vec![],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{params}");
        let response: SearchResponse = serde_json::from_slice(&bytes).unwrap();
        let ids: Vec<_> = response
            .items
            .iter()
            .map(|item| item.workshop_id.0)
            .collect();
        (response.total, ids)
    };

    for (params, expected) in [
        (
            "?sort=updated_asc",
            [9006, 9003, 9001, 9002, 9004, 9007, 9005],
        ),
        ("?sort=title", [9004, 9005, 9006, 9001, 9003, 9002, 9007]),
        (
            "?sort=subscriptions",
            [9004, 9005, 9006, 9002, 9003, 9007, 9001],
        ),
        ("?sort=newest", [9007, 9003, 9002, 9006, 9004, 9001, 9005]),
        ("?sort=updated", [9005, 9007, 9004, 9002, 9001, 9003, 9006]),
        ("", [9005, 9004, 9006, 9002, 9003, 9007, 9001]),
    ] {
        assert_eq!(search(params).await, (7, expected.to_vec()), "{params}");
    }
    for params in ["?q=%20MiSSIon_%20", "?q=mission_&sort=relevance"] {
        assert_eq!(
            search(params).await,
            (6, vec![9001, 9002, 9003, 9005, 9004, 9006])
        );
    }
    assert_eq!(
        search("?q=mission_&sort=updated").await,
        (6, vec![9005, 9004, 9002, 9001, 9003, 9006])
    );
    assert_eq!(
        search("?q=mission_&page=2&pageSize=2").await,
        (6, vec![9003, 9005])
    );
}

#[tokio::test]
async fn read_through_ttl_force_refresh_and_stale_fallback_keep_file_url_fresh() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let mut v1 = sample_item(
        1001,
        "Chernobyl Chapter One",
        1000,
        Some("https://cdn.steamusercontent.com/ugc/1001_v1.vpk"),
    );
    v1.dependencies = vec![WorkshopDependency {
        workshop_id: WorkshopId(2001),
        title: None,
        required: true,
    }];
    upstream.insert_item(v1);

    let state = ServerState::in_memory_with_upstream("admin-secret-123", upstream.clone()).unwrap();
    let app = build_router(state.clone());

    // 1. First batch request triggers read-through fetch from SteamKit CM
    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/items/batch",
        vec![],
        Some(json!({ "ids": ["1001"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let batch: BatchItemsResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(batch.items.len(), 1);
    assert_eq!(
        batch.items[0].file_url.as_deref(),
        Some("https://cdn.steamusercontent.com/ugc/1001_v1.vpk")
    );
    assert_eq!(batch.items[0].previews.len(), 1);
    assert_eq!(batch.items[0].dependencies.len(), 1);
    assert_eq!(batch.items[0].source, MetadataSource::CatalogServer);
    assert_eq!(upstream.fetch_count(WorkshopId(1001)), 1);

    // 2. Addon updates on Steam Workshop with a new CDN file_url
    let v2 = sample_item(
        1001,
        "Chernobyl Chapter One",
        2000,
        Some("https://cdn.steamusercontent.com/ugc/1001_v2.vpk"),
    );
    upstream.insert_item(v2);

    // Within TTL without forceRefresh, cached snapshot is served without hitting Steam CM
    let (status, bytes) = json_request(&app, "GET", "/api/v1/items/1001", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let cached_item: WorkshopItemRecord = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        cached_item.file_url.as_deref(),
        Some("https://cdn.steamusercontent.com/ugc/1001_v1.vpk")
    );
    assert_eq!(upstream.fetch_count(WorkshopId(1001)), 1);

    // 3. Explicit ?refresh=true refreshes from Steam CM and updates file_url + revision history
    let (status, bytes) =
        json_request(&app, "GET", "/api/v1/items/1001?refresh=true", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let refreshed_item: WorkshopItemRecord = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        refreshed_item.file_url.as_deref(),
        Some("https://cdn.steamusercontent.com/ugc/1001_v2.vpk")
    );
    assert_eq!(refreshed_item.time_updated, Some(2000));
    assert_eq!(upstream.fetch_count(WorkshopId(1001)), 2);
    assert_eq!(
        state.repo.list_revisions(WorkshopId(1001)).unwrap().len(),
        2
    );

    // 4. If Steam CM becomes temporarily unreachable, non-forced reads gracefully serve cached data
    upstream.set_fail_fetches(true);
    let (status, bytes) = json_request(&app, "GET", "/api/v1/items/1001", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let fallback_item: WorkshopItemRecord = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        fallback_item.file_url.as_deref(),
        Some("https://cdn.steamusercontent.com/ugc/1001_v2.vpk")
    );
}

#[tokio::test]
async fn tombstone_preserves_history_without_polluting_db_and_respects_admin_override() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let mut item = sample_item(
        3001,
        "Classic Survival Map",
        1500,
        Some("https://cdn.steamusercontent.com/ugc/3001.vpk"),
    );
    item.dependencies = vec![WorkshopDependency {
        workshop_id: WorkshopId(3002),
        title: Some("Shared Textures".into()),
        required: true,
    }];
    upstream.insert_item(item);

    let state =
        ServerState::in_memory_with_upstream("strong-admin-pass", upstream.clone()).unwrap();
    let app = build_router(state.clone());

    // Populate #3001 into cache
    let (status, _) = json_request(&app, "GET", "/api/v1/items/3001", vec![], None).await;
    assert_eq!(status, StatusCode::OK);

    // Upstream removes #3001 (result = 9)
    upstream.insert_outcome(
        WorkshopId(3001),
        WorkshopFetchOutcome::Removed {
            workshop_id: WorkshopId(3001),
            source: MetadataSource::SteamKit,
            observed_at: Utc::now(),
            reason: Some("Steam CM result=9".into()),
        },
    );

    // Admin refreshes #3001 -> turns into tombstone preserving title, previews, and dependencies!
    let admin_hdr = vec![("X-Admin-Token", "strong-admin-pass")];
    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/admin/items/3001/refresh",
        admin_hdr.clone(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let detail: AdminItemDetailResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(detail.item.status, WorkshopItemStatus::Removed);
    assert!(detail.item.file_url.is_none());
    assert!(detail.removed_at.is_some());
    assert_eq!(detail.item.title, "Classic Survival Map");
    assert_eq!(detail.item.previews.len(), 1);
    assert_eq!(detail.item.dependencies.len(), 1);
    assert_eq!(detail.item.dependencies[0].workshop_id, WorkshopId(3002));

    // Querying a never-cached non-existent ID (#9999999) returns 404 on GET and negative-cached Removed on batch without polluting DB
    let (status, _) = json_request(&app, "GET", "/api/v1/items/9999999", vec![], None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(state.repo.stats().unwrap().total_items, 1);

    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/items/batch",
        vec![],
        Some(json!({ "ids": ["9999999"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let neg_batch: BatchItemsResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(neg_batch.items.len(), 1);
    assert_eq!(neg_batch.items[0].status, WorkshopItemStatus::Removed);
    assert_eq!(upstream.fetch_count(WorkshopId(9999999)), 1);
    assert_eq!(state.repo.stats().unwrap().total_items, 1);

    // Admin overrides #3001 with custom mirror file_url and Active status
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/admin/items/3001/override",
        admin_hdr.clone(),
        Some(json!({
            "status": "active",
            "fileUrl": "https://mirror.example.com/archive/3001.vpk",
            "reason": "Restored via archive mirror"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Refresh while locked returns 409 Conflict and upstream sync does not overwrite it
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/admin/items/3001/refresh",
        admin_hdr.clone(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Clearing override unlocks item and re-syncs upstream status
    let (status, bytes) = json_request(
        &app,
        "DELETE",
        "/api/v1/admin/items/3001/override",
        admin_hdr,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let unlocked: AdminItemDetailResponse = serde_json::from_slice(&bytes).unwrap();
    assert!(!unlocked.is_admin_override);
    assert_eq!(unlocked.item.status, WorkshopItemStatus::Removed);
}

#[tokio::test]
async fn collection_auto_hydration_and_catalog_backend_provider_roundtrip() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let mut col = sample_item(5000, "Friday Coop Collection", 3000, None);
    col.file_type = WorkshopFileType::Collection;
    col.collection_children = vec![WorkshopId(5001), WorkshopId(5002)];
    upstream.insert_item(col);
    upstream.insert_item(sample_item(
        5001,
        "Member Addon One",
        3001,
        Some("https://cdn.steamusercontent.com/ugc/5001.vpk"),
    ));
    upstream.insert_item(sample_item(
        5002,
        "Member Addon Two",
        3002,
        Some("https://cdn.steamusercontent.com/ugc/5002.vpk"),
    ));

    let state = ServerState::in_memory_with_upstream("admin-key-123", upstream).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let provider =
        CatalogBackendProvider::new(format!("http://{addr}"), Arc::new(MirrorRouter::empty()));

    // Fetching collection automatically read-through fetches both the collection and its child items
    let fetched_col = provider.fetch_collection(WorkshopId(5000)).await.unwrap();
    assert_eq!(fetched_col.title.as_deref(), Some("Friday Coop Collection"));
    assert_eq!(
        fetched_col.child_ids,
        vec![WorkshopId(5001), WorkshopId(5002)]
    );
    assert_eq!(fetched_col.items.len(), 2);
    assert_eq!(
        fetched_col.items[0].file_url.as_deref(),
        Some("https://cdn.steamusercontent.com/ugc/5001.vpk")
    );

    // Fetching normal item via /collections/{id} returns 404
    assert!(provider.fetch_collection(WorkshopId(5001)).await.is_err());
}

#[tokio::test]
async fn admin_session_toml_config_incremental_backfill_and_steam_auth_flows_work() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("catalog.toml");
    let db_path = dir.path().join("catalog.db");

    // 1. First run auto-generates catalog.toml with strong random admin password
    let (mut cfg, created) = CatalogConfig::load_or_init(&config_path).unwrap();
    assert!(created);
    assert!(config_path.is_file());
    assert!(cfg.admin.password.starts_with("cat_"));
    cfg.server.db_path = db_path;
    cfg.save_to(&config_path).unwrap();

    let (reloaded, created_again) = CatalogConfig::load_or_init(&config_path).unwrap();
    assert!(!created_again);
    let initial_password = reloaded.admin.password.clone();

    let upstream = Arc::new(MockWorkshopUpstream::new());
    upstream.insert_query_page(
        QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE,
        MATCHING_FILE_TYPE_ITEMS,
        "*",
        WorkshopQueryPage {
            total: 2,
            next_cursor: None,
            items: vec![
                sample_item(
                    7001,
                    "Updated Map A",
                    5500,
                    Some("https://cdn.steamusercontent.com/ugc/7001.vpk"),
                ),
                sample_item(
                    7002,
                    "Updated Map B",
                    5400,
                    Some("https://cdn.steamusercontent.com/ugc/7002.vpk"),
                ),
            ],
        },
    );
    upstream.insert_query_page(
        QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
        MATCHING_FILE_TYPE_ITEMS,
        "*",
        WorkshopQueryPage {
            total: 2,
            next_cursor: Some("cursor_page_2".into()),
            items: vec![sample_item(
                7003,
                "Backfill Item",
                4000,
                Some("https://cdn.steamusercontent.com/ugc/7003.vpk"),
            )],
        },
    );

    let store = Arc::new(CatalogConfigStore::new(Some(config_path.clone()), reloaded));
    let repo = Arc::new(ServerRepo::open(&store.get().server.db_path).unwrap());
    let state = ServerState::with_components(store.clone(), repo, upstream);
    let app = build_router(state.clone());

    // 2. Admin UI HTML and empty-password 503 security check
    let (status, _, html) = raw_request(&app, "GET", "/admin/", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&html).contains("Left4Addons Catalog Console"));

    let empty_app = build_router(ServerState::in_memory("").unwrap());
    let (status, _) = json_request(&empty_app, "GET", "/api/v1/admin/stats", vec![], None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    // 3. Login via /api/v1/admin/auth/login sets HttpOnly cookie and authenticates subsequent Cookie requests
    let (status, login_headers, login_bytes) = raw_request(
        &app,
        "POST",
        "/api/v1/admin/auth/login",
        vec![],
        Some(json!({ "username": "admin", "password": initial_password })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let login_resp: AdminLoginResponse = serde_json::from_slice(&login_bytes).unwrap();
    let set_cookie = login_headers
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.contains("HttpOnly"));
    let cookie_pair = set_cookie.split(';').next().unwrap();

    let (status, me_bytes) = json_request(
        &app,
        "GET",
        "/api/v1/admin/auth/me",
        vec![("Cookie", cookie_pair)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let me: AdminSessionInfo = serde_json::from_slice(&me_bytes).unwrap();
    assert!(me.authenticated);
    assert_eq!(me.username.as_deref(), Some("admin"));

    // 4. Update sync config & trigger incremental + backfill sync
    let bearer = format!("Bearer {}", login_resp.token);
    let (status, _) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/sync",
        vec![("Authorization", &bearer)],
        Some(
            serde_json::to_value(CatalogSyncConfigDto {
                enable_read_through: true,
                item_ttl_secs: 600,
                enable_incremental_sync: true,
                incremental_interval_secs: 120,
                incremental_pages_per_tick: 2,
                enable_backfill: true,
                backfill_interval_secs: 10,
                backfill_batch_size: 25,
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, sync_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/admin/sync/trigger",
        vec![("Cookie", cookie_pair)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sync_status: CatalogSyncStatus = serde_json::from_slice(&sync_bytes).unwrap();
    assert_eq!(sync_status.last_incremental_upserted, 2);
    assert_eq!(sync_status.high_water_time_updated, Some(5500));

    let bf_count = state.sync.run_backfill_step_once().await.unwrap();
    assert_eq!(bf_count, 1);

    let (status, ov_bytes) = json_request(
        &app,
        "GET",
        "/api/v1/admin/overview",
        vec![("Cookie", cookie_pair)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let overview: CatalogOverviewResponse = serde_json::from_slice(&ov_bytes).unwrap();
    assert_eq!(overview.stats.total_items, 3);
    assert_eq!(
        overview.sync.backfill_cursor.as_deref(),
        Some("cursor_page_2")
    );
    assert_eq!(overview.sync.backfill_synced_total, 1);

    // 5. SteamKit CM QR & Credentials + Guard auth flows
    let (status, qr_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/admin/steam/auth/qr/start",
        vec![("Cookie", cookie_pair)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let qr_flow: SteamAuthFlowStatus = serde_json::from_slice(&qr_bytes).unwrap();
    assert_eq!(qr_flow.state, SteamAuthFlowState::WaitingForScan);
    assert!(qr_flow.qr_svg.as_deref().unwrap().contains("<svg"));

    let (status, cred_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/admin/steam/auth/credentials",
        vec![("Cookie", cookie_pair)],
        Some(json!({ "accountName": "valve_bot", "password": "secret" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cred_flow: SteamAuthFlowStatus = serde_json::from_slice(&cred_bytes).unwrap();
    assert_eq!(cred_flow.state, SteamAuthFlowState::WaitingForCode);

    let (status, guard_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/admin/steam/auth/guard",
        vec![("Cookie", cookie_pair)],
        Some(json!({ "flowId": cred_flow.flow_id, "code": "A1B2C" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let guard_done: SteamAuthFlowStatus = serde_json::from_slice(&guard_bytes).unwrap();
    assert_eq!(guard_done.state, SteamAuthFlowState::Succeeded);
    assert_eq!(state.upstream.status().await.mode, SteamAuthMode::Account);

    // 6. Change admin password and verify persistence in catalog.toml on disk
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/admin/auth/password",
        vec![("Cookie", cookie_pair)],
        Some(json!({
            "currentPassword": initial_password,
            "newPassword": "updated-super-strong-pass"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (disk_cfg, _) = CatalogConfig::load_or_init(&config_path).unwrap();
    assert_eq!(disk_cfg.admin.password, "updated-super-strong-pass");
    assert_eq!(disk_cfg.sync.item_ttl_secs, 600);
    assert!(disk_cfg.sync.enable_backfill);

    // Batch > 200 rejected with 400 Bad Request
    let too_many: Vec<String> = (1..=201).map(|i| i.to_string()).collect();
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/items/batch",
        vec![],
        Some(json!({ "ids": too_many })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
