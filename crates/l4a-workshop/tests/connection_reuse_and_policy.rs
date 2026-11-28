mod support;

use l4a_mirror::MirrorRouter;
use l4a_protocol::{SearchQuery, WorkshopId};
use l4a_workshop::{
    CatalogBackendProvider, DownloadRequest, ResilientDownloader, SteamWebApiProvider,
    WorkshopError, WorkshopProvider,
};
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use support::{seed_partial_with_etag_raw, start_counting_keep_alive_server};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn catalog_backend_reuses_tcp_keep_alive_connections() {
    let mirror = Arc::new(MirrorRouter::empty());
    mirror.set_no_proxy_for_testing(true);

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

    let catalog = CatalogBackendProvider::new(catalog_url, mirror);
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

    let conns = catalog_served.lock().unwrap();
    assert_eq!(conns.len(), 3);
    let unique: HashSet<usize> = conns.iter().copied().collect();
    assert_eq!(
        unique.len(),
        1,
        "CatalogBackendProvider should reuse one serving TCP connection across batch, collection, and search"
    );
}

#[tokio::test]
async fn steam_web_api_reuses_tcp_keep_alive_connections() {
    let mirror = Arc::new(MirrorRouter::empty());
    mirror.set_no_proxy_for_testing(true);

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

    let steam = SteamWebApiProvider::with_base_url(steam_url, mirror);
    let col = steam.fetch_collection(WorkshopId(300)).await.unwrap();
    assert_eq!(col.items.len(), 1);
    tokio::task::yield_now().await;
    assert_eq!(
        steam.fetch_items(&[WorkshopId(301)]).await.unwrap().len(),
        2
    );

    let conns = steam_served.lock().unwrap();
    assert_eq!(conns.len(), 3);
    let unique: HashSet<usize> = conns.iter().copied().collect();
    assert_eq!(
        unique.len(),
        1,
        "SteamWebApiProvider should reuse one serving TCP connection across collection and item calls"
    );
}

#[tokio::test]
async fn downloader_reuses_tcp_keep_alive_connections() {
    let mirror = Arc::new(MirrorRouter::empty());
    mirror.set_no_proxy_for_testing(true);

    let (cdn_url, cdn_served) = start_counting_keep_alive_server(|_, _| {
        (200, "application/octet-stream", b"vpk-bytes".to_vec())
    })
    .await;
    let dir = tempdir().unwrap();
    let downloader = ResilientDownloader::new(dir.path().join("cache"), mirror);
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

    let conns = cdn_served.lock().unwrap();
    assert_eq!(conns.len(), 2);
    let unique: HashSet<usize> = conns.iter().copied().collect();
    assert_eq!(
        unique.len(),
        1,
        "ResilientDownloader should reuse one serving TCP connection across sequential downloads"
    );
}

#[tokio::test]
async fn downloader_416_restart_respects_live_https_only_toggle() {
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
    let wid = WorkshopId(555);
    let req_url = format!("{dl_url}/ugc/555.vpk");
    seed_partial_with_etag_raw(
        &cache_dir,
        wid,
        &req_url,
        "555.vpk",
        b"stale-partial",
        Some(100),
        Some("\"v1\""),
    );

    let downloader = ResilientDownloader::new(&cache_dir, dl_mirror);
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
}

#[tokio::test]
async fn steam_web_api_batches_respect_live_https_only_toggle() {
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

    let provider = SteamWebApiProvider::with_base_url(api_url, api_mirror);
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
