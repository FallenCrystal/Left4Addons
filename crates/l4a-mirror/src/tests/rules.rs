use super::support::base_rule;
use crate::{MirrorError, MirrorRouter};
use std::collections::HashMap;
use tempfile::tempdir;

#[test]
fn object_rule_rewrites_subdomain_host_and_headers() {
    let mut r = base_rule("$1.mirror.example.com");
    r.domain = Some("steamstatic.com".to_string());
    r.subdomains = Some(vec!["store".to_string(), "media".to_string()]);
    r.allow_www = true;
    r.keep_host = true;
    r.insecure_tls = true;
    r.headers = Some(HashMap::from([("X-Mirror".into(), "on".into())]));

    let router = MirrorRouter::from_rules(vec![r]);
    let res = router.resolve("https://www.media.steamstatic.com/img.jpg?v=1");
    assert_eq!(res.url, "https://media.mirror.example.com/img.jpg?v=1");
    assert_eq!(
        res.original_host.as_deref(),
        Some("www.media.steamstatic.com")
    );
    assert_eq!(res.headers.get("X-Mirror").map(String::as_str), Some("on"));
    assert!(res.insecure_tls);
    assert!(res.matched_rule);
}

#[test]
fn string_rule_handles_optional_group_wildcard_and_port() {
    let mut r = base_rule("$2.proxy.local:8443");
    r.rules = Some("[www.]?*.steamcommunity.com".to_string());

    let router = MirrorRouter::from_rules(vec![r]);
    let res = router.resolve("https://www.workshop.steamcommunity.com/sharedfiles?id=1");
    assert_eq!(
        res.url,
        "https://workshop.proxy.local:8443/sharedfiles?id=1"
    );
    assert_eq!(res.original_host, None);
}

#[test]
fn semicolon_rules_and_invalid_regex_are_handled_gracefully() {
    let mut invalid = base_rule("broken.local");
    invalid.regex = Some("[unclosed".to_string());

    let mut multi = base_rule("cdn.mirror.local");
    multi.rules = Some("cdn.steamstatic.com; community.steamstatic.com".to_string());

    let router = MirrorRouter::from_rules(vec![invalid, multi]);
    assert_eq!(router.rule_count(), 1);
    assert_eq!(
        router.resolve("https://community.steamstatic.com/a").url,
        "https://cdn.mirror.local/a"
    );
    assert_eq!(
        router.resolve("https://untouched.example.com/b").url,
        "https://untouched.example.com/b"
    );
}

#[test]
fn loads_rules_from_json_file_and_handles_missing_file() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("missing.json");
    assert_eq!(MirrorRouter::from_file(&missing).unwrap().rule_count(), 0);

    let file = dir.path().join("mirrors.json");
    std::fs::write(
        &file,
        r#"[{"domain":"steampowered.com","subdomains":["api"],"target":"127.0.0.1:9000","keep-host":true}]"#,
    )
    .unwrap();

    let router = MirrorRouter::from_file(&file).unwrap();
    assert_eq!(router.rule_count(), 1);
    let res = router.resolve("https://api.steampowered.com/ISteamRemoteStorage/");
    assert_eq!(res.url, "https://127.0.0.1:9000/ISteamRemoteStorage/");
    assert_eq!(res.original_host.as_deref(), Some("api.steampowered.com"));
}

#[test]
fn rewrites_ports_cleanly_including_clearing_and_port_80() {
    let mut clear_port = base_rule("cdn.mirror.local");
    clear_port.rules = Some("api.example.com:9000".to_string());

    let mut keep_80 = base_rule("proxy.local:80");
    keep_80.rules = Some("cdn.example.com".to_string());

    let router = MirrorRouter::from_rules(vec![clear_port, keep_80]);
    assert_eq!(
        router.resolve("https://api.example.com:9000/a.vpk").url,
        "https://cdn.mirror.local/a.vpk"
    );
    assert_eq!(
        router.resolve("https://cdn.example.com/b.vpk").url,
        "https://proxy.local:80/b.vpk"
    );
}

#[test]
fn routes_legacy_steam_cdn_links_according_to_mirror_and_https_policy() {
    let mut rule = base_rule("mirror.local:8080");
    rule.rules = Some("cloud.steampowered.com".to_string());
    rule.keep_host = true;
    let router = MirrorRouter::from_rules(vec![rule]);
    let url = "http://cloud.steampowered.com/ugc/123/cover.jpg";

    let direct = MirrorRouter::empty();
    direct.set_https_only(true);
    assert_eq!(
        direct.validate_url(url).unwrap().url,
        "https://cloud.steampowered.com/ugc/123/cover.jpg"
    );
    assert_eq!(
        direct
            .validate_url("http://cloud4.steampowered.com/ugc/123/cover.jpg")
            .unwrap()
            .url,
        "https://cloud4.steampowered.com/ugc/123/cover.jpg"
    );
    let mirrored = router.validate_url(url).unwrap();
    assert_eq!(mirrored.url, "http://mirror.local:8080/ugc/123/cover.jpg");
    assert_eq!(
        mirrored.original_host.as_deref(),
        Some("cloud.steampowered.com")
    );

    router.set_https_only(true);
    assert_eq!(
        router.validate_url(url).unwrap().url,
        "https://mirror.local:8080/ugc/123/cover.jpg"
    );
    for other in [
        "http://evil-steamuserimages.akamaihd.net/cover.jpg",
        "http://cloud.steampowered.com/other/cover.jpg",
    ] {
        assert!(matches!(
            router.validate_url(other),
            Err(MirrorError::HttpsRequired)
        ));
    }
}
