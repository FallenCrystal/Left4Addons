use super::support::{base_rule, start_keep_alive_server};
use crate::redirect::PAYLOAD_HEADERS;
use crate::{MirrorError, MirrorRouter, MirrorRule, RedirectRequestOptions, RedirectSendError};
use reqwest::header::LOCATION;
use reqwest::{Method, StatusCode};
use std::collections::HashMap;
use std::time::Duration;

#[tokio::test]
async fn follows_location_without_waiting_for_optional_response_body() {
    use reqwest::ResponseBuilderExt;

    let router = MirrorRouter::empty();
    let redirected = tokio::time::timeout(
        Duration::from_secs(2),
        router.send_with_redirects(
            Method::GET,
            "https://mirror.example.com/start",
            RedirectRequestOptions::default(),
            |request, _, _| request,
            |_| Ok::<(), ()>(()),
            |request, hop| async move {
                let url = request.build()?.url().clone();
                let response = http::Response::builder().url(url);
                let response = if hop == 0 {
                    let body = reqwest::Body::wrap_stream(futures_util::stream::pending::<
                        std::io::Result<&'static [u8]>,
                    >());
                    response
                        .status(StatusCode::FOUND)
                        .header(LOCATION, "/item")
                        .body(body)
                } else {
                    response.body(reqwest::Body::from("ok"))
                };
                Ok(response.unwrap().into())
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        redirected.final_resolved.url,
        "https://mirror.example.com/item"
    );
    assert_eq!(redirected.response.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn follows_utf8_location_paths() {
    let (base_url, requests) = start_keep_alive_server(|req| {
        if req.path == "/start" {
            (
                "302 Found".into(),
                "Location: /封面.jpg\r\n".into(),
                String::new(),
            )
        } else {
            ("200 OK".into(), String::new(), "cover".into())
        }
    })
    .await;
    let router = MirrorRouter::empty();
    router.set_no_proxy_for_testing(true);

    let redirected = router
        .send_with_redirects(
            Method::GET,
            &format!("{base_url}/start"),
            RedirectRequestOptions {
                timeout: Some(Duration::from_secs(5)),
                ..Default::default()
            },
            |req, _, _| req,
            |_| Ok::<(), ()>(()),
            |req, _| req.send(),
        )
        .await
        .unwrap();

    assert_eq!(redirected.response.text().await.unwrap(), "cover");
    assert_eq!(requests.lock().unwrap()[1].path, "/%E5%B0%81%E9%9D%A2.jpg");
}

#[tokio::test]
async fn redirect_strips_mirror_payload_headers_when_post_becomes_get() {
    let (base_url, requests) = start_keep_alive_server(|req| match req.path.as_str() {
        "/start" => (
            "307 Temporary Redirect".into(),
            "Location: /post\r\n".into(),
            String::new(),
        ),
        "/post" => (
            "303 See Other".into(),
            "Location: /final\r\n".into(),
            String::new(),
        ),
        _ => ("200 OK".into(), String::new(), "done".into()),
    })
    .await;
    let mut rule = base_rule(base_url.strip_prefix("http://").unwrap());
    rule.rules = Some("catalog.example.test".into());
    rule.headers = Some(HashMap::from([
        ("Content-Length".into(), "2".into()),
        ("Content-Type".into(), "application/json".into()),
        ("Content-Encoding".into(), "identity".into()),
        ("X-Mirror".into(), "present".into()),
    ]));
    let router = MirrorRouter::from_rules(vec![rule]);
    router.set_no_proxy_for_testing(true);

    let response = router
        .send_with_redirects(
            Method::POST,
            "http://catalog.example.test/start",
            RedirectRequestOptions {
                timeout: Some(Duration::from_secs(5)),
                ..Default::default()
            },
            |request, _, method| {
                if *method == Method::POST {
                    request.body("{}")
                } else {
                    request
                }
            },
            |_| Ok::<(), ()>(()),
            |request, _| request.send(),
        )
        .await
        .unwrap();
    assert_eq!(response.response.text().await.unwrap(), "done");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].headers["content-length"], "2");
    assert_eq!(requests[2].method, "GET");
    assert_eq!(requests[2].headers["x-mirror"], "present");
    for header in PAYLOAD_HEADERS {
        assert!(!requests[2].headers.contains_key(*header));
    }
}

#[tokio::test]
async fn exhausted_timeout_stops_before_sending() {
    let result = MirrorRouter::empty()
        .send_with_redirects(
            Method::GET,
            "https://mirror.example.com/item",
            RedirectRequestOptions {
                timeout: Some(Duration::ZERO),
                ..Default::default()
            },
            |request, _, _| request,
            |_| Ok::<(), ()>(()),
            |_, _| async { panic!("the deadline is exhausted") },
        )
        .await;
    assert!(matches!(result, Err(RedirectSendError::Timeout)));
}

#[tokio::test]
async fn redirects_keep_credentials_and_send_a_sanitized_referer_on_the_same_origin() {
    let (other_origin, other_requests) =
        start_keep_alive_server(|_| ("200 OK".into(), String::new(), "done".into())).await;
    let (origin, requests) = start_keep_alive_server(move |req| {
        let location = if req.path == "/start" {
            "/next".to_string()
        } else {
            format!("{other_origin}/final")
        };
        (
            "307 Temporary Redirect".into(),
            format!("Location: {location}\r\n"),
            String::new(),
        )
    })
    .await;
    let mut url = url::Url::parse(&format!("{origin}/start")).unwrap();
    url.set_username("fixture").unwrap();
    url.set_password(Some("test :@")).unwrap();
    url.set_fragment(Some("not-for-the-server"));
    let router = MirrorRouter::empty();
    router.set_no_proxy_for_testing(true);

    let result = router
        .send_with_redirects(
            Method::GET,
            url.as_str(),
            RedirectRequestOptions {
                timeout: Some(Duration::from_secs(5)),
                ..Default::default()
            },
            |req, _, _| req,
            |_| Ok::<(), ()>(()),
            |req, _| req.send(),
        )
        .await
        .unwrap();

    assert_eq!(result.response.text().await.unwrap(), "done");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let authorization = &requests[0].headers["authorization"];
    assert!(authorization.starts_with("Basic "));
    assert_eq!(requests[1].path, "/next");
    assert_eq!(
        requests[1].headers.get("authorization"),
        Some(authorization)
    );
    assert!(!requests[0].headers.contains_key("referer"));
    assert_eq!(requests[1].headers["referer"], format!("{origin}/start"));
    let other_requests = other_requests.lock().unwrap();
    assert_eq!(other_requests.len(), 1);
    assert!(!other_requests[0].headers.contains_key("authorization"));
    assert!(!other_requests[0].headers.contains_key("referer"));
}

#[tokio::test]
async fn mirror_can_return_to_source_after_switching_rules() {
    let (source_url, source_requests) =
        start_keep_alive_server(|_| ("200 OK".into(), String::new(), "from-source".into())).await;
    let source_authority = source_url.strip_prefix("http://").unwrap();
    let source_item = format!("{source_url}/item");
    let redirect_to_source = format!("Location: {source_item}\r\n");
    let (mirror_url, mirror_requests) = start_keep_alive_server(move |req| {
        let location = if req.path == "/item" {
            "Location: /mirror-step\r\n".into()
        } else {
            redirect_to_source.clone()
        };
        ("302 Found".into(), location, String::new())
    })
    .await;
    let target = mirror_url.strip_prefix("http://").unwrap();
    let entry_rule = MirrorRule {
        rules: Some("entry.example.test".into()),
        ..base_rule(target)
    };
    let source_rule = MirrorRule {
        rules: Some(source_authority.into()),
        keep_host: true,
        insecure_tls: true,
        headers: Some(HashMap::from([("X-Mirror-Route".into(), "source".into())])),
        ..base_rule(target)
    };
    let router = MirrorRouter::from_rules(vec![entry_rule, source_rule]);
    router.set_no_proxy_for_testing(true);
    let mut matched_mirror = Vec::new();

    let redirected = router
        .send_with_redirects(
            Method::GET,
            "http://entry.example.test/start",
            RedirectRequestOptions {
                timeout: Some(Duration::from_secs(5)),
                ..Default::default()
            },
            |req, _, _| req,
            |hop| {
                matched_mirror.push(hop.matched_mirror_rule);
                Ok::<(), ()>(())
            },
            |req, _| req.send(),
        )
        .await
        .unwrap();

    assert_eq!(redirected.final_resolved.url, source_item);
    assert!(redirected.final_resolved.original_host.is_none());
    assert!(!redirected.final_resolved.insecure_tls);
    assert_eq!(redirected.response.text().await.unwrap(), "from-source");
    assert_eq!(matched_mirror, [true, true, false]);

    let mirror = mirror_requests.lock().unwrap();
    let paths: Vec<_> = mirror.iter().map(|req| req.path.as_str()).collect();
    assert_eq!(paths, ["/start", "/item", "/mirror-step"]);
    assert_eq!(
        mirror[1].headers.get("x-mirror-route").map(String::as_str),
        Some("source")
    );
    let source = source_requests.lock().unwrap();
    assert_eq!(source.len(), 1);
    assert_eq!(source[0].headers.get("x-mirror-route"), None);
}

#[tokio::test]
async fn mirror_can_return_to_an_earlier_source_in_the_chain() {
    use reqwest::ResponseBuilderExt;

    let router = MirrorRouter::from_rules(vec![
        MirrorRule {
            rules: Some("first.example.test".into()),
            ..base_rule("first.mirror.test")
        },
        MirrorRule {
            rules: Some("second.example.test".into()),
            ..base_rule("second.mirror.test")
        },
    ]);
    let steps = [
        (
            "https://first.mirror.test/start",
            Some("https://second.example.test/edge"),
        ),
        (
            "https://second.mirror.test/edge",
            Some("https://first.example.test/file"),
        ),
        ("https://first.example.test/file", None),
    ];
    let redirected = router
        .send_with_redirects(
            Method::GET,
            "https://first.example.test/start",
            RedirectRequestOptions::default(),
            |req, _, _| req,
            |_| Ok::<(), ()>(()),
            |req, hop| async move {
                let url = req.build()?.url().clone();
                let (expected, location) = steps[hop];
                assert_eq!(url.as_str(), expected);
                let mut response = http::Response::builder().url(url);
                if let Some(location) = location {
                    response = response
                        .status(StatusCode::FOUND)
                        .header(LOCATION, location);
                }
                Ok(response.body(reqwest::Body::from("")).unwrap().into())
            },
        )
        .await
        .unwrap();

    assert_eq!(redirected.final_resolved.url, steps[2].0);
    assert!(!redirected.final_resolved.matched_rule);
}

#[tokio::test]
async fn direct_mirror_fallback_obeys_https_policy_and_stays_direct_on_upgrade() {
    use reqwest::ResponseBuilderExt;

    let rule = MirrorRule {
        rules: Some("source.example.test".into()),
        ..base_rule("mirror.example.test")
    };
    let router = MirrorRouter::from_rules(vec![rule]);
    let source_http = "http://source.example.test/item";
    let source_https = "https://source.example.test/item";
    let steps = [
        ("https://mirror.example.test/item", Some(source_http)),
        (source_http, Some(source_https)),
        (source_https, None),
    ];

    for https_only in [false, true] {
        let redirected = router
            .send_with_redirects(
                Method::GET,
                source_https,
                RedirectRequestOptions {
                    https_only_override: Some(https_only),
                    ..Default::default()
                },
                |req, _, _| req,
                |_| Ok::<(), ()>(()),
                |req, hop| async move {
                    let url = req.build()?.url().clone();
                    let (expected_url, location) = steps[hop];
                    assert_eq!(url.as_str(), expected_url);
                    assert!(!https_only || hop == 0);
                    let mut response = http::Response::builder().url(url);
                    if let Some(location) = location {
                        response = response
                            .status(StatusCode::FOUND)
                            .header(LOCATION, location);
                    }
                    Ok(response.body(reqwest::Body::from("")).unwrap().into())
                },
            )
            .await;

        if https_only {
            assert!(matches!(
                redirected,
                Err(RedirectSendError::Mirror(MirrorError::HttpsRequired))
            ));
        } else {
            assert_eq!(redirected.unwrap().final_resolved.url, source_https);
        }
    }
}

#[tokio::test]
async fn https_downgrade_rejects_mirror_headers_and_inline_credentials() {
    use reqwest::ResponseBuilderExt;

    let mut rule = base_rule("source.example.test");
    rule.rules = Some("source.example.test".into());
    rule.headers = Some(HashMap::from([(
        "Authorization".into(),
        "Bearer fixture-only".into(),
    )]));

    for (router, intermediate, location) in [
        (
            MirrorRouter::from_rules(vec![rule.clone()]),
            None,
            "http://source.example.test/final",
        ),
        (
            MirrorRouter::from_rules(vec![rule]),
            Some("http://fallback.example.test/next"),
            "http://source.example.test/final",
        ),
        (
            MirrorRouter::empty(),
            None,
            "http://fixture:fixture-only@source.example.test/final",
        ),
    ] {
        let result = router
            .send_with_redirects(
                Method::GET,
                "https://source.example.test/start",
                RedirectRequestOptions::default(),
                |req, _, _| req,
                |_| Ok::<(), ()>(()),
                |req, hop| async move {
                    let location = match (hop, intermediate) {
                        (0, Some(intermediate)) => intermediate,
                        (0, None) | (1, Some(_)) => location,
                        _ => panic!("the credentialed HTTP hop should not be requested"),
                    };
                    let response = http::Response::builder()
                        .url(req.build()?.url().clone())
                        .status(StatusCode::FOUND)
                        .header(LOCATION, location)
                        .body(reqwest::Body::from(""))
                        .unwrap();
                    Ok(response.into())
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(RedirectSendError::Mirror(MirrorError::InvalidRedirect(_)))
        ));
    }
}
