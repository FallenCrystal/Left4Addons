mod support;

use axum::{
    body::Body,
    http::{
        header::{self, CONTENT_RANGE, ETAG, RANGE},
        HeaderMap, HeaderValue, StatusCode, Uri,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures_util::{stream, StreamExt};
use l4a_mirror::MirrorRouter;
use l4a_protocol::WorkshopId;
use l4a_workshop::{DownloadRequest, ResilientDownloader, WorkshopError};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use support::{seed_partial, start_router, wait_for_progress};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Notify};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn downloader_restarts_on_redirect_target_change_and_for_legacy_partials() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let old_target = format!("http://{}/addon.vpk", listener.local_addr().unwrap());
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
    let interrupted = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        assert!(socket.read(&mut [0; 1024]).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nETag: \"v1\"\r\nConnection: close\r\n\r\nab")
            .await
            .unwrap();
        wait_for_progress(&mut progress_rx, "partial download", |p| p.downloaded == 2).await;
        socket.shutdown().await.unwrap();
    });

    let range_requests = Arc::new(AtomicUsize::new(0));
    let requests = Arc::clone(&range_requests);
    let replacement_status = Arc::new(Mutex::new(StatusCode::SERVICE_UNAVAILABLE));
    let status = Arc::clone(&replacement_status);
    let new_server = start_router(Router::new().route(
        "/addon.vpk",
        get(move |headers: HeaderMap| {
            let requests = Arc::clone(&requests);
            let status = *status.lock().unwrap();
            async move {
                if let Some(range) = headers.get(header::RANGE) {
                    assert_eq!(range, "bytes=2-");
                    requests.fetch_add(1, Ordering::SeqCst);
                    return (
                        StatusCode::PARTIAL_CONTENT,
                        [
                            (header::CONTENT_RANGE, "bytes 2-3/4"),
                            (header::ETAG, "\"v1\""),
                        ],
                        "YZ",
                    )
                        .into_response();
                }
                match status {
                    StatusCode::OK => (status, "WXYZ").into_response(),
                    StatusCode::PARTIAL_CONTENT => {
                        (status, [(header::CONTENT_RANGE, "bytes 2-3/4")], "YZ").into_response()
                    }
                    _ => status.into_response(),
                }
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
    let saved_meta = std::fs::read(&meta_path).unwrap();
    let mut legacy: serde_json::Value = serde_json::from_slice(&saved_meta).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("resourceFingerprint");

    switched.store(true, Ordering::SeqCst);
    let error = downloader
        .download(&req, CancellationToken::new(), None)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        WorkshopError::HttpStatus { status: 503, .. }
    ));
    assert_eq!(std::fs::read(&part_path).unwrap(), partial);
    assert_eq!(std::fs::read(&meta_path).unwrap(), saved_meta);
    assert_eq!(range_requests.load(Ordering::SeqCst), 1);

    *replacement_status.lock().unwrap() = StatusCode::OK;
    let result = downloader
        .download(&req, CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(result.resumed_from, 0);
    assert_eq!(std::fs::read(&req.dest_path).unwrap(), b"WXYZ");
    assert_eq!(range_requests.load(Ordering::SeqCst), 2);

    std::fs::remove_file(&req.dest_path).unwrap();
    std::fs::write(&part_path, &partial).unwrap();
    let legacy_meta = serde_json::to_vec(&legacy).unwrap();
    std::fs::write(&meta_path, &legacy_meta).unwrap();
    *replacement_status.lock().unwrap() = StatusCode::PARTIAL_CONTENT;
    let error = downloader
        .download(&req, CancellationToken::new(), None)
        .await
        .unwrap_err();
    assert!(matches!(error, WorkshopError::IntegrityMismatch(_)));
    assert_eq!(std::fs::read(&part_path).unwrap(), partial);
    assert_eq!(std::fs::read(&meta_path).unwrap(), legacy_meta);

    *replacement_status.lock().unwrap() = StatusCode::OK;
    let result = downloader
        .download(&req, CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(result.resumed_from, 0);
    assert_eq!(std::fs::read(&req.dest_path).unwrap(), b"WXYZ");
    assert_eq!(range_requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn downloader_resumes_rotated_signatures_with_a_matching_strong_etag() {
    const OLD_QUERY: &str = "version=1&signature=old&expires=100";
    const NEW_QUERY: &str = "version=1&signature=new&expires=200";
    const STRONG_ETAG: &str = "\"file-v1\"";
    const WEAK_ETAG: &str = "W/\"file-v1\"";

    for (via_redirect, query, old_etag, etag, expected_offset, expected_requests) in [
        (true, NEW_QUERY, STRONG_ETAG, STRONG_ETAG, 3, 2),
        (false, NEW_QUERY, STRONG_ETAG, STRONG_ETAG, 3, 2),
        (
            true,
            "version=2&signature=new&expires=200",
            STRONG_ETAG,
            STRONG_ETAG,
            0,
            3,
        ),
        (
            false,
            "version=2&signature=new&expires=200",
            STRONG_ETAG,
            STRONG_ETAG,
            0,
            2,
        ),
        (true, NEW_QUERY, WEAK_ETAG, WEAK_ETAG, 0, 2),
        (true, NEW_QUERY, STRONG_ETAG, "\"file-v2\"", 0, 3),
    ] {
        let requests = Arc::new(AtomicUsize::new(0));
        let downloads = Arc::clone(&requests);
        let redirect_requests = Arc::new(AtomicUsize::new(0));
        let full_body = if expected_offset > 0 {
            "abcdef"
        } else {
            "UVWXYZ"
        };
        let prefix_downloaded = Arc::new(Notify::new());
        let progress = Arc::clone(&prefix_downloaded);
        let app = Router::new()
            .route(
                "/download",
                get(move || {
                    let query = if redirect_requests.fetch_add(1, Ordering::SeqCst) == 0 {
                        OLD_QUERY
                    } else {
                        query
                    };
                    async move {
                        (
                            StatusCode::FOUND,
                            [(header::LOCATION, format!("/addon.vpk?{query}"))],
                        )
                    }
                }),
            )
            .route(
                "/addon.vpk",
                get(move |headers: HeaderMap, uri: Uri| {
                    downloads.fetch_add(1, Ordering::SeqCst);
                    let prefix_downloaded = Arc::clone(&prefix_downloaded);
                    async move {
                        if uri.query() == Some(OLD_QUERY) {
                            let body = Body::from_stream(
                                stream::iter([Ok::<_, std::io::Error>("abc")]).chain(stream::once(
                                    async move {
                                        prefix_downloaded.notified().await;
                                        Err(std::io::Error::other("interrupted transfer"))
                                    },
                                )),
                            );
                            return (
                                StatusCode::OK,
                                [(header::CONTENT_LENGTH, "6"), (header::ETAG, old_etag)],
                                body,
                            )
                                .into_response();
                        }
                        if let Some(range) = headers.get(header::RANGE) {
                            assert_eq!(range, "bytes=3-");
                            return (
                                StatusCode::PARTIAL_CONTENT,
                                [(header::CONTENT_RANGE, "bytes 3-5/6"), (header::ETAG, etag)],
                                &full_body[3..],
                            )
                                .into_response();
                        }
                        (StatusCode::OK, [(header::ETAG, etag)], full_body).into_response()
                    }
                }),
            );

        let base_url = start_router(app).await;
        let dir = tempdir().unwrap();
        let mirror = Arc::new(MirrorRouter::empty());
        mirror.set_no_proxy_for_testing(true);
        let downloader = ResilientDownloader::new(dir.path(), mirror)
            .with_progress_intervals(Duration::ZERO, Duration::from_secs(1));
        let mut request = DownloadRequest {
            workshop_id: WorkshopId(785),
            url: if via_redirect {
                format!("{base_url}/download")
            } else {
                format!("{base_url}/addon.vpk?{OLD_QUERY}")
            },
            dest_path: dir.path().join("addon.vpk"),
            max_retries: 0,
        };

        let interrupted = timeout(
            Duration::from_secs(5),
            downloader.download(
                &request,
                CancellationToken::new(),
                Some(Arc::new(move |download| {
                    if download.downloaded == 3 {
                        progress.notify_one();
                    }
                })),
            ),
        )
        .await
        .unwrap();
        assert!(matches!(interrupted, Err(WorkshopError::Network(_))));
        assert_eq!(
            std::fs::read(dir.path().join("785.vpk.part")).unwrap(),
            b"abc"
        );

        if !via_redirect {
            request.url = format!("{base_url}/addon.vpk?{query}");
        }
        let result = downloader
            .download(&request, CancellationToken::new(), None)
            .await
            .unwrap();
        assert_eq!(
            result.resumed_from, expected_offset,
            "redirect={via_redirect} {query} {old_etag} {etag}"
        );
        assert_eq!(
            std::fs::read(&request.dest_path).unwrap(),
            full_body.as_bytes()
        );
        assert_eq!(requests.load(Ordering::SeqCst), expected_requests);
    }
}

#[tokio::test]
async fn downloader_restarts_when_a_resume_cannot_be_validated() {
    const STRONG: &str = "\"v1\"";
    const WEAK: &str = "W/\"v1\"";
    for (saved_total, saved_etag, etag, range, suffix, expected_ranges) in [
        (Some(6), None, None, "bytes 3-5/6", "XYZ", 0),
        (Some(6), Some(WEAK), Some(WEAK), "bytes 3-5/6", "XYZ", 0),
        (Some(6), Some(STRONG), None, "bytes 3-5/6", "XYZ", 1),
        (Some(6), Some(STRONG), Some(STRONG), "bytes 3-4/6", "def", 1),
        (
            Some(6),
            Some(STRONG),
            Some(STRONG),
            "bytes 3-6/*",
            "defg",
            1,
        ),
        (None, Some(STRONG), Some(STRONG), "bytes 3-4/*", "de", 1),
    ] {
        let ranges = Arc::new(AtomicUsize::new(0));
        let requested_ranges = Arc::clone(&ranges);
        let full_body = if etag == Some(STRONG) {
            "abcdef"
        } else {
            "UVWXYZ"
        };
        let base_url = start_router(Router::new().route(
            "/addon.vpk",
            get(move |headers: HeaderMap| {
                let requested_ranges = Arc::clone(&requested_ranges);
                async move {
                    let mut response = if let Some(value) = headers.get(RANGE) {
                        assert_eq!(value, "bytes=3-");
                        requested_ranges.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::PARTIAL_CONTENT,
                            [(CONTENT_RANGE, range)],
                            suffix,
                        )
                            .into_response()
                    } else {
                        (StatusCode::OK, full_body).into_response()
                    };
                    if let Some(etag) = etag {
                        response
                            .headers_mut()
                            .insert(ETAG, HeaderValue::from_static(etag));
                    }
                    response
                }
            }),
        ))
        .await;

        let dir = tempdir().unwrap();
        let mirror = Arc::new(MirrorRouter::empty());
        mirror.set_no_proxy_for_testing(true);
        let request = DownloadRequest {
            workshop_id: WorkshopId(1),
            url: format!("{base_url}/addon.vpk"),
            dest_path: dir.path().join("addon.vpk"),
            max_retries: 0,
        };
        seed_partial(dir.path(), &request, b"abc", saved_total, saved_etag);
        let result = ResilientDownloader::new(dir.path(), mirror)
            .download(&request, CancellationToken::new(), None)
            .await
            .unwrap();

        assert_eq!(result.resumed_from, 0, "{saved_etag:?} {etag:?} {range}");
        assert_eq!(
            std::fs::read(&request.dest_path).unwrap(),
            full_body.as_bytes()
        );
        assert_eq!(ranges.load(Ordering::SeqCst), expected_ranges);
    }
}

#[tokio::test]
async fn downloader_rolls_back_a_chunked_body_that_exceeds_its_content_range() {
    let first_chunk_written = Arc::new(Notify::new());
    let progress = Arc::clone(&first_chunk_written);
    let base_url = start_router(Router::new().route(
        "/addon.vpk",
        get(move |headers: HeaderMap| {
            assert_eq!(headers.get(RANGE).unwrap(), "bytes=3-");
            let first_chunk_written = Arc::clone(&first_chunk_written);
            async move {
                let body = Body::from_stream(stream::iter([Ok::<_, std::io::Error>("d")]).chain(
                    stream::once(async move {
                        first_chunk_written.notified().await;
                        Ok("ef")
                    }),
                ));
                (
                    StatusCode::PARTIAL_CONTENT,
                    [(CONTENT_RANGE, "bytes 3-4/6"), (ETAG, "\"v1\"")],
                    body,
                )
            }
        }),
    ))
    .await;

    let dir = tempdir().unwrap();
    let mirror = Arc::new(MirrorRouter::empty());
    mirror.set_no_proxy_for_testing(true);
    let request = DownloadRequest {
        workshop_id: WorkshopId(1),
        url: format!("{base_url}/addon.vpk"),
        dest_path: dir.path().join("addon.vpk"),
        max_retries: 0,
    };
    seed_partial(dir.path(), &request, b"abc", Some(6), Some("\"v1\""));
    let downloader = ResilientDownloader::new(dir.path(), mirror)
        .with_progress_intervals(Duration::ZERO, Duration::from_secs(1));
    let result = timeout(
        Duration::from_secs(5),
        downloader.download(
            &request,
            CancellationToken::new(),
            Some(Arc::new(move |download| {
                if download.downloaded == 4 {
                    progress.notify_one();
                }
            })),
        ),
    )
    .await
    .unwrap();

    assert!(matches!(result, Err(WorkshopError::IntegrityMismatch(_))));
    assert_eq!(
        std::fs::read(dir.path().join("1.vpk.part")).unwrap(),
        b"abc"
    );
    assert!(!request.dest_path.exists());
}
