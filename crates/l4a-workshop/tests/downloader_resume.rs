mod support;

use axum::{
    http::{
        header::{CONTENT_RANGE, ETAG, RANGE},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use l4a_mirror::MirrorRouter;
use l4a_protocol::WorkshopId;
use l4a_workshop::{DownloadRequest, ResilientDownloader, WorkshopError};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use support::{seed_partial, seed_partial_with_etag_raw, start_router};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

fn sample_payload() -> Vec<u8> {
    (0..1024).map(|i| (i % 251) as u8).collect()
}

async fn start_addon_server(payload: Vec<u8>) -> (String, Arc<AtomicUsize>) {
    let payload_arc = Arc::new(payload);
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/addon.vpk",
            get({
                let data = payload_arc.clone();
                let etag_ref = Arc::new("\"etag-v1\"".to_string());
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
                let data = payload_arc;
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

    (start_router(app).await, attempts)
}

#[tokio::test]
async fn downloader_resumes_from_partial_with_matching_strong_etag() {
    let payload = sample_payload();
    let (base_url, _) = start_addon_server(payload.clone()).await;
    let dir = tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    let downloader = ResilientDownloader::new(&cache_dir, Arc::new(MirrorRouter::empty()));

    let wid = WorkshopId::new(777).unwrap();
    let url = format!("{base_url}/addon.vpk");
    seed_partial_with_etag_raw(
        &cache_dir,
        wid,
        &url,
        "777.vpk",
        &payload[..300],
        Some(payload.len() as u64),
        Some("\"etag-v1\""),
    );

    let dest = dir.path().join("loading/777.vpk");
    let res = downloader
        .download(
            &DownloadRequest {
                workshop_id: wid,
                url,
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
}

#[tokio::test]
async fn downloader_restarts_when_etag_mismatches_or_was_unquoted() {
    let payload = sample_payload();
    let (base_url, _) = start_addon_server(payload.clone()).await;
    let dir = tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    let downloader = ResilientDownloader::new(&cache_dir, Arc::new(MirrorRouter::empty()));
    let url = format!("{base_url}/addon.vpk");

    for (id, etag, max_retries) in [(778, "\"etag-old\"", 0), (782, "etag-stale", 1)] {
        let wid = WorkshopId::new(id).unwrap();
        let dest_name = format!("{id}.vpk");
        seed_partial_with_etag_raw(
            &cache_dir,
            wid,
            &url,
            &dest_name,
            &payload[..200],
            Some(payload.len() as u64),
            Some(etag),
        );

        let dest = dir.path().join("loading").join(&dest_name);
        let result = downloader
            .download(
                &DownloadRequest {
                    workshop_id: wid,
                    url: url.clone(),
                    dest_path: dest.clone(),
                    max_retries,
                },
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(result.resumed_from, 0);
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        assert!(!cache_dir.join(format!("{id}.vpk.part")).exists());
    }
}

#[tokio::test]
async fn downloader_retries_transient_server_errors() {
    let payload = sample_payload();
    let (base_url, attempts) = start_addon_server(payload.clone()).await;
    let dir = tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    let downloader = ResilientDownloader::new(&cache_dir, Arc::new(MirrorRouter::empty()));

    let dest = dir.path().join("loading/779.vpk");
    downloader
        .download(
            &DownloadRequest {
                workshop_id: WorkshopId::new(779).unwrap(),
                url: format!("{base_url}/flaky.vpk"),
                dest_path: dest.clone(),
                max_retries: 2,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(std::fs::read(&dest).unwrap(), payload);
}

#[tokio::test]
async fn downloader_aborts_immediately_when_cancelled() {
    let payload = sample_payload();
    let (base_url, _) = start_addon_server(payload).await;
    let dir = tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    let downloader = ResilientDownloader::new(&cache_dir, Arc::new(MirrorRouter::empty()));

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
}

#[tokio::test]
async fn downloader_recovers_from_416_on_oversized_partial() {
    let payload = sample_payload();
    let (base_url, _) = start_addon_server(payload.clone()).await;
    let dir = tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    let downloader = ResilientDownloader::new(&cache_dir, Arc::new(MirrorRouter::empty()));

    let wid = WorkshopId::new(781).unwrap();
    let url = format!("{base_url}/addon.vpk");
    seed_partial_with_etag_raw(
        &cache_dir,
        wid,
        &url,
        "781.vpk",
        &vec![9u8; 2000],
        Some(payload.len() as u64),
        Some("\"etag-v1\""),
    );
    let dest = dir.path().join("loading/781.vpk");
    let res = downloader
        .download(
            &DownloadRequest {
                workshop_id: wid,
                url,
                dest_path: dest.clone(),
                max_retries: 1,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(res.resumed_from, 0);
    assert_eq!(std::fs::read(&dest).unwrap(), payload);
}

#[tokio::test]
async fn downloader_rejects_truncated_body_before_promoting_destination() {
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

    let dir = tempdir().unwrap();
    let downloader =
        ResilientDownloader::new(dir.path().join("cache"), Arc::new(MirrorRouter::empty()));
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

#[tokio::test]
async fn downloader_preserves_partial_when_resume_returns_no_content() {
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
                        [(CONTENT_RANGE, "bytes 3-5/*"), (ETAG, "\"v1\"")],
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
    seed_partial(dir.path(), &request, b"abc", Some(6), Some("\"v1\""));
    let partial_path = dir.path().join("1.vpk.part");
    let meta_path = dir.path().join("1.part.meta.json");
    let meta = std::fs::read(&meta_path).unwrap();

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
