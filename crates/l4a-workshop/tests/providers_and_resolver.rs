mod support;

use axum::{
    extract::Query,
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::{get, post},
    Form, Json, Router,
};
use chrono::Utc;
use l4a_mirror::MirrorRouter;
use l4a_protocol::{
    MetadataSource, SearchQuery, WorkshopDependency, WorkshopFetchOutcome, WorkshopFileType,
    WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use l4a_steam::MockSteamBridge;
use l4a_workshop::{
    CatalogBackendProvider, SourceRoutingPolicy, SteamBridgeProvider, SteamWebApiProvider,
    WebScraperPlaceholderProvider, WorkshopError, WorkshopProvider, WorkshopResolver,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::start_router;

#[tokio::test]
async fn steam_web_api_maps_active_removed_and_private_statuses() {
    let app = Router::new().route(
        "/ISteamRemoteStorage/GetPublishedFileDetails/v1/",
        post(|Form(_params): Form<HashMap<String, String>>| async {
            Json(json!({
                "response": {
                    "publishedfiledetails": [
                        {
                            "publishedfileid": "101",
                            "result": 1,
                            "title": "Active Map",
                            "file_url": "https://cdn.example/101.vpk",
                            "file_size": "2048",
                            "time_updated": 1700000100,
                            "tags": [{"tag": "Campaign"}]
                        },
                        {
                            "publishedfileid": "102",
                            "result": 9
                        },
                        {
                            "publishedfileid": "103",
                            "result": 15
                        },
                        {
                            "publishedfileid": "104",
                            "result": 16
                        },
                        {
                            "publishedfileid": "105",
                            "result": 2
                        },
                        {
                            "publishedfileid": "106"
                        }
                    ]
                }
            }))
        }),
    );

    let base_url = start_router(app).await;
    let provider = SteamWebApiProvider::with_base_url(base_url, Arc::new(MirrorRouter::empty()));
    let ids = [101, 102, 103, 104, 105, 106].map(WorkshopId);

    let outcomes = provider.fetch_items(&ids).await.unwrap();
    assert_eq!(outcomes.len(), 3);

    match &outcomes[0] {
        WorkshopFetchOutcome::Found { item } => {
            assert_eq!(item.title, "Active Map");
            assert_eq!(
                item.file_url.as_deref(),
                Some("https://cdn.example/101.vpk")
            );
            assert_eq!(item.file_size, Some(2048));
            assert_eq!(item.tags, vec!["Campaign"]);
        }
        other => panic!("unexpected outcome[0]: {other:?}"),
    }
    assert!(matches!(outcomes[1], WorkshopFetchOutcome::Removed { .. }));
    assert!(matches!(
        outcomes[2],
        WorkshopFetchOutcome::PrivateOrUnavailable { .. }
    ));
}

#[tokio::test]
async fn resolver_merges_sdk_dependencies_with_web_api_file_url_and_skips_scraper_placeholder() {
    let sdk = Arc::new(MockSteamBridge::new(true));
    let id_1 = WorkshopId::new(501).unwrap();
    let dep_id = WorkshopId::new(502).unwrap();
    sdk.insert_item(WorkshopItemRecord {
        workshop_id: id_1,
        file_type: WorkshopFileType::Item,
        status: WorkshopItemStatus::Active,
        title: "Campaign Part 1".into(),
        description: None,
        creator_steam_id: Some("76561198000000010".into()),
        creator_name: Some("Author".into()),
        preview_url: None,
        previews: Vec::new(),
        file_url: None,
        file_size: Some(4096),
        time_created: None,
        time_updated: Some(1700000200),
        tags: vec!["Campaign".into()],
        dependencies: vec![WorkshopDependency {
            workshop_id: dep_id,
            title: None,
            required: true,
        }],
        collection_children: Vec::new(),
        subscriptions: None,
        favorites: None,
        source: MetadataSource::SteamworksSdk,
        fetched_at: Utc::now(),
    });

    let app = Router::new().route(
        "/ISteamRemoteStorage/GetPublishedFileDetails/v1/",
        post(|| async {
            Json(json!({
                "response": {
                    "publishedfiledetails": [
                        {
                            "publishedfileid": "501",
                            "result": 1,
                            "title": "Campaign Part 1",
                            "file_url": "https://cdn.example/501.vpk"
                        },
                        {
                            "publishedfileid": "503",
                            "result": 9
                        }
                    ]
                }
            }))
        }),
    );
    let base_url = start_router(app).await;

    let scraper = Arc::new(WebScraperPlaceholderProvider::placeholder());
    assert!(matches!(
        scraper.search(&SearchQuery::default()).await,
        Err(WorkshopError::ScraperNotImplemented(_))
    ));
    assert!(!scraper.capabilities().search);

    let resolver = WorkshopResolver::new(
        vec![
            scraper,
            Arc::new(SteamBridgeProvider::new(sdk)),
            Arc::new(SteamWebApiProvider::with_base_url(
                base_url,
                Arc::new(MirrorRouter::empty()),
            )),
        ],
        SourceRoutingPolicy::default(),
    );

    let id_removed = WorkshopId::new(503).unwrap();
    let (outcomes, warnings) = resolver.fetch_items(&[id_1, id_removed]).await.unwrap();
    assert!(warnings.is_empty());
    assert_eq!(outcomes.len(), 2);

    match &outcomes[0] {
        WorkshopFetchOutcome::Found { item } => {
            assert_eq!(
                item.file_url.as_deref(),
                Some("https://cdn.example/501.vpk")
            );
            assert_eq!(item.dependencies.len(), 1);
            assert_eq!(item.dependencies[0].workshop_id, dep_id);
        }
        other => panic!("expected merged item, got {other:?}"),
    }

    assert!(matches!(outcomes[1], WorkshopFetchOutcome::Removed { .. }));
}

#[tokio::test]
async fn catalog_backend_accepts_legacy_sources_without_falling_back_to_sdk() {
    let catalog_app = Router::new().route(
        "/api/v1/items/batch",
        post(|| async {
            Json(json!({
                "items": [
                    {
                        "workshopId": "901",
                        "fileType": "item",
                        "status": "active",
                        "title": "Standalone Skin",
                        "fileUrl": "https://cdn.steamusercontent.com/ugc/901/skin.vpk",
                        "dependencies": [],
                        "source": "user_contribution",
                        "fetchedAt": "2026-11-21T00:00:00Z"
                    },
                    {
                        "workshopId": "902",
                        "fileType": "item",
                        "status": "active",
                        "title": "Current Catalog Item",
                        "fileUrl": "https://cdn.steamusercontent.com/ugc/902/item.vpk",
                        "dependencies": [],
                        "source": "catalog_server",
                        "fetchedAt": "2026-11-21T00:00:00Z"
                    }
                ],
                "missingIds": []
            }))
        }),
    );
    let catalog_url = start_router(catalog_app).await;

    // Unavailable SDK bridge: if resolver wrongly falls through for empty dependencies, it would emit a warning!
    let broken_sdk = Arc::new(MockSteamBridge::new(false));
    let resolver = WorkshopResolver::new(
        vec![
            Arc::new(CatalogBackendProvider::new(
                catalog_url,
                Arc::new(MirrorRouter::empty()),
            )),
            Arc::new(SteamBridgeProvider::new(broken_sdk)),
        ],
        SourceRoutingPolicy::default(),
    );

    let ids = [WorkshopId::new(901).unwrap(), WorkshopId::new(902).unwrap()];
    let (outcomes, warnings) = resolver.fetch_items(&ids).await.unwrap();
    assert!(
        warnings.is_empty(),
        "Catalog hit with file_url and authoritative empty dependencies should not invoke SDK: {warnings:?}"
    );
    assert_eq!(
        outcomes
            .iter()
            .map(WorkshopFetchOutcome::workshop_id)
            .collect::<Vec<_>>(),
        ids
    );
}

#[tokio::test]
async fn resolver_search_preserves_provider_errors_and_uses_default_pagination() {
    let seen_query = Arc::new(Mutex::new(HashMap::<String, String>::new()));
    let seen_clone = Arc::clone(&seen_query);
    let ok_app = Router::new().route(
        "/api/v1/items/search",
        get(move |Query(params): Query<HashMap<String, String>>| {
            let seen = Arc::clone(&seen_clone);
            async move {
                *seen.lock().unwrap() = params;
                Json(json!({
                    "items": [],
                    "total": 0,
                    "page": 1,
                    "pageSize": 30,
                    "source": "catalog_server"
                }))
            }
        }),
    );
    let ok_url = start_router(ok_app).await;
    let ok_resolver = WorkshopResolver::new(
        vec![Arc::new(CatalogBackendProvider::new(
            ok_url,
            Arc::new(MirrorRouter::empty()),
        ))],
        SourceRoutingPolicy::default(),
    );
    let (resp, warnings) = ok_resolver.search(&SearchQuery::default()).await.unwrap();
    assert!(warnings.is_empty());
    assert_eq!(resp.page, 1);
    assert_eq!(resp.page_size, 30);
    {
        let recorded = seen_query.lock().unwrap();
        assert_eq!(recorded.get("page").map(String::as_str), Some("1"));
        assert_eq!(recorded.get("pageSize").map(String::as_str), Some("30"));
    }

    let failing_app = Router::new().route(
        "/api/v1/items/search",
        get(|| async { (StatusCode::BAD_GATEWAY, "upstream unavailable") }),
    );
    let failing_url = start_router(failing_app).await;
    let failing_resolver = WorkshopResolver::new(
        vec![Arc::new(CatalogBackendProvider::new(
            failing_url,
            Arc::new(MirrorRouter::empty()),
        ))],
        SourceRoutingPolicy::default(),
    );
    let err = failing_resolver
        .search(&SearchQuery::default())
        .await
        .expect_err("502 response should fail search");
    assert!(
        matches!(err, WorkshopError::HttpStatus { status: 502, .. }),
        "expected HttpStatus 502 instead of Unsupported, got {err:?}"
    );

    let unsupported_resolver = WorkshopResolver::new(
        vec![Arc::new(WebScraperPlaceholderProvider::placeholder())],
        SourceRoutingPolicy::default(),
    );
    let err = unsupported_resolver
        .search(&SearchQuery::default())
        .await
        .expect_err("placeholder-only resolver should be Unsupported");
    assert!(matches!(err, WorkshopError::Unsupported(_)));
}

#[tokio::test(start_paused = true)]
async fn catalog_backend_search_deadline_accommodates_steam_timeout_and_sqlite_fallback() {
    use l4a_workshop::providers::catalog_backend::SEARCH_TIMEOUT;

    // Combined server budget: 15s Steam + 2s local search queue wait + 5s SQLite fallback = 22s.
    let combined_server_budget = Duration::from_secs(15 + 2 + 5);
    assert!(
        SEARCH_TIMEOUT >= combined_server_budget + Duration::from_secs(5),
        "SEARCH_TIMEOUT ({SEARCH_TIMEOUT:?}) must exceed combined server budget plus transfer margin"
    );

    let fallback_app = Router::new().route(
        "/api/v1/items/search",
        get(|| async {
            // Simulate full 15s Steam timeout + 2s queue wait + 4.5s SQLite fallback (21.5s > old 20s timeout).
            tokio::time::sleep(Duration::from_millis(21_500)).await;
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-l4a-search-mode",
                HeaderValue::from_static("local-fallback"),
            );
            (
                headers,
                Json(json!({
                    "items": [
                        {
                            "workshopId": "777001",
                            "fileType": "item",
                            "status": "active",
                            "title": "Recovered SQLite Fallback Item",
                            "tags": ["Campaigns"],
                            "dependencies": [],
                            "source": "catalog_server",
                            "fetchedAt": "2026-11-24T00:00:00Z"
                        }
                    ],
                    "total": 1,
                    "page": 1,
                    "pageSize": 30,
                    "source": "catalog_server"
                })),
            )
        }),
    );

    let url = start_router(fallback_app).await;
    let provider = CatalogBackendProvider::new(url, Arc::new(MirrorRouter::empty()));
    assert_eq!(provider.search_timeout(), SEARCH_TIMEOUT);

    let resp = provider
        .search(&SearchQuery {
            query: Some("Recovered".to_string()),
            ..SearchQuery::default()
        })
        .await
        .expect("21.5s server fallback must complete within desktop SEARCH_TIMEOUT");
    assert_eq!(resp.total, 1);
    assert_eq!(resp.items[0].workshop_id, WorkshopId(777001));
}
