use super::*;
use l4a_logging::http::AuditedSend;
use l4a_steam::MockSteamBridge;
use l4a_workshop::{SourceRoutingPolicy, WorkshopResolver};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn start_routed_keep_alive_server() -> (SocketAddr, Arc<Mutex<Vec<(String, usize)>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let next_conn_id = Arc::new(AtomicUsize::new(1));
    let served_conns = Arc::new(Mutex::new(Vec::<(String, usize)>::new()));
    let served_conns_bg = Arc::clone(&served_conns);

    tokio::spawn(async move {
        // Minimal valid JPEG header bytes (`image::guess_format` checks magic bytes).
        let jpeg_bytes: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F'];
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let conn_id = next_conn_id.fetch_add(1, Ordering::SeqCst);
            let served_conn = Arc::clone(&served_conns_bg);
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
                    let path = header_text
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    let content_length = header_text
                        .lines()
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
                    let is_expected_post =
                        header_text.starts_with("POST ") && &pending[..content_length] == b"{}";
                    pending.drain(..content_length);

                    served_conn.lock().unwrap().push((path.clone(), conn_id));
                    let redirect = match path.as_str() {
                        "/redirect/get" => Some("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/blocked\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n"),
                        "/redirect/post" => Some("HTTP/1.1 307 Temporary Redirect\r\nLocation: /routed-final/post\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n"),
                        _ => None,
                    };
                    if let Some(head) = redirect {
                        if socket.write_all(head.as_bytes()).await.is_err() {
                            break;
                        }
                    } else {
                        if path == "/routed-final/post" {
                            assert!(is_expected_post, "307 should preserve the method and body");
                        }
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                            jpeg_bytes.len()
                        );
                        if socket.write_all(head.as_bytes()).await.is_err()
                            || socket.write_all(jpeg_bytes).await.is_err()
                        {
                            break;
                        }
                    }
                }
            });
        }
    });

    (addr, served_conns)
}

#[tokio::test]
async fn covers_and_routed_requests_reuse_tcp_keep_alive_connections() {
    let (addr, served_conns) = start_routed_keep_alive_server().await;

    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    fs::create_dir_all(&loading).unwrap();
    let mirror = Arc::new(MirrorRouter::from_rules(vec![
        serde_json::from_value(serde_json::json!({
            "rules": "images.steamusercontent.com",
            "target": addr.to_string(),
        }))
        .unwrap(),
        serde_json::from_value(serde_json::json!({
            "rules": "filter.insecure.test",
            "target": addr.to_string(),
            "insecure-tls": true,
        }))
        .unwrap(),
    ]));
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

    // 1. Multiple cover fetches share a single keep-alive TCP connection.
    for idx in 1..=3 {
        let (mime, bytes) = engine
            .fetch_workshop_cover(&format!(
                "http://images.steamusercontent.com/ugc/{idx}/cover.jpg"
            ))
            .await
            .unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(!bytes.is_empty());
        tokio::task::yield_now().await;
    }
    {
        let recorded = served_conns.lock().unwrap();
        assert_eq!(recorded.len(), 3);
        let cover_conns: HashSet<usize> = recorded.iter().map(|(_, id)| *id).collect();
        assert_eq!(
            cover_conns.len(),
            1,
            "fetch_workshop_cover should reuse one serving TCP connection across distinct cover URLs"
        );
    }

    // 2. The insecure mirror follows same-origin redirects on its pooled connection.
    let resp_get = engine
        .prepare_routed_get(
            "http://filter.insecure.test/redirect/get",
            Duration::from_secs(5),
        )
        .unwrap()
        .send_audited_quiet("test.routed.cross_origin.get")
        .await
        .unwrap();
    assert_eq!(resp_get.status(), reqwest::StatusCode::FOUND);
    let _ = resp_get.bytes().await.unwrap();
    tokio::task::yield_now().await;

    let resp_post = engine
        .prepare_routed_post(
            "http://filter.insecure.test/redirect/post",
            Duration::from_secs(5),
        )
        .unwrap()
        .body("{}")
        .send_audited_quiet("test.routed.same_origin.post")
        .await
        .unwrap();
    assert_eq!(resp_post.status(), reqwest::StatusCode::OK);
    let _ = resp_post.bytes().await.unwrap();

    let recorded = served_conns.lock().unwrap();
    assert_eq!(recorded.len(), 6);
    assert_eq!(recorded[5].0, "/routed-final/post");
    let routed_conns: HashSet<usize> = recorded[3..].iter().map(|(_, id)| *id).collect();
    assert_eq!(
        routed_conns.len(),
        1,
        "routed GET, POST and its same-origin redirect should reuse a single connection"
    );
    assert_ne!(
        recorded[0].1, recorded[3].1,
        "verified cover pool and insecure-tls routed pool must remain separate connections"
    );
}

#[tokio::test]
async fn shared_mirror_router_does_not_override_engine_https_only_policy() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    fs::create_dir_all(&loading).unwrap();

    let shared_mirror = Arc::new(MirrorRouter::empty());
    let strict_engine = L4aEngine::with_dependencies(
        AppPaths::from_runtime_dir(dir.path().join("runtime-strict")),
        Arc::new(LocalDb::open_in_memory().unwrap()),
        AppConfig {
            loading_dir: loading.to_string_lossy().into_owned(),
            https_only: true,
            ..Default::default()
        },
        Arc::clone(&shared_mirror),
        Arc::new(MockSteamBridge::new(false)),
        Arc::new(WorkshopResolver::new(
            Vec::new(),
            SourceRoutingPolicy::default(),
        )),
        Arc::new(NullEventSink),
    )
    .unwrap();

    // Second engine sharing the same MirrorRouter flips shared_mirror to https_only = false.
    let permissive_engine = L4aEngine::with_dependencies(
        AppPaths::from_runtime_dir(dir.path().join("runtime-permissive")),
        Arc::new(LocalDb::open_in_memory().unwrap()),
        AppConfig {
            loading_dir: loading.to_string_lossy().into_owned(),
            https_only: false,
            ..Default::default()
        },
        Arc::clone(&shared_mirror),
        Arc::new(MockSteamBridge::new(false)),
        Arc::new(WorkshopResolver::new(
            Vec::new(),
            SourceRoutingPolicy::default(),
        )),
        Arc::new(NullEventSink),
    )
    .unwrap();

    // Strict engine MUST still reject plain HTTP even though shared_mirror was mutated.
    assert!(strict_engine
        .prepare_routed_get("http://filter.example.com/info", Duration::from_secs(5))
        .is_err());
    assert!(strict_engine
        .prepare_routed_post("http://filter.example.com/predict", Duration::from_secs(5))
        .is_err());
    assert!(strict_engine
        .fetch_workshop_cover("http://other.example.com/cover.jpg")
        .await
        .is_err());

    // Permissive engine continues to accept HTTP according to its own config.
    assert!(permissive_engine
        .prepare_routed_get("http://filter.example.com/info", Duration::from_secs(5))
        .is_ok());
}
