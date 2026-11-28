use super::support::{
    base_rule, distinct_serving_conns, start_keep_alive_server, start_self_signed_tls_server,
    RecordedRequest,
};
use crate::{MirrorRouter, RedirectRequestOptions, RedirectSendError};
use reqwest::{Method, StatusCode};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct TlsIsolationFixture {
    router: MirrorRouter,
    server_a_log: Arc<Mutex<Vec<RecordedRequest>>>,
    server_b_log: Arc<Mutex<Vec<RecordedRequest>>>,
}

async fn setup_tls_isolation_fixture() -> TlsIsolationFixture {
    // Server B: untrusted self-signed TLS server NOT covered by any mirror rule.
    let (server_b_url, server_b_log) =
        start_self_signed_tls_server(|_| ("200 OK".into(), String::new(), "secret-b".into())).await;

    // Server A: self-signed TLS mirror server. `/ok` returns 200, `/cross` redirects to Server B.
    let server_b_redirect = format!("Location: {server_b_url}/secret\r\n");
    let (server_a_url, server_a_log) = start_self_signed_tls_server(move |req| {
        if req.path == "/cross" {
            ("302 Found".into(), server_b_redirect.clone(), String::new())
        } else {
            ("200 OK".into(), String::new(), "ok-a".into())
        }
    })
    .await;
    let target_a = server_a_url.strip_prefix("https://").unwrap().to_string();

    let mut insecure_rule = base_rule(&target_a);
    insecure_rule.rules = Some("mirror.insecure.test".to_string());
    insecure_rule.insecure_tls = true;
    insecure_rule.keep_host = true;

    let mut strict_rule = base_rule(&target_a);
    strict_rule.rules = Some("mirror.strict.test".to_string());
    strict_rule.insecure_tls = false;

    let router = MirrorRouter::from_rules(vec![insecure_rule, strict_rule]);
    router.set_no_proxy_for_testing(true);

    TlsIsolationFixture {
        router,
        server_a_log,
        server_b_log,
    }
}

#[tokio::test]
async fn real_tls_isolates_verified_and_insecure_pools() {
    let fixture = setup_tls_isolation_fixture().await;

    // 1. Strict route to the same self-signed authority MUST fail TLS verification.
    let strict_err = fixture
        .router
        .send_with_redirects(
            Method::GET,
            "https://mirror.strict.test/ok",
            RedirectRequestOptions {
                timeout: Some(Duration::from_secs(5)),
                ..Default::default()
            },
            |r, _, _| r,
            |_| Ok::<(), ()>(()),
            |r, _| r.send(),
        )
        .await;
    assert!(
        matches!(strict_err, Err(RedirectSendError::Request(_))),
        "strict route must reject self-signed TLS cert, got {strict_err:?}"
    );

    // 2. Insecure mirror route succeeds when https_only == false and reuses the TLS connection.
    for path in ["/ok", "/ok2"] {
        let res = fixture
            .router
            .send_with_redirects(
                Method::GET,
                &format!("https://mirror.insecure.test{path}"),
                RedirectRequestOptions {
                    timeout: Some(Duration::from_secs(5)),
                    ..Default::default()
                },
                |r, _, _| r,
                |_| Ok::<(), ()>(()),
                |r, _| r.send(),
            )
            .await
            .expect("insecure mirror route should succeed when https_only=false");
        assert_eq!(res.response.text().await.unwrap(), "ok-a");
        tokio::task::yield_now().await;
    }
    assert_eq!(distinct_serving_conns(&fixture.server_a_log), 1);

    // 3. Enabling https_only on the SAME router immediately enforces certificate verification.
    fixture.router.set_https_only(true);
    let upgraded_err = fixture
        .router
        .send_with_redirects(
            Method::GET,
            "https://mirror.insecure.test/ok",
            RedirectRequestOptions {
                timeout: Some(Duration::from_secs(5)),
                ..Default::default()
            },
            |r, _, _| r,
            |_| Ok::<(), ()>(()),
            |r, _| r.send(),
        )
        .await;
    assert!(
        matches!(upgraded_err, Err(RedirectSendError::Request(_))),
        "enabling https_only must disable insecure-tls and fail self-signed TLS, got {upgraded_err:?}"
    );
}

#[tokio::test]
async fn real_tls_blocks_cross_origin_insecure_leak() {
    let fixture = setup_tls_isolation_fixture().await;

    // 1. Cross-origin redirect from insecure mirror to untrusted Server B MUST fail TLS verification.
    let cross_err = fixture
        .router
        .send_with_redirects(
            Method::GET,
            "https://mirror.insecure.test/cross",
            RedirectRequestOptions {
                timeout: Some(Duration::from_secs(5)),
                ..Default::default()
            },
            |r, _, _| r,
            |_| Ok::<(), ()>(()),
            |r, _| r.send(),
        )
        .await;
    assert!(
        matches!(cross_err, Err(RedirectSendError::Request(_))),
        "cross-origin redirect from insecure mirror must use verified client and fail self-signed TLS, got {cross_err:?}"
    );
    assert_eq!(
        fixture.server_b_log.lock().unwrap().len(),
        0,
        "Server B must not receive an HTTP request after TLS rejection"
    );

    // 2. Direct raw `.get().send()` on insecure mirror stops on cross-origin redirect instead of leaking TLS bypass.
    let (raw_req, _) = fixture
        .router
        .get(
            "https://mirror.insecure.test/cross",
            Some(Duration::from_secs(5)),
        )
        .unwrap();
    let raw_resp = raw_req.send().await.unwrap();
    assert_eq!(raw_resp.status(), StatusCode::FOUND);
    assert_eq!(fixture.server_b_log.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn redirects_preserve_mirror_headers_same_origin_and_strip_cross_origin_without_pool_leak() {
    let (origin_b_url, origin_b_log) =
        start_keep_alive_server(|_| ("200 OK".into(), String::new(), "from-b".into())).await;
    let redirect_to_b = format!("Location: {origin_b_url}/final\r\n");

    let (origin_a_url, origin_a_log) =
        start_keep_alive_server(move |req| match req.path.as_str() {
            "/same-hop" => (
                "302 Found".into(),
                "Location: /same-final\r\n".into(),
                String::new(),
            ),
            "/cross-hop" => ("302 Found".into(), redirect_to_b.clone(), String::new()),
            _ => ("200 OK".into(), String::new(), "from-a".into()),
        })
        .await;
    let target_a = origin_a_url.strip_prefix("http://").unwrap().to_string();

    let mut rule = base_rule(&target_a);
    rule.rules = Some("mirrored.example.test".to_string());
    rule.keep_host = true;
    rule.headers = Some(HashMap::from([(
        "X-Mirror-Secret".to_string(),
        "top-secret".to_string(),
    )]));

    let router = MirrorRouter::from_rules(vec![rule]);
    router.set_no_proxy_for_testing(true);

    // 1. Same-origin redirect keeps Host and mirror headers.
    let same = router
        .send_with_redirects(
            Method::GET,
            "http://mirrored.example.test/same-hop",
            RedirectRequestOptions::default(),
            |r, _, _| r,
            |_| Ok::<(), ()>(()),
            |r, _| r.send(),
        )
        .await
        .unwrap();
    assert_eq!(same.response.text().await.unwrap(), "from-a");
    tokio::task::yield_now().await;

    // 2. Cross-origin redirect drops Host and mirror headers before contacting Origin B.
    let cross = router
        .send_with_redirects(
            Method::GET,
            "http://mirrored.example.test/cross-hop",
            RedirectRequestOptions::default(),
            |r, _, _| r,
            |_| Ok::<(), ()>(()),
            |r, _| r.send(),
        )
        .await
        .unwrap();
    assert_eq!(cross.response.text().await.unwrap(), "from-b");
    tokio::task::yield_now().await;

    // 3. Direct request to Origin A (without mirror rule) on the SAME pooled client sends no mirror headers.
    let direct = router
        .send_with_redirects(
            Method::GET,
            &format!("{origin_a_url}/plain"),
            RedirectRequestOptions::default(),
            |r, _, _| r,
            |_| Ok::<(), ()>(()),
            |r, _| r.send(),
        )
        .await
        .unwrap();
    assert_eq!(direct.response.text().await.unwrap(), "from-a");

    let a_entries = origin_a_log.lock().unwrap();
    assert_eq!(a_entries.len(), 4);
    assert_eq!(
        a_entries[1].headers.get("host").map(String::as_str),
        Some("mirrored.example.test")
    );
    assert_eq!(
        a_entries[1]
            .headers
            .get("x-mirror-secret")
            .map(String::as_str),
        Some("top-secret")
    );
    assert_eq!(a_entries[3].path, "/plain");
    assert_eq!(a_entries[3].headers.get("x-mirror-secret"), None);
    assert_ne!(
        a_entries[3].headers.get("host").map(String::as_str),
        Some("mirrored.example.test")
    );
    drop(a_entries);
    assert_eq!(distinct_serving_conns(&origin_a_log), 1);

    let b_entries = origin_b_log.lock().unwrap();
    assert_eq!(b_entries.len(), 1);
    assert_eq!(b_entries[0].headers.get("x-mirror-secret"), None);
    assert_ne!(
        b_entries[0].headers.get("host").map(String::as_str),
        Some("mirrored.example.test")
    );
}
