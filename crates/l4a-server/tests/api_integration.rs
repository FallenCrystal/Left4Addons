use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use chrono::Utc;
use http_body_util::BodyExt;
use l4a_mirror::MirrorRouter;
use l4a_protocol::{
    AuditLogEntry, BatchItemsResponse, CatalogStats, ConflictDecision, CreateContributorRequest,
    CreateContributorResponse, ItemSubmission, MetadataSource, ScraperPlaceholderStatus,
    SearchResponse, SubmissionBatchRequest, SubmissionBatchResponse, TrustTier, WorkshopDependency,
    WorkshopFetchOutcome, WorkshopFileType, WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use l4a_server::{build_router, ServerState};
use l4a_workshop::{CatalogBackendProvider, WorkshopProvider};
use serde_json::json;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower::ServiceExt;

async fn json_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    headers: Vec<(&str, &str)>,
    body: Option<serde_json::Value>,
) -> (StatusCode, Vec<u8>) {
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
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

#[tokio::test]
async fn search_items_supports_catalog_sorts_and_title_relevance() {
    let state = ServerState::in_memory("test-admin").unwrap();
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
                    file_url: None,
                    file_size: None,
                    time_created: created,
                    time_updated: updated,
                    tags: vec![],
                    dependencies: vec![],
                    collection_children: vec![],
                    subscriptions,
                    favorites: None,
                    source: MetadataSource::AdminOverride,
                    fetched_at: Utc::now(),
                },
                100,
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
async fn catalog_server_handles_trust_conflict_resolution_dependencies_and_admin_flows() {
    let state = ServerState::in_memory("super-secret-admin").unwrap();
    let app = build_router(state.clone());

    // 1. Health & Admin UI smoke check
    let (status, _) = json_request(&app, "GET", "/api/v1/health", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, html_bytes) = json_request(&app, "GET", "/admin", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&html_bytes).contains("Left4Addons v2 Catalog Server"));

    // 2. Unauthorized contributor create rejected; authorized succeeds
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/admin/contributors",
        vec![],
        Some(json!({ "name": "Alice", "trustTier": "trusted_player" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/admin/contributors",
        vec![("X-Admin-Token", "super-secret-admin")],
        Some(
            serde_json::to_value(CreateContributorRequest {
                name: "TrustedAlice".into(),
                steam_id: Some("76561198000000100".into()),
                trust_tier: TrustTier::TrustedPlayer,
                trust_weight: None,
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let alice: CreateContributorResponse = serde_json::from_slice(&bytes).unwrap();

    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/admin/contributors",
        vec![("X-Admin-Token", "super-secret-admin")],
        Some(
            serde_json::to_value(CreateContributorRequest {
                name: "CommunityBob".into(),
                steam_id: None,
                trust_tier: TrustTier::Community,
                trust_weight: None,
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let bob: CreateContributorResponse = serde_json::from_slice(&bytes).unwrap();

    // 3. TrustedAlice submits item #1001 (missing file_url, active)
    let wid = WorkshopId::new(1001).unwrap();
    let dep1 = WorkshopId::new(2001).unwrap();
    let dep2 = WorkshopId::new(2002).unwrap();
    let alice_auth = format!("Bearer {}", alice.token);
    let bob_auth = format!("Bearer {}", bob.token);

    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/submissions",
        vec![("Authorization", &alice_auth)],
        Some(
            serde_json::to_value(SubmissionBatchRequest {
                items: vec![ItemSubmission {
                    workshop_id: wid,
                    file_type: WorkshopFileType::Item,
                    status: WorkshopItemStatus::Active,
                    title: "Dark Carnival Remix".into(),
                    description: Some("First snapshot".into()),
                    creator_steam_id: Some("76561198000000999".into()),
                    creator_name: Some("Mapper".into()),
                    preview_url: None,
                    file_url: None,
                    file_size: Some(12345),
                    time_created: Some(1700000000),
                    time_updated: Some(1700001000),
                    tags: vec!["Campaign".into()],
                    dependencies: vec![WorkshopDependency {
                        workshop_id: dep1,
                        title: Some("Asset Pack 1".into()),
                        required: true,
                    }],
                    collection_children: vec![],
                    subscriptions: Some(500),
                    favorites: Some(100),
                    observed_source: MetadataSource::SteamworksSdk,
                    observed_at: Utc::now(),
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let resp: SubmissionBatchResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(resp.receipts[0].decision, ConflictDecision::Accepted);

    // 4. CommunityBob tries to maliciously mark #1001 as Removed -> FlaggedForReview, item stays Active!
    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/submissions",
        vec![("Authorization", &bob_auth)],
        Some(
            serde_json::to_value(SubmissionBatchRequest {
                items: vec![ItemSubmission {
                    workshop_id: wid,
                    file_type: WorkshopFileType::Item,
                    status: WorkshopItemStatus::Removed,
                    title: "Hijacked Title".into(),
                    description: None,
                    creator_steam_id: None,
                    creator_name: None,
                    preview_url: None,
                    file_url: None,
                    file_size: None,
                    time_created: None,
                    time_updated: Some(1700002000),
                    tags: vec![],
                    dependencies: vec![],
                    collection_children: vec![],
                    subscriptions: None,
                    favorites: None,
                    observed_source: MetadataSource::UserContribution,
                    observed_at: Utc::now(),
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let resp: SubmissionBatchResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        resp.receipts[0].decision,
        ConflictDecision::FlaggedForReview
    );

    // 5a. CommunityBob tries poisoning non-allowlisted file_url -> FlaggedForReview, file_url untouched!
    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/submissions",
        vec![("Authorization", &bob_auth)],
        Some(
            serde_json::to_value(SubmissionBatchRequest {
                items: vec![ItemSubmission {
                    workshop_id: wid,
                    file_type: WorkshopFileType::Item,
                    status: WorkshopItemStatus::Active,
                    title: "Should Not Overwrite".into(),
                    description: None,
                    creator_steam_id: None,
                    creator_name: None,
                    preview_url: None,
                    file_url: Some("https://evil.example/1001.vpk".into()),
                    file_size: None,
                    time_created: None,
                    time_updated: Some(1700001000),
                    tags: vec![],
                    dependencies: vec![],
                    collection_children: vec![],
                    subscriptions: None,
                    favorites: None,
                    observed_source: MetadataSource::SteamWebApi,
                    observed_at: Utc::now(),
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let resp: SubmissionBatchResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        resp.receipts[0].decision,
        ConflictDecision::FlaggedForReview
    );

    // 5b. CommunityBob tries injecting allowlisted file_url -> RejectedLowerTrust (cannot poison file_url/deps)!
    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/submissions",
        vec![("Authorization", &bob_auth)],
        Some(
            serde_json::to_value(SubmissionBatchRequest {
                items: vec![ItemSubmission {
                    workshop_id: wid,
                    file_type: WorkshopFileType::Item,
                    status: WorkshopItemStatus::Active,
                    title: "Should Not Overwrite".into(),
                    description: None,
                    creator_steam_id: None,
                    creator_name: None,
                    preview_url: None,
                    file_url: Some("https://steamusercontent.com/ugc/1001.vpk".into()),
                    file_size: None,
                    time_created: None,
                    time_updated: Some(1700001000),
                    tags: vec![],
                    dependencies: vec![],
                    collection_children: vec![],
                    subscriptions: None,
                    favorites: None,
                    observed_source: MetadataSource::SteamWebApi,
                    observed_at: Utc::now(),
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let resp: SubmissionBatchResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        resp.receipts[0].decision,
        ConflictDecision::RejectedLowerTrust
    );

    // 5c. CommunityBob enriches missing benign preview_url (Steam CDN allowlisted) -> Merged without overwriting title/file_url!
    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/submissions",
        vec![("Authorization", &bob_auth)],
        Some(
            serde_json::to_value(SubmissionBatchRequest {
                items: vec![ItemSubmission {
                    workshop_id: wid,
                    file_type: WorkshopFileType::Item,
                    status: WorkshopItemStatus::Active,
                    title: "Should Not Overwrite".into(),
                    description: None,
                    creator_steam_id: None,
                    creator_name: None,
                    preview_url: Some(
                        "https://steamuserimages-a.akamaihd.net/ugc/1001/preview.jpg".into(),
                    ),
                    file_url: None,
                    file_size: None,
                    time_created: None,
                    time_updated: Some(1700001000),
                    tags: vec![],
                    dependencies: vec![],
                    collection_children: vec![],
                    subscriptions: None,
                    favorites: None,
                    observed_source: MetadataSource::UserContribution,
                    observed_at: Utc::now(),
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let resp: SubmissionBatchResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(resp.receipts[0].decision, ConflictDecision::Merged);

    let (status, bytes) = json_request(&app, "GET", "/api/v1/items/1001", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let stored: WorkshopItemRecord = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(stored.status, WorkshopItemStatus::Active);
    assert_eq!(stored.title, "Dark Carnival Remix");
    assert_eq!(stored.file_url, None);
    assert_eq!(
        stored.preview_url.as_deref(),
        Some("https://steamuserimages-a.akamaihd.net/ugc/1001/preview.jpg")
    );

    // 6. TrustedAlice submits newer revision where dep1 is replaced by dep2 -> dependency history preserved
    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/submissions",
        vec![("Authorization", &alice_auth)],
        Some(
            serde_json::to_value(SubmissionBatchRequest {
                items: vec![ItemSubmission {
                    workshop_id: wid,
                    file_type: WorkshopFileType::Item,
                    status: WorkshopItemStatus::Active,
                    title: "Dark Carnival Remix v2".into(),
                    description: None,
                    creator_steam_id: None,
                    creator_name: None,
                    preview_url: None,
                    file_url: None,
                    file_size: Some(22345),
                    time_created: None,
                    time_updated: Some(1700005000),
                    tags: vec!["Campaign".into()],
                    dependencies: vec![WorkshopDependency {
                        workshop_id: dep2,
                        title: Some("Asset Pack 2".into()),
                        required: true,
                    }],
                    collection_children: vec![],
                    subscriptions: None,
                    favorites: None,
                    observed_source: MetadataSource::SteamworksSdk,
                    observed_at: Utc::now(),
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let resp: SubmissionBatchResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(resp.receipts[0].decision, ConflictDecision::Accepted);
    assert_eq!(state.repo.list_removed_deps(wid).unwrap(), vec![dep1]);
    assert!(state.repo.list_revisions(wid).unwrap().len() >= 2);

    // 7. Disable CommunityBob -> next submission gets 403 Forbidden
    let patch_uri = format!("/api/v1/admin/contributors/{}", bob.contributor.id);
    let (status, _) = json_request(
        &app,
        "PATCH",
        &patch_uri,
        vec![("X-Admin-Token", "super-secret-admin")],
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/submissions",
        vec![("Authorization", &bob_auth)],
        Some(json!({ "items": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 8. Admin override sets Removed & verifies audit/stats/scraper placeholder
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/admin/items/1001/override",
        vec![("X-Admin-Token", "super-secret-admin")],
        Some(json!({ "status": "removed", "reason": "Confirmed DMCA takedown" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/items/batch",
        vec![],
        Some(json!({ "ids": ["1001", "9999"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let batch: BatchItemsResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.items[0].status, WorkshopItemStatus::Removed);
    assert_eq!(batch.missing_ids, vec![WorkshopId::new(9999).unwrap()]);

    let (_, audit_bytes) = json_request(
        &app,
        "GET",
        "/api/v1/admin/audit",
        vec![("X-Admin-Token", "super-secret-admin")],
        None,
    )
    .await;
    let audits: Vec<AuditLogEntry> = serde_json::from_slice(&audit_bytes).unwrap();
    assert!(audits
        .iter()
        .any(|a| a.decision == ConflictDecision::FlaggedForReview));

    let (_, stats_bytes) = json_request(
        &app,
        "GET",
        "/api/v1/admin/stats",
        vec![("X-Admin-Token", "super-secret-admin")],
        None,
    )
    .await;
    let stats: CatalogStats = serde_json::from_slice(&stats_bytes).unwrap();
    assert_eq!(stats.removed_items, 1);

    let (_, scraper_bytes) = json_request(
        &app,
        "GET",
        "/api/v1/admin/scraper/status",
        vec![("X-Admin-Token", "super-secret-admin")],
        None,
    )
    .await;
    let scraper: ScraperPlaceholderStatus = serde_json::from_slice(&scraper_bytes).unwrap();
    assert!(!scraper.implemented);
}

#[tokio::test]
async fn workshop_catalog_provider_roundtrips_with_live_l4a_server() {
    let state = ServerState::in_memory("admin-key").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base_url = format!("http://{addr}");
    let cid = l4a_protocol::ContributorId::generate();
    let plain = "l4a_live_client_token";
    state
        .repo
        .create_contributor(
            &cid,
            "DesktopNode",
            None,
            &l4a_server::auth::hash_token(plain),
            TrustTier::TrustedPlayer,
            50,
        )
        .unwrap();

    let provider = CatalogBackendProvider::new(&base_url, Arc::new(MirrorRouter::empty()));
    let wid = WorkshopId::new(4242).unwrap();
    let receipt = provider
        .submit_observations(
            plain,
            &SubmissionBatchRequest {
                items: vec![ItemSubmission {
                    workshop_id: wid,
                    file_type: WorkshopFileType::Item,
                    status: WorkshopItemStatus::Active,
                    title: "Live Server Addon".into(),
                    description: Some("Synced over HTTP".into()),
                    creator_steam_id: None,
                    creator_name: Some("Tester".into()),
                    preview_url: None,
                    file_url: Some("https://steamusercontent.com/ugc/4242.vpk".into()),
                    file_size: Some(888),
                    time_created: Some(100),
                    time_updated: Some(200),
                    tags: vec!["Weapons".into()],
                    dependencies: vec![],
                    collection_children: vec![],
                    subscriptions: None,
                    favorites: None,
                    observed_source: MetadataSource::SteamworksSdk,
                    observed_at: Utc::now(),
                }],
            },
        )
        .await
        .unwrap();
    assert_eq!(receipt.receipts[0].decision, ConflictDecision::Accepted);

    let fetched = provider.fetch_items(&[wid]).await.unwrap();
    assert_eq!(fetched.len(), 1);
    match &fetched[0] {
        WorkshopFetchOutcome::Found { item } => {
            assert_eq!(item.title, "Live Server Addon");
            assert_eq!(item.source, MetadataSource::CatalogServer);
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[tokio::test]
async fn server_security_search_collection_batch_limit_and_conflict_regressions() {
    // 1. Empty admin token refuses admin access cleanly with 503 Service Unavailable
    let empty_state = ServerState::in_memory("").unwrap();
    let empty_app = build_router(empty_state);
    let (status, _) = json_request(
        &empty_app,
        "GET",
        "/api/v1/admin/stats",
        vec![("X-Admin-Token", "")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    // 2. Non-empty admin token rejects empty X-Admin-Token header
    let state = ServerState::in_memory("strong-admin-token").unwrap();
    let app = build_router(state.clone());
    let (status, _) = json_request(
        &app,
        "GET",
        "/api/v1/admin/stats",
        vec![("X-Admin-Token", "")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Create two Community contributors + one Official contributor
    let create_contrib = |name: &str, tier: TrustTier| {
        let app = app.clone();
        let name = name.to_string();
        async move {
            let (st, bytes) = json_request(
                &app,
                "POST",
                "/api/v1/admin/contributors",
                vec![("X-Admin-Token", "strong-admin-token")],
                Some(
                    serde_json::to_value(CreateContributorRequest {
                        name,
                        steam_id: None,
                        trust_tier: tier,
                        trust_weight: None,
                    })
                    .unwrap(),
                ),
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            serde_json::from_slice::<CreateContributorResponse>(&bytes).unwrap()
        }
    };
    let comm1 = create_contrib("CommOne", TrustTier::Community).await;
    let comm2 = create_contrib("CommTwo", TrustTier::Community).await;
    let official = create_contrib("OfficialNode", TrustTier::Official).await;

    let wid = WorkshopId::new(8001).unwrap();
    let dep = WorkshopId::new(8002).unwrap();

    // CommOne inserts item 8001 with tag Campaign and dep 8002
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/submissions",
        vec![("Authorization", &format!("Bearer {}", comm1.token))],
        Some(
            serde_json::to_value(SubmissionBatchRequest {
                items: vec![ItemSubmission {
                    workshop_id: wid,
                    file_type: WorkshopFileType::Item,
                    status: WorkshopItemStatus::Active,
                    title: "Community Campaign".into(),
                    description: Some("Test 100% match".into()),
                    creator_steam_id: None,
                    creator_name: None,
                    preview_url: None,
                    file_url: None,
                    file_size: None,
                    time_created: Some(100),
                    time_updated: Some(200),
                    tags: vec!["Campaign".into(), "Coop".into()],
                    dependencies: vec![WorkshopDependency {
                        workshop_id: dep,
                        title: None,
                        required: true,
                    }],
                    collection_children: vec![],
                    subscriptions: Some(42),
                    favorites: None,
                    observed_source: MetadataSource::SteamworksSdk,
                    observed_at: Utc::now(),
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 3. Equal-weight Community cannot immediately delist another Community's Active item
    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/submissions",
        vec![("Authorization", &format!("Bearer {}", comm2.token))],
        Some(
            serde_json::to_value(SubmissionBatchRequest {
                items: vec![ItemSubmission {
                    workshop_id: wid,
                    file_type: WorkshopFileType::Item,
                    status: WorkshopItemStatus::Removed,
                    title: "Community Campaign".into(),
                    description: None,
                    creator_steam_id: None,
                    creator_name: None,
                    preview_url: None,
                    file_url: None,
                    file_size: None,
                    time_created: None,
                    time_updated: Some(300),
                    tags: vec![],
                    dependencies: vec![],
                    collection_children: vec![],
                    subscriptions: None,
                    favorites: None,
                    observed_source: MetadataSource::UserContribution,
                    observed_at: Utc::now(),
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let delist_resp: SubmissionBatchResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        delist_resp.receipts[0].decision,
        ConflictDecision::FlaggedForReview
    );

    // 4. SteamWebApi update with newer time_updated and empty dependencies preserves existing deps!
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/submissions",
        vec![("Authorization", &format!("Bearer {}", official.token))],
        Some(
            serde_json::to_value(SubmissionBatchRequest {
                items: vec![ItemSubmission {
                    workshop_id: wid,
                    file_type: WorkshopFileType::Item,
                    status: WorkshopItemStatus::Active,
                    title: "Community Campaign Updated".into(),
                    description: None,
                    creator_steam_id: None,
                    creator_name: None,
                    preview_url: None,
                    file_url: Some("https://steamusercontent.com/ugc/8001.vpk".into()),
                    file_size: None,
                    time_created: None,
                    time_updated: Some(400),
                    tags: vec!["Campaign".into()],
                    dependencies: vec![],
                    collection_children: vec![],
                    subscriptions: None,
                    favorites: None,
                    observed_source: MetadataSource::SteamWebApi,
                    observed_at: Utc::now(),
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stored = state.repo.get_item(wid).unwrap().unwrap();
    assert_eq!(stored.record.dependencies.len(), 1);
    assert_eq!(stored.record.dependencies[0].workshop_id, dep);

    // 5. ?tag=campaign query works accurately & escaped LIKE works
    let (status, bytes) = json_request(
        &app,
        "GET",
        "/api/v1/items/search?tag=campaign&q=100%25",
        vec![],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let search_res: l4a_protocol::SearchResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(search_res.total, 1);
    assert_eq!(search_res.items[0].workshop_id, wid);

    // 6. GET /api/v1/collections/{normal_item} returns 404
    let (status, _) = json_request(&app, "GET", "/api/v1/collections/8001", vec![], None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 7. Batch > 200 rejected with 400 Bad Request
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

    // 8. AdminOverride uses trust weight 200 so Official (100) cannot overwrite it
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/admin/items/8001/override",
        vec![("X-Admin-Token", "strong-admin-token")],
        Some(json!({ "status": "removed", "reason": "Locked by admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/submissions",
        vec![("Authorization", &format!("Bearer {}", official.token))],
        Some(
            serde_json::to_value(SubmissionBatchRequest {
                items: vec![ItemSubmission {
                    workshop_id: wid,
                    file_type: WorkshopFileType::Item,
                    status: WorkshopItemStatus::Active,
                    title: "Official Overwrite Attempt".into(),
                    description: None,
                    creator_steam_id: None,
                    creator_name: None,
                    preview_url: None,
                    file_url: None,
                    file_size: None,
                    time_created: None,
                    time_updated: Some(999999),
                    tags: vec![],
                    dependencies: vec![],
                    collection_children: vec![],
                    subscriptions: None,
                    favorites: None,
                    observed_source: MetadataSource::SteamworksSdk,
                    observed_at: Utc::now(),
                }],
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ov_attempt: SubmissionBatchResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        ov_attempt.receipts[0].decision,
        ConflictDecision::RejectedLowerTrust
    );
}

#[tokio::test]
async fn catalog_write_errors_roll_back_batches_and_admin_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let state = ServerState::open(&l4a_server::ServerConfig {
        bind_addr: "127.0.0.1:0".into(),
        db_path: path.clone(),
        admin_token: "admin-token".into(),
    })
    .unwrap();
    let contributor = l4a_protocol::ContributorId::generate();
    state
        .repo
        .create_contributor(
            &contributor,
            "Tester",
            None,
            &l4a_server::auth::hash_token("fixture-token"),
            TrustTier::Community,
            20,
        )
        .unwrap();
    let sql = rusqlite::Connection::open(path).unwrap();
    sql.execute_batch(
        "CREATE TRIGGER fail_audit BEFORE INSERT ON submission_audit_log
         WHEN NEW.workshop_id = 102 BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
    )
    .unwrap();
    let app = build_router(state.clone());
    let batch = json!({"items": [
        {"workshopId": "101", "title": "First", "status": "active",
         "observedSource": "steamworks_sdk", "observedAt": Utc::now(),
         "dependencies": [{"workshopId": "103", "required": true}]},
        {"workshopId": "102", "title": "Second", "status": "active",
         "observedSource": "steamworks_sdk", "observedAt": Utc::now()}
    ]});
    let headers = vec![("authorization", "Bearer fixture-token")];
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/submissions",
        headers.clone(),
        Some(batch.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    for table in [
        "workshop_items",
        "workshop_item_dependencies",
        "workshop_item_revisions",
        "submission_audit_log",
    ] {
        let count: i64 = sql
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    assert!(state.repo.list_contributors().unwrap()[0]
        .last_seen_at
        .is_none());

    sql.execute_batch("DROP TRIGGER fail_audit").unwrap();
    let (status, body) = json_request(
        &app,
        "POST",
        "/api/v1/submissions",
        headers.clone(),
        Some(batch),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let receipts: SubmissionBatchResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        receipts
            .receipts
            .iter()
            .map(|r| r.workshop_id.0)
            .collect::<Vec<_>>(),
        [101, 102]
    );

    sql.execute_batch(
        "CREATE TRIGGER fail_admin_audit BEFORE INSERT ON submission_audit_log
         WHEN NEW.contributor_name = 'admin' BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
    )
    .unwrap();
    let admin = vec![("x-admin-token", "admin-token")];
    let uri = "/api/v1/admin/items/101/override";
    let (status, _) = json_request(
        &app,
        "POST",
        uri,
        admin.clone(),
        Some(json!({"status": "removed", "dependencies": []})),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let original = state
        .repo
        .get_item(WorkshopId(101))
        .unwrap()
        .unwrap()
        .record;
    assert_eq!(original.status, WorkshopItemStatus::Active);
    assert_eq!(original.dependencies.len(), 1);
    assert_eq!(state.repo.list_revisions(WorkshopId(101)).unwrap().len(), 1);

    sql.execute_batch("DROP TRIGGER fail_admin_audit").unwrap();
    let (status, _) = json_request(
        &app,
        "POST",
        uri,
        admin.clone(),
        Some(json!({"status": "removed"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        state
            .repo
            .get_item(WorkshopId(101))
            .unwrap()
            .unwrap()
            .record
            .dependencies
            .len(),
        1
    );
    let (status, _) = json_request(
        &app,
        "POST",
        uri,
        admin,
        Some(json!({"status": "removed", "dependencies": []})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(state
        .repo
        .get_item(WorkshopId(101))
        .unwrap()
        .unwrap()
        .record
        .dependencies
        .is_empty());

    // Hold a competing writer while the request starts, then disable the contributor before releasing it.
    sql.execute_batch("BEGIN IMMEDIATE; UPDATE contributors SET enabled = 0")
        .unwrap();
    let submit = tokio::spawn(async move {
        json_request(
            &app,
            "POST",
            "/api/v1/submissions",
            headers,
            Some(json!({"items": []})),
        )
        .await
    });
    tokio::task::yield_now().await;
    sql.execute_batch("COMMIT").unwrap();
    assert_eq!(submit.await.unwrap().0, StatusCode::FORBIDDEN);
}
