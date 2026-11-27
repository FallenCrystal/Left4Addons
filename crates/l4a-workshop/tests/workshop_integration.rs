use axum::{
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
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
    DownloadProgress, DownloadRequest, ResilientDownloader, SourceRoutingPolicy,
    SteamBridgeProvider, SteamWebApiProvider, WebScraperPlaceholderProvider, WorkshopError,
    WorkshopProvider, WorkshopResolver,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

async fn start_router(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn direct_resource_fingerprint(url: &str) -> String {
    let resource = serde_json::to_vec(&json!([url, null, {}])).unwrap();
    format!("{:x}", Sha256::digest(resource))
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
            Arc::new(l4a_workshop::CatalogBackendProvider::new(
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
                                return (
                                    StatusCode::RANGE_NOT_SATISFIABLE,
                                    out_headers,
                                    Vec::new(),
                                )
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
                            (
                                StatusCode::PARTIAL_CONTENT,
                                out_headers,
                                data[start..].to_vec(),
                            )
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
            "resourceFingerprint": direct_resource_fingerprint(&format!("{base_url}/addon.vpk")),
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
            "resourceFingerprint": direct_resource_fingerprint(&format!("{base_url}/addon.vpk")),
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
            "resourceFingerprint": direct_resource_fingerprint(&format!("{base_url}/addon.vpk")),
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
            "resourceFingerprint": direct_resource_fingerprint(&format!("{base_url}/addon.vpk")),
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

async fn wait_for_progress(
    samples: &mut mpsc::UnboundedReceiver<DownloadProgress>,
    description: &str,
    matches: impl Fn(&DownloadProgress) -> bool,
) -> DownloadProgress {
    timeout(Duration::from_secs(5), async {
        while let Some(sample) = samples.recv().await {
            if matches(&sample) {
                return sample;
            }
        }
        panic!("progress stream ended before {description}");
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {description}"))
}

#[tokio::test]
async fn downloader_preserves_partial_when_resume_returns_no_content() {
    use axum::http::header::{CONTENT_RANGE, RANGE};

    let attempts = Arc::new(AtomicUsize::new(0));
    let base_url = start_router(Router::new().route(
        "/addon.vpk",
        get(move |headers: HeaderMap| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                assert_eq!(headers.get(RANGE).unwrap(), "bytes=3-");
                if attempt == 0 {
                    StatusCode::NO_CONTENT.into_response()
                } else {
                    (
                        StatusCode::PARTIAL_CONTENT,
                        [(CONTENT_RANGE, "bytes 3-5/6")],
                        "def",
                    )
                        .into_response()
                }
            }
        }),
    ))
    .await;

    let dir = tempdir().unwrap();
    let mirror = Arc::new(MirrorRouter::empty());
    mirror.set_no_proxy_for_testing(true);
    let downloader = ResilientDownloader::new(dir.path(), mirror);
    let request = DownloadRequest {
        workshop_id: WorkshopId(1),
        url: format!("{base_url}/addon.vpk"),
        dest_path: dir.path().join("addon.vpk"),
        max_retries: 0,
    };
    let partial_path = dir.path().join("1.vpk.part");
    let meta_path = dir.path().join("1.part.meta.json");
    let meta = serde_json::to_vec(&json!({
        "workshopId": "1",
        "resolvedUrl": request.url,
        "resourceFingerprint": direct_resource_fingerprint(&request.url),
        "destFileName": "addon.vpk",
        "totalSize": 6
    }))
    .unwrap();
    std::fs::write(&partial_path, b"abc").unwrap();
    std::fs::write(&meta_path, &meta).unwrap();

    let result = downloader
        .download(&request, CancellationToken::new(), None)
        .await;
    assert!(matches!(
        result,
        Err(WorkshopError::HttpStatus { status: 204, .. })
    ));
    assert_eq!(std::fs::read(&partial_path).unwrap(), b"abc");
    assert_eq!(std::fs::read(&meta_path).unwrap(), meta);
    assert!(!request.dest_path.exists());

    let result = downloader
        .download(&request, CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(result.resumed_from, 3);
    assert_eq!(std::fs::read(&request.dest_path).unwrap(), b"abcdef");
}

#[tokio::test]
async fn downloader_restarts_on_redirect_target_change_and_for_legacy_partials() {
    use axum::http::header;
    use std::sync::atomic::AtomicBool;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let old_target = format!("http://{}/addon.vpk", listener.local_addr().unwrap());
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
    let interrupted = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        assert!(socket.read(&mut [0; 1024]).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nab")
            .await
            .unwrap();
        wait_for_progress(&mut progress_rx, "partial download", |p| p.downloaded == 2).await;
        socket.shutdown().await.unwrap();
    });

    let range_requests = Arc::new(AtomicUsize::new(0));
    let requests = Arc::clone(&range_requests);
    let new_server = start_router(Router::new().route(
        "/addon.vpk",
        get(move |headers: HeaderMap| {
            let requests = Arc::clone(&requests);
            async move {
                if let Some(range) = headers.get(header::RANGE) {
                    assert_eq!(range, "bytes=2-");
                    requests.fetch_add(1, Ordering::SeqCst);
                    return (
                        StatusCode::PARTIAL_CONTENT,
                        [(header::CONTENT_RANGE, "bytes 2-3/4")],
                        "YZ",
                    )
                        .into_response();
                }
                (StatusCode::OK, "WXYZ").into_response()
            }
        }),
    ))
    .await;
    let new_target = format!("{new_server}/addon.vpk");
    let switched = Arc::new(AtomicBool::new(false));
    let use_new_target = Arc::clone(&switched);
    let entry = start_router(Router::new().route(
        "/redirect",
        get(move || {
            let target = if use_new_target.load(Ordering::SeqCst) {
                new_target.clone()
            } else {
                old_target.clone()
            };
            async move { (StatusCode::FOUND, [(header::LOCATION, target)]) }
        }),
    ))
    .await;

    let dir = tempdir().unwrap();
    let mirror = Arc::new(MirrorRouter::empty());
    mirror.set_no_proxy_for_testing(true);
    let downloader = ResilientDownloader::new(dir.path(), mirror)
        .with_progress_intervals(Duration::ZERO, Duration::from_secs(1));
    let req = DownloadRequest {
        workshop_id: WorkshopId(784),
        url: format!("{entry}/redirect"),
        dest_path: dir.path().join("addon.vpk"),
        max_retries: 0,
    };
    let result = downloader
        .download(
            &req,
            CancellationToken::new(),
            Some(Arc::new(move |p| {
                let _ = progress_tx.send(p);
            })),
        )
        .await;
    assert!(matches!(result, Err(WorkshopError::Network(_))));
    interrupted.await.unwrap();

    let part_path = dir.path().join("784.vpk.part");
    let meta_path = dir.path().join("784.part.meta.json");
    let partial = std::fs::read(&part_path).unwrap();
    assert_eq!(partial, b"ab");
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("resourceFingerprint");

    switched.store(true, Ordering::SeqCst);
    let result = downloader
        .download(&req, CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(result.resumed_from, 0);
    assert_eq!(std::fs::read(&req.dest_path).unwrap(), b"WXYZ");
    assert_eq!(range_requests.load(Ordering::SeqCst), 1);

    std::fs::remove_file(&req.dest_path).unwrap();
    std::fs::write(part_path, partial).unwrap();
    std::fs::write(meta_path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let result = downloader
        .download(&req, CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(result.resumed_from, 0);
    assert_eq!(std::fs::read(&req.dest_path).unwrap(), b"WXYZ");
    assert_eq!(range_requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn downloader_refreshes_speed_on_tick_drops_on_stall_and_excludes_resumed_bytes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let resume_from = 4096usize;
    let burst_one = 16 * 1024usize;
    let burst_two = 8 * 1024usize;
    let tail = 64usize;
    let total = resume_from + burst_one + burst_two + tail;
    let (send_sample, mut samples) = mpsc::unbounded_channel();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut req_buf = [0u8; 1024];
        let _ = stream.read(&mut req_buf).await;
        let header = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {resume_from}-{}/{total}\r\nContent-Length: {}\r\nETag: \"speed-v1\"\r\nConnection: close\r\n\r\n",
            total - 1,
            burst_one + burst_two + tail
        );
        stream.write_all(header.as_bytes()).await.unwrap();
        let initial = wait_for_progress(&mut samples, "initial progress", |_| true).await;
        assert_eq!(initial.downloaded, resume_from as u64);
        assert_eq!(initial.speed_bps, Some(0));

        // The first speed sample must exclude the resumed bytes while the body is idle.
        let idle =
            wait_for_progress(&mut samples, "speed before receiving new bytes", |_| true).await;
        assert_eq!(idle.downloaded, resume_from as u64);
        assert_eq!(idle.speed_bps, Some(0));

        // Send the first burst across several chunks and hold the body open during the stall.
        for _ in 0..8 {
            stream.write_all(&vec![0xAB; burst_one / 8]).await.unwrap();
        }
        let first_offset = (resume_from + burst_one) as u64;
        wait_for_progress(&mut samples, "positive speed after the first burst", |p| {
            p.downloaded == first_offset && p.speed_bps.unwrap_or(0) > 0
        })
        .await;
        wait_for_progress(&mut samples, "zero speed while stalled", |p| {
            p.downloaded == first_offset && p.speed_bps == Some(0)
        })
        .await;

        stream.write_all(&vec![0xCD; burst_two]).await.unwrap();
        wait_for_progress(&mut samples, "speed recovery after the second burst", |p| {
            p.downloaded == (resume_from + burst_one + burst_two) as u64
                && p.speed_bps.unwrap_or(0) > 0
        })
        .await;

        stream.write_all(&vec![0xEF; tail]).await.unwrap();
        stream.shutdown().await.unwrap();
        samples
    });

    let dir = tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let wid = WorkshopId::new(901).unwrap();
    std::fs::write(cache_dir.join("901.vpk.part"), vec![0x11; resume_from]).unwrap();
    std::fs::write(
        cache_dir.join("901.part.meta.json"),
        serde_json::to_vec(&json!({
            "workshopId": "901",
            "resolvedUrl": format!("http://{addr}/stream.vpk"),
            "resourceFingerprint": direct_resource_fingerprint(&format!("http://{addr}/stream.vpk")),
            "destFileName": "901.vpk",
            "totalSize": total,
            "etag": "\"speed-v1\"",
            "lastModified": null
        }))
        .unwrap(),
    )
    .unwrap();

    let downloader = ResilientDownloader::new(&cache_dir, Arc::new(MirrorRouter::empty()))
        .with_progress_intervals(Duration::from_millis(30), Duration::from_millis(120));

    let dest = dir.path().join("loading/901.vpk");
    let result = downloader
        .download(
            &DownloadRequest {
                workshop_id: wid,
                url: format!("http://{addr}/stream.vpk"),
                dest_path: dest,
                max_retries: 0,
            },
            CancellationToken::new(),
            Some(Arc::new(move |p| {
                let _ = send_sample.send(p);
            })),
        )
        .await;
    let mut samples = timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    let res = result.unwrap();

    assert_eq!(res.resumed_from, resume_from as u64);
    assert_eq!(res.bytes_written, total as u64);

    let final_sample = std::iter::from_fn(|| samples.try_recv().ok())
        .last()
        .unwrap();
    assert_eq!(final_sample.percent, 100);
}

#[tokio::test]
async fn resolver_search_preserves_provider_errors_and_uses_default_pagination() {
    use axum::extract::Query;
    use axum::routing::get;
    use std::collections::HashMap;

    let seen_query = Arc::new(std::sync::Mutex::new(HashMap::<String, String>::new()));
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
        vec![Arc::new(l4a_workshop::CatalogBackendProvider::new(
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
        vec![Arc::new(l4a_workshop::CatalogBackendProvider::new(
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
    use axum::routing::get;
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
    let provider = l4a_workshop::CatalogBackendProvider::new(url, Arc::new(MirrorRouter::empty()));
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

async fn start_counting_keep_alive_server<F>(
    handler: F,
) -> (String, Arc<std::sync::Mutex<Vec<usize>>>)
where
    F: Fn(&str, &str) -> (u16, &'static str, Vec<u8>) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let next_conn_id = Arc::new(AtomicUsize::new(1));
    let served_conns = Arc::new(std::sync::Mutex::new(Vec::<usize>::new()));
    let handler = Arc::new(handler);

    let served_conns_bg = Arc::clone(&served_conns);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let conn_id = next_conn_id.fetch_add(1, Ordering::SeqCst);
            let served_conn = Arc::clone(&served_conns_bg);
            let handler_conn = Arc::clone(&handler);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let mut pending = Vec::new();
                loop {
                    let header_end = loop {
                        if let Some(pos) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
                            break Some(pos + 4);
                        }
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => break None,
                            Ok(n) => pending.extend_from_slice(&buf[..n]),
                        }
                    };
                    let Some(end) = header_end else {
                        break;
                    };
                    let header_text = String::from_utf8_lossy(&pending[..end]).into_owned();
                    let mut lines = header_text.lines();
                    let request_line = lines.next().unwrap_or_default();
                    let mut parts = request_line.split_whitespace();
                    let method = parts.next().unwrap_or_default().to_string();
                    let path = parts.next().unwrap_or_default().to_string();
                    let content_length = lines
                        .find_map(|line| {
                            let (k, v) = line.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    pending.drain(..end);
                    while pending.len() < content_length {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => pending.extend_from_slice(&buf[..n]),
                        }
                    }
                    pending.drain(..content_length);

                    served_conn.lock().unwrap().push(conn_id);
                    let (status, content_type, body) = handler_conn(&method, &path);
                    let reason = match status {
                        200 => "OK",
                        404 => "Not Found",
                        416 => "Range Not Satisfiable",
                        _ => "Status",
                    };
                    let head = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                        body.len()
                    );
                    if socket.write_all(head.as_bytes()).await.is_err()
                        || socket.write_all(&body).await.is_err()
                    {
                        break;
                    }
                }
            });
        }
    });

    (format!("http://{addr}"), served_conns)
}

#[tokio::test]
async fn catalog_steam_web_api_and_downloader_reuse_tcp_keep_alive_connections() {
    use std::collections::HashSet;

    let mirror = Arc::new(MirrorRouter::empty());
    mirror.set_no_proxy_for_testing(true);

    // 1. CatalogBackendProvider: batch (15s), collection (15s), search (30s) all share one TCP connection.
    let (catalog_url, catalog_served) =
        start_counting_keep_alive_server(|method, path| match (method, path) {
            ("POST", "/api/v1/items/batch") => (
                200,
                "application/json",
                serde_json::to_vec(&json!({
                    "items": [{
                        "workshopId": "101",
                        "fileType": "item",
                        "status": "active",
                        "title": "Pooled Map",
                        "tags": [],
                        "dependencies": [],
                        "source": "catalog_server",
                        "fetchedAt": "2026-11-24T00:00:00Z"
                    }],
                    "missingIds": []
                }))
                .unwrap(),
            ),
            ("GET", "/api/v1/collections/202") => (
                200,
                "application/json",
                serde_json::to_vec(&json!({
                    "collectionId": "202",
                    "status": "active",
                    "title": "Pooled Collection",
                    "childIds": [],
                    "items": [],
                    "source": "catalog_server",
                    "fetchedAt": "2026-11-24T00:00:00Z"
                }))
                .unwrap(),
            ),
            ("GET", p) if p.starts_with("/api/v1/items/search") => (
                200,
                "application/json",
                serde_json::to_vec(&json!({
                    "items": [],
                    "total": 0,
                    "page": 1,
                    "pageSize": 30,
                    "source": "catalog_server"
                }))
                .unwrap(),
            ),
            _ => (404, "text/plain", b"not found".to_vec()),
        })
        .await;

    let catalog = l4a_workshop::CatalogBackendProvider::new(catalog_url, Arc::clone(&mirror));
    assert_eq!(
        catalog.fetch_items(&[WorkshopId(101)]).await.unwrap().len(),
        1
    );
    tokio::task::yield_now().await;
    assert_eq!(
        catalog
            .fetch_collection(WorkshopId(202))
            .await
            .unwrap()
            .collection_id,
        WorkshopId(202)
    );
    tokio::task::yield_now().await;
    assert_eq!(
        catalog.search(&SearchQuery::default()).await.unwrap().total,
        0
    );
    {
        let conns = catalog_served.lock().unwrap();
        assert_eq!(conns.len(), 3);
        let unique: HashSet<usize> = conns.iter().copied().collect();
        assert_eq!(
            unique.len(),
            1,
            "CatalogBackendProvider should reuse one serving TCP connection across batch, collection, and search"
        );
    }

    // 2. SteamWebApiProvider: fetch_collection + fetch_items reuse one TCP connection.
    let (steam_url, steam_served) = start_counting_keep_alive_server(|_, path| match path {
        "/ISteamRemoteStorage/GetCollectionDetails/v1/" => (
            200,
            "application/json",
            serde_json::to_vec(&json!({
                "response": {
                    "collectiondetails": [{
                        "publishedfileid": "300",
                        "result": 1,
                        "children": [{"publishedfileid": "301"}]
                    }]
                }
            }))
            .unwrap(),
        ),
        "/ISteamRemoteStorage/GetPublishedFileDetails/v1/" => (
            200,
            "application/json",
            serde_json::to_vec(&json!({
                "response": {
                    "publishedfiledetails": [
                        {"publishedfileid": "300", "result": 1, "title": "Collection 300"},
                        {"publishedfileid": "301", "result": 1, "title": "Item 301"}
                    ]
                }
            }))
            .unwrap(),
        ),
        _ => (404, "text/plain", b"not found".to_vec()),
    })
    .await;

    let steam = SteamWebApiProvider::with_base_url(steam_url, Arc::clone(&mirror));
    let col = steam.fetch_collection(WorkshopId(300)).await.unwrap();
    assert_eq!(col.items.len(), 1);
    tokio::task::yield_now().await;
    assert_eq!(
        steam.fetch_items(&[WorkshopId(301)]).await.unwrap().len(),
        2
    );
    {
        let conns = steam_served.lock().unwrap();
        assert_eq!(conns.len(), 3);
        let unique: HashSet<usize> = conns.iter().copied().collect();
        assert_eq!(
            unique.len(),
            1,
            "SteamWebApiProvider should reuse one serving TCP connection across collection and item calls"
        );
    }

    // 3. ResilientDownloader: sequential downloads reuse one streaming client & TCP connection.
    let (cdn_url, cdn_served) = start_counting_keep_alive_server(|_, _| {
        (200, "application/octet-stream", b"vpk-bytes".to_vec())
    })
    .await;
    let dir = tempdir().unwrap();
    let downloader = ResilientDownloader::new(dir.path().join("cache"), Arc::clone(&mirror));
    for id in [401u64, 402u64] {
        let dest = dir.path().join(format!("{id}.vpk"));
        let res = downloader
            .download(
                &DownloadRequest {
                    workshop_id: WorkshopId(id),
                    url: format!("{cdn_url}/ugc/{id}.vpk"),
                    dest_path: dest,
                    max_retries: 0,
                },
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(res.bytes_written, 9);
        tokio::task::yield_now().await;
    }
    {
        let conns = cdn_served.lock().unwrap();
        assert_eq!(conns.len(), 2);
        let unique: HashSet<usize> = conns.iter().copied().collect();
        assert_eq!(
            unique.len(),
            1,
            "ResilientDownloader should reuse one serving TCP connection across sequential downloads"
        );
    }
}

#[tokio::test]
async fn downloader_416_restart_and_steam_web_api_batches_respect_live_https_only_toggle() {
    // 1. Downloader 416 restart must re-check live https_only and reject plain HTTP restart.
    let dl_mirror = Arc::new(MirrorRouter::empty());
    dl_mirror.set_no_proxy_for_testing(true);
    let dl_mirror_flip = Arc::clone(&dl_mirror);
    let (dl_url, dl_served) = start_counting_keep_alive_server(move |_, _| {
        // Simulate save_config enabling https_only while the resume GET is in flight.
        dl_mirror_flip.set_https_only(true);
        (416, "text/plain", b"range not satisfiable".to_vec())
    })
    .await;

    let dir = tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let wid = WorkshopId(555);
    let req_url = format!("{dl_url}/ugc/555.vpk");
    std::fs::write(cache_dir.join("555.vpk.part"), b"stale-partial").unwrap();
    std::fs::write(
        cache_dir.join("555.part.meta.json"),
        serde_json::to_vec(&json!({
            "workshopId": "555",
            "resolvedUrl": req_url,
            "resourceFingerprint": direct_resource_fingerprint(&req_url),
            "destFileName": "555.vpk",
            "totalSize": 100,
            "etag": null,
            "lastModified": null
        }))
        .unwrap(),
    )
    .unwrap();

    let downloader = ResilientDownloader::new(&cache_dir, Arc::clone(&dl_mirror));
    let dl_err = downloader
        .download(
            &DownloadRequest {
                workshop_id: wid,
                url: req_url,
                dest_path: dir.path().join("555.vpk"),
                max_retries: 0,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("416 restart must fail after https_only is enabled mid-flight");
    assert!(matches!(dl_err, WorkshopError::Network(_)));
    assert_eq!(
        dl_served.lock().unwrap().len(),
        1,
        "downloader must not issue a plain-HTTP 416 restart once https_only is enabled"
    );

    // 2. SteamWebApiProvider multi-batch fetch_items must re-check live https_only before chunk 2.
    let api_mirror = Arc::new(MirrorRouter::empty());
    api_mirror.set_no_proxy_for_testing(true);
    let api_mirror_flip = Arc::clone(&api_mirror);
    let (api_url, api_served) = start_counting_keep_alive_server(move |_, _| {
        api_mirror_flip.set_https_only(true);
        (
            200,
            "application/json",
            serde_json::to_vec(&json!({
                "response": { "publishedfiledetails": [] }
            }))
            .unwrap(),
        )
    })
    .await;

    let provider = SteamWebApiProvider::with_base_url(api_url, Arc::clone(&api_mirror));
    let ids: Vec<WorkshopId> = (1..=105).map(WorkshopId).collect();
    let api_err = provider
        .fetch_items(&ids)
        .await
        .expect_err("second Steam Web API batch must fail after https_only is enabled");
    assert!(matches!(api_err, WorkshopError::Network(_)));
    assert_eq!(
        api_served.lock().unwrap().len(),
        1,
        "second Steam Web API batch must not be sent over plain HTTP once https_only is enabled"
    );
}
