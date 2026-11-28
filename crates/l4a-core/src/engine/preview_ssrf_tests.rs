use super::*;
use l4a_steam::MockSteamBridge;
use l4a_workshop::{SourceRoutingPolicy, WorkshopResolver};
use std::sync::atomic::AtomicUsize;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
async fn filter_preview_image_blocks_cross_origin_ssrf_redirects_and_path_escapes() {
    // Private internal service that must NEVER be contacted via redirect.
    let private_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let private_addr = private_listener.local_addr().unwrap();
    let private_hits = Arc::new(AtomicUsize::new(0));
    let private_hits_bg = Arc::clone(&private_hits);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = private_listener.accept().await else {
                break;
            };
            private_hits_bg.fetch_add(1, Ordering::SeqCst);
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nsecret")
                .await;
        }
    });

    // Mirrored Steam CDN fixture: `/ugc/ssrf` redirects to private_addr; `/ugc/same` redirects same-origin.
    let mirror_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mirror_addr = mirror_listener.local_addr().unwrap();
    let mut jpeg_bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg_bytes)
        .encode(&[255, 0, 0], 1, 1, image::ExtendedColorType::Rgb8)
        .unwrap();
    let jpeg_bytes = Arc::new(jpeg_bytes);
    let images_served = Arc::new(AtomicUsize::new(0));
    let images_served_bg = Arc::clone(&images_served);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = mirror_listener.accept().await else {
                break;
            };
            let jpeg_bytes = Arc::clone(&jpeg_bytes);
            let images_served = Arc::clone(&images_served_bg);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let Ok(n) = socket.read(&mut buf).await else {
                    return;
                };
                let text = String::from_utf8_lossy(&buf[..n]);
                let path = text
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                if path == "/ugc/ssrf/cover.jpg" {
                    let resp = format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{private_addr}/internal.jpg\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                } else if path == "/ugc/same/cover.jpg" {
                    let resp = "HTTP/1.1 302 Found\r\nLocation: /ugc/same-final/cover.jpg\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(resp.as_bytes()).await;
                } else {
                    images_served.fetch_add(1, Ordering::SeqCst);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        jpeg_bytes.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&jpeg_bytes).await;
                }
            });
        }
    });

    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    fs::create_dir_all(&loading).unwrap();
    let mirror = Arc::new(MirrorRouter::from_rules(vec![serde_json::from_value(
        serde_json::json!({
            "rules": "mirror.cdn.test",
            "target": mirror_addr.to_string(),
        }),
    )
    .unwrap()]));
    mirror.set_no_proxy_for_testing(true);
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
        Arc::new(WorkshopResolver::new(
            Vec::new(),
            SourceRoutingPolicy::default(),
        )),
        Arc::new(NullEventSink),
    )
    .unwrap();

    // 1. Cross-origin redirect to an unmirrored private address MUST be rejected before connecting.
    let ssrf_err = crate::filter::evaluator::load_single_image(
        &engine,
        "http://mirror.cdn.test/ugc/ssrf/cover.jpg",
    )
    .await;
    assert!(
        matches!(ssrf_err, Err(CoreError::InvalidInput(_))),
        "cross-origin SSRF redirect must be rejected, got {ssrf_err:?}"
    );
    assert_eq!(
        private_hits.load(Ordering::SeqCst),
        0,
        "private service must never be contacted via filter image redirect"
    );

    // 2. Filtering verifies the same-origin redirect and caches the validated image.
    let cover_url = "http://mirror.cdn.test/ugc/same/cover.jpg";
    engine.fetch_workshop_cover(cover_url).await.unwrap();
    assert_eq!(images_served.load(Ordering::SeqCst), 1);
    for _ in 0..2 {
        let filtered = crate::filter::evaluator::load_single_image(&engine, cover_url)
            .await
            .expect("same-origin redirect on configured mirror should succeed");
        assert_eq!(filtered.mime, "image/jpeg");
        assert_eq!(images_served.load(Ordering::SeqCst), 2);
    }

    // 3. Direct Steam CDN hop escaping cloud.steampowered.com `/ugc/` path or downgrading to HTTP is rejected.
    let bad_path = url::Url::parse("https://cloud.steampowered.com/other/secret.jpg").unwrap();
    let bad_resolved = l4a_mirror::ResolvedRequest {
        url: bad_path.to_string(),
        original_host: None,
        headers: HashMap::new(),
        insecure_tls: false,
        matched_rule: false,
    };
    assert!(
        crate::filter::evaluator::validate_filter_image_redirect_hop(&l4a_mirror::RedirectHop {
            hop: 1,
            url: &bad_path,
            same_origin: true,
            matched_mirror_rule: false,
            resolved: &bad_resolved,
        })
        .is_err()
    );
}
