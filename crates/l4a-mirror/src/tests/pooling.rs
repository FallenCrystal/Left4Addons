use super::support::{
    base_rule, distinct_serving_conns, serve_http_stream, start_keep_alive_server,
    start_self_signed_tls_server, test_tls_config, RecordedRequest,
};
use crate::pool::MAX_CUSTOM_TIMEOUT_CLIENTS;
use crate::MirrorRouter;
use reqwest::StatusCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[tokio::test]
async fn reuses_http2_connection_and_preserves_host_overrides_over_http1() {
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = listener.local_addr().unwrap().to_string();
    let mut tls_config = test_tls_config();
    tls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(tls_config));
    let h2_connections = Arc::new(Mutex::new(Vec::new()));
    let h1_requests = Arc::new(Mutex::new(Vec::new()));
    let h2_connections_bg = Arc::clone(&h2_connections);
    let h1_requests_bg = Arc::clone(&h1_requests);
    tokio::spawn(async move {
        let mut conn_id = 0;
        while let Ok((socket, _)) = listener.accept().await {
            conn_id += 1;
            let acceptor = acceptor.clone();
            let h2_connections = Arc::clone(&h2_connections_bg);
            let h1_requests = Arc::clone(&h1_requests_bg);
            tokio::spawn(async move {
                let stream = acceptor.accept(socket).await.unwrap();
                if stream.get_ref().1.alpn_protocol() == Some(b"h2") {
                    let mut connection = h2::server::handshake(stream).await.unwrap();
                    while let Some(Ok((_, mut respond))) = connection.accept().await {
                        h2_connections.lock().unwrap().push(conn_id);
                        respond
                            .send_response(http::Response::new(()), true)
                            .unwrap();
                    }
                } else {
                    serve_http_stream(
                        stream,
                        conn_id,
                        h1_requests,
                        Arc::new(|_: &RecordedRequest| {
                            ("200 OK".into(), String::new(), String::new())
                        }),
                    )
                    .await;
                }
            });
        }
    });

    let mut h2_rule = base_rule(&target);
    h2_rule.rules = Some("h2.mirror.test".into());
    h2_rule.insecure_tls = true;
    let mut keep_host_rule = h2_rule.clone();
    keep_host_rule.rules = Some("keep-host.mirror.test".into());
    keep_host_rule.keep_host = true;
    let router = MirrorRouter::from_rules(vec![h2_rule, keep_host_rule]);
    router.set_no_proxy_for_testing(true);

    for timeout in [Duration::from_secs(4), Duration::from_secs(5)] {
        let (request, _) = router
            .get("https://h2.mirror.test/item", Some(timeout))
            .unwrap();
        let response = request.send().await.unwrap();
        assert_eq!(response.version(), reqwest::Version::HTTP_2);
        assert_eq!(response.status(), StatusCode::OK);
        response.bytes().await.unwrap();
    }
    assert_eq!(*h2_connections.lock().unwrap(), [1, 1]);

    let (request, _) = router
        .get(
            "https://keep-host.mirror.test/item",
            Some(Duration::from_secs(5)),
        )
        .unwrap();
    let response = request.send().await.unwrap();
    assert_eq!(response.version(), reqwest::Version::HTTP_11);
    let requests = h1_requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].headers["host"], "keep-host.mirror.test");
}

#[tokio::test]
async fn reuses_client_and_tcp_connection_across_different_request_timeouts() {
    let (base_url, log) =
        start_keep_alive_server(|_| ("200 OK".into(), String::new(), "ok".into())).await;
    let router = MirrorRouter::empty();
    router.set_no_proxy_for_testing(true);

    for (method_is_post, timeout) in [
        (false, Some(Duration::from_secs(5))),
        (true, Some(Duration::from_secs(15))),
        (false, Some(Duration::from_secs(30))),
        (true, None),
    ] {
        let (req, _) = if method_is_post {
            router.post(&format!("{base_url}/post"), timeout).unwrap()
        } else {
            router.get(&format!("{base_url}/get"), timeout).unwrap()
        };
        let resp = req.body("payload").send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await.unwrap(), "ok");
        tokio::task::yield_now().await;
    }

    assert_eq!(log.lock().unwrap().len(), 4);
    assert_eq!(router.cached_client_count(), 1);
    assert_eq!(
        distinct_serving_conns(&log),
        1,
        "sequential requests with different per-request timeouts should reuse one serving TCP connection"
    );
}

#[tokio::test]
async fn cloned_router_shares_cached_client_pool_and_tcp_connection() {
    let (base_url, log) =
        start_keep_alive_server(|_| ("200 OK".into(), String::new(), "shared".into())).await;
    let router = MirrorRouter::empty();
    router.set_no_proxy_for_testing(true);
    let cloned = router.clone();

    let (req1, _) = router
        .get(&format!("{base_url}/a"), Some(Duration::from_secs(10)))
        .unwrap();
    assert_eq!(req1.send().await.unwrap().text().await.unwrap(), "shared");
    tokio::task::yield_now().await;

    let (req2, _) = cloned
        .get(&format!("{base_url}/b"), Some(Duration::from_secs(20)))
        .unwrap();
    assert_eq!(req2.send().await.unwrap().text().await.unwrap(), "shared");

    assert_eq!(log.lock().unwrap().len(), 2);
    assert_eq!(router.cached_client_count(), 1);
    assert_eq!(cloned.cached_client_count(), 1);
    assert_eq!(distinct_serving_conns(&log), 1);
}

#[tokio::test]
async fn streaming_client_reuses_client_and_tcp_connection() {
    let (base_url, log) =
        start_keep_alive_server(|_| ("200 OK".into(), String::new(), "chunk-data".into())).await;
    let router = MirrorRouter::empty();
    router.set_no_proxy_for_testing(true);

    for path in ["/file1.vpk", "/file2.vpk"] {
        let (client, resolved) = router
            .streaming_client_for(
                &format!("{base_url}{path}"),
                Duration::from_secs(15),
                Duration::from_secs(60),
            )
            .unwrap();
        let resp = router
            .prepare_resolved_get(&client, &resolved)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.text().await.unwrap(), "chunk-data");
        tokio::task::yield_now().await;
    }

    assert_eq!(log.lock().unwrap().len(), 2);
    assert_eq!(router.cached_client_count(), 1);
    assert_eq!(distinct_serving_conns(&log), 1);
}

#[tokio::test]
async fn no_redirect_get_and_post_preserve_policy_and_reuse_connection_for_insecure_rules() {
    let (base_url, log) = start_self_signed_tls_server(|_| {
        (
            "302 Found".into(),
            "Location: https://127.0.0.1:1/never-follow\r\n".into(),
            String::new(),
        )
    })
    .await;
    let target = base_url.strip_prefix("https://").unwrap().to_string();
    let mut rule = base_rule(&target);
    rule.rules = Some("filter.mirror.test".to_string());
    rule.insecure_tls = true;
    rule.keep_host = true;

    let router = MirrorRouter::from_rules(vec![rule]);
    router.set_no_proxy_for_testing(true);

    let (get_req, resolved) = router
        .get_no_redirect(
            "https://filter.mirror.test/info",
            Some(Duration::from_secs(10)),
        )
        .unwrap();
    assert_eq!(
        resolved.original_host.as_deref(),
        Some("filter.mirror.test")
    );
    assert!(resolved.insecure_tls);
    let resp = get_req.send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    let _ = resp.bytes().await.unwrap();
    tokio::task::yield_now().await;

    let (post_req, _) = router
        .post_no_redirect(
            "https://filter.mirror.test/predict",
            Some(Duration::from_secs(10)),
        )
        .unwrap();
    let resp = post_req.body("{}").send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    let _ = resp.bytes().await.unwrap();

    let entries = log.lock().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].method, "GET");
    assert_eq!(entries[1].method, "POST");
    assert_eq!(
        entries[0].headers.get("host").map(String::as_str),
        Some("filter.mirror.test")
    );
    assert_eq!(
        entries[1].headers.get("host").map(String::as_str),
        Some("filter.mirror.test")
    );
    drop(entries);
    assert_eq!(router.cached_client_count(), 1);
    assert_eq!(distinct_serving_conns(&log), 1);
}

#[test]
fn custom_timeout_cache_is_bounded_and_cannot_crowd_out_streaming_or_core_pools() {
    let router = MirrorRouter::empty();
    assert_eq!(router.cached_client_count(), 0);

    // Fill legacy custom-timeout bucket past its limit.
    for secs in 1..=(MAX_CUSTOM_TIMEOUT_CLIENTS as u64 + 10) {
        let _ = router
            .build_client_for("https://example.com/d", Some(Duration::from_secs(secs)))
            .unwrap();
    }
    assert_eq!(router.cached_client_count(), MAX_CUSTOM_TIMEOUT_CLIENTS);

    // Streaming clients live in their own bucket and are never crowded out.
    let _ = router
        .streaming_client_for(
            "https://example.com/file.vpk",
            Duration::from_secs(15),
            Duration::from_secs(60),
        )
        .unwrap();
    assert_eq!(router.cached_streaming_client_count(), 1);
    let _ = router
        .streaming_client_for(
            "https://example.com/file2.vpk",
            Duration::from_secs(15),
            Duration::from_secs(60),
        )
        .unwrap();
    assert_eq!(router.cached_streaming_client_count(), 1);

    // Core clients live in their own bucket and remain cached.
    let _ = router
        .get("https://example.com/core", Some(Duration::from_secs(5)))
        .unwrap();
    let count_after_core = router.cached_client_count();
    let _ = router
        .post("https://example.com/core2", Some(Duration::from_secs(30)))
        .unwrap();
    assert_eq!(router.cached_client_count(), count_after_core);
}
