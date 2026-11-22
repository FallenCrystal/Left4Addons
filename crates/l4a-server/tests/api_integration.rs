use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use chrono::Utc;
use http_body_util::BodyExt;
use l4a_mirror::MirrorRouter;
use l4a_protocol::{
    AdminItemDetailResponse, AdminLoginResponse, AdminSessionInfo, BatchItemsResponse,
    CatalogOverviewResponse, CatalogSyncConfigDto, CatalogSyncStatus, MetadataSource,
    SearchResponse, SteamAuthFlowState, SteamAuthFlowStatus, SteamAuthMode, WorkshopDependency,
    WorkshopFetchOutcome, WorkshopFileType, WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
    WorkshopPreview,
};
use l4a_server::{
    build_router,
    config::{CatalogConfig, CatalogConfigStore},
    db::ServerRepo,
    upstream::{
        MockWorkshopUpstream, WorkshopObservation, WorkshopQueryPage, MATCHING_FILE_TYPE_ITEMS,
        QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE, QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
    },
    ServerState,
};
use l4a_workshop::{CatalogBackendProvider, WorkshopProvider};
use serde_json::json;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower::ServiceExt;

async fn raw_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    headers: Vec<(&str, &str)>,
    body: Option<serde_json::Value>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        builder = builder.header(k, v);
    }
    let req = if let Some(val) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&val).unwrap()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };

    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let resp_headers = resp.headers().clone();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, resp_headers, bytes)
}

async fn json_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    headers: Vec<(&str, &str)>,
    body: Option<serde_json::Value>,
) -> (StatusCode, Vec<u8>) {
    let (status, _, bytes) = raw_request(app, method, uri, headers, body).await;
    (status, bytes)
}

fn sample_item(id: u64, title: &str, updated: i64, file_url: Option<&str>) -> WorkshopItemRecord {
    WorkshopItemRecord {
        workshop_id: WorkshopId(id),
        file_type: WorkshopFileType::Item,
        status: WorkshopItemStatus::Active,
        title: title.to_string(),
        description: Some(format!("Description for {title} 100%")),
        creator_steam_id: Some("76561198000000123".to_string()),
        creator_name: Some("Mapper".to_string()),
        preview_url: Some(format!(
            "https://cdn.steamusercontent.com/ugc/{id}_cover.jpg"
        )),
        previews: vec![WorkshopPreview {
            preview_id: Some(format!("p_{id}")),
            url: Some(format!(
                "https://cdn.steamusercontent.com/ugc/{id}_shot1.jpg"
            )),
            youtube_video_id: None,
            preview_type: 0,
        }],
        file_url: file_url.map(str::to_owned),
        file_size: Some(1024 * 1024),
        time_created: Some(1_700_000_000),
        time_updated: Some(updated),
        tags: vec!["Campaigns".to_string(), "Maps".to_string()],
        dependencies: Vec::new(),
        collection_children: Vec::new(),
        subscriptions: Some(500),
        favorites: Some(50),
        source: MetadataSource::SteamKit,
        fetched_at: Utc::now(),
    }
}

#[tokio::test]
async fn search_items_supports_catalog_sorts_and_title_relevance() {
    let state = ServerState::in_memory("test-admin-password").unwrap();
    for (id, title, description, created, updated, subscriptions) in [
        (9001, "MISSION_", None, Some(6), Some(20), None),
        (9002, "mission_ Zulu", None, Some(60), Some(50), Some(8)),
        (9003, "Mission_ alpha", None, Some(60), Some(10), Some(8)),
        (
            9004,
            "An earlier Mission_ text",
            None,
            Some(30),
            Some(50),
            Some(100),
        ),
        (
            9005,
            "Beta",
            Some("A mission_ in the description"),
            None,
            Some(100),
            Some(100),
        ),
        (
            9006,
            "beta",
            Some("A mission_ in the description"),
            Some(30),
            None,
            Some(100),
        ),
        (9007, "MissionX", None, Some(99), Some(75), Some(1)),
    ] {
        state
            .repo
            .upsert_item(
                &WorkshopItemRecord {
                    workshop_id: WorkshopId(id),
                    file_type: WorkshopFileType::Item,
                    status: WorkshopItemStatus::Active,
                    title: title.into(),
                    description: description.map(str::to_owned),
                    creator_steam_id: None,
                    creator_name: None,
                    preview_url: None,
                    previews: vec![],
                    file_url: None,
                    file_size: None,
                    time_created: created,
                    time_updated: updated,
                    tags: vec!["Campaign".into()],
                    dependencies: vec![],
                    collection_children: vec![],
                    subscriptions,
                    favorites: None,
                    source: MetadataSource::AdminOverride,
                    fetched_at: Utc::now(),
                },
                true,
                false,
            )
            .unwrap();
    }
    let app = build_router(state);
    let search = async |params: &str| {
        let (status, bytes) = json_request(
            &app,
            "GET",
            &format!("/api/v1/items/search{params}"),
            vec![],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{params}");
        let response: SearchResponse = serde_json::from_slice(&bytes).unwrap();
        let ids: Vec<_> = response
            .items
            .iter()
            .map(|item| item.workshop_id.0)
            .collect();
        (response.total, ids)
    };

    for (params, expected) in [
        (
            "?sort=updated_asc",
            [9006, 9003, 9001, 9002, 9004, 9007, 9005],
        ),
        ("?sort=title", [9004, 9005, 9006, 9001, 9003, 9002, 9007]),
        (
            "?sort=subscriptions",
            [9004, 9005, 9006, 9002, 9003, 9007, 9001],
        ),
        ("?sort=newest", [9007, 9003, 9002, 9006, 9004, 9001, 9005]),
        ("?sort=updated", [9005, 9007, 9004, 9002, 9001, 9003, 9006]),
        ("", [9005, 9004, 9006, 9002, 9003, 9007, 9001]),
    ] {
        assert_eq!(search(params).await, (7, expected.to_vec()), "{params}");
    }
    for params in ["?q=%20MiSSIon_%20", "?q=mission_&sort=relevance"] {
        assert_eq!(search(params).await, (4, vec![9001, 9002, 9003, 9004]));
    }
    for (sort, expected) in [
        ("relevance", [9001, 9002, 9003, 9004]),
        ("updated", [9004, 9002, 9001, 9003]),
        ("updated_asc", [9003, 9001, 9002, 9004]),
        ("title", [9004, 9001, 9003, 9002]),
        ("subscriptions", [9004, 9002, 9003, 9001]),
        ("newest", [9003, 9002, 9004, 9001]),
    ] {
        for (page, items) in [(1, &expected[..2]), (2, &expected[2..]), (3, &[][..])] {
            let params = format!("?q=mission_&sort={sort}&page={page}&pageSize=2");
            assert_eq!(search(&params).await, (4, items.to_vec()), "{params}");
        }
    }
    assert_eq!(
        search("?q=in%20the%20description").await,
        (2, vec![9005, 9006])
    );
}

#[tokio::test]
async fn search_keeps_camel_case_aliases_and_validates_polyphonic_readings() {
    let state = ServerState::in_memory("test-admin-password").unwrap();
    for (id, title, description) in [
        (41, "[L4D2] Left4Dead2Remastered", None),
        (42, "Fixture A", Some("[b]DayBreak[/b]")),
        (43, "Fixture B", Some("NightOfTheLivingDead2Remastered")),
        (44, "银行行长", None),
        (45, "银行转账", None),
        (46, "银行 行长", None),
    ] {
        let mut item = sample_item(id, title, 100, None);
        item.description = description.map(str::to_owned);
        state.repo.upsert_item(&item, false, false).unwrap();
    }
    let app = build_router(state);

    for (query, id) in [
        ("Left4Dead2Remastered", 41),
        ("left4dead2remastered", 41),
        ("left4dead2remaster", 41),
        ("DayBreak", 42),
        ("daybreak", 42),
        ("nightofthelivingdead2remastered", 43),
        ("yinhanghangzhang", 44),
        ("yin+hang+hang+zhang", 44),
    ] {
        let params = format!("q={query}");
        assert_eq!(
            search_result_ids(&app, &params).await,
            (1, vec![id]),
            "{query}"
        );
    }
}

#[tokio::test]
async fn description_fallback_checks_only_visible_text() {
    let state = ServerState::in_memory("test-admin-password").unwrap();
    for (id, title, description) in [
        (
            41,
            "Fixture A",
            Some("[img]HtTpS://example.test/mixedcasefilename.png\n[b]Still visible[/b] mission [url=https://example.test/mission_backup?x=%21]Read more[/url] Steam://url/CommunityFilePage/123456"),
        ),
        (42, "Fixture B", Some("Visible mission_ text, 80％ complete")),
        (43, "Download 100％", None),
    ] {
        let mut item = sample_item(id, title, 100, None);
        item.description = description.map(str::to_owned);
        if id == 42 {
            item.tags.push("Description only".into());
        }
        state.repo.upsert_item(&item, false, false).unwrap();
    }
    let app = build_router(state);
    for (params, expected) in [
        ("q=still+visible", vec![41]),
        ("q=read+more", vec![41]),
        ("q=mission_", vec![42]),
        ("q=_", vec![42]),
        ("q=mixedcasefilename", vec![]),
        ("q=123456", vec![]),
        ("q=%25", vec![43]),
        ("q=%EF%BC%85", vec![43]),
        ("q=%EF%BC%85&tag=DESCRIPTION+ONLY", vec![42]),
    ] {
        let total = expected.len() as u64;
        assert_eq!(
            search_result_ids(&app, params).await,
            (total, expected),
            "{params}"
        );
    }
}

#[tokio::test]
async fn workshop_links_are_exact_and_numeric_search_merges_distinct_candidates() {
    let state = ServerState::in_memory("test-admin-password").unwrap();
    for (id, title) in [(42, "Fixture 42"), (421, "Fixture A"), (88, "Another 42")] {
        let mut item = sample_item(id, title, 100, None);
        item.description = None;
        state.repo.upsert_item(&item, false, false).unwrap();
    }
    let app = build_router(state);

    for link in [
        "steam://url/CommunityFilePage/42",
        "https://steamcommunity.com/sharedfiles/filedetails/?searchtext=fixture&id=42",
        "https://steamcommunity.com/workshop/filedetails/?id=42",
    ] {
        let params = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("q", link)
            .finish();
        assert_eq!(search_result_ids(&app, &params).await, (1, vec![42]));
        assert_eq!(
            search_result_ids(&app, &format!("{params}&page=2")).await,
            (1, vec![])
        );
        assert_eq!(
            search_result_ids(&app, &format!("{params}&tag=Missing")).await,
            (0, vec![])
        );
    }
    for invalid in [
        "https://steamcommunity.com/groups/example?id=42",
        "https://example.test/steamcommunity.com/sharedfiles/filedetails/?id=42",
        "https://steamcommunity.com/sharedfiles/filedetails/?id=42extra",
    ] {
        let params = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("q", invalid)
            .finish();
        assert_eq!(
            search_result_ids(&app, &params).await,
            (0, vec![]),
            "{invalid}"
        );
    }
    assert_eq!(
        search_result_ids(&app, "q=42").await,
        (3, vec![42, 88, 421])
    );
    assert_eq!(search_result_ids(&app, "q=%2300042").await, (1, vec![42]));
}

async fn search_result_ids(app: &axum::Router, params: &str) -> (u64, Vec<u64>) {
    let (status, bytes) = json_request(
        app,
        "GET",
        &format!("/api/v1/items/search?{params}"),
        vec![],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{params}");
    let response: SearchResponse = serde_json::from_slice(&bytes).unwrap();
    let ids = response
        .items
        .iter()
        .map(|item| item.workshop_id.0)
        .collect();
    (response.total, ids)
}

#[tokio::test]
async fn search_handles_spaceless_compounds_pinyin_and_avoids_description_noise() {
    let state = ServerState::in_memory("test-admin-password").unwrap();
    let insert = |id: u64,
                  title: &str,
                  creator: Option<&str>,
                  desc: Option<&str>,
                  tags: &[&str],
                  subs: u64| {
        let mut item = sample_item(id, title, 1_700_000_000 + id as i64, None);
        item.creator_name = creator.map(str::to_owned);
        item.description = desc.map(str::to_owned);
        item.tags = tags.iter().map(|t| (*t).to_string()).collect();
        item.subscriptions = Some(subs);
        state.repo.upsert_item(&item, false, false).unwrap();
    };

    insert(
        2750097524,
        "Day Break v2",
        Some("Hoangzp"),
        Some("Re upload the light and old version map"),
        &["Campaigns", "Co-op"],
        36_450,
    );
    insert(
        3761588869,
        "Paradise of Doom (Part 1)",
        Some("Honest"),
        Some("Samples: Dannbo: Daybreak Sand Texture"),
        &["Campaigns", "Co-op"],
        34_607,
    );
    insert(
        990001,
        "Friday Breaking",
        Some("Tester"),
        None,
        &["Campaigns"],
        50_000,
    );
    for part in 1..=6u64 {
        let desc = if part == 1 {
            Some("[h2]Maps[/h2] [*]Residential (earlydays1.bsp)")
        } else {
            Some("[b]Necessary content for [url=https://steamcommunity.com/sharedfiles/filedetails/?id=3560883926]this campaign[/url][/b]")
        };
        insert(
            3560883920 + part,
            &format!("Early Days PART {part}/6"),
            Some("perfect_buddy"),
            desc,
            &["Campaigns", "Co-op"],
            46_600 - part * 100,
        );
    }
    insert(
        3832010631,
        "L4D2 - Custom Campaigns: Extended Subtitles",
        Some("Random"),
        Some("Supported: Prague • Early Days • Hell Ride"),
        &["UI", "Scripts"],
        84,
    );
    insert(
        2396847377,
        "广州增城 （Zengcheng）Lv8.06",
        Some("长远的梦"),
        Some("zc1_m1\nzc1_m2\nroutes of Zengcheng"),
        &["Campaigns", "Single Player", "Co-op"],
        1_577_877,
    );
    insert(
        2802325803,
        "宜昌市V5.01",
        Some("JohNny.R"),
        Some("图内含有止痛药及M60和女巫MOD"),
        &["Campaigns", "Co-op"],
        107_136,
    );
    insert(
        3578078027,
        "宜昌市2(YiChangCity2) V1.2 Part1/2",
        Some("JohNny.R"),
        Some("宜昌市续集（另外有个叫tom汤姆的家伙冒充增城作者，希望大家擦亮眼睛！）"),
        &["Campaigns", "Co-op"],
        55_495,
    );
    insert(
        3756711360,
        "Pearl River Rage v1.0",
        Some("5号停机坪"),
        Some("The map scenes are based on Liwan District and Haizhou District, Guangzhou."),
        &["Campaigns", "Co-op"],
        105_818,
    );
    insert(
        3087112386,
        "【Punishing: Gray Raven】曲·启明·泛用式替换Nick",
        Some("热油糕手"),
        Some("本模型素材的最终版权归【广州库洛科技有限公司】所有\nThe final copyright of this model material belongs to Guangzhou Kuluo Technology Co., Ltd"),
        &["Survivors", "Nick"],
        3_168,
    );
    insert(
        3115675033,
        "NanningCity V1.16.1",
        Some("HerobrineAce"),
        Some("南宁市战役地图"),
        &["Campaigns", "Co-op"],
        544_522,
    );

    let app = build_router(state);
    let search_ids = async |params: &str| {
        let (status, bytes) = json_request(
            &app,
            "GET",
            &format!("/api/v1/items/search?{params}"),
            vec![],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{params}");
        let response: SearchResponse = serde_json::from_slice(&bytes).unwrap();
        let ids: Vec<u64> = response.items.iter().map(|i| i.workshop_id.0).collect();
        (response.total, ids)
    };

    // DayBreak & lowercase daybreak find Day Break v2 and exclude Paradise of Doom & Friday Breaking
    for q in ["q=DayBreak", "q=daybreak", "q=Day%20Break", "q=day_break"] {
        assert_eq!(search_ids(q).await, (1, vec![2750097524]), "{q}");
    }

    // EarlyDays & lowercase earlydays find all 6 parts and exclude Extended Subtitles
    let expected_early: Vec<u64> = (1..=6).map(|p| 3560883920 + p).collect();
    for q in ["q=EarlyDays", "q=earlydays", "q=Early%20Days"] {
        assert_eq!(search_ids(q).await, (6, expected_early.clone()), "{q}");
    }

    // 增城 / zengcheng find 广州增城 without pulling in 宜昌市2 from description rant
    for q in ["q=zengcheng", "q=%E5%A2%9E%E5%9F%8E"] {
        assert_eq!(search_ids(q).await, (1, vec![2396847377]), "{q}");
    }

    // 广州 / guangzhou / guang zhou find 广州增城 without pulling in Pearl River Rage or Kuro models
    for q in [
        "q=%E5%B9%BF%E5%B7%9E",
        "q=guangzhou",
        "q=guang%20zhou",
        "q=guangzhouzengcheng",
    ] {
        assert_eq!(search_ids(q).await, (1, vec![2396847377]), "{q}");
    }

    // Explicit Survivors tag with 广州 falls back to description and finds the survivor model
    assert_eq!(
        search_ids("q=%E5%B9%BF%E5%B7%9E&tag=Survivors").await,
        (1, vec![3087112386])
    );

    // yichang finds both 宜昌市2 (direct title) and 宜昌市V5.01 (pinyin title)
    assert_eq!(
        search_ids("q=yichang").await,
        (2, vec![3578078027, 2802325803])
    );

    // Description fallback still works when primary fields have zero hits (map code, city in desc, company)
    assert_eq!(search_ids("q=zc1_m1").await, (1, vec![2396847377]));
    assert_eq!(
        search_ids("q=Liwan%20District").await,
        (1, vec![3756711360])
    );
    assert_eq!(
        search_ids("q=%E5%8D%97%E5%AE%81").await,
        (1, vec![3115675033])
    );
    assert_eq!(
        search_ids("q=%E5%BA%93%E6%B4%9B").await,
        (1, vec![3087112386])
    );

    // Creator search & Steam URL direct search
    assert_eq!(
        search_ids("q=%E9%95%BF%E8%BF%9C%E7%9A%84%E6%A2%A6").await,
        (1, vec![2396847377])
    );
    assert_eq!(
        search_ids(
            "q=https%3A%2F%2Fsteamcommunity.com%2Fsharedfiles%2Ffiledetails%2F%3Fid%3D2396847377"
        )
        .await,
        (1, vec![2396847377])
    );
}

#[tokio::test]
async fn read_through_ttl_force_refresh_and_stale_fallback_keep_file_url_fresh() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let mut v1 = sample_item(
        1001,
        "Chernobyl Chapter One",
        1000,
        Some("https://cdn.steamusercontent.com/ugc/1001_v1.vpk"),
    );
    v1.dependencies = vec![WorkshopDependency {
        workshop_id: WorkshopId(2001),
        title: None,
        required: true,
    }];
    upstream.insert_item(v1);

    let state = ServerState::in_memory_with_upstream("admin-secret-123", upstream.clone()).unwrap();
    let app = build_router(state.clone());

    // 1. First batch request triggers read-through fetch from SteamKit CM
    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/items/batch",
        vec![],
        Some(json!({ "ids": ["1001"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let batch: BatchItemsResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(batch.items.len(), 1);
    assert_eq!(
        batch.items[0].file_url.as_deref(),
        Some("https://cdn.steamusercontent.com/ugc/1001_v1.vpk")
    );
    assert_eq!(batch.items[0].previews.len(), 1);
    assert_eq!(batch.items[0].dependencies.len(), 1);
    assert_eq!(batch.items[0].source, MetadataSource::CatalogServer);
    assert_eq!(upstream.fetch_count(WorkshopId(1001)), 1);

    // 2. Addon updates on Steam Workshop with a new CDN file_url
    let v2 = sample_item(
        1001,
        "Chernobyl Chapter One",
        2000,
        Some("https://cdn.steamusercontent.com/ugc/1001_v2.vpk"),
    );
    upstream.insert_item(v2);

    // Within TTL without forceRefresh, cached snapshot is served without hitting Steam CM
    let (status, bytes) = json_request(&app, "GET", "/api/v1/items/1001", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let cached_item: WorkshopItemRecord = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        cached_item.file_url.as_deref(),
        Some("https://cdn.steamusercontent.com/ugc/1001_v1.vpk")
    );
    assert_eq!(upstream.fetch_count(WorkshopId(1001)), 1);

    // 3. Explicit ?refresh=true refreshes from Steam CM and updates file_url + revision history
    let (status, bytes) =
        json_request(&app, "GET", "/api/v1/items/1001?refresh=true", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let refreshed_item: WorkshopItemRecord = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        refreshed_item.file_url.as_deref(),
        Some("https://cdn.steamusercontent.com/ugc/1001_v2.vpk")
    );
    assert_eq!(refreshed_item.time_updated, Some(2000));
    assert_eq!(upstream.fetch_count(WorkshopId(1001)), 2);
    assert_eq!(
        state.repo.list_revisions(WorkshopId(1001)).unwrap().len(),
        2
    );

    // 4. If Steam CM becomes temporarily unreachable, non-forced reads gracefully serve cached data
    upstream.set_fail_fetches(true);
    let (status, bytes) = json_request(&app, "GET", "/api/v1/items/1001", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let fallback_item: WorkshopItemRecord = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        fallback_item.file_url.as_deref(),
        Some("https://cdn.steamusercontent.com/ugc/1001_v2.vpk")
    );
}

#[tokio::test]
async fn tombstone_preserves_history_without_polluting_db_and_respects_admin_override() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let mut item = sample_item(
        3001,
        "Classic Survival Map",
        1500,
        Some("https://cdn.steamusercontent.com/ugc/3001.vpk"),
    );
    item.dependencies = vec![WorkshopDependency {
        workshop_id: WorkshopId(3002),
        title: Some("Shared Textures".into()),
        required: true,
    }];
    upstream.insert_item(item);

    let state =
        ServerState::in_memory_with_upstream("strong-admin-pass", upstream.clone()).unwrap();
    let app = build_router(state.clone());

    // Populate #3001 into cache
    let (status, _) = json_request(&app, "GET", "/api/v1/items/3001", vec![], None).await;
    assert_eq!(status, StatusCode::OK);

    // Upstream removes #3001 (result = 9)
    upstream.insert_outcome(
        WorkshopId(3001),
        WorkshopFetchOutcome::Removed {
            workshop_id: WorkshopId(3001),
            source: MetadataSource::SteamKit,
            observed_at: Utc::now(),
            reason: Some("Steam CM result=9".into()),
        },
    );

    // Admin refreshes #3001 -> turns into tombstone preserving title, previews, and dependencies!
    let admin_hdr = vec![("X-Admin-Token", "strong-admin-pass")];
    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/admin/items/3001/refresh",
        admin_hdr.clone(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let detail: AdminItemDetailResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(detail.item.status, WorkshopItemStatus::Removed);
    assert!(detail.item.file_url.is_none());
    assert!(detail.removed_at.is_some());
    assert_eq!(detail.item.title, "Classic Survival Map");
    assert_eq!(detail.item.previews.len(), 1);
    assert_eq!(detail.item.dependencies.len(), 1);
    assert_eq!(detail.item.dependencies[0].workshop_id, WorkshopId(3002));

    // Querying a never-cached non-existent ID (#9999999) returns 404 on GET and negative-cached Removed on batch without polluting DB
    let (status, _) = json_request(&app, "GET", "/api/v1/items/9999999", vec![], None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(state.repo.stats().unwrap().total_items, 1);

    let (status, bytes) = json_request(
        &app,
        "POST",
        "/api/v1/items/batch",
        vec![],
        Some(json!({ "ids": ["9999999"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let neg_batch: BatchItemsResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(neg_batch.items.len(), 1);
    assert_eq!(neg_batch.items[0].status, WorkshopItemStatus::Removed);
    assert_eq!(upstream.fetch_count(WorkshopId(9999999)), 1);
    assert_eq!(state.repo.stats().unwrap().total_items, 1);

    // Admin overrides #3001 with custom mirror file_url and Active status
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/admin/items/3001/override",
        admin_hdr.clone(),
        Some(json!({
            "status": "active",
            "fileUrl": "https://mirror.example.com/archive/3001.vpk",
            "reason": "Restored via archive mirror"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Refresh while locked returns 409 Conflict and upstream sync does not overwrite it
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/admin/items/3001/refresh",
        admin_hdr.clone(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Clearing override unlocks item and re-syncs upstream status
    let (status, bytes) = json_request(
        &app,
        "DELETE",
        "/api/v1/admin/items/3001/override",
        admin_hdr,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let unlocked: AdminItemDetailResponse = serde_json::from_slice(&bytes).unwrap();
    assert!(!unlocked.is_admin_override);
    assert_eq!(unlocked.item.status, WorkshopItemStatus::Removed);
}

#[tokio::test]
async fn collection_auto_hydration_and_catalog_backend_provider_roundtrip() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let mut col = sample_item(5000, "Friday Coop Collection", 3000, None);
    col.file_type = WorkshopFileType::Collection;
    col.collection_children = vec![WorkshopId(5001), WorkshopId(5002)];
    upstream.insert_item(col);
    upstream.insert_item(sample_item(
        5001,
        "Member Addon One",
        3001,
        Some("https://cdn.steamusercontent.com/ugc/5001.vpk"),
    ));
    upstream.insert_item(sample_item(
        5002,
        "Member Addon Two",
        3002,
        Some("https://cdn.steamusercontent.com/ugc/5002.vpk"),
    ));

    let state = ServerState::in_memory_with_upstream("admin-key-123", upstream).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let provider =
        CatalogBackendProvider::new(format!("http://{addr}"), Arc::new(MirrorRouter::empty()));

    // Fetching collection automatically read-through fetches both the collection and its child items
    let fetched_col = provider.fetch_collection(WorkshopId(5000)).await.unwrap();
    assert_eq!(fetched_col.title.as_deref(), Some("Friday Coop Collection"));
    assert_eq!(
        fetched_col.child_ids,
        vec![WorkshopId(5001), WorkshopId(5002)]
    );
    assert_eq!(fetched_col.items.len(), 2);
    assert!(fetched_col
        .items
        .iter()
        .all(|item| item.source == MetadataSource::CatalogServer));
    assert_eq!(
        fetched_col.items[0].file_url.as_deref(),
        Some("https://cdn.steamusercontent.com/ugc/5001.vpk")
    );

    // Fetching normal item via /collections/{id} returns 404
    assert!(provider.fetch_collection(WorkshopId(5001)).await.is_err());
}

#[tokio::test]
async fn admin_session_toml_config_incremental_backfill_and_steam_auth_flows_work() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("catalog.toml");
    let db_path = dir.path().join("catalog.db");

    // 1. First run auto-generates catalog.toml with strong random admin password
    let (mut cfg, created) = CatalogConfig::load_or_init(&config_path).unwrap();
    assert!(created);
    assert!(config_path.is_file());
    assert!(cfg.admin.password.starts_with("cat_"));
    cfg.server.db_path = db_path;
    cfg.save_to(&config_path).unwrap();

    let (reloaded, created_again) = CatalogConfig::load_or_init(&config_path).unwrap();
    assert!(!created_again);
    let initial_password = reloaded.admin.password.clone();

    let upstream = Arc::new(MockWorkshopUpstream::new());
    upstream.insert_query_page(
        QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE,
        MATCHING_FILE_TYPE_ITEMS,
        "*",
        WorkshopQueryPage {
            total: 2,
            next_cursor: None,
            outcomes: vec![
                WorkshopFetchOutcome::Found {
                    item: sample_item(
                        7001,
                        "Updated Map A",
                        5500,
                        Some("https://cdn.steamusercontent.com/ugc/7001.vpk"),
                    ),
                }
                .into(),
                WorkshopFetchOutcome::Found {
                    item: sample_item(
                        7002,
                        "Updated Map B",
                        5400,
                        Some("https://cdn.steamusercontent.com/ugc/7002.vpk"),
                    ),
                }
                .into(),
            ],
        },
    );
    upstream.insert_query_page(
        QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
        MATCHING_FILE_TYPE_ITEMS,
        "*",
        WorkshopQueryPage {
            total: 2,
            next_cursor: Some("cursor_page_2".into()),
            outcomes: vec![WorkshopFetchOutcome::Found {
                item: sample_item(
                    7003,
                    "Backfill Item",
                    4000,
                    Some("https://cdn.steamusercontent.com/ugc/7003.vpk"),
                ),
            }
            .into()],
        },
    );

    let store = Arc::new(CatalogConfigStore::new(Some(config_path.clone()), reloaded));
    let repo = Arc::new(ServerRepo::open(&store.get().server.db_path).unwrap());
    let state = ServerState::with_components(store.clone(), repo, upstream);
    let app = build_router(state.clone());

    // 2. Admin UI HTML and empty-password 503 security check
    let (status, _, html) = raw_request(&app, "GET", "/admin/", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&html).contains("Left4Addons Catalog Console"));

    let empty_app = build_router(ServerState::in_memory("").unwrap());
    let (status, _) = json_request(&empty_app, "GET", "/api/v1/admin/stats", vec![], None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    // 3. Login via /api/v1/admin/auth/login sets HttpOnly cookie and authenticates subsequent Cookie requests
    let (status, login_headers, login_bytes) = raw_request(
        &app,
        "POST",
        "/api/v1/admin/auth/login",
        vec![],
        Some(json!({ "username": "admin", "password": initial_password })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let login_resp: AdminLoginResponse = serde_json::from_slice(&login_bytes).unwrap();
    let set_cookie = login_headers
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("; Secure"));
    let cookie_pair = set_cookie.split(';').next().unwrap();

    let (status, me_bytes) = json_request(
        &app,
        "GET",
        "/api/v1/admin/auth/me",
        vec![("Cookie", cookie_pair)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let me: AdminSessionInfo = serde_json::from_slice(&me_bytes).unwrap();
    assert!(me.authenticated);
    assert_eq!(me.username.as_deref(), Some("admin"));

    let csrf_token = me.csrf_token.as_deref().unwrap();
    let cookie_write = vec![("Cookie", cookie_pair), ("X-L4A-CSRF", csrf_token)];
    for headers in [
        vec![("Cookie", cookie_pair)],
        vec![("Cookie", cookie_pair), ("X-L4A-CSRF", "wrong-token")],
    ] {
        let (status, _) =
            json_request(&app, "POST", "/api/v1/admin/sync/trigger", headers, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    // 4. Update sync config & trigger incremental + backfill sync
    let bearer = format!("Bearer {}", login_resp.token);
    let (status, _) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/sync",
        vec![("Authorization", &bearer)],
        Some(
            serde_json::to_value(CatalogSyncConfigDto {
                enable_read_through: true,
                item_ttl_secs: 600,
                enable_incremental_sync: true,
                incremental_interval_secs: 120,
                incremental_pages_per_tick: 2,
                enable_backfill: true,
                backfill_interval_secs: 10,
                backfill_batch_size: 25,
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, sync_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/admin/sync/trigger",
        cookie_write.clone(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sync_status: CatalogSyncStatus = serde_json::from_slice(&sync_bytes).unwrap();
    assert_eq!(sync_status.last_incremental_upserted, 2);
    assert_eq!(sync_status.high_water_time_updated, Some(5500));

    let bf_count = state.sync.run_backfill_step_once().await.unwrap();
    assert_eq!(bf_count, 1);

    let (status, ov_bytes) = json_request(
        &app,
        "GET",
        "/api/v1/admin/overview",
        vec![("Cookie", cookie_pair)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let overview: CatalogOverviewResponse = serde_json::from_slice(&ov_bytes).unwrap();
    assert_eq!(overview.stats.total_items, 3);
    assert_eq!(
        overview.sync.backfill_cursor.as_deref(),
        Some("cursor_page_2")
    );
    assert_eq!(overview.sync.backfill_synced_total, 1);

    // 5. SteamKit CM QR & Credentials + Guard auth flows
    let (status, qr_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/admin/steam/auth/qr/start",
        cookie_write.clone(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let qr_flow: SteamAuthFlowStatus = serde_json::from_slice(&qr_bytes).unwrap();
    assert_eq!(qr_flow.state, SteamAuthFlowState::WaitingForScan);
    assert!(qr_flow.qr_svg.as_deref().unwrap().contains("<svg"));

    let (status, cred_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/admin/steam/auth/credentials",
        cookie_write.clone(),
        Some(json!({ "accountName": "valve_bot", "password": "secret" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cred_flow: SteamAuthFlowStatus = serde_json::from_slice(&cred_bytes).unwrap();
    assert_eq!(cred_flow.state, SteamAuthFlowState::WaitingForCode);

    let (status, guard_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/admin/steam/auth/guard",
        cookie_write.clone(),
        Some(json!({ "flowId": cred_flow.flow_id, "code": "A1B2C" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let guard_done: SteamAuthFlowStatus = serde_json::from_slice(&guard_bytes).unwrap();
    assert_eq!(guard_done.state, SteamAuthFlowState::Succeeded);
    assert_eq!(state.upstream.status().await.mode, SteamAuthMode::Account);

    // 6. Change admin password and verify persistence in catalog.toml on disk
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/admin/auth/password",
        cookie_write.clone(),
        Some(json!({
            "currentPassword": initial_password,
            "newPassword": "updated-super-strong-pass"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (disk_cfg, _) = CatalogConfig::load_or_init(&config_path).unwrap();
    assert_eq!(disk_cfg.admin.password, "updated-super-strong-pass");
    assert_eq!(disk_cfg.sync.item_ttl_secs, 600);
    assert!(disk_cfg.sync.enable_backfill);

    // Batch > 200 rejected with 400 Bad Request
    let too_many: Vec<String> = (1..=201).map(|i| i.to_string()).collect();
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/items/batch",
        vec![],
        Some(json!({ "ids": too_many })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test]
fn upgrades_legacy_revisions_and_locks_existing_admin_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    {
        let repo = ServerRepo::open(&path).unwrap();
        let mut override_item = sample_item(8001, "Legacy override", 100, None);
        override_item.description = Some("LegacyDayBreak".into());
        override_item.source = MetadataSource::AdminOverride;
        repo.upsert_item(&override_item, true, true).unwrap();
        let mut private = sample_item(
            8003,
            "Account only",
            100,
            Some("https://example.test/private"),
        );
        private.status = WorkshopItemStatus::PrivateOrUnavailable;
        repo.upsert_item(&private, false, true).unwrap();
    }

    // Reproduce an older cache with a version 1 search index.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP INDEX idx_revisions_wid;
         ALTER TABLE workshop_item_revisions RENAME TO revision_fixture;
         CREATE TABLE workshop_item_revisions (
             id INTEGER PRIMARY KEY AUTOINCREMENT, workshop_id INTEGER NOT NULL,
             status TEXT NOT NULL, title TEXT NOT NULL, time_updated INTEGER,
             dependencies_json TEXT NOT NULL, source TEXT NOT NULL,
             trust_score INTEGER NOT NULL, recorded_at TEXT NOT NULL
         );
         INSERT INTO workshop_item_revisions
             SELECT id, workshop_id, status, title, time_updated, dependencies_json,
                    source, 60, recorded_at
             FROM revision_fixture;
         DROP TABLE revision_fixture;
         CREATE INDEX idx_revisions_wid ON workshop_item_revisions(workshop_id, id DESC);
         ALTER TABLE workshop_items DROP COLUMN is_admin_override;
         ALTER TABLE workshop_items DROP COLUMN previews_json;
         DROP TABLE workshop_items_fts;
         CREATE VIRTUAL TABLE workshop_items_fts USING fts5(
             title_tokens, title_compact, title_pinyin, creator_tokens, desc_tokens
         );
         INSERT INTO workshop_items_fts (rowid, title_tokens, desc_tokens)
         VALUES (8001, 'legacy override', 'legacy day break'), (8003, 'account only', '');
         UPDATE sync_state SET value = '1' WHERE key = 'search_index_version';",
    )
    .unwrap();
    drop(conn);

    let repo = ServerRepo::open(&path).unwrap();
    let outcome = |id| {
        WorkshopObservation::from(WorkshopFetchOutcome::Found {
            item: sample_item(id, "Steam update", 200, None),
        })
    };
    let summary = repo
        .apply_upstream_outcomes(&[outcome(8001), outcome(8002)])
        .unwrap();
    assert_eq!(summary.skipped_overrides, 1);
    assert_eq!(summary.active_upserted, 1);

    let stored = repo.get_item(WorkshopId(8001)).unwrap().unwrap();
    assert!(stored.is_admin_override);
    assert_eq!(stored.record.title, "Legacy override");
    let historical = repo.list_revisions(WorkshopId(8001)).unwrap();
    assert_eq!(historical.len(), 1);
    assert_eq!(historical[0].title, "Legacy override");
    let private = repo.get_item(WorkshopId(8003)).unwrap().unwrap().record;
    assert_eq!(private.status, WorkshopItemStatus::PrivateOrUnavailable);
    assert_eq!(private.title, "Workshop #8003");
    assert!(private.file_url.is_none());
    for (query, expected) in [("legacydaybreak", 1), ("account only", 0)] {
        let res = repo
            .search(
                &l4a_protocol::SearchQuery {
                    query: Some(query.into()),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        assert_eq!(res.total, expected, "{query}");
    }
    let written = repo.list_revisions(WorkshopId(8002)).unwrap();
    assert_eq!(written.len(), 1);
    assert!(written[0].id > historical[0].id);

    // Reopening a legacy DB without the FTS5 table automatically builds the search index.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE IF EXISTS workshop_items_fts;
         DELETE FROM sync_state WHERE key = 'search_index_version';",
    )
    .unwrap();
    drop(conn);

    let rebuilt_repo = ServerRepo::open(&path).unwrap();
    let res = rebuilt_repo
        .search(
            &l4a_protocol::SearchQuery {
                query: Some("LegacyOverride".into()),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    assert_eq!(res.total, 1);
    assert_eq!(res.items[0].workshop_id, WorkshopId(8001));
}

#[test]
fn upstream_refresh_reindexes_only_changed_effective_search_fields() {
    let repo = ServerRepo::open_in_memory().unwrap();
    let mut item = sample_item(8501, "Original map", 100, None);
    item.description = Some("DayBreak".into());
    repo.upsert_item(&item, false, true).unwrap();

    // Abort the refresh if it tries to touch an FTS content row.
    repo.with_transaction(|tx| {
        tx.execute_batch(
            "CREATE TEMP TRIGGER fail_search_delete BEFORE DELETE ON workshop_items_fts_content
              BEGIN SELECT RAISE(ABORT, 'unexpected search index delete'); END;
             CREATE TEMP TRIGGER fail_search_insert BEFORE INSERT ON workshop_items_fts_content
              BEGIN SELECT RAISE(ABORT, 'unexpected search index insert'); END;",
        )?;
        Ok(())
    })
    .unwrap();
    let refresh = |item: &WorkshopItemRecord| {
        let observation = WorkshopFetchOutcome::Found { item: item.clone() }.into();
        let summary = repo.apply_upstream_outcomes(&[observation]).unwrap();
        assert_eq!(summary.active_upserted, 1);
    };
    item.fetched_at += chrono::TimeDelta::seconds(1);
    refresh(&item);
    item.creator_name = None;
    item.description = None;
    item.fetched_at += chrono::TimeDelta::seconds(1);
    refresh(&item);
    let stored = repo.get_item(item.workshop_id).unwrap().unwrap();
    assert_eq!(stored.last_verified_at, item.fetched_at);
    assert_eq!(stored.record.creator_name.as_deref(), Some("Mapper"));
    assert_eq!(stored.record.description.as_deref(), Some("DayBreak"));

    repo.with_transaction(|tx| {
        tx.execute_batch("DROP TRIGGER fail_search_delete; DROP TRIGGER fail_search_insert;")?;
        Ok(())
    })
    .unwrap();
    let total = |query: &str| {
        repo.search(
            &l4a_protocol::SearchQuery {
                query: Some(query.into()),
                ..Default::default()
            },
            None,
        )
        .unwrap()
        .total
    };
    item.creator_name = Some("OtherAuthor".into());
    item.fetched_at += chrono::TimeDelta::seconds(1);
    refresh(&item);
    assert_eq!(total("otherauthor"), 1);
    assert_eq!(total("mapper"), 0);
    assert_eq!(total("daybreak"), 1);

    item.creator_name = None;
    item.description = Some("EarlyDays".into());
    item.fetched_at += chrono::TimeDelta::seconds(1);
    refresh(&item);
    assert_eq!(total("earlydays"), 1);
    assert_eq!(total("daybreak"), 0);
    assert_eq!(total("otherauthor"), 1);

    item.title = "Renamed map".into();
    item.fetched_at += chrono::TimeDelta::seconds(1);
    refresh(&item);
    assert_eq!(total("renamed"), 1);
    assert_eq!(total("original"), 0);
}

#[tokio::test]
async fn incremental_checkpoint_resumes_after_restart_and_ignores_read_through() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let mut config = CatalogConfig::default();
    config.sync.incremental_pages_per_tick = 1;
    let store = Arc::new(CatalogConfigStore::new(None, config));
    let open = || {
        ServerState::with_components(
            store.clone(),
            Arc::new(ServerRepo::open(&path).unwrap()),
            upstream.clone(),
        )
    };
    let page = |id, updated, next: Option<&str>| WorkshopQueryPage {
        total: 4,
        next_cursor: next.map(str::to_owned),
        outcomes: vec![WorkshopFetchOutcome::Found {
            item: sample_item(id, "Incremental item", updated, None),
        }
        .into()],
    };
    let install = |cursor, page| {
        upstream.insert_query_page(
            QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE,
            MATCHING_FILE_TYPE_ITEMS,
            cursor,
            page,
        );
    };

    let state = open();
    install("*", page(8101, 100, None));
    state.sync.run_incremental_once().await.unwrap();
    assert_eq!(
        state.sync.status().await.unwrap().high_water_time_updated,
        Some(100)
    );

    upstream.insert_item(sample_item(8102, "Read through", 999, None));
    state
        .sync
        .resolve_single_item(WorkshopId(8102), false)
        .await
        .unwrap();
    install("*", page(8103, 300, Some("next")));
    install("next", page(8104, 200, Some("boundary")));
    install("boundary", page(8105, 50, Some("older")));

    state.sync.run_incremental_once().await.unwrap();
    assert_eq!(
        state.sync.status().await.unwrap().high_water_time_updated,
        Some(100)
    );
    drop(state);

    let state = open();
    state.sync.run_incremental_once().await.unwrap();
    assert!(state.repo.get_item(WorkshopId(8104)).unwrap().is_some());
    assert_eq!(
        state.sync.status().await.unwrap().high_water_time_updated,
        Some(100)
    );
    state.sync.run_incremental_once().await.unwrap();
    assert_eq!(
        state.sync.status().await.unwrap().high_water_time_updated,
        Some(300)
    );
}

#[tokio::test]
async fn config_write_failure_preserves_password_settings_and_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.toml");
    std::fs::create_dir(&path).unwrap();
    let mut config = CatalogConfig::default();
    config.admin.password = "old-password".into();
    let store = Arc::new(CatalogConfigStore::new(Some(path.clone()), config));
    let state = ServerState::with_components(
        store.clone(),
        Arc::new(ServerRepo::open_in_memory().unwrap()),
        Arc::new(MockWorkshopUpstream::new()),
    );
    let app = build_router(state.clone());

    let old_interval = store.get().sync.incremental_interval_secs;
    assert!(store
        .update(|cfg| cfg.sync.incremental_interval_secs = 321)
        .is_err());
    assert_eq!(store.get().sync.incremental_interval_secs, old_interval);
    let other_session = state.auth.login("admin", "old-password").unwrap();
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/admin/auth/password",
        vec![("X-Admin-Token", "old-password")],
        Some(json!({"currentPassword": "old-password", "newPassword": "new-password"})),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(store.get().admin.password, "old-password");
    assert!(state.auth.login("admin", "old-password").is_ok());
    assert!(state.auth.login("admin", "new-password").is_err());
    let (status, _) = json_request(
        &app,
        "GET",
        "/api/v1/admin/stats",
        vec![("X-Admin-Token", &other_session.token)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn restricted_details_stay_out_of_public_item_and_search_responses() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state = ServerState::in_memory_with_upstream("admin-password", upstream.clone()).unwrap();
    let app = build_router(state.clone());
    let public = sample_item(8201, "Public title", 100, None);
    state.repo.upsert_item(&public, false, true).unwrap();

    let mut private = sample_item(
        8201,
        "Account only",
        200,
        Some("https://example.test/private"),
    );
    private.status = WorkshopItemStatus::PrivateOrUnavailable;
    upstream.insert_item(private);
    let (status, bytes) =
        json_request(&app, "GET", "/api/v1/items/8201?refresh=true", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let item: WorkshopItemRecord = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(item.status, WorkshopItemStatus::PrivateOrUnavailable);
    assert_eq!(item.source, MetadataSource::CatalogServer);
    assert_eq!(item.title, "Workshop #8201");
    assert!(item.file_url.is_none());
    assert!(item.preview_url.is_none());

    let (status, bytes) =
        json_request(&app, "GET", "/api/v1/items/search?q=8201", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let results: SearchResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(results.items.len(), 1);
    assert_eq!(results.items[0].source, MetadataSource::CatalogServer);
    assert_eq!(results.items[0].title, "Workshop #8201");

    // Scrubbed private metadata must not linger in the FTS5 search index either.
    for leaked in ["Public", "Account"] {
        let (status, bytes) = json_request(
            &app,
            "GET",
            &format!("/api/v1/items/search?q={leaked}"),
            vec![],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let results: SearchResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            results.total, 0,
            "scrubbed term {leaked} leaked into search"
        );
    }
}

#[tokio::test]
async fn cancelling_refresh_request_does_not_strand_singleflight() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Arc::new(ServerRepo::open(&dir.path().join("catalog.db")).unwrap());
    let store = Arc::new(CatalogConfigStore::new(None, CatalogConfig::default()));
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let wid = WorkshopId(8301);
    upstream.insert_item(sample_item(wid.as_u64(), "Refresh item", 100, None));
    let state = ServerState::with_components(store, repo.clone(), upstream.clone());

    // Hold the writer so the refresh is in flight when its originating request is aborted.
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let hold_writer = tokio::task::spawn_blocking(move || {
        repo.with_transaction(|_| {
            locked_tx.send(()).unwrap();
            let _ = release_rx.blocking_recv();
            Ok(())
        })
    });
    locked_rx.await.unwrap();
    let sync = state.sync.clone();
    let leader = tokio::spawn(async move { sync.resolve_single_item(wid, false).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while upstream.fetch_count(wid) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    leader.abort();
    assert!(leader.await.unwrap_err().is_cancelled());
    release_tx.send(()).unwrap();
    hold_writer.await.unwrap().unwrap();

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.sync.resolve_single_item(wid, true),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert_eq!(result.workshop_id, wid);
}

#[test]
fn rejects_old_steam_versions_and_observations_after_a_tombstone() {
    let repo = ServerRepo::open(std::path::Path::new(":memory:")).unwrap();
    let now = Utc::now();
    let wid = WorkshopId(8401);
    let found = |wid: WorkshopId, ver, offset| {
        let mut item = sample_item(
            wid.as_u64(),
            "Public map",
            ver,
            Some(if ver == 200 {
                "https://example.test/v2.vpk"
            } else {
                "https://example.test/v1.vpk"
            }),
        );
        item.fetched_at = now + chrono::Duration::seconds(offset);
        WorkshopObservation::from(WorkshopFetchOutcome::Found { item })
    };
    let private = |wid, offset| {
        WorkshopObservation::from(WorkshopFetchOutcome::PrivateOrUnavailable {
            workshop_id: wid,
            source: MetadataSource::SteamKit,
            observed_at: now + chrono::Duration::seconds(offset),
            reason: None,
        })
    };

    let initial = found(wid, 200, 0);
    repo.apply_upstream_outcomes(&[initial]).unwrap();
    let summary = repo
        .apply_upstream_outcomes(&[found(wid, 100, 1), private(wid, -1)])
        .unwrap();
    assert_eq!(summary.skipped_stale, 2);
    let current = repo.get_item(wid).unwrap().unwrap();
    assert_eq!(current.last_verified_at, now);
    assert_eq!(
        current.record.file_url.as_deref(),
        Some("https://example.test/v2.vpk")
    );
    assert_eq!(current.record.time_updated, Some(200));

    repo.apply_upstream_outcomes(&[private(wid, 2)]).unwrap();
    let summary = repo
        .apply_upstream_outcomes(&[found(wid, 200, 1), found(wid, 100, 3)])
        .unwrap();
    assert_eq!(summary.skipped_stale, 2);
    let current = repo.get_item(wid).unwrap().unwrap().record;
    assert_eq!(current.status, WorkshopItemStatus::PrivateOrUnavailable);
    assert!(current.file_url.is_none());

    let unknown = WorkshopId(8402);
    repo.apply_upstream_outcomes(&[private(unknown, 2)])
        .unwrap();
    repo.apply_upstream_outcomes(&[found(unknown, 200, 1)])
        .unwrap();
    assert!(repo.get_item(unknown).unwrap().is_none());

    repo.apply_upstream_outcomes(&[found(wid, 200, 4)]).unwrap();
    assert_eq!(
        repo.get_item(wid).unwrap().unwrap().record.status,
        WorkshopItemStatus::Active
    );
}

#[test]
fn empty_steam_gallery_replaces_cached_previews_and_dependency_order_is_ignored() {
    let repo = ServerRepo::open_in_memory().unwrap();
    let mut item = sample_item(8451, "Map", 200, None);
    item.dependencies = [8481, 8471]
        .into_iter()
        .map(|id| WorkshopDependency {
            workshop_id: WorkshopId(id),
            title: None,
            required: true,
        })
        .collect();
    let wid = item.workshop_id;
    let observation = |item| WorkshopObservation::from(WorkshopFetchOutcome::Found { item });
    repo.apply_upstream_outcomes(&[observation(item.clone())])
        .unwrap();
    assert_eq!(
        repo.get_item(wid).unwrap().unwrap().record.previews.len(),
        1
    );

    item.fetched_at += chrono::Duration::milliseconds(1);
    repo.apply_upstream_outcomes(&[observation(item.clone())])
        .unwrap();
    assert_eq!(repo.list_revisions(wid).unwrap().len(), 1);

    item.fetched_at += chrono::Duration::milliseconds(1);
    item.time_updated = Some(300);
    item.previews.clear();
    repo.apply_upstream_outcomes(&[observation(item)]).unwrap();
    assert!(repo
        .get_item(wid)
        .unwrap()
        .unwrap()
        .record
        .previews
        .is_empty());
    assert_eq!(repo.list_revisions(wid).unwrap().len(), 2);
}

#[tokio::test]
async fn search_excludes_expired_and_unlisted_rows_but_direct_lookups_still_work() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    state
        .config_store
        .update(|cfg| cfg.sync.enable_read_through = false)
        .unwrap();
    let app = build_router(state.clone());

    let expired = WorkshopItemRecord {
        fetched_at: Utc::now() - chrono::Duration::hours(2),
        ..sample_item(
            8501,
            "Map expired",
            100,
            Some("https://example.test/old.vpk"),
        )
    };
    state.repo.upsert_item(&expired, false, true).unwrap();
    state
        .repo
        .upsert_item(&sample_item(8502, "Map listed", 200, None), false, true)
        .unwrap();
    upstream.insert_outcome(
        WorkshopId(8503),
        WorkshopObservation {
            outcome: WorkshopFetchOutcome::Found {
                item: sample_item(
                    8503,
                    "Map unlisted",
                    300,
                    Some("https://example.test/unlisted.vpk"),
                ),
            },
            listed: false,
        },
    );

    for id in [8501, 8503] {
        let suffix = if id == 8503 { "?refresh=true" } else { "" };
        let (status, data) = json_request(
            &app,
            "GET",
            &format!("/api/v1/items/{id}{suffix}"),
            vec![],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let item: WorkshopItemRecord = serde_json::from_slice(&data).unwrap();
        assert!(item.file_url.is_some());
    }

    let (status, data) = json_request(
        &app,
        "GET",
        "/api/v1/items/search?q=map&pageSize=1",
        vec![],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items: SearchResponse = serde_json::from_slice(&data).unwrap();
    assert_eq!(items.total, 1);
    assert_eq!(items.items[0].workshop_id, WorkshopId(8502));

    let (status, data) = json_request(
        &app,
        "GET",
        "/api/v1/admin/items/search?q=map",
        vec![("X-Admin-Token", "test-admin-password")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items: SearchResponse = serde_json::from_slice(&data).unwrap();
    assert_eq!(items.total, 3);

    upstream.insert_outcome(
        WorkshopId(8501),
        WorkshopFetchOutcome::PrivateOrUnavailable {
            workshop_id: WorkshopId(8501),
            source: MetadataSource::SteamKit,
            observed_at: Utc::now(),
            reason: None,
        },
    );
    let workers = state.sync.spawn_background_workers();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let item = state
                .repo
                .get_item(WorkshopId(8501))
                .unwrap()
                .unwrap()
                .record;
            if item.status == WorkshopItemStatus::PrivateOrUnavailable {
                assert!(item.file_url.is_none());
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    workers.cancel();
}

#[tokio::test]
async fn private_dependency_titles_are_removed_from_public_parents() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    let app = build_router(state.clone());
    state
        .repo
        .upsert_item(
            &sample_item(8601, "Dependency title", 100, None),
            false,
            true,
        )
        .unwrap();
    let mut parent = sample_item(8602, "Parent", 200, None);
    parent.dependencies = vec![WorkshopDependency {
        workshop_id: WorkshopId(8601),
        title: None,
        required: true,
    }];
    state.repo.upsert_item(&parent, false, true).unwrap();
    assert_eq!(
        state
            .repo
            .get_item(WorkshopId(8602))
            .unwrap()
            .unwrap()
            .record
            .dependencies[0]
            .title
            .as_deref(),
        Some("Dependency title")
    );

    upstream.insert_outcome(
        WorkshopId(8601),
        WorkshopFetchOutcome::PrivateOrUnavailable {
            workshop_id: WorkshopId(8601),
            source: MetadataSource::SteamKit,
            observed_at: Utc::now(),
            reason: None,
        },
    );
    state
        .sync
        .resolve_single_item(WorkshopId(8601), true)
        .await
        .unwrap();
    let title: Option<String> = state.repo.with_transaction(|tx| Ok(tx.query_row(
        "SELECT dep_title FROM workshop_item_dependencies WHERE parent_workshop_id = 8602 AND dep_workshop_id = 8601",
        [], |row| row.get(0),
    )?)).unwrap();
    assert!(title.is_none());

    upstream.insert_item(parent);
    let (status, data) =
        json_request(&app, "GET", "/api/v1/items/8602?refresh=true", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let item: WorkshopItemRecord = serde_json::from_slice(&data).unwrap();
    assert_eq!(item.dependencies[0].workshop_id, WorkshopId(8601));
    assert!(item.dependencies[0].title.is_none());
}

#[tokio::test]
async fn batch_keeps_successful_chunks_and_falls_back_only_for_failed_ids() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    let app = build_router(state.clone());
    for id in 1..=100 {
        upstream.insert_item(sample_item(id, "Fetched map", 200, None));
    }
    state
        .repo
        .upsert_item(
            &sample_item(
                101,
                "Cached map",
                100,
                Some("https://example.test/cached.vpk"),
            ),
            false,
            true,
        )
        .unwrap();
    upstream.fail_details_containing(WorkshopId(102));
    let ids: Vec<_> = (1..=102).map(|id| id.to_string()).collect();
    let (status, data) = json_request(
        &app,
        "POST",
        "/api/v1/items/batch",
        vec![],
        Some(json!({ "ids": ids, "forceRefresh": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let batch: BatchItemsResponse = serde_json::from_slice(&data).unwrap();
    assert_eq!(batch.items.len(), 101);
    assert_eq!(
        batch.items[100].file_url.as_deref(),
        Some("https://example.test/cached.vpk")
    );
    assert_eq!(batch.missing_ids, vec![WorkshopId(102)]);
}
