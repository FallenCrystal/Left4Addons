use axum::{
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Form, Json, Router,
};
use chrono::Utc;
use l4a_mirror::MirrorRouter;
use l4a_protocol::{
    MetadataSource, SearchQuery, WorkshopDependency, WorkshopFileType, WorkshopFetchOutcome,
    WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use l4a_steam::MockSteamBridge;
use l4a_workshop::{
    DownloadRequest, ResilientDownloader, SourceRoutingPolicy, SteamBridgeProvider,
    SteamWebApiProvider, WebScraperPlaceholderProvider, WorkshopError, WorkshopProvider,
    WorkshopResolver,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

async fn start_router(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

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
                        }
                    ]
                }
            }))
        }),
    );

    let base_url = start_router(app).await;
    let provider = SteamWebApiProvider::with_base_url(base_url, Arc::new(MirrorRouter::empty()));
    let ids = vec![
        WorkshopId::new(101).unwrap(),
        WorkshopId::new(102).unwrap(),
        WorkshopId::new(103).unwrap(),
    ];

    let outcomes = provider.fetch_items(&ids).await.unwrap();
    assert_eq!(outcomes.len(), 3);

    match &outcomes[0] {
        WorkshopFetchOutcome::Found { item } => {
            assert_eq!(item.title, "Active Map");
            assert_eq!(item.file_url.as_deref(), Some("https://cdn.example/101.vpk"));
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
            assert_eq!(item.file_url.as_deref(), Some("https://cdn.example/501.vpk"));
            assert_eq!(item.dependencies.len(), 1);
            assert_eq!(item.dependencies[0].workshop_id, dep_id);
        }
        other => panic!("expected merged item, got {other:?}"),
    }

    assert!(matches!(outcomes[1], WorkshopFetchOutcome::Removed { .. }));
}

#[tokio::test]
async fn downloader_supports_resume_etag_reset_retry_and_cancellation() {
    let payload: Vec<u8> = (0..1024).map(|i| (i % 251) as u8).collect();
    let payload_arc = Arc::new(payload.clone());
    let attempts = Arc::new(AtomicUsize::new(0));

    let app = Router::new()
        .route(
            "/addon.vpk",
            get({
                let data = payload_arc.clone();
                let etag_ref = Arc::new("etag-v1".to_string());
                move |headers: HeaderMap| {
                    let data = data.clone();
                    let etag_val = etag_ref.clone();
                    async move {
                        let mut out_headers = HeaderMap::new();
                        out_headers.insert("ETag", HeaderValue::from_str(&etag_val).unwrap());
                        if let Some(range) = headers.get("Range").and_then(|v| v.to_str().ok()) {
                            let start: usize = range
                                .trim_start_matches("bytes=")
                                .trim_end_matches('-')
                                .parse()
                                .unwrap();
                            if start >= data.len() {
                                return (StatusCode::RANGE_NOT_SATISFIABLE, out_headers, Vec::new())
                                    .into_response();
                            }
                            out_headers.insert(
                                "Content-Range",
                                HeaderValue::from_str(&format!(
                                    "bytes {start}-{}/{}",
                                    data.len() - 1,
                                    data.len()
                                ))
                                .unwrap(),
                            );
                            (StatusCode::PARTIAL_CONTENT, out_headers, data[start..].to_vec())
                                .into_response()
                        } else {
                            (StatusCode::OK, out_headers, (*data).clone()).into_response()
                        }
                    }
                }
            }),
        )
        .route(
            "/flaky.vpk",
            get({
                let data = payload_arc.clone();
                let attempts = attempts.clone();
                move || {
                    let data = data.clone();
                    let attempts = attempts.clone();
                    async move {
                        let n = attempts.fetch_add(1, Ordering::SeqCst);
                        if n == 0 {
                            (StatusCode::INTERNAL_SERVER_ERROR, "transient").into_response()
                        } else {
                            (StatusCode::OK, (*data).clone()).into_response()
                        }
                    }
                }
            }),
        );

    let base_url = start_router(app).await;
    let dir = tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    let downloader = ResilientDownloader::new(&cache_dir, Arc::new(MirrorRouter::empty()));

    // 1. Pre-seed partial file + meta to exercise 206 resume path
    let wid = WorkshopId::new(777).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(cache_dir.join("777.vpk.part"), &payload[..300]).unwrap();
    std::fs::write(
        cache_dir.join("777.part.meta.json"),
        serde_json::to_vec(&json!({
            "workshopId": "777",
            "resolvedUrl": format!("{base_url}/addon.vpk"),
            "destFileName": "777.vpk",
            "totalSize": payload.len(),
            "etag": "etag-v1",
            "lastModified": null
        }))
        .unwrap(),
    )
    .unwrap();

    let dest = dir.path().join("loading/777.vpk");
    let res = downloader
        .download(
            &DownloadRequest {
                workshop_id: wid,
                url: format!("{base_url}/addon.vpk"),
                dest_path: dest.clone(),
                max_retries: 1,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();

    assert_eq!(res.resumed_from, 300);
    assert_eq!(std::fs::read(&dest).unwrap(), payload);

    // 2. ETag mismatch should cleanly reject and remove mismatched partial state
    let wid_mismatch = WorkshopId::new(778).unwrap();
    std::fs::write(cache_dir.join("778.vpk.part"), &payload[..200]).unwrap();
    std::fs::write(
        cache_dir.join("778.part.meta.json"),
        serde_json::to_vec(&json!({
            "workshopId": "778",
            "resolvedUrl": format!("{base_url}/addon.vpk"),
            "destFileName": "778.vpk",
            "totalSize": payload.len(),
            "etag": "etag-old",
            "lastModified": null
        }))
        .unwrap(),
    )
    .unwrap();

    let err = downloader
        .download(
            &DownloadRequest {
                workshop_id: wid_mismatch,
                url: format!("{base_url}/addon.vpk"),
                dest_path: dir.path().join("loading/778.vpk"),
                max_retries: 0,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, WorkshopError::IntegrityMismatch(_)));
    assert!(!cache_dir.join("778.vpk.part").exists());

    // 3. Flaky 500 retry succeeds on 2nd attempt
    let dest_flaky = dir.path().join("loading/779.vpk");
    downloader
        .download(
            &DownloadRequest {
                workshop_id: WorkshopId::new(779).unwrap(),
                url: format!("{base_url}/flaky.vpk"),
                dest_path: dest_flaky.clone(),
                max_retries: 2,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest_flaky).unwrap(), payload);

    // 4. Pre-cancelled token aborts immediately
    let token = CancellationToken::new();
    token.cancel();
    let cancel_err = downloader
        .download(
            &DownloadRequest {
                workshop_id: WorkshopId::new(780).unwrap(),
                url: format!("{base_url}/addon.vpk"),
                dest_path: dir.path().join("loading/780.vpk"),
                max_retries: 1,
            },
            token,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(cancel_err, WorkshopError::Cancelled));

    // 5. HTTP 416 on oversized stale partial recovers automatically from byte 0
    let wid_416 = WorkshopId::new(781).unwrap();
    std::fs::write(cache_dir.join("781.vpk.part"), vec![9u8; 2000]).unwrap();
    std::fs::write(
        cache_dir.join("781.part.meta.json"),
        serde_json::to_vec(&json!({
            "workshopId": "781",
            "resolvedUrl": format!("{base_url}/addon.vpk"),
            "destFileName": "781.vpk",
            "totalSize": payload.len(),
            "etag": "etag-v1",
            "lastModified": null
        }))
        .unwrap(),
    )
    .unwrap();
    let dest_416 = dir.path().join("loading/781.vpk");
    let res_416 = downloader
        .download(
            &DownloadRequest {
                workshop_id: wid_416,
                url: format!("{base_url}/addon.vpk"),
                dest_path: dest_416.clone(),
                max_retries: 1,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(res_416.resumed_from, 0);
    assert_eq!(std::fs::read(&dest_416).unwrap(), payload);

    // 6. ETag mismatch with max_retries=1 self-heals from byte 0 after cleanup
    let wid_etag_retry = WorkshopId::new(782).unwrap();
    std::fs::write(cache_dir.join("782.vpk.part"), &payload[..200]).unwrap();
    std::fs::write(
        cache_dir.join("782.part.meta.json"),
        serde_json::to_vec(&json!({
            "workshopId": "782",
            "resolvedUrl": format!("{base_url}/addon.vpk"),
            "destFileName": "782.vpk",
            "totalSize": payload.len(),
            "etag": "etag-stale",
            "lastModified": null
        }))
        .unwrap(),
    )
    .unwrap();
    let dest_etag = dir.path().join("loading/782.vpk");
    downloader
        .download(
            &DownloadRequest {
                workshop_id: wid_etag_retry,
                url: format!("{base_url}/addon.vpk"),
                dest_path: dest_etag.clone(),
                max_retries: 1,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest_etag).unwrap(), payload);

    // 7. Truncated response (actual body < Content-Length) must NOT be promoted to dest_path
    let trunc_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let trunc_addr = trunc_listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = trunc_listener.accept().await {
            let mut req_buf = [0u8; 512];
            let _ = stream.read(&mut req_buf).await;
            let header = b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(header).await;
            let _ = stream.write_all(&[1u8; 200]).await;
            let _ = stream.shutdown().await;
        }
    });

    let dest_trunc = dir.path().join("loading/783.vpk");
    let trunc_err = downloader
        .download(
            &DownloadRequest {
                workshop_id: WorkshopId::new(783).unwrap(),
                url: format!("http://{trunc_addr}/truncated.vpk"),
                dest_path: dest_trunc.clone(),
                max_retries: 0,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(trunc_err, WorkshopError::Network(_)));
    assert!(!dest_trunc.exists());
}
