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
        MockWorkshopUpstream, SteamSearchRequest, SteamSearchTarget, WorkshopObservation,
        WorkshopQueryPage, MATCHING_FILE_TYPE_COLLECTIONS, MATCHING_FILE_TYPE_ITEMS,
        QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE, QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
        QUERY_TYPE_RANKED_BY_TEXT_SEARCH, QUERY_TYPE_RANKED_BY_TOTAL_UNIQUE_SUBSCRIPTIONS,
        QUERY_TYPE_RANKED_BY_TREND, QUERY_TYPE_RANKED_BY_VOTE,
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
                false,
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
            false,
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
            false,
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
async fn public_search_keeps_indexed_items_past_ttl_and_excludes_read_through_and_unlisted() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    let app = build_router(state.clone());

    // 1. An indexed catalog item older than item_ttl_secs stays visible in public homepage & search.
    let older_indexed = WorkshopItemRecord {
        fetched_at: Utc::now() - chrono::Duration::hours(2),
        subscriptions: Some(250_000),
        ..sample_item(
            8501,
            "Map indexed earlier",
            100,
            Some("https://example.test/old.vpk"),
        )
    };
    state.repo.upsert_item(&older_indexed, false, true).unwrap();
    let mut newer_indexed = sample_item(8502, "Map indexed recently", 200, None);
    newer_indexed.subscriptions = Some(10_000);
    state.repo.upsert_item(&newer_indexed, false, true).unwrap();

    // 2. A client read-through pull (e.g. syncing a user's installed addons) caches items for ID lookup,
    //    but MUST NOT place them onto the public Workshop homepage or search results.
    let mut installed_addon_a = sample_item(
        8504,
        "Map personal installed A",
        350,
        Some("https://example.test/installed_a.vpk"),
    );
    installed_addon_a.subscriptions = Some(9_000_000);
    let mut installed_addon_b = sample_item(
        8505,
        "Map personal installed B",
        360,
        Some("https://example.test/installed_b.vpk"),
    );
    installed_addon_b.subscriptions = Some(8_000_000);
    upstream.insert_item(installed_addon_a.clone());
    upstream.insert_item(installed_addon_b);
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
            consumer_appid: None,
        },
    );

    // Client pulls installed addons via batch read-through and unlisted item via direct GET.
    let (status, batch_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/items/batch",
        vec![],
        Some(json!({ "ids": ["8504", "8505"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let batch: BatchItemsResponse = serde_json::from_slice(&batch_bytes).unwrap();
    assert_eq!(batch.items.len(), 2);
    assert_eq!(upstream.fetch_count(WorkshopId(8504)), 1);

    // Subsequent direct lookup hits SQLite cache without hitting upstream again.
    let (status, _) = json_request(&app, "GET", "/api/v1/items/8504", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(upstream.fetch_count(WorkshopId(8504)), 1);

    // Direct lookup of unlisted item still works.
    let (status, data) =
        json_request(&app, "GET", "/api/v1/items/8503?refresh=true", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let unlisted_item: WorkshopItemRecord = serde_json::from_slice(&data).unwrap();
    assert!(unlisted_item.file_url.is_some());

    // Public Workshop homepage (no query) shows ONLY catalog-indexed items (including >TTL item 8501),
    // and NONE of the read-through-only installed addons or unlisted items.
    assert_eq!(search_result_ids(&app, "").await, (2, vec![8501, 8502]));
    assert_eq!(
        search_result_ids(&app, "q=map").await,
        (2, vec![8501, 8502])
    );
    assert_eq!(search_result_ids(&app, "q=personal").await, (0, vec![]));

    // Refreshing an already-indexed item via read-through does not displace other older indexed items.
    upstream.insert_item(newer_indexed.clone());
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/items/batch",
        vec![],
        Some(json!({ "ids": ["8502"], "forceRefresh": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(search_result_ids(&app, "").await, (2, vec![8501, 8502]));

    // Admin search still sees all cached rows (indexed + read-through + unlisted).
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
    assert_eq!(items.total, 5);

    // Admin single-item refresh preserves cache-only status and does NOT leak read-through item onto homepage.
    let (status, _) = json_request(
        &app,
        "POST",
        "/api/v1/admin/items/8505/refresh",
        vec![("X-Admin-Token", "test-admin-password")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(search_result_ids(&app, "").await, (2, vec![8501, 8502]));

    // The public feed can promote an entry even when the ID lookup has a newer snapshot.
    upstream.insert_query_page(
        QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE,
        MATCHING_FILE_TYPE_ITEMS,
        "*",
        WorkshopQueryPage {
            total: 1,
            next_cursor: None,
            outcomes: vec![WorkshopFetchOutcome::Found {
                item: WorkshopItemRecord {
                    fetched_at: installed_addon_a.fetched_at - chrono::Duration::seconds(1),
                    ..installed_addon_a
                },
            }
            .into()],
        },
    );
    assert_eq!(state.sync.run_incremental_once().await.unwrap(), 1);
    assert_eq!(
        search_result_ids(&app, "").await,
        (3, vec![8504, 8501, 8502])
    );

    // Admin overrides publish a listed cache entry while preserving an unlisted entry's visibility.
    for id in [8505, 8503] {
        let (status, _) = json_request(
            &app,
            "POST",
            &format!("/api/v1/admin/items/{id}/override"),
            vec![("X-Admin-Token", "test-admin-password")],
            Some(json!({ "status": "active", "title": "Curated map" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    assert_eq!(search_result_ids(&app, "q=curated").await, (1, vec![8505]));
}

#[tokio::test]
async fn listing_recheck_hides_restricted_metadata_without_incremental_sync() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    state
        .config_store
        .update(|cfg| {
            cfg.sync.enable_incremental_sync = false;
            cfg.sync.enable_read_through = false;
        })
        .unwrap();
    let app = build_router(state.clone());

    let private_id = WorkshopId(8601);
    let unlisted_id = WorkshopId(8602);
    for (id, title) in [
        (private_id, "Formerly public"),
        (unlisted_id, "Unlisted map"),
    ] {
        let mut item = sample_item(
            id.as_u64(),
            title,
            100,
            Some("https://example.test/map.vpk"),
        );
        item.fetched_at -= chrono::Duration::hours(2);
        state.repo.upsert_item(&item, false, true).unwrap();
    }
    assert_eq!(search_result_ids(&app, "").await, (2, vec![8601, 8602]));
    upstream.insert_outcome(
        private_id,
        WorkshopFetchOutcome::PrivateOrUnavailable {
            workshop_id: private_id,
            source: MetadataSource::SteamKit,
            observed_at: Utc::now(),
            reason: None,
        },
    );
    upstream.insert_outcome(
        unlisted_id,
        WorkshopObservation {
            outcome: WorkshopFetchOutcome::Found {
                item: sample_item(
                    8602,
                    "Unlisted map",
                    100,
                    Some("https://example.test/map.vpk"),
                ),
            },
            listed: false,
            consumer_appid: None,
        },
    );
    let _workers = state.sync.spawn_background_workers().drop_guard();
    let tombstone = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let item = state.repo.get_item(private_id).unwrap().unwrap().record;
            if item.status == WorkshopItemStatus::PrivateOrUnavailable {
                break item;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(tombstone.title, "Workshop #8601");
    assert!(tombstone.file_url.is_none());
    assert!(tombstone.preview_url.is_none());
    assert!(tombstone.previews.is_empty());
    assert_eq!(search_result_ids(&app, "").await, (1, vec![8601]));
    assert_eq!(search_result_ids(&app, "q=formerly").await, (0, vec![]));
    assert_eq!(search_result_ids(&app, "q=8602").await, (0, vec![]));
    assert!(state
        .sync
        .status()
        .await
        .unwrap()
        .last_incremental_at
        .is_none());
}

#[tokio::test]
async fn migration_keeps_legacy_rows_cache_only_until_rediscovered_by_a_feed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    let backfilled = sample_item(9101, "Backfilled item", 100, None);
    let mut read_through = sample_item(9102, "Personal cached item", 100, None);
    read_through.fetched_at -= chrono::Duration::hours(2);
    {
        let repo = ServerRepo::open(&path).unwrap();
        repo.upsert_item(&backfilled, false, true).unwrap();
        repo.upsert_item(&read_through, false, true).unwrap();
        let mut admin_override = sample_item(9103, "Admin override", 100, None);
        admin_override.source = MetadataSource::AdminOverride;
        repo.upsert_item(&admin_override, true, true).unwrap();
        repo.set_sync_state("incremental_scan", r#"{"checkpoint":100}"#)
            .unwrap();
        repo.set_sync_state("backfill_cursor", "DONE").unwrap();
        repo.set_sync_state("backfill_synced_total", "1").unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP INDEX IF EXISTS idx_workshop_items_catalog_public;
             DROP INDEX IF EXISTS idx_workshop_items_catalog_public_v2;
             DROP INDEX IF EXISTS idx_workshop_items_catalog_relevance_v1;
             ALTER TABLE workshop_items DROP COLUMN catalog_indexed;",
        )
        .unwrap();
    }

    let upstream = Arc::new(MockWorkshopUpstream::new());
    let repo = Arc::new(ServerRepo::open(&path).unwrap());
    let store = Arc::new(CatalogConfigStore::new(None, CatalogConfig::default()));
    let state = ServerState::with_components(store, repo.clone(), upstream.clone());
    let app = build_router(state.clone());
    assert_eq!(search_result_ids(&app, "").await, (1, vec![9103]));
    assert_eq!(
        repo.get_item(WorkshopId(9102)).unwrap().unwrap().record,
        read_through
    );
    let sync = state.sync.status().await.unwrap();
    assert!(sync.high_water_time_updated.is_none());
    assert!(sync.backfill_cursor.is_none());
    assert_eq!(sync.backfill_synced_total, 0);

    upstream.insert_query_page(
        QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
        MATCHING_FILE_TYPE_ITEMS,
        "*",
        WorkshopQueryPage {
            total: 1,
            next_cursor: None,
            outcomes: vec![WorkshopFetchOutcome::Found { item: backfilled }.into()],
        },
    );
    assert_eq!(state.sync.run_backfill_step_once().await.unwrap(), 1);
    assert_eq!(search_result_ids(&app, "").await, (2, vec![9101, 9103]));
    assert_eq!(search_result_ids(&app, "q=personal").await, (0, vec![]));

    let reopened = ServerRepo::open(&path).unwrap();
    assert_eq!(
        reopened
            .get_sync_state("backfill_cursor")
            .unwrap()
            .as_deref(),
        Some("DONE")
    );
    assert_eq!(reopened.search(&Default::default(), true).unwrap().total, 2);
}

#[tokio::test]
async fn backfill_counts_membership_only_promotions_with_its_cursor() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    let app = build_router(state.clone());
    let repo = &state.repo;

    for id in [9301, 9302, 9303] {
        upstream.insert_outcome(
            WorkshopId(id),
            WorkshopObservation {
                outcome: WorkshopFetchOutcome::Found {
                    item: sample_item(
                        id,
                        "New cache metadata",
                        200,
                        Some("https://example.test/new.vpk"),
                    ),
                },
                listed: id != 9303,
                consumer_appid: None,
            },
        );
    }
    let ids = [WorkshopId(9301), WorkshopId(9302), WorkshopId(9303)];
    state.sync.resolve_batch(&ids, false).await.unwrap();
    assert_eq!(search_result_ids(&app, "").await, (0, vec![]));

    let feed_item = |id: WorkshopId, version, offset| {
        let last_verified = repo.get_item(id).unwrap().unwrap().last_verified_at;
        WorkshopFetchOutcome::Found {
            item: WorkshopItemRecord {
                fetched_at: last_verified + chrono::Duration::seconds(offset),
                ..sample_item(id.as_u64(), "Old feed metadata", version, None)
            },
        }
        .into()
    };
    upstream.insert_query_page(
        QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
        MATCHING_FILE_TYPE_ITEMS,
        "*",
        WorkshopQueryPage {
            total: 3,
            next_cursor: Some("next".into()),
            outcomes: vec![
                feed_item(ids[0], 200, -1),
                feed_item(ids[1], 100, 1),
                feed_item(ids[2], 200, -1),
            ],
        },
    );

    // Fail checkpointing after the page has been applied, then retry the same stale snapshots.
    repo.with_transaction(|tx| {
        tx.execute_batch(
            "CREATE TRIGGER fail_backfill_checkpoint BEFORE INSERT ON sync_state
             WHEN NEW.key = 'backfill_cursor'
             BEGIN SELECT RAISE(ABORT, 'checkpoint write failed'); END;",
        )?;
        Ok(())
    })
    .unwrap();
    assert!(state.sync.run_backfill_step_once().await.is_err());
    assert_eq!(search_result_ids(&app, "").await, (0, vec![]));
    let sync = state.sync.status().await.unwrap();
    assert!(sync.backfill_cursor.is_none());
    assert_eq!(sync.backfill_synced_total, 0);

    repo.with_transaction(|tx| {
        tx.execute_batch("DROP TRIGGER fail_backfill_checkpoint")?;
        Ok(())
    })
    .unwrap();
    assert_eq!(state.sync.run_backfill_step_once().await.unwrap(), 2);
    let sync = state.sync.status().await.unwrap();
    assert_eq!(sync.backfill_cursor.as_deref(), Some("next"));
    assert_eq!(sync.backfill_synced_total, 2);
    assert_eq!(
        search_result_ids(&app, "q=new+cache").await,
        (2, vec![9301, 9302])
    );
    for id in &ids[..2] {
        let item = repo.get_item(*id).unwrap().unwrap().record;
        assert_eq!(item.title, "New cache metadata");
        assert_eq!(item.time_updated, Some(200));
        assert_eq!(
            item.file_url.as_deref(),
            Some("https://example.test/new.vpk")
        );
    }
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

#[tokio::test]
async fn batch_keeps_valid_matches_alongside_ids_outside_sqlite_range() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    upstream.insert_item(sample_item(42, "Valid uncached map", 100, None));
    upstream.insert_item(sample_item(u64::MAX - 1, "Unstorable map", 100, None));
    let app = build_router(state);

    let (status, data) = json_request(
        &app,
        "POST",
        "/api/v1/items/batch",
        vec![],
        Some(
            json!({ "ids": [u64::MAX.to_string(), "42".to_string(), (u64::MAX - 1).to_string()] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let batch: BatchItemsResponse = serde_json::from_slice(&data).unwrap();
    assert_eq!(batch.items.len(), 2);
    assert_eq!(batch.items[0].workshop_id, WorkshopId(42));
    assert_eq!(batch.items[0].title, "Valid uncached map");
    assert_eq!(batch.items[1].workshop_id, WorkshopId(u64::MAX));
    assert_eq!(batch.items[1].status, WorkshopItemStatus::Removed);
    assert_eq!(batch.missing_ids, vec![WorkshopId(u64::MAX - 1)]);
}

async fn search_with_mode(
    app: &axum::Router,
    uri: &str,
    headers: Vec<(&str, &str)>,
) -> (StatusCode, Option<String>, SearchResponse) {
    let (status, resp_headers, bytes) = raw_request(app, "GET", uri, headers, None).await;
    let mode = resp_headers
        .get("x-l4a-search-mode")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body: SearchResponse = serde_json::from_slice(&bytes).unwrap();
    (status, mode, body)
}

fn steam_req(
    query_type: u32,
    file_type: u32,
    page: u32,
    num_per_page: u32,
    search_text: Option<&str>,
    tag: Option<&str>,
    language: i32,
) -> SteamSearchRequest {
    SteamSearchRequest {
        query_type,
        file_type,
        page,
        num_per_page,
        search_text: search_text.map(str::to_string),
        required_tags: tag.map(|t| vec![t.to_string()]).unwrap_or_default(),
        language,
        days: (query_type == QUERY_TYPE_RANKED_BY_TREND).then_some(7),
        search_text_target: None,
    }
}

fn insert_steam_search_results(
    upstream: &MockWorkshopUpstream,
    request: SteamSearchRequest,
    outcomes: &[WorkshopObservation],
) {
    for size in [request.num_per_page, 100] {
        for (index, page) in outcomes.chunks(size as usize).enumerate() {
            upstream.insert_search_page(
                SteamSearchRequest {
                    page: index as u32 + 1,
                    num_per_page: size,
                    ..request.clone()
                },
                WorkshopQueryPage {
                    total: outcomes.len() as u64,
                    next_cursor: None,
                    outcomes: page.to_vec(),
                },
            );
        }
    }
}

#[tokio::test]
async fn hybrid_search_replicates_steam_relevance_promotes_local_pinyin_and_paginates_cleanly() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    // Local DB has description noise for "Early Days", a Chinese map for pinyin, and a camelCase map.
    let mut noise = sample_item(
        3832010631,
        "L4D2 - Custom Campaigns: Extended Subtitles",
        100,
        None,
    );
    noise.description = Some("Supported: Prague • Early Days • Hell Ride".into());
    noise.subscriptions = Some(84);
    state.repo.upsert_item(&noise, false, true).unwrap();

    let mut gz = sample_item(2396847377, "广州增城 （Zengcheng）Lv8.06", 200, None);
    gz.creator_name = Some("长远的梦".into());
    gz.subscriptions = Some(1_577_877);
    state.repo.upsert_item(&gz, false, true).unwrap();

    let mut day_break = sample_item(2750097524, "Day Break v2", 300, None);
    day_break.subscriptions = Some(36_450);
    state.repo.upsert_item(&day_break, false, true).unwrap();

    // The six Steam matches span two response pages of size 4.
    let early_part = |id: u64, title: &str, subs: u64| {
        let mut item = sample_item(
            id,
            title,
            500,
            Some(&format!("https://cdn.steamusercontent.com/ugc/{id}.vpk")),
        );
        item.subscriptions = Some(subs);
        WorkshopObservation::from(WorkshopFetchOutcome::Found { item })
    };
    insert_steam_search_results(
        &upstream,
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            4,
            Some("Early Days"),
            None,
            0,
        ),
        &[
            early_part(3560883926, "Early Days PART 1/6", 46_600),
            early_part(3561257025, "Early Days PART 6/6", 45_852),
            early_part(3560889923, "Early Days PART 5/6", 45_852),
            early_part(3560888138, "Early Days PART 4/6", 45_874),
            early_part(3560886975, "Early Days PART 3/6", 45_909),
            early_part(3560886114, "Early Days PART 2/6", 45_927),
        ],
    );

    // For pinyin "guangzhou", Steam only has weak description hits while local has 广州增城 (tier 4).
    let weak_steam = |id: u64, title: &str| {
        let mut item = sample_item(id, title, 400, None);
        item.description = Some("Map set near guangzhou".into());
        WorkshopObservation::from(WorkshopFetchOutcome::Found { item })
    };
    insert_steam_search_results(
        &upstream,
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            2,
            Some("guangzhou"),
            None,
            0,
        ),
        &[
            weak_steam(7001, "Weak Map A"),
            weak_steam(7002, "Weak Map B"),
            weak_steam(7003, "Weak Map C"),
        ],
    );

    let app = build_router(state);

    // 1. Steam Early Days ordering preserved; local description noise stays behind Steam pages.
    let (status, mode, p1) = search_with_mode(
        &app,
        "/api/v1/items/search?q=Early%20Days&page=1&pageSize=4",
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mode.as_deref(), Some("hybrid"));
    assert_eq!(p1.total, 7);
    assert_eq!(
        p1.items.iter().map(|i| i.workshop_id.0).collect::<Vec<_>>(),
        vec![3560883926, 3561257025, 3560889923, 3560888138]
    );

    let (status, mode, p2) = search_with_mode(
        &app,
        "/api/v1/items/search?q=Early%20Days&page=2&pageSize=4",
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mode.as_deref(), Some("hybrid"));
    assert_eq!(
        p2.items.iter().map(|i| i.workshop_id.0).collect::<Vec<_>>(),
        vec![3560886975, 3560886114, 3832010631]
    );

    // 2. Pinyin local strong hit promotes ahead of weak Steam hits, straddling pages cleanly.
    let (_, mode, gz_p1) = search_with_mode(
        &app,
        "/api/v1/items/search?q=guangzhou&page=1&pageSize=2",
        vec![],
    )
    .await;
    assert_eq!(mode.as_deref(), Some("hybrid"));
    assert_eq!(gz_p1.total, 4);
    assert_eq!(
        gz_p1
            .items
            .iter()
            .map(|i| i.workshop_id.0)
            .collect::<Vec<_>>(),
        vec![2396847377, 7001]
    );

    let (_, mode, gz_p2) = search_with_mode(
        &app,
        "/api/v1/items/search?q=guangzhou&page=2&pageSize=2",
        vec![],
    )
    .await;
    assert_eq!(mode.as_deref(), Some("hybrid"));
    assert_eq!(
        gz_p2
            .items
            .iter()
            .map(|i| i.workshop_id.0)
            .collect::<Vec<_>>(),
        vec![7002, 7003]
    );

    // 3. Spaceless compound query where Steam returns empty delegates to local matches.
    let (_, mode, db_res) = search_with_mode(&app, "/api/v1/items/search?q=DayBreak", vec![]).await;
    assert_eq!(mode.as_deref(), Some("hybrid"));
    assert_eq!(db_res.total, 1);
    assert_eq!(db_res.items[0].workshop_id, WorkshopId(2750097524));
}

#[tokio::test]
async fn hybrid_search_propagates_language_target_filetype_sorts_and_local_only_modes() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    let mut local_popular = sample_item(9501, "Local Top Map", 1000, None);
    local_popular.subscriptions = Some(100_000);
    local_popular.time_created = Some(500);
    state.repo.upsert_item(&local_popular, false, true).unwrap();

    let mut local_mid = sample_item(9502, "Alpha Local Map", 3000, None);
    local_mid.subscriptions = Some(20_000);
    local_mid.time_created = Some(3000);
    state.repo.upsert_item(&local_mid, false, true).unwrap();

    let mut local_removed = sample_item(9503, "Removed Local Map", 500, None);
    local_removed.status = WorkshopItemStatus::Removed;
    local_removed.time_created = Some(400);
    state.repo.upsert_item(&local_removed, false, true).unwrap();

    let mut steam_high = sample_item(9601, "Steam Popular Map", 2000, None);
    steam_high.subscriptions = Some(50_000);
    steam_high.time_created = Some(2000);
    let mut steam_low = sample_item(9602, "Steam Newer Map", 4000, None);
    steam_low.subscriptions = Some(10_000);
    steam_low.time_created = Some(4000);

    for sort_qt in [
        QUERY_TYPE_RANKED_BY_TOTAL_UNIQUE_SUBSCRIPTIONS,
        QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE,
        QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
    ] {
        upstream.insert_search_page(
            steam_req(sort_qt, MATCHING_FILE_TYPE_ITEMS, 1, 10, None, None, 0),
            WorkshopQueryPage {
                total: 2,
                next_cursor: None,
                outcomes: vec![
                    WorkshopObservation::from(WorkshopFetchOutcome::Found {
                        item: steam_high.clone(),
                    }),
                    WorkshopObservation::from(WorkshopFetchOutcome::Found {
                        item: steam_low.clone(),
                    }),
                ],
            },
        );
    }

    let app = build_router(state);

    // Language mapping: ?lang=zh-CN -> 6, ?lang=en -> 0, Accept-Language: zh-TW -> 7
    let _ = search_with_mode(
        &app,
        "/api/v1/items/search?q=Early%20Days&lang=zh-CN",
        vec![],
    )
    .await;
    let _ = search_with_mode(
        &app,
        "/api/v1/items/search?q=Early%20Days&lang=en",
        vec![("Accept-Language", "zh-CN,zh;q=0.9")],
    )
    .await;
    let _ = search_with_mode(
        &app,
        "/api/v1/items/search?q=Early%20Days",
        vec![
            ("Accept-Language", "en-US;q=0.4"),
            ("Accept-Language", "zh-TW;q=0.9"),
        ],
    )
    .await;
    let _ = search_with_mode(
        &app,
        "/api/v1/items/search?q=Early%20Days&searchTarget=title&fileType=collection&sort=trend",
        vec![],
    )
    .await;

    let recorded = upstream.recorded_searches();
    assert_eq!(recorded[0].language, 6);
    assert_eq!(recorded[1].language, 0);
    assert_eq!(recorded[2].language, 7);
    assert_eq!(recorded[3].query_type, QUERY_TYPE_RANKED_BY_TREND);
    assert_eq!(recorded[3].days, Some(7));
    assert_eq!(recorded[3].file_type, MATCHING_FILE_TYPE_COLLECTIONS);
    assert_eq!(
        recorded[3].search_text_target,
        Some(SteamSearchTarget::Title)
    );

    // Deterministic merge sorts across Steam + local items
    let (_, mode, subs) = search_with_mode(
        &app,
        "/api/v1/items/search?sort=subscriptions&pageSize=10",
        vec![],
    )
    .await;
    assert_eq!(mode.as_deref(), Some("hybrid"));
    assert_eq!(
        subs.items
            .iter()
            .map(|i| i.workshop_id.0)
            .collect::<Vec<_>>(),
        vec![9501, 9601, 9502, 9602, 9503]
    );

    let (_, _, upd) = search_with_mode(
        &app,
        "/api/v1/items/search?sort=updated&pageSize=10",
        vec![],
    )
    .await;
    assert_eq!(
        upd.items
            .iter()
            .map(|i| i.workshop_id.0)
            .collect::<Vec<_>>(),
        vec![9602, 9502, 9601, 9501, 9503]
    );

    let (_, _, newest) =
        search_with_mode(&app, "/api/v1/items/search?sort=newest&pageSize=10", vec![]).await;
    assert_eq!(
        newest
            .items
            .iter()
            .map(|i| i.workshop_id.0)
            .collect::<Vec<_>>(),
        vec![9602, 9502, 9601, 9501, 9503]
    );

    // Local-only modes do not call upstream search_files
    let count_before = upstream.recorded_searches().len();
    for uri in [
        "/api/v1/items/search?sort=title",
        "/api/v1/items/search?sort=updated_asc",
        "/api/v1/items/search?status=removed",
    ] {
        let (status, mode, _) = search_with_mode(&app, uri, vec![]).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(mode.as_deref(), Some("local"), "{uri}");
    }
    let (status, bytes) = json_request(
        &app,
        "GET",
        "/api/v1/admin/items/search?q=Local",
        vec![("X-Admin-Token", "test-admin-password")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let admin_res: SearchResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(admin_res.total, 3);
    assert_eq!(upstream.recorded_searches().len(), count_before);
}

#[tokio::test]
async fn hybrid_search_respects_admin_overrides_privacy_creator_enrichment_and_cache_warming() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    let now = Utc::now();

    // 5101: Admin override
    let override_item = sample_item(5101, "Steam Title 5101", 100, None);
    state.repo.upsert_item(&override_item, false, true).unwrap();
    state
        .repo
        .apply_admin_override(
            WorkshopId(5101),
            "admin",
            l4a_protocol::AdminItemOverrideRequest {
                status: WorkshopItemStatus::Active,
                title: Some("Overridden Title 5101".into()),
                description: None,
                file_url: Some("https://mirror.example.com/5101.vpk".into()),
                preview_url: None,
                dependencies: None,
                reason: Some("Manual fix".into()),
            },
        )
        .unwrap();

    // 5102: Locally tombstoned PrivateOrUnavailable (fresher than Steam mock observation)
    let mut priv_seed = sample_item(5102, "Was Public 5102", 100, None);
    priv_seed.fetched_at = now;
    state.repo.upsert_item(&priv_seed, false, true).unwrap();
    state
        .repo
        .apply_upstream_outcomes(&[WorkshopObservation {
            outcome: WorkshopFetchOutcome::PrivateOrUnavailable {
                workshop_id: WorkshopId(5102),
                source: MetadataSource::SteamKit,
                observed_at: now + chrono::Duration::seconds(60),
                reason: None,
            },
            listed: false,
            consumer_appid: None,
        }])
        .unwrap();

    // 5103: Locally unlisted (is_listed = false)
    let mut unlisted_seed = sample_item(5103, "Unlisted 5103", 100, None);
    unlisted_seed.fetched_at = now + chrono::Duration::seconds(60);
    state
        .repo
        .apply_upstream_outcomes(&[WorkshopObservation {
            outcome: WorkshopFetchOutcome::Found {
                item: unlisted_seed,
            },
            listed: false,
            consumer_appid: None,
        }])
        .unwrap();

    // 5105: Locally cached creator_name
    let mut known_creator = sample_item(5105, "Creator Enrich 5105", 100, None);
    known_creator.creator_name = Some("KnownMapper".into());
    state.repo.upsert_item(&known_creator, false, true).unwrap();

    // 5107: Canonical English item already indexed locally
    let canonical = sample_item(5107, "Canonical English Map", 100, None);
    state.repo.upsert_item(&canonical, false, true).unwrap();

    let mut s5101 = sample_item(5101, "Steam Title 5101", 200, None);
    s5101.fetched_at = now;
    let mut s5102 = sample_item(5102, "Leaked Private 5102", 200, None);
    s5102.fetched_at = now;
    let mut s5103 = sample_item(5103, "Leaked Unlisted 5103", 200, None);
    s5103.fetched_at = now;
    let mut s5104 = sample_item(5104, "Wrong App 5104", 200, None);
    s5104.fetched_at = now;
    let mut s5105 = sample_item(5105, "Creator Enrich 5105", 200, None);
    s5105.creator_name = None;
    s5105.fetched_at = now + chrono::Duration::seconds(10);
    let mut s5106 = sample_item(
        5106,
        "中文新图 5106",
        200,
        Some("https://cdn.steamusercontent.com/ugc/5106.vpk"),
    );
    s5106.fetched_at = now + chrono::Duration::seconds(10);
    let mut s5107 = sample_item(5107, "中文译名地图", 200, None);
    s5107.fetched_at = now + chrono::Duration::seconds(10);

    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("Test"),
            None,
            6,
        ),
        WorkshopQueryPage {
            total: 7,
            next_cursor: None,
            outcomes: vec![
                WorkshopObservation::from(WorkshopFetchOutcome::Found { item: s5101 }),
                WorkshopObservation::from(WorkshopFetchOutcome::Found { item: s5102 }),
                WorkshopObservation::from(WorkshopFetchOutcome::Found { item: s5103 }),
                WorkshopObservation {
                    outcome: WorkshopFetchOutcome::Found { item: s5104 },
                    listed: true,
                    consumer_appid: Some(4000),
                },
                WorkshopObservation::from(WorkshopFetchOutcome::Found { item: s5105 }),
                WorkshopObservation::from(WorkshopFetchOutcome::Found { item: s5106 }),
                WorkshopObservation::from(WorkshopFetchOutcome::Found { item: s5107 }),
            ],
        },
    );

    let app = build_router(state.clone());
    let (status, mode, res) =
        search_with_mode(&app, "/api/v1/items/search?q=Test&lang=zh-CN", vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mode.as_deref(), Some("hybrid"));

    let ids: Vec<u64> = res.items.iter().map(|i| i.workshop_id.0).collect();
    assert_eq!(ids, vec![5101, 5105, 5106, 5107]);
    assert_eq!(res.items[0].title, "Overridden Title 5101");
    assert_eq!(res.items[0].source, MetadataSource::AdminOverride);
    assert_eq!(res.items[1].creator_name.as_deref(), Some("KnownMapper"));
    assert_eq!(res.items[3].title, "中文译名地图");

    // Wait briefly for async CacheOnly warming of 5106
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while state.repo.get_item(WorkshopId(5106)).unwrap().is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    // Subsequent GET /items/5106 hits SQLite cache even when upstream fails
    upstream.set_fail_fetches(true);
    let (status, bytes) = json_request(&app, "GET", "/api/v1/items/5106", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let cached_5106: WorkshopItemRecord = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        cached_5106.file_url.as_deref(),
        Some("https://cdn.steamusercontent.com/ugc/5106.vpk")
    );

    // 5106 was CacheOnly-warmed so it does NOT leak onto empty public browse; 5107 kept its canonical English title!
    let (_, _, browse) = search_with_mode(&app, "/api/v1/items/search", vec![]).await;
    let browse_ids: Vec<u64> = browse.items.iter().map(|i| i.workshop_id.0).collect();
    assert!(!browse_ids.contains(&5106));
    assert_eq!(
        state
            .repo
            .get_item(WorkshopId(5107))
            .unwrap()
            .unwrap()
            .record
            .title,
        "Canonical English Map"
    );
}

#[tokio::test]
async fn hybrid_search_direct_url_long_id_fallback_timeout_and_cache_dedup() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    state
        .config_store
        .update(|cfg| cfg.search.steam_timeout_ms = 500)
        .unwrap();

    let local_fallback_item = sample_item(6200, "Local Survival Map", 100, None);
    state
        .repo
        .upsert_item(&local_fallback_item, false, true)
        .unwrap();

    let s6101 = sample_item(
        6101,
        "Direct URL Map",
        200,
        Some("https://cdn.steamusercontent.com/ugc/6101.vpk"),
    );
    upstream.insert_item(s6101);
    upstream.insert_outcome(
        WorkshopId(6102),
        WorkshopObservation {
            outcome: WorkshopFetchOutcome::Found {
                item: sample_item(6102, "Wrong App Map", 200, None),
            },
            listed: true,
            consumer_appid: Some(4000),
        },
    );
    state
        .repo
        .apply_upstream_outcomes(&[WorkshopObservation {
            outcome: WorkshopFetchOutcome::PrivateOrUnavailable {
                workshop_id: WorkshopId(6103),
                source: MetadataSource::SteamKit,
                observed_at: Utc::now(),
                reason: None,
            },
            listed: false,
            consumer_appid: None,
        }])
        .unwrap();
    let s610400 = sample_item(610400, "Long Numeric Match", 300, None);
    upstream.insert_item(s610400);

    let s_local_text = sample_item(6201, "Steam Survival Match", 400, None);
    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("Survival"),
            None,
            0,
        ),
        WorkshopQueryPage {
            total: 1,
            next_cursor: None,
            outcomes: vec![WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item: s_local_text,
            })],
        },
    );

    let app = build_router(state);

    // 1. Direct URL lookup via fetch_details
    let (_, mode, url_res) = search_with_mode(
        &app,
        "/api/v1/items/search?q=https%3A%2F%2Fsteamcommunity.com%2Fsharedfiles%2Ffiledetails%2F%3Fid%3D6101",
        vec![],
    )
    .await;
    assert_eq!(mode.as_deref(), Some("hybrid"));
    assert_eq!(url_res.total, 1);
    assert_eq!(url_res.items[0].workshop_id, WorkshopId(6101));

    // Wrong consumer_appid and locally blocked PrivateOrUnavailable return 0 items
    let (_, _, wrong_app) = search_with_mode(
        &app,
        "/api/v1/items/search?q=https%3A%2F%2Fsteamcommunity.com%2Fsharedfiles%2Ffiledetails%2F%3Fid%3D6102",
        vec![],
    )
    .await;
    assert_eq!(wrong_app.total, 0);

    let (_, _, blocked_priv) = search_with_mode(
        &app,
        "/api/v1/items/search?q=https%3A%2F%2Fsteamcommunity.com%2Fsharedfiles%2Ffiledetails%2F%3Fid%3D6103",
        vec![],
    )
    .await;
    assert_eq!(blocked_priv.total, 0);
    assert_eq!(upstream.fetch_count(WorkshopId(6103)), 0);

    // 2. 6+ digit ID lookup prepends tier-0 exact hit
    let (_, mode, num_res) = search_with_mode(&app, "/api/v1/items/search?q=610400", vec![]).await;
    assert_eq!(mode.as_deref(), Some("hybrid"));
    assert_eq!(num_res.total, 1);
    assert_eq!(num_res.items[0].workshop_id, WorkshopId(610400));

    // 3. Repeated identical search hits in-memory cache (1 Steam RPC)
    let before = upstream.recorded_searches().len();
    let (_, mode1, r1) = search_with_mode(&app, "/api/v1/items/search?q=Survival", vec![]).await;
    let (_, mode2, r2) = search_with_mode(&app, "/api/v1/items/search?q=Survival", vec![]).await;
    assert_eq!(mode1.as_deref(), Some("hybrid"));
    assert_eq!(mode2.as_deref(), Some("hybrid"));
    assert_eq!(r1.items.len(), 2);
    assert_eq!(r2.items.len(), 2);
    assert_eq!(upstream.recorded_searches().len() - before, 1);

    // 4. Steam search failure falls back gracefully to local catalog
    upstream.set_fail_searches(true);
    let (status, mode, fb) = search_with_mode(&app, "/api/v1/items/search?q=Local", vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mode.as_deref(), Some("local-fallback"));
    assert_eq!(fb.total, 1);
    assert_eq!(fb.items[0].workshop_id, WorkshopId(6200));

    // 5. Steam search timeout falls back gracefully to local catalog
    upstream.set_fail_searches(false);
    upstream.set_search_delay(Some(std::time::Duration::from_millis(800)));
    let (status, mode, to_fb) =
        search_with_mode(&app, "/api/v1/items/search?q=Local", vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mode.as_deref(), Some("local-fallback"));
    assert_eq!(to_fb.total, 1);
    assert_eq!(to_fb.items[0].workshop_id, WorkshopId(6200));
}

#[tokio::test]
async fn exact_search_checks_the_consumer_app_of_public_detail_cache_rows() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    for (id, app_id) in [(610201, 4000), (610202, 550), (610203, 4000)] {
        let item = sample_item(id, "A public result", 100, None);
        if id == 610203 {
            // Older caches have confirmed listing visibility without recording the app.
            state
                .repo
                .apply_upstream_outcomes_with_policy(
                    &[WorkshopFetchOutcome::Found { item: item.clone() }.into()],
                    l4a_server::db::CatalogIndexPolicy::CacheOnly,
                )
                .unwrap();
        }
        upstream.insert_outcome(
            WorkshopId(id),
            WorkshopObservation {
                outcome: WorkshopFetchOutcome::Found { item },
                listed: true,
                consumer_appid: Some(app_id),
            },
        );
    }
    let app = build_router(state);
    for id in [610201, 610202] {
        let (status, _) =
            json_request(&app, "GET", &format!("/api/v1/items/{id}"), vec![], None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(upstream.fetch_count(WorkshopId(id)), 1);
    }
    for (id, expected) in [(610201, vec![]), (610202, vec![610202]), (610203, vec![])] {
        let query = format!("q=steam%3A%2F%2Furl%2FCommunityFilePage%2F{id}");
        assert_eq!(
            search_result_ids(&app, &query).await,
            (expected.len() as u64, expected)
        );
        assert_eq!(upstream.fetch_count(WorkshopId(id)), 1);
    }
}

#[tokio::test]
async fn catalog_backend_provider_search_forwards_language_target_and_file_type() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let mut col = sample_item(7701, "Coop Collection", 300, None);
    col.file_type = WorkshopFileType::Collection;
    col.collection_children = vec![WorkshopId(7702)];
    upstream.insert_search_page(
        SteamSearchRequest {
            query_type: QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            file_type: MATCHING_FILE_TYPE_COLLECTIONS,
            page: 1,
            num_per_page: 24,
            search_text: Some("Coop".into()),
            required_tags: vec!["Campaigns".into()],
            language: 6,
            days: None,
            search_text_target: Some(SteamSearchTarget::Title),
        },
        WorkshopQueryPage {
            total: 1,
            next_cursor: None,
            outcomes: vec![WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item: col,
            })],
        },
    );

    let state = ServerState::in_memory_with_upstream("admin-key-123", upstream.clone()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let provider =
        CatalogBackendProvider::new(format!("http://{addr}"), Arc::new(MirrorRouter::empty()));
    let resp = provider
        .search(&l4a_protocol::SearchQuery {
            query: Some("Coop".into()),
            tag: Some("Campaigns".into()),
            status: None,
            sort: None,
            page: 1,
            page_size: 24,
            language: Some("zh-CN".into()),
            search_target: Some("title".into()),
            file_type: Some(WorkshopFileType::Collection),
        })
        .await
        .unwrap();

    assert_eq!(resp.total, 1);
    assert_eq!(resp.items[0].workshop_id, WorkshopId(7701));
    assert_eq!(resp.items[0].file_type, WorkshopFileType::Collection);
    let recorded = upstream.recorded_searches();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].language, 6);
    assert_eq!(recorded[0].file_type, MATCHING_FILE_TYPE_COLLECTIONS);
    assert_eq!(
        recorded[0].search_text_target,
        Some(SteamSearchTarget::Title)
    );
}

#[tokio::test]
async fn hybrid_search_suppresses_cached_steam_hit_after_newer_restriction_or_removal() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    state
        .config_store
        .update(|cfg| cfg.search.cache_steam_results = false)
        .unwrap();

    let t0 = Utc::now();
    let mut s9101 = sample_item(
        9101,
        "Restricted Later 9101",
        500,
        Some("https://cdn.steamusercontent.com/ugc/9101.vpk"),
    );
    s9101.fetched_at = t0;
    let mut s9102 = sample_item(
        9102,
        "Removed Later 9102",
        500,
        Some("https://cdn.steamusercontent.com/ugc/9102.vpk"),
    );
    s9102.fetched_at = t0;
    state.repo.upsert_item(&s9102, false, true).unwrap();

    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("Later"),
            None,
            0,
        ),
        WorkshopQueryPage {
            total: 2,
            next_cursor: None,
            outcomes: vec![
                WorkshopObservation::from(WorkshopFetchOutcome::Found { item: s9101 }),
                WorkshopObservation::from(WorkshopFetchOutcome::Found { item: s9102 }),
            ],
        },
    );

    let app = build_router(state.clone());
    let (_, mode1, first) = search_with_mode(&app, "/api/v1/items/search?q=Later", vec![]).await;
    assert_eq!(mode1.as_deref(), Some("hybrid"));
    assert_eq!(first.items.len(), 2);

    // 9101 becomes negative-cached PrivateOrUnavailable without a SQLite row; 9102 becomes Removed in SQLite.
    let t1 = t0 + chrono::Duration::seconds(30);
    state
        .repo
        .apply_upstream_outcomes(&[
            WorkshopObservation {
                outcome: WorkshopFetchOutcome::PrivateOrUnavailable {
                    workshop_id: WorkshopId(9101),
                    source: MetadataSource::SteamKit,
                    observed_at: t1,
                    reason: None,
                },
                listed: false,
                consumer_appid: None,
            },
            WorkshopObservation {
                outcome: WorkshopFetchOutcome::Removed {
                    workshop_id: WorkshopId(9102),
                    source: MetadataSource::SteamKit,
                    observed_at: t1,
                    reason: None,
                },
                listed: true,
                consumer_appid: None,
            },
        ])
        .unwrap();

    // Cached Steam page must now suppress both items.
    let (_, mode2, second) = search_with_mode(&app, "/api/v1/items/search?q=Later", vec![]).await;
    assert_eq!(mode2.as_deref(), Some("hybrid"));
    assert!(second.items.is_empty());
    assert_eq!(upstream.recorded_searches().len(), 1);
}

#[tokio::test]
async fn hybrid_search_keeps_deterministic_sort_offsets_across_response_pages() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    let mut local = sample_item(9200, "Local Top Sub", 100, None);
    local.subscriptions = Some(90_000);
    state.repo.upsert_item(&local, false, true).unwrap();

    let outcomes: Vec<_> = (1..=13)
        .map(|index| {
            let id = 9200 + index;
            let mut item = sample_item(id, &format!("Steam Item {id}"), 200, None);
            item.subscriptions = Some(80_000 - 1000 * index);
            WorkshopObservation::from(WorkshopFetchOutcome::Found { item })
        })
        .collect();
    insert_steam_search_results(
        &upstream,
        steam_req(
            QUERY_TYPE_RANKED_BY_TOTAL_UNIQUE_SUBSCRIPTIONS,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            1,
            None,
            None,
            0,
        ),
        &outcomes,
    );
    let app = build_router(state);
    let mut collected = Vec::new();
    for page in 1..=14 {
        let (_, mode, resp) = search_with_mode(
            &app,
            &format!("/api/v1/items/search?page={page}&pageSize=1"),
            vec![],
        )
        .await;
        assert_eq!(mode.as_deref(), Some("hybrid"));
        assert_eq!(resp.total, 14);
        assert_eq!(resp.items.len(), 1);
        collected.push(resp.items[0].workshop_id.0);
    }
    assert_eq!(collected, (9200..=9213).collect::<Vec<_>>());
}

#[tokio::test]
async fn hybrid_search_uses_large_scan_pages_on_both_sides_of_the_old_cutoff() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    state
        .config_store
        .update(|cfg| cfg.search.cache_steam_results = false)
        .unwrap();
    let app = build_router(state);

    for (query, total) in [("ScanA", 576_u64), ("ScanB", 577)] {
        let outcomes: Vec<_> = (100_000..100_000 + total)
            .map(|id| {
                WorkshopFetchOutcome::Found {
                    item: sample_item(id, "Steam scan result", 100, None),
                }
                .into()
            })
            .collect();
        insert_steam_search_results(
            &upstream,
            steam_req(
                QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
                MATCHING_FILE_TYPE_ITEMS,
                1,
                24,
                Some(query),
                None,
                0,
            ),
            &outcomes,
        );
        let page = total.div_ceil(24);
        let start = 100_000 + (page - 1) * 24;
        assert_eq!(
            search_result_ids(&app, &format!("q={query}&page={page}&pageSize=24")).await,
            (total, (start..100_000 + total).collect()),
        );
        let searches: Vec<_> = upstream
            .recorded_searches()
            .into_iter()
            .filter(|req| req.search_text.as_deref() == Some(query))
            .collect();
        assert_eq!(searches.len(), 7);
        assert_eq!(searches[0].num_per_page, 24);
        assert!(searches[1..].iter().all(|req| req.num_per_page == 100));
    }
}

#[tokio::test]
async fn hybrid_search_uses_native_paging_for_the_entire_broad_query() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    let mut local = sample_item(9199, "Local Top Sub", 100, None);
    local.subscriptions = Some(90_000);
    state.repo.upsert_item(&local, false, true).unwrap();

    for (page, id) in [(1, 9201), (13, 9213)] {
        upstream.insert_search_page(
            steam_req(
                QUERY_TYPE_RANKED_BY_TOTAL_UNIQUE_SUBSCRIPTIONS,
                MATCHING_FILE_TYPE_ITEMS,
                page,
                1,
                None,
                None,
                0,
            ),
            WorkshopQueryPage {
                total: 2401,
                next_cursor: None,
                outcomes: vec![WorkshopObservation::from(WorkshopFetchOutcome::Found {
                    item: sample_item(id, &format!("Steam Map {id}"), 200, None),
                })],
            },
        );
    }

    let app = build_router(state);
    for (page, id) in [(1, 9201), (13, 9213)] {
        let (status, mode, resp) = search_with_mode(
            &app,
            &format!("/api/v1/items/search?page={page}&pageSize=1"),
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(mode.as_deref(), Some("hybrid"));
        assert_eq!(resp.total, 2401);
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].workshop_id.0, id);
    }
    assert_eq!(upstream.recorded_searches().len(), 2);
}

#[tokio::test]
async fn hybrid_search_paginates_relevance_after_deduping_promotions_and_first_page_matches() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    for id in [9301, 9302] {
        let item = sample_item(id, "ExactTitle", 100, None);
        state.repo.upsert_item(&item, false, true).unwrap();
    }

    let outcomes: Vec<_> = (9301..=9313)
        .map(|id| {
            WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item: sample_item(id, &format!("Steam Map {id}"), 200, None),
            })
        })
        .collect();
    insert_steam_search_results(
        &upstream,
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            1,
            Some("ExactTitle"),
            None,
            0,
        ),
        &outcomes,
    );

    let app = build_router(state);
    let expected = [9302, 9301].into_iter().chain(9303..=9313);
    for (idx, id) in expected.enumerate() {
        let (_, mode, resp) = search_with_mode(
            &app,
            &format!(
                "/api/v1/items/search?q=ExactTitle&pageSize=1&page={}",
                idx + 1
            ),
            vec![],
        )
        .await;
        assert_eq!(mode.as_deref(), Some("hybrid"));
        assert_eq!(resp.total, 13);
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].workshop_id.0, id);
    }
}

#[tokio::test]
async fn hybrid_search_deduplicates_a_large_steam_order_result_set_before_the_local_tail() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    for id in [10000, 10150, 11000] {
        let item = sample_item(id, &format!("Overlap Map {id}"), 100, None);
        state.repo.upsert_item(&item, false, true).unwrap();
    }

    let insert_page = |page, size| {
        let start = 10000 + u64::from((page - 1) * size);
        let end = (start + u64::from(size)).min(10241);
        upstream.insert_search_page(
            steam_req(
                QUERY_TYPE_RANKED_BY_VOTE,
                MATCHING_FILE_TYPE_ITEMS,
                page,
                size,
                Some("Overlap"),
                None,
                0,
            ),
            WorkshopQueryPage {
                total: 241,
                next_cursor: None,
                outcomes: (start..end)
                    .map(|id| {
                        WorkshopObservation::from(WorkshopFetchOutcome::Found {
                            item: sample_item(id, &format!("Overlap Map {id}"), 200, None),
                        })
                    })
                    .collect(),
            },
        );
    };
    insert_page(1, 10);
    for page in 1..=3 {
        insert_page(page, 100);
    }

    let app = build_router(state);
    for (page, expected) in [
        (1, (10000..10010).collect()),
        (25, vec![10240, 11000]),
        (26, Vec::new()),
    ] {
        assert_eq!(
            search_result_ids(
                &app,
                &format!("q=Overlap&sort=toprated&page={page}&pageSize=10"),
            )
            .await,
            (242, expected),
        );
    }
}

#[tokio::test]
async fn hybrid_search_deduplicates_deep_local_matches_before_paging() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    for idx in 0..612u64 {
        let mut item = sample_item(
            80_000 + idx,
            &format!("Other Campaign {idx}"),
            1_700_000_000 + idx as i64,
            None,
        );
        item.description = Some("DeepReach supplemental".into());
        item.subscriptions = Some(1_000 - idx);
        state.repo.upsert_item(&item, false, true).unwrap();
    }

    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            24,
            Some("DeepReach"),
            None,
            0,
        ),
        WorkshopQueryPage {
            total: 2,
            next_cursor: None,
            outcomes: vec![
                WorkshopObservation::from(WorkshopFetchOutcome::Found {
                    item: sample_item(89_001, "DeepReach Steam One", 2_000, None),
                }),
                WorkshopObservation::from(WorkshopFetchOutcome::Found {
                    item: sample_item(80_600, "DeepReach Steam Two", 2_000, None),
                }),
            ],
        },
    );
    let app = build_router(state);
    let (_, mode1, p1) = search_with_mode(
        &app,
        "/api/v1/items/search?q=DeepReach&page=1&pageSize=24",
        vec![],
    )
    .await;
    assert_eq!(mode1.as_deref(), Some("hybrid"));
    assert_eq!(p1.total, 613);
    assert_eq!(p1.items.len(), 24);

    let (_, mode, resp) = search_with_mode(
        &app,
        "/api/v1/items/search?q=DeepReach&page=26&pageSize=24",
        vec![],
    )
    .await;
    assert_eq!(mode.as_deref(), Some("hybrid"));
    assert_eq!(resp.total, 613);
    assert_eq!(
        resp.items
            .iter()
            .map(|item| item.workshop_id.0)
            .collect::<Vec<_>>(),
        (80_598..80_612)
            .filter(|&id| id != 80_600)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn hybrid_search_persists_confirmed_restrictions_without_cache_warming() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    state
        .config_store
        .update(|cfg| cfg.search.cache_steam_results = false)
        .unwrap();
    let now = Utc::now();
    for (id, title) in [
        (9401, "Became Unlisted"),
        (9403, "Became Private"),
        (9404, "Still Public"),
    ] {
        let mut item = sample_item(id, title, 100, Some("https://example.test/map.vpk"));
        item.fetched_at = now;
        state.repo.upsert_item(&item, false, true).unwrap();
    }

    let mut steam_unlisted = sample_item(9401, "Became Unlisted", 200, None);
    steam_unlisted.subscriptions = Some(50_000);
    steam_unlisted.fetched_at = now + chrono::Duration::seconds(30);
    let mut steam_public = sample_item(9402, "Public Remaining", 200, None);
    steam_public.subscriptions = Some(10_000);
    let mut stale_unlisted = sample_item(9404, "Still Public", 99, None);
    stale_unlisted.fetched_at = now + chrono::Duration::seconds(30);

    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TOTAL_UNIQUE_SUBSCRIPTIONS,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            None,
            None,
            6,
        ),
        WorkshopQueryPage {
            total: 4,
            next_cursor: None,
            outcomes: vec![
                WorkshopObservation {
                    outcome: WorkshopFetchOutcome::Found {
                        item: steam_unlisted,
                    },
                    listed: false,
                    consumer_appid: Some(550),
                },
                WorkshopObservation::from(WorkshopFetchOutcome::Found { item: steam_public }),
                WorkshopObservation::from(WorkshopFetchOutcome::PrivateOrUnavailable {
                    workshop_id: WorkshopId(9403),
                    source: MetadataSource::SteamKit,
                    observed_at: now + chrono::Duration::seconds(30),
                    reason: None,
                }),
                WorkshopObservation {
                    outcome: WorkshopFetchOutcome::Found {
                        item: stale_unlisted,
                    },
                    listed: false,
                    consumer_appid: Some(550),
                },
            ],
        },
    );

    let app = build_router(state.clone());
    let (_, mode, resp) = search_with_mode(
        &app,
        "/api/v1/items/search?sort=subscriptions&lang=zh-CN",
        vec![],
    )
    .await;
    assert_eq!(mode.as_deref(), Some("hybrid"));
    assert_eq!(resp.total, 2);
    assert_eq!(
        resp.items
            .iter()
            .map(|i| i.workshop_id.0)
            .collect::<Vec<_>>(),
        vec![9402, 9404]
    );
    assert!(state.repo.get_item(WorkshopId(9402)).unwrap().is_none());
    assert_eq!(
        state
            .repo
            .get_item(WorkshopId(9401))
            .unwrap()
            .unwrap()
            .is_listed,
        Some(false)
    );
    let private = state
        .repo
        .get_item(WorkshopId(9403))
        .unwrap()
        .unwrap()
        .record;
    assert_eq!(private.status, WorkshopItemStatus::PrivateOrUnavailable);
    assert_eq!(private.title, "Workshop #9403");
    assert!(private.file_url.is_none());

    upstream.set_fail_searches(true);
    let (_, mode, fallback) = search_with_mode(&app, "/api/v1/items/search?q=Became", vec![]).await;
    assert_eq!(mode.as_deref(), Some("local-fallback"));
    assert_eq!(fallback.total, 0);
    assert_eq!(
        search_result_ids(&app, "q=Became&sort=title").await,
        (0, vec![])
    );
    assert_eq!(
        search_result_ids(&app, "q=Still&sort=title").await,
        (1, vec![9404])
    );
}

#[tokio::test]
async fn hybrid_search_shares_single_timeout_budget_across_multi_page_fetches() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    state
        .config_store
        .update(|cfg| cfg.search.steam_timeout_ms = 500)
        .unwrap();

    let mut local = sample_item(9450, "Budget Local Item", 100, None);
    local.subscriptions = Some(500);
    state.repo.upsert_item(&local, false, true).unwrap();

    for p in 1..=3u32 {
        let mut item = sample_item(9450 + u64::from(p), &format!("Steam P{p}"), 200, None);
        item.subscriptions = Some(10_000 - u64::from(p) * 100);
        upstream.insert_search_page(
            steam_req(
                QUERY_TYPE_RANKED_BY_TOTAL_UNIQUE_SUBSCRIPTIONS,
                MATCHING_FILE_TYPE_ITEMS,
                p,
                1,
                None,
                None,
                0,
            ),
            WorkshopQueryPage {
                total: 3,
                next_cursor: None,
                outcomes: vec![WorkshopObservation::from(WorkshopFetchOutcome::Found {
                    item,
                })],
            },
        );
    }

    upstream.set_search_delay(Some(std::time::Duration::from_millis(320)));
    let app = build_router(state);
    let started = std::time::Instant::now();
    let (status, mode, resp) = search_with_mode(
        &app,
        "/api/v1/items/search?sort=subscriptions&page=3&pageSize=1",
        vec![],
    )
    .await;
    let elapsed = started.elapsed();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(mode.as_deref(), Some("local-fallback"));
    assert!(
        upstream.recorded_searches().len() >= 2,
        "expected page 1 to complete and subsequent pages to share the same budget"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(900),
        "expected shared timeout budget around 500ms, took {elapsed:?}"
    );
    assert_eq!(resp.total, 1);
}

#[tokio::test]
async fn hybrid_search_falls_back_when_required_steam_page_fails() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    state
        .repo
        .upsert_item(
            &sample_item(9471, "MultiPage Fallback A", 200, None),
            false,
            true,
        )
        .unwrap();
    state
        .repo
        .upsert_item(
            &sample_item(9472, "MultiPage Fallback B", 100, None),
            false,
            true,
        )
        .unwrap();

    let first_page = WorkshopQueryPage {
        total: 101,
        next_cursor: None,
        outcomes: vec![WorkshopObservation::from(
            WorkshopFetchOutcome::PrivateOrUnavailable {
                workshop_id: WorkshopId(9471),
                source: MetadataSource::SteamKit,
                observed_at: Utc::now() + chrono::Duration::seconds(30),
                reason: None,
            },
        )],
    };
    for size in [1, 100] {
        upstream.insert_search_page(
            steam_req(
                QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
                MATCHING_FILE_TYPE_ITEMS,
                1,
                size,
                Some("MultiPage"),
                None,
                0,
            ),
            first_page.clone(),
        );
    }
    upstream.fail_search_request(steam_req(
        QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
        MATCHING_FILE_TYPE_ITEMS,
        2,
        100,
        Some("MultiPage"),
        None,
        0,
    ));

    let app = build_router(state);
    let (status, mode, resp) = search_with_mode(
        &app,
        "/api/v1/items/search?q=MultiPage&page=1&pageSize=1",
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mode.as_deref(), Some("local-fallback"));
    assert_eq!(resp.total, 1);
    assert_eq!(resp.items[0].workshop_id, WorkshopId(9472));
}

#[tokio::test]
async fn hybrid_search_checks_indexed_exact_id_independently_of_the_requested_page() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    let exact = sample_item(610400, "Indexed exact ID", 200, None);
    state.repo.upsert_item(&exact, false, true).unwrap();
    upstream.insert_item(exact);
    let mut prefix_match = sample_item(6104001, "Popular prefix match", 100, None);
    prefix_match.subscriptions = Some(10_000);
    state.repo.upsert_item(&prefix_match, false, true).unwrap();

    let app = build_router(state);
    for (page, expected) in [(1, 6104001), (2, 610400)] {
        assert_eq!(
            search_result_ids(
                &app,
                &format!("q=610400&sort=subscriptions&page={page}&pageSize=1"),
            )
            .await,
            (2, vec![expected]),
        );
    }
    assert_eq!(upstream.fetch_count(WorkshopId(610400)), 0);
}

#[tokio::test]
async fn hybrid_search_keeps_steam_hits_with_localized_tag_display_names() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    let mut item = sample_item(9491, "Localized Tag Campaign", 200, None);
    item.tags = vec!["战役".into()];
    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("Campaign"),
            Some("Campaigns"),
            6,
        ),
        WorkshopQueryPage {
            total: 1,
            next_cursor: None,
            outcomes: vec![WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item,
            })],
        },
    );

    let app = build_router(state);
    let (_, mode, resp) = search_with_mode(
        &app,
        "/api/v1/items/search?q=Campaign&tag=Campaigns&lang=zh-CN",
        vec![],
    )
    .await;
    assert_eq!(mode.as_deref(), Some("hybrid"));
    assert_eq!(resp.total, 1);
    assert_eq!(resp.items[0].workshop_id, WorkshopId(9491));
}

#[tokio::test]
async fn hybrid_search_respects_enable_read_through_switch_for_exact_lookups() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    state
        .config_store
        .update(|cfg| cfg.sync.enable_read_through = false)
        .unwrap();

    upstream.insert_item(sample_item(6501, "Blocked Read Through URL", 300, None));
    upstream.insert_item(sample_item(
        650100,
        "Blocked Read Through Numeric",
        300,
        None,
    ));
    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("TextAllowed"),
            None,
            0,
        ),
        WorkshopQueryPage {
            total: 1,
            next_cursor: None,
            outcomes: vec![WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item: sample_item(6502, "TextAllowed Steam Item", 300, None),
            })],
        },
    );

    let app = build_router(state.clone());
    let (_, url_mode, url_resp) = search_with_mode(
        &app,
        "/api/v1/items/search?q=https%3A%2F%2Fsteamcommunity.com%2Fsharedfiles%2Ffiledetails%2F%3Fid%3D6501",
        vec![],
    )
    .await;
    assert_eq!(url_mode.as_deref(), Some("local"));
    assert_eq!(url_resp.total, 0);
    assert_eq!(upstream.fetch_count(WorkshopId(6501)), 0);
    assert!(state.repo.get_item(WorkshopId(6501)).unwrap().is_none());

    let (_, _, num_resp) = search_with_mode(&app, "/api/v1/items/search?q=650100", vec![]).await;
    assert_eq!(num_resp.total, 0);
    assert_eq!(upstream.fetch_count(WorkshopId(650100)), 0);

    let (_, text_mode, text_resp) =
        search_with_mode(&app, "/api/v1/items/search?q=TextAllowed", vec![]).await;
    assert_eq!(text_mode.as_deref(), Some("hybrid"));
    assert_eq!(text_resp.total, 1);
    assert_eq!(text_resp.items[0].workshop_id, WorkshopId(6502));
}

#[tokio::test]
async fn hybrid_search_omitting_file_type_excludes_indexed_collections() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    state
        .config_store
        .update(|cfg| cfg.search.cache_ttl_secs = 0)
        .unwrap();

    let mut col = sample_item(9701, "Coop Collection Mega", 300, None);
    col.file_type = WorkshopFileType::Collection;
    state.repo.upsert_item(&col, false, true).unwrap();
    state
        .repo
        .upsert_item(&sample_item(9702, "Coop Item Map", 200, None), false, true)
        .unwrap();

    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("Coop"),
            None,
            0,
        ),
        WorkshopQueryPage {
            total: 1,
            next_cursor: None,
            outcomes: vec![WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item: sample_item(9703, "Coop Steam Item", 400, None),
            })],
        },
    );

    let app = build_router(state);
    let (_, mode, items_only) = search_with_mode(&app, "/api/v1/items/search?q=Coop", vec![]).await;
    assert_eq!(mode.as_deref(), Some("hybrid"));
    let hybrid_ids: Vec<u64> = items_only.items.iter().map(|i| i.workshop_id.0).collect();
    assert!(!hybrid_ids.contains(&9701));
    assert!(hybrid_ids.contains(&9702));
    assert!(hybrid_ids.contains(&9703));

    upstream.set_fail_searches(true);
    let (_, fb_mode, fb) = search_with_mode(&app, "/api/v1/items/search?q=Coop", vec![]).await;
    assert_eq!(fb_mode.as_deref(), Some("local-fallback"));
    assert_eq!(
        fb.items.iter().map(|i| i.workshop_id.0).collect::<Vec<_>>(),
        vec![9702]
    );

    let (_, _, col_res) = search_with_mode(
        &app,
        "/api/v1/items/search?q=Coop&fileType=collection&sort=title",
        vec![],
    )
    .await;
    assert_eq!(
        col_res
            .items
            .iter()
            .map(|i| i.workshop_id.0)
            .collect::<Vec<_>>(),
        vec![9701]
    );
}

#[tokio::test]
async fn hybrid_search_sets_vary_accept_language_while_preserving_cors() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    let app = build_router(state);

    let (status, resp_headers, _) = raw_request(
        &app,
        "GET",
        "/api/v1/items/search?q=VaryTest",
        vec![("Origin", "https://example.test")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        resp_headers
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok()),
        Some("*")
    );
    let vary_tokens: Vec<String> = resp_headers
        .get_all(header::VARY)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(','))
        .map(|s| s.trim().to_ascii_lowercase())
        .collect();
    assert!(
        vary_tokens.iter().any(|t| t == "accept-language"),
        "missing accept-language in Vary: {vary_tokens:?}"
    );
    assert!(
        vary_tokens.iter().any(|t| t == "origin"),
        "CORS vary token lost: {vary_tokens:?}"
    );
}

#[tokio::test]
async fn hybrid_search_native_page_keeps_steam_results_with_exact_id_and_skips_out_of_range_pages()
{
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    upstream.insert_item(sample_item(610999, "Exact Steam Hit", 400, None));
    for (page, ids) in [(1, vec![9901, 9902]), (2, vec![610999, 9903])] {
        upstream.insert_search_page(
            steam_req(
                QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
                MATCHING_FILE_TYPE_ITEMS,
                page,
                2,
                Some("610999"),
                None,
                0,
            ),
            WorkshopQueryPage {
                total: 2500,
                next_cursor: None,
                outcomes: ids
                    .into_iter()
                    .map(|id| {
                        WorkshopObservation::from(WorkshopFetchOutcome::Found {
                            item: sample_item(id, &format!("Broad Steam {id}"), 200, None),
                        })
                    })
                    .collect(),
            },
        );
    }

    let app = build_router(state);
    let (_, mode1, p1) = search_with_mode(
        &app,
        "/api/v1/items/search?q=610999&page=1&pageSize=2",
        vec![],
    )
    .await;
    assert_eq!(mode1.as_deref(), Some("hybrid"));
    assert_eq!(p1.total, 2500);
    assert_eq!(
        p1.items.iter().map(|i| i.workshop_id.0).collect::<Vec<_>>(),
        vec![9901, 9902]
    );

    let (_, mode2, p2) = search_with_mode(
        &app,
        "/api/v1/items/search?q=610999&page=2&pageSize=2",
        vec![],
    )
    .await;
    assert_eq!(mode2.as_deref(), Some("hybrid"));
    assert_eq!(p2.total, 2500);
    assert_eq!(
        p2.items.iter().map(|i| i.workshop_id.0).collect::<Vec<_>>(),
        vec![610999, 9903]
    );

    let searches_before = upstream.recorded_searches().len();
    let (_, mode_oob, oob) = search_with_mode(
        &app,
        "/api/v1/items/search?q=610999&page=5000&pageSize=2",
        vec![],
    )
    .await;
    assert_eq!(mode_oob.as_deref(), Some("hybrid"));
    assert_eq!(oob.total, 2500);
    assert!(oob.items.is_empty());
    assert_eq!(upstream.recorded_searches().len(), searches_before);
}

#[tokio::test]
async fn hybrid_search_admin_override_rescues_restricted_items_and_blocks_private_overrides() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    state
        .repo
        .upsert_item(&sample_item(9801, "Old Map 9801", 100, None), false, true)
        .unwrap();
    state
        .repo
        .apply_admin_override(
            WorkshopId(9801),
            "admin",
            l4a_protocol::AdminItemOverrideRequest {
                status: WorkshopItemStatus::Active,
                title: Some("Rescued Override 9801".into()),
                description: None,
                file_url: Some("https://mirror.example.com/9801.vpk".into()),
                preview_url: None,
                dependencies: None,
                reason: Some("Restore removed map".into()),
            },
        )
        .unwrap();

    state
        .repo
        .upsert_item(
            &sample_item(
                9802,
                "Secret Private Override",
                100,
                Some("https://secret.example.com/9802.vpk"),
            ),
            false,
            true,
        )
        .unwrap();
    state
        .repo
        .apply_admin_override(
            WorkshopId(9802),
            "admin",
            l4a_protocol::AdminItemOverrideRequest {
                status: WorkshopItemStatus::PrivateOrUnavailable,
                title: Some("Secret Private Override".into()),
                description: None,
                file_url: Some("https://secret.example.com/9802.vpk".into()),
                preview_url: None,
                dependencies: None,
                reason: Some("Admin hide".into()),
            },
        )
        .unwrap();

    // Complete mode: Steam returns 9801 as Removed and 9802 as Active
    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("Rescued"),
            None,
            0,
        ),
        WorkshopQueryPage {
            total: 2,
            next_cursor: None,
            outcomes: vec![
                WorkshopObservation::from(WorkshopFetchOutcome::Removed {
                    workshop_id: WorkshopId(9801),
                    source: MetadataSource::SteamKit,
                    observed_at: Utc::now() + chrono::Duration::seconds(30),
                    reason: None,
                }),
                WorkshopObservation::from(WorkshopFetchOutcome::Found {
                    item: sample_item(9802, "Leaked Steam 9802", 200, None),
                }),
            ],
        },
    );

    // NativePage mode: total > 2400 with same observations
    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("NativeOverride"),
            None,
            0,
        ),
        WorkshopQueryPage {
            total: 2500,
            next_cursor: None,
            outcomes: vec![
                WorkshopObservation::from(WorkshopFetchOutcome::Removed {
                    workshop_id: WorkshopId(9801),
                    source: MetadataSource::SteamKit,
                    observed_at: Utc::now() + chrono::Duration::seconds(30),
                    reason: None,
                }),
                WorkshopObservation::from(WorkshopFetchOutcome::Found {
                    item: sample_item(9802, "Leaked Steam 9802", 200, None),
                }),
            ],
        },
    );

    let app = build_router(state);
    for q in ["Rescued", "NativeOverride"] {
        let (_, mode, resp) =
            search_with_mode(&app, &format!("/api/v1/items/search?q={q}"), vec![]).await;
        assert_eq!(mode.as_deref(), Some("hybrid"));
        assert_eq!(
            resp.items.iter().map(|i| i.workshop_id.0).collect::<Vec<_>>(),
            vec![9801],
            "query {q} should rescue active AdminOverride 9801 and block private AdminOverride 9802"
        );
        assert_eq!(resp.items[0].title, "Rescued Override 9801");
        assert_eq!(resp.items[0].source, MetadataSource::AdminOverride);
    }
}

#[tokio::test]
async fn hybrid_search_restrictions_preserve_canonical_english_and_skip_inserting_unknown_unlisted()
{
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    state
        .config_store
        .update(|cfg| cfg.search.cache_steam_results = false)
        .unwrap();

    let now = Utc::now();
    let mut existing = sample_item(9811, "Canonical English Unlisted", 100, None);
    existing.fetched_at = now;
    state.repo.upsert_item(&existing, false, true).unwrap();

    let mut localized_unlisted = sample_item(9811, "中文未列出译名", 200, None);
    localized_unlisted.fetched_at = now + chrono::Duration::seconds(30);
    let mut unknown_unlisted = sample_item(9812, "Never Cached Unlisted", 200, None);
    unknown_unlisted.fetched_at = now + chrono::Duration::seconds(30);

    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("PreserveCanonical"),
            None,
            6,
        ),
        WorkshopQueryPage {
            total: 2,
            next_cursor: None,
            outcomes: vec![
                WorkshopObservation {
                    outcome: WorkshopFetchOutcome::Found {
                        item: localized_unlisted,
                    },
                    listed: false,
                    consumer_appid: Some(550),
                },
                WorkshopObservation {
                    outcome: WorkshopFetchOutcome::Found {
                        item: unknown_unlisted,
                    },
                    listed: false,
                    consumer_appid: Some(550),
                },
            ],
        },
    );

    let app = build_router(state.clone());
    let (_, mode, resp) = search_with_mode(
        &app,
        "/api/v1/items/search?q=PreserveCanonical&lang=zh-CN",
        vec![],
    )
    .await;
    assert_eq!(mode.as_deref(), Some("hybrid"));
    assert!(resp.items.is_empty());

    let updated = state.repo.get_item(WorkshopId(9811)).unwrap().unwrap();
    assert_eq!(updated.is_listed, Some(false));
    assert_eq!(updated.record.title, "Canonical English Unlisted");
    assert!(state.repo.get_item(WorkshopId(9812)).unwrap().is_none());
}

#[tokio::test]
async fn hybrid_search_handles_empty_params_unknown_status_exact_collection_trend_and_promoted_freshness(
) {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    // 1. Unknown status item
    let mut unknown_item = sample_item(9821, "Unknown Status Item", 100, None);
    unknown_item.status = WorkshopItemStatus::Unknown;
    state.repo.upsert_item(&unknown_item, false, true).unwrap();

    // 2. Local item for deterministic local_with_exact sort check
    let mut prefix_popular = sample_item(6106001, "Popular Prefix Match", 100, None);
    prefix_popular.subscriptions = Some(50_000);
    state
        .repo
        .upsert_item(&prefix_popular, false, true)
        .unwrap();
    let mut exact_low_subs = sample_item(610600, "Exact Low Subs", 200, None);
    exact_low_subs.subscriptions = Some(10);
    upstream.insert_item(exact_low_subs);

    // 3. Description-only numeric lookup must skip exact GetDetails
    upstream.insert_item(sample_item(610500, "Title Only 610500", 200, None));

    // 4. Bare 6+ digit collection lookup without explicit fileType
    let mut col = sample_item(349900, "Numeric Collection", 300, None);
    col.file_type = WorkshopFileType::Collection;
    upstream.insert_item(col);

    // 5. Empty-query sort=trend must not append old local items
    let mut steam_trend = sample_item(9825, "Trending Map", 400, None);
    steam_trend.subscriptions = Some(1_000);
    upstream.insert_search_page(
        SteamSearchRequest {
            query_type: QUERY_TYPE_RANKED_BY_TREND,
            file_type: MATCHING_FILE_TYPE_ITEMS,
            page: 1,
            num_per_page: 30,
            search_text: None,
            required_tags: Vec::new(),
            language: 0,
            days: Some(7),
            search_text_target: None,
        },
        WorkshopQueryPage {
            total: 1,
            next_cursor: None,
            outcomes: vec![WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item: steam_trend,
            })],
        },
    );

    // 6. A promoted match beyond the first response page keeps its localized Steam metadata.
    state
        .repo
        .upsert_item(&sample_item(9832, "PromoFresh", 100, None), false, true)
        .unwrap();
    insert_steam_search_results(
        &upstream,
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            1,
            Some("PromoFresh"),
            None,
            6,
        ),
        &[
            WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item: sample_item(9831, "Steam First Page", 200, None),
            }),
            WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item: sample_item(9832, "中文新鲜标题", 300, None),
            }),
        ],
    );

    let app = build_router(state);

    // Empty documented query params succeed with 200 OK
    let (status, _, _) = search_with_mode(
        &app,
        "/api/v1/items/search?q=&tag=&status=&sort=&page=&pageSize=&lang=&searchTarget=&fileType=",
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Oversized query is rejected with 400 Bad Request
    let long_q = "a".repeat(600);
    let (bad_status, _) = json_request(
        &app,
        "GET",
        &format!("/api/v1/items/search?q={long_q}"),
        vec![],
        None,
    )
    .await;
    assert_eq!(bad_status, StatusCode::BAD_REQUEST);

    // status=unknown routes to local mode
    let (_, unk_mode, unk_resp) =
        search_with_mode(&app, "/api/v1/items/search?status=unknown", vec![]).await;
    assert_eq!(unk_mode.as_deref(), Some("local"));
    assert_eq!(
        unk_resp
            .items
            .iter()
            .map(|i| i.workshop_id.0)
            .collect::<Vec<_>>(),
        vec![9821]
    );

    // searchTarget=description skips exact GetDetails lookup
    let (_, _, desc_resp) = search_with_mode(
        &app,
        "/api/v1/items/search?q=610500&searchTarget=description",
        vec![],
    )
    .await;
    assert_eq!(desc_resp.total, 0);
    assert_eq!(upstream.fetch_count(WorkshopId(610500)), 0);

    // Bare 6+ digit collection ID lookup returns the collection
    let (_, _, col_resp) = search_with_mode(&app, "/api/v1/items/search?q=349900", vec![]).await;
    assert_eq!(col_resp.total, 1);
    assert_eq!(col_resp.items[0].workshop_id, WorkshopId(349900));
    assert_eq!(col_resp.items[0].file_type, WorkshopFileType::Collection);

    // Empty-query sort=trend uses native Steam results without appending local DB
    let (_, trend_mode, trend_resp) =
        search_with_mode(&app, "/api/v1/items/search?sort=trend", vec![]).await;
    assert_eq!(trend_mode.as_deref(), Some("hybrid"));
    assert_eq!(trend_resp.total, 1);
    assert_eq!(
        trend_resp
            .items
            .iter()
            .map(|i| i.workshop_id.0)
            .collect::<Vec<_>>(),
        vec![9825]
    );

    // The promoted local hit keeps the fresh localized Steam title.
    let (_, promo_mode, promo_resp) = search_with_mode(
        &app,
        "/api/v1/items/search?q=PromoFresh&page=1&pageSize=1&lang=zh-CN",
        vec![],
    )
    .await;
    assert_eq!(promo_mode.as_deref(), Some("hybrid"));
    assert_eq!(promo_resp.items[0].workshop_id, WorkshopId(9832));
    assert_eq!(promo_resp.items[0].title, "中文新鲜标题");

    // Deterministic sort on local_with_exact orders by subscriptions instead of forcing exact first
    upstream.set_fail_searches(true);
    let (_, fb_mode, fb_resp) = search_with_mode(
        &app,
        "/api/v1/items/search?q=610600&sort=subscriptions",
        vec![],
    )
    .await;
    assert_eq!(fb_mode.as_deref(), Some("local-fallback"));
    assert_eq!(
        fb_resp
            .items
            .iter()
            .map(|i| i.workshop_id.0)
            .collect::<Vec<_>>(),
        vec![6106001, 610600]
    );
}

#[tokio::test]
async fn hybrid_search_coalesces_concurrent_requests_and_isolates_leader_timeout() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    state
        .config_store
        .update(|cfg| cfg.search.steam_timeout_ms = 500)
        .unwrap();

    let outcomes: Vec<_> = (9851..=9951)
        .map(|id| {
            WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item: sample_item(id, &format!("Coalesce {id}"), 200, None),
            })
        })
        .collect();
    insert_steam_search_results(
        &upstream,
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            100,
            Some("Coalesce"),
            None,
            0,
        ),
        &outcomes,
    );

    // 320ms per RPC: Request A spends 320ms on page 1, leaving only ~180ms when it starts page 2.
    // Request B starts at 350ms (page 1 is cached!) and joins page 2 inflight.
    // Because the leader runs with the full 500ms steam_timeout (not A's 180ms remainder), B succeeds!
    upstream.set_search_delay(Some(std::time::Duration::from_millis(320)));
    let app = build_router(state);

    let app_a = app.clone();
    let app_b = app.clone();
    let task_a = tokio::spawn(async move {
        search_with_mode(
            &app_a,
            "/api/v1/items/search?q=Coalesce&page=1&pageSize=100",
            vec![],
        )
        .await
    });
    let task_b = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(360)).await;
        search_with_mode(
            &app_b,
            "/api/v1/items/search?q=Coalesce&page=2&pageSize=100",
            vec![],
        )
        .await
    });

    let ((_, mode_a, _), (_, mode_b, resp_b)) = tokio::try_join!(task_a, task_b).unwrap();
    assert_eq!(mode_a.as_deref(), Some("local-fallback"));
    assert_eq!(mode_b.as_deref(), Some("hybrid"));
    assert_eq!(resp_b.total, 101);
    assert_eq!(resp_b.items[0].workshop_id, WorkshopId(9951));
    assert_eq!(upstream.recorded_searches().len(), 2);
}

#[tokio::test]
async fn unlisted_observations_for_unknown_items_suppress_cached_search_hits_without_blocking_by_id_lookup(
) {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    state
        .config_store
        .update(|cfg| cfg.search.cache_steam_results = false)
        .unwrap();

    let t0 = Utc::now();
    let mut public_v1 = sample_item(
        9901,
        "Cached Public Hit 9901",
        100,
        Some("https://cdn.steamusercontent.com/ugc/9901_v1.vpk"),
    );
    public_v1.fetched_at = t0;

    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("CachedPublicHit"),
            None,
            0,
        ),
        WorkshopQueryPage {
            total: 1,
            next_cursor: None,
            outcomes: vec![WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item: public_v1,
            })],
        },
    );

    let app = build_router(state.clone());
    // 1. Prime the Steam page cache without inserting 9901 into SQLite.
    let (_, mode1, first) =
        search_with_mode(&app, "/api/v1/items/search?q=CachedPublicHit", vec![]).await;
    assert_eq!(mode1.as_deref(), Some("hybrid"));
    assert_eq!(first.items.len(), 1);
    assert!(state.repo.get_item(WorkshopId(9901)).unwrap().is_none());

    // 2. Another search observes 9901 as unlisted at t1 > t0 (and revision 200).
    let mut unlisted_v2 = sample_item(
        9901,
        "Cached Public Hit 9901",
        200,
        Some("https://cdn.steamusercontent.com/ugc/9901_v2.vpk"),
    );
    unlisted_v2.fetched_at = t0 + chrono::Duration::seconds(30);
    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("ObserveUnlisted"),
            None,
            0,
        ),
        WorkshopQueryPage {
            total: 1,
            next_cursor: None,
            outcomes: vec![WorkshopObservation {
                outcome: WorkshopFetchOutcome::Found {
                    item: unlisted_v2.clone(),
                },
                listed: false,
                consumer_appid: Some(550),
            }],
        },
    );

    let (_, _, second) =
        search_with_mode(&app, "/api/v1/items/search?q=ObserveUnlisted", vec![]).await;
    assert!(second.items.is_empty());
    // Still no SQLite row created for unknown unlisted item:
    assert!(state.repo.get_item(WorkshopId(9901)).unwrap().is_none());

    // 3. Re-querying the cached first page must now suppress 9901 via the search-only marker.
    let (_, mode3, cached_after_unlisted) =
        search_with_mode(&app, "/api/v1/items/search?q=CachedPublicHit", vec![]).await;
    assert_eq!(mode3.as_deref(), Some("hybrid"));
    assert!(cached_after_unlisted.items.is_empty());
    assert_eq!(upstream.recorded_searches().len(), 2);

    // 4. A newer RPC returning an older Steam revision (v100 < v200) is also suppressed by the marker.
    let mut lagging_public_v1 = sample_item(
        9901,
        "Cached Public Hit 9901",
        100,
        Some("https://cdn.steamusercontent.com/ugc/9901_v1.vpk"),
    );
    lagging_public_v1.fetched_at = t0 + chrono::Duration::seconds(60);
    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("LaggingPublic"),
            None,
            0,
        ),
        WorkshopQueryPage {
            total: 1,
            next_cursor: None,
            outcomes: vec![WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item: lagging_public_v1,
            })],
        },
    );
    let (_, _, lagging_res) =
        search_with_mode(&app, "/api/v1/items/search?q=LaggingPublic", vec![]).await;
    assert!(lagging_res.items.is_empty());

    // 5. By-ID and batch lookup MUST NOT be blocked by the search-only unlisted marker!
    upstream.insert_outcome(
        WorkshopId(9901),
        WorkshopObservation {
            outcome: WorkshopFetchOutcome::Found { item: unlisted_v2 },
            listed: false,
            consumer_appid: Some(550),
        },
    );
    let (get_status, get_bytes) =
        json_request(&app, "GET", "/api/v1/items/9901", vec![], None).await;
    assert_eq!(get_status, StatusCode::OK);
    let by_id: WorkshopItemRecord = serde_json::from_slice(&get_bytes).unwrap();
    assert_eq!(
        by_id.file_url.as_deref(),
        Some("https://cdn.steamusercontent.com/ugc/9901_v2.vpk")
    );
    assert_eq!(upstream.fetch_count(WorkshopId(9901)), 1);
}

#[tokio::test]
async fn overlay_enforces_steam_revision_ordering_before_relisting_unlisted_or_restricted_items() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    state
        .config_store
        .update(|cfg| cfg.search.cache_ttl_secs = 0)
        .unwrap();

    let t0 = Utc::now();
    let mut unlisted_v200 = sample_item(
        9911,
        "Unlisted V200",
        200,
        Some("https://cdn.steamusercontent.com/ugc/9911_v200.vpk"),
    );
    unlisted_v200.fetched_at = t0;
    state
        .repo
        .apply_upstream_outcomes(&[WorkshopObservation {
            outcome: WorkshopFetchOutcome::Found {
                item: unlisted_v200,
            },
            listed: false,
            consumer_appid: Some(550),
        }])
        .unwrap();

    // A later search RPC (t0 + 60s) returns an older public revision (time_updated = 100 < 200).
    let mut stale_public_v100 = sample_item(9911, "Public V100", 100, None);
    stale_public_v100.fetched_at = t0 + chrono::Duration::seconds(60);
    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("RevisionOrder"),
            None,
            0,
        ),
        WorkshopQueryPage {
            total: 1,
            next_cursor: None,
            outcomes: vec![WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item: stale_public_v100,
            })],
        },
    );

    let app = build_router(state);
    let (_, _, stale_resp) =
        search_with_mode(&app, "/api/v1/items/search?q=RevisionOrder", vec![]).await;
    assert!(
        stale_resp.items.is_empty(),
        "older Steam revision must not override newer unlisted state"
    );

    // Once Steam returns the current/newer revision (time_updated = 200) as public at t0 + 90s, search exposes it.
    let mut relisted_v200 = sample_item(9911, "Public V200", 200, None);
    relisted_v200.fetched_at = t0 + chrono::Duration::seconds(90);
    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("RevisionOrder"),
            None,
            0,
        ),
        WorkshopQueryPage {
            total: 1,
            next_cursor: None,
            outcomes: vec![WorkshopObservation::from(WorkshopFetchOutcome::Found {
                item: relisted_v200,
            })],
        },
    );
    let (_, _, fresh_resp) =
        search_with_mode(&app, "/api/v1/items/search?q=RevisionOrder", vec![]).await;
    assert_eq!(fresh_resp.items.len(), 1);
    assert_eq!(fresh_resp.items[0].workshop_id, WorkshopId(9911));
}

#[tokio::test]
async fn visibility_only_update_does_not_reset_detail_ttl_for_single_or_batch_lookup() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();
    state
        .config_store
        .update(|cfg| cfg.search.cache_steam_results = false)
        .unwrap();

    let expired_at = Utc::now() - chrono::Duration::hours(2);
    for id in [9921, 9922] {
        let mut stale_cached = sample_item(
            id,
            &format!("Stale Download {id}"),
            100,
            Some(&format!("https://cdn.example.test/{id}_expired_v1.vpk")),
        );
        stale_cached.fetched_at = expired_at;
        state.repo.upsert_item(&stale_cached, false, true).unwrap();
    }

    // Search marks both 9921 and 9922 unlisted right now without refreshing their details.
    let now = Utc::now();
    let unlisted_search_outcomes: Vec<_> = [9921, 9922]
        .into_iter()
        .map(|id| {
            let mut obs_item = sample_item(id, &format!("Stale Download {id}"), 200, None);
            obs_item.fetched_at = now;
            WorkshopObservation {
                outcome: WorkshopFetchOutcome::Found { item: obs_item },
                listed: false,
                consumer_appid: Some(550),
            }
        })
        .collect();
    upstream.insert_search_page(
        steam_req(
            QUERY_TYPE_RANKED_BY_TEXT_SEARCH,
            MATCHING_FILE_TYPE_ITEMS,
            1,
            30,
            Some("MarkUnlistedOnly"),
            None,
            0,
        ),
        WorkshopQueryPage {
            total: 2,
            next_cursor: None,
            outcomes: unlisted_search_outcomes,
        },
    );

    let app = build_router(state.clone());
    let (_, _, search_resp) =
        search_with_mode(&app, "/api/v1/items/search?q=MarkUnlistedOnly", vec![]).await;
    assert!(search_resp.items.is_empty());

    let meta_9921 = state.repo.get_item(WorkshopId(9921)).unwrap().unwrap();
    assert_eq!(meta_9921.is_listed, Some(false));
    assert_eq!(meta_9921.last_verified_at, expired_at);
    assert!(meta_9921.visibility_verified_at >= now);

    // Upstream holds fresh download URLs for both unlisted items.
    for id in [9921, 9922] {
        upstream.insert_outcome(
            WorkshopId(id),
            WorkshopObservation {
                outcome: WorkshopFetchOutcome::Found {
                    item: sample_item(
                        id,
                        &format!("Stale Download {id}"),
                        200,
                        Some(&format!("https://cdn.example.test/{id}_fresh_v2.vpk")),
                    ),
                },
                listed: false,
                consumer_appid: Some(550),
            },
        );
    }

    // Single by-ID lookup sees expired detail TTL and calls GetDetails immediately!
    let (status_single, bytes_single) =
        json_request(&app, "GET", "/api/v1/items/9921", vec![], None).await;
    assert_eq!(status_single, StatusCode::OK);
    let single_item: WorkshopItemRecord = serde_json::from_slice(&bytes_single).unwrap();
    assert_eq!(
        single_item.file_url.as_deref(),
        Some("https://cdn.example.test/9921_fresh_v2.vpk")
    );
    assert_eq!(upstream.fetch_count(WorkshopId(9921)), 1);

    // Batch lookup also sees expired detail TTL and calls GetDetails immediately!
    let (status_batch, bytes_batch) = json_request(
        &app,
        "POST",
        "/api/v1/items/batch",
        vec![],
        Some(json!({ "ids": ["9922"], "forceRefresh": false })),
    )
    .await;
    assert_eq!(status_batch, StatusCode::OK);
    let batch_resp: BatchItemsResponse = serde_json::from_slice(&bytes_batch).unwrap();
    assert_eq!(batch_resp.items.len(), 1);
    assert_eq!(
        batch_resp.items[0].file_url.as_deref(),
        Some("https://cdn.example.test/9922_fresh_v2.vpk")
    );
    assert_eq!(upstream.fetch_count(WorkshopId(9922)), 1);
}

#[tokio::test]
async fn bare_collection_id_stays_searchable_after_indexing_and_title_only_numeric_skips_id_injection(
) {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    // Indexed collection 349900, prefix item 3499001, and prefix collection 3499002.
    let mut indexed_col = sample_item(349900, "Indexed Friday Collection", 300, None);
    indexed_col.file_type = WorkshopFileType::Collection;
    state.repo.upsert_item(&indexed_col, false, true).unwrap();

    let prefix_item = sample_item(3499001, "Prefix Item", 200, None);
    state.repo.upsert_item(&prefix_item, false, true).unwrap();

    let mut prefix_col = sample_item(3499002, "Prefix Collection Excluded", 250, None);
    prefix_col.file_type = WorkshopFileType::Collection;
    state.repo.upsert_item(&prefix_col, false, true).unwrap();

    // Unindexed item 610700 (title without number) vs indexed item 610701 (title with number).
    upstream.insert_item(sample_item(610700, "Silent Forest", 400, None));
    let titled_num = sample_item(610701, "Campaign 610700 Redux", 410, None);
    state.repo.upsert_item(&titled_num, false, true).unwrap();

    let app = build_router(state);

    // 1. Bare 6+ digit collection ID matches after indexing in both hybrid and local-only sorts,
    //    without widening numeric prefix search to prefix_col (3499002).
    assert_eq!(
        search_result_ids(&app, "q=349900").await,
        (2, vec![349900, 3499001])
    );
    assert_eq!(
        search_result_ids(&app, "q=349900&sort=title").await,
        (2, vec![349900, 3499001])
    );
    // Explicit fileType=item still excludes the collection!
    assert_eq!(
        search_result_ids(&app, "q=349900&fileType=item").await,
        (1, vec![3499001])
    );
    // Explicit tag mismatch still excludes the collection!
    assert_eq!(
        search_result_ids(&app, "q=349900&tag=Survivors").await,
        (0, vec![])
    );

    // 2. searchTarget=title skips bare-ID GetDetails injection while matching titles with the number.
    assert_eq!(
        search_result_ids(&app, "q=610700&searchTarget=title").await,
        (1, vec![610701])
    );
    assert_eq!(
        search_result_ids(&app, "q=610700&searchTarget=title&sort=title").await,
        (1, vec![610701])
    );
    assert_eq!(upstream.fetch_count(WorkshopId(610700)), 0);

    // 3. Direct Workshop URL lookup still resolves 610700 even with searchTarget=title!
    assert_eq!(
        search_result_ids(
            &app,
            "q=https%3A%2F%2Fsteamcommunity.com%2Fsharedfiles%2Ffiledetails%2F%3Fid%3D610700&searchTarget=title"
        )
        .await,
        (1, vec![610700])
    );
    assert_eq!(upstream.fetch_count(WorkshopId(610700)), 1);
}

#[tokio::test]
async fn backfill_advances_across_empty_pages_and_stops_on_terminal_or_stalled_cursors() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    // Page 1 at "*" has 0 outcomes but supplies a valid new continuation cursor.
    upstream.insert_query_page(
        QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
        MATCHING_FILE_TYPE_ITEMS,
        "*",
        WorkshopQueryPage {
            total: 1,
            next_cursor: Some("cursor_page_empty_gap".into()),
            outcomes: Vec::new(),
        },
    );
    // Page 2 supplies an item and then stalls by repeating the same cursor.
    upstream.insert_query_page(
        QUERY_TYPE_RANKED_BY_PUBLICATION_DATE,
        MATCHING_FILE_TYPE_ITEMS,
        "cursor_page_empty_gap",
        WorkshopQueryPage {
            total: 1,
            next_cursor: Some("cursor_page_empty_gap".into()),
            outcomes: vec![WorkshopFetchOutcome::Found {
                item: sample_item(7701, "After Empty Page", 500, None),
            }
            .into()],
        },
    );

    assert_eq!(state.sync.run_backfill_step_once().await.unwrap(), 0);
    let after_first = state.sync.status().await.unwrap();
    assert_eq!(
        after_first.backfill_cursor.as_deref(),
        Some("cursor_page_empty_gap")
    );
    assert_eq!(after_first.remote_workshop_total, Some(1));

    assert_eq!(state.sync.run_backfill_step_once().await.unwrap(), 1);
    let after_second = state.sync.status().await.unwrap();
    assert_eq!(after_second.backfill_cursor.as_deref(), Some("DONE"));
    assert_eq!(after_second.backfill_synced_total, 1);

    let app = build_router(state);
    assert_eq!(search_result_ids(&app, "").await, (1, vec![7701]));
}

#[test]
fn upgraded_v2_database_migrates_browse_expression_indexes_without_temp_btree_sort() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v2_upgrade.db");
    {
        let repo = ServerRepo::open(&path).unwrap();
        for id in 1..=5u64 {
            let mut item = sample_item(id, &format!("Browse {id}"), 100 + id as i64, None);
            item.subscriptions = (id % 2 == 0).then_some(id * 100);
            repo.upsert_item(&item, false, true).unwrap();
        }
    }
    // Simulate a v2 database that already had the legacy two-column idx_workshop_items_catalog_public.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP INDEX IF EXISTS idx_workshop_items_catalog_public_v2;
             DROP INDEX IF EXISTS idx_workshop_items_catalog_relevance_v1;
             CREATE INDEX idx_workshop_items_catalog_public
                 ON workshop_items(catalog_indexed, is_listed);",
        )
        .unwrap();
    }

    // Reopen to run migrate().
    let reopened = ServerRepo::open(&path).unwrap();
    assert_eq!(reopened.search(&Default::default(), true).unwrap().total, 5);
    drop(reopened);

    let conn = rusqlite::Connection::open(&path).unwrap();
    let legacy_exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_workshop_items_catalog_public')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!legacy_exists, "legacy 2-column index should be dropped");

    let explain = |order_by: &str| -> Vec<String> {
        let sql = format!(
            "EXPLAIN QUERY PLAN
             SELECT w.workshop_id, w.title, COALESCE(w.subscriptions, 0),
                    COALESCE(w.time_updated, 0), COALESCE(w.time_created, 0),
                    w.status = 'active', w.is_admin_override
             FROM workshop_items w
             WHERE w.catalog_indexed = 1 AND w.is_listed = 1 AND w.file_type = 'item'
             ORDER BY {order_by}
             LIMIT 30 OFFSET 0"
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        stmt.query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };

    let subs_plan = explain("COALESCE(w.subscriptions, 0) DESC, w.workshop_id ASC").join(" | ");
    assert!(
        subs_plan.contains("idx_workshop_items_catalog_public_v2"),
        "expected v2 browse index in plan, got: {subs_plan}"
    );
    assert!(
        !subs_plan.contains("TEMP B-TREE"),
        "subscription browse must not sort with a temp B-tree: {subs_plan}"
    );

    let rel_plan = explain(
        "COALESCE(w.subscriptions, 0) DESC, COALESCE(w.time_updated, 0) DESC, w.workshop_id ASC",
    )
    .join(" | ");
    assert!(
        rel_plan.contains("idx_workshop_items_catalog_relevance_v1"),
        "expected relevance browse index in plan, got: {rel_plan}"
    );
    assert!(
        !rel_plan.contains("TEMP B-TREE"),
        "default relevance browse must not sort with a temp B-tree: {rel_plan}"
    );
}

#[tokio::test]
async fn search_preserves_terms_beyond_sixteen_across_hybrid_and_local_fallback() {
    let upstream = Arc::new(MockWorkshopUpstream::new());
    let state =
        ServerState::in_memory_with_upstream("test-admin-password", upstream.clone()).unwrap();

    let words: Vec<String> = (1..=19).map(|i| format!("term{i:02}")).collect();
    let partial_16 = words[..16].join(" ");
    let mut all_20_words = words;
    all_20_words.push("finalkeyword".to_string());
    let full_20 = all_20_words.join(" ");

    let mut miss_tail = sample_item(9951, &partial_16, 100, None);
    miss_tail.description = Some(partial_16);
    state.repo.upsert_item(&miss_tail, false, true).unwrap();

    let mut match_all = sample_item(9952, &full_20, 200, None);
    match_all.description = None;
    state.repo.upsert_item(&match_all, false, true).unwrap();

    let app = build_router(state);
    let encoded_q = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("q", &full_20)
        .finish();

    // Hybrid search (empty Steam results -> local supplementation) excludes 9951 and keeps 9952.
    assert_eq!(search_result_ids(&app, &encoded_q).await, (1, vec![9952]));

    // Local fallback when Steam fails also excludes 9951 and keeps 9952.
    upstream.set_fail_searches(true);
    let (_, mode, fb_resp) = search_with_mode(
        &app,
        &format!("/api/v1/items/search?{encoded_q}&language=zh-CN"),
        vec![],
    )
    .await;
    assert_eq!(mode.as_deref(), Some("local-fallback"));
    assert_eq!(
        fb_resp
            .items
            .iter()
            .map(|i| i.workshop_id.0)
            .collect::<Vec<_>>(),
        vec![9952]
    );
}

#[tokio::test]
async fn filter_disabled_returns_public_disabled_info_and_503_on_predict() {
    let state = ServerState::in_memory("test-admin-password").unwrap();
    let app = build_router(state);

    let (status, bytes) = json_request(&app, "GET", "/api/v1/filter/info", vec![], None).await;
    assert_eq!(status, StatusCode::OK);
    let info: l4a_protocol::CatalogFilterPublicInfo = serde_json::from_slice(&bytes).unwrap();
    assert!(!info.enabled);
    assert!(!info.ready);
    assert_eq!(info.max_images, 2);

    let (pred_status, pred_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "title": "Any Addon" })),
    )
    .await;
    assert_eq!(pred_status, StatusCode::SERVICE_UNAVAILABLE);
    let err_val: serde_json::Value = serde_json::from_slice(&pred_bytes).unwrap();
    assert!(err_val["error"].as_str().is_some_and(|s| !s.is_empty()));

    let (ov_status, ov_bytes) = json_request(
        &app,
        "GET",
        "/api/v1/admin/overview",
        vec![("x-admin-token", "test-admin-password")],
        None,
    )
    .await;
    assert_eq!(ov_status, StatusCode::OK);
    let overview: CatalogOverviewResponse = serde_json::from_slice(&ov_bytes).unwrap();
    assert!(!overview.filter.config.enabled);
    assert!(!overview.filter.ready);
}

#[tokio::test]
async fn filter_upstream_http_predict_calibration_cache_and_toml_persistence() {
    use axum::routing::{get, post};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let predict_calls = Arc::new(AtomicUsize::new(0));
    let captured_requests: Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));

    let calls_cl = predict_calls.clone();
    let reqs_cl = captured_requests.clone();
    let mock_upstream = axum::Router::new()
        .route(
            "/health",
            get(|| async {
                axum::Json(json!({
                    "status": "ok",
                    "model": "MockLeft4Filters",
                    "version": "2.4.0"
                }))
            }),
        )
        .route(
            "/predict",
            post(move |axum::Json(payload): axum::Json<serde_json::Value>| {
                let calls = calls_cl.clone();
                let reqs = reqs_cl.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    reqs.lock().unwrap().push(payload);
                    axum::Json(json!({
                        "prob": 0.88,
                        "goreProb": 0.82,
                        "nsfwProb": 0.45,
                        "textProb": 0.75,
                        "textGoreProb": 0.70,
                        "textNsfwProb": 0.35,
                        "titleProb": 0.72,
                        "descriptionProb": 0.40
                    }))
                }
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_addr = listener.local_addr().unwrap();
    let upstream_url = format!("http://{mock_addr}");
    let mock_task = tokio::spawn(async move {
        let _ = axum::serve(listener, mock_upstream).await;
    });

    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("catalog.toml");
    let mut initial_cfg = CatalogConfig::default();
    initial_cfg.admin.password = "filter-secret-pass".to_string();
    initial_cfg.server.db_path = dir.path().join("catalog.db");
    initial_cfg.save_to(&config_path).unwrap();

    let (state, _) = ServerState::open_config_file(&config_path).unwrap();
    let app = build_router(state);
    let admin_hdr = vec![("x-admin-token", "filter-secret-pass")];

    // 1. Enable UpstreamHttp via PATCH /api/v1/admin/config/filter and verify persistence to catalog.toml.
    let enable_dto = l4a_protocol::CatalogFilterConfigDto {
        enabled: true,
        backend: l4a_protocol::CatalogFilterBackendKind::UpstreamHttp,
        deploy_dir: None,
        python_bin: None,
        upstream_url: Some(upstream_url.clone()),
        cache_ttl_secs: 3600,
        params: l4a_protocol::FilterPresetId::Recommended.default_params(),
    };
    let (patch_status, patch_bytes) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr.clone(),
        Some(serde_json::to_value(&enable_dto).unwrap()),
    )
    .await;
    assert_eq!(patch_status, StatusCode::OK);
    let status_resp: l4a_protocol::CatalogFilterStatus =
        serde_json::from_slice(&patch_bytes).unwrap();
    assert!(status_resp.ready);
    assert_eq!(status_resp.model_name.as_deref(), Some("MockLeft4Filters"));
    assert_eq!(status_resp.model_version.as_deref(), Some("2.4.0"));

    let persisted_raw = std::fs::read_to_string(&config_path).unwrap();
    let persisted_cfg: CatalogConfig = toml::from_str(&persisted_raw).unwrap();
    assert!(persisted_cfg.filter.enabled);
    assert_eq!(
        persisted_cfg.filter.backend,
        l4a_protocol::CatalogFilterBackendKind::UpstreamHttp
    );
    assert_eq!(
        persisted_cfg.filter.upstream_url.as_deref(),
        Some(upstream_url.as_str())
    );

    // 2. Public GET /api/v1/filter/info reflects ready status and model metadata.
    let (info_code, info_bytes) =
        json_request(&app, "GET", "/api/v1/filter/info", vec![], None).await;
    assert_eq!(info_code, StatusCode::OK);
    let info: l4a_protocol::CatalogFilterPublicInfo = serde_json::from_slice(&info_bytes).unwrap();
    assert!(info.enabled);
    assert!(info.ready);
    assert_eq!(info.model_name.as_deref(), Some("MockLeft4Filters"));

    // 3. Public POST /api/v1/filter/predict ignores client-injected parameter fields and caches identical inputs.
    let req_body = json!({
        "title": "Extreme Gore Pack",
        "description": "Bloody textures",
        "tags": ["Textures"],
        "goreWeight": 0.0,
        "params": { "goreWeight": 0.0, "nsfwWeight": 0.0 }
    });
    let (p1_code, p1_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(req_body.clone()),
    )
    .await;
    assert_eq!(p1_code, StatusCode::OK);
    let p1: l4a_protocol::FilterProbabilities = serde_json::from_slice(&p1_bytes).unwrap();
    assert!((p1.prob - 0.88).abs() < 1e-5);
    assert!((p1.gore_prob - 0.82).abs() < 1e-5);
    assert!((p1.nsfw_prob - 0.45).abs() < 1e-5);
    assert_eq!(predict_calls.load(Ordering::SeqCst), 1);

    // Second identical request hits TTL cache without calling upstream again.
    let (p2_code, p2_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(req_body.clone()),
    )
    .await;
    assert_eq!(p2_code, StatusCode::OK);
    let p2: l4a_protocol::FilterProbabilities = serde_json::from_slice(&p2_bytes).unwrap();
    assert_eq!(p1, p2);
    assert_eq!(predict_calls.load(Ordering::SeqCst), 1);

    let (st_code, st_bytes) = json_request(
        &app,
        "GET",
        "/api/v1/admin/filter/status",
        admin_hdr.clone(),
        None,
    )
    .await;
    assert_eq!(st_code, StatusCode::OK);
    let st: l4a_protocol::CatalogFilterStatus = serde_json::from_slice(&st_bytes).unwrap();
    assert_eq!(st.total_requests, 2);
    assert_eq!(st.cache_hits, 1);

    // 4. Switch server calibration to NsfwOnly (goreWeight = 0.0) via admin PATCH:
    //    invalidates cache, zeroes out goreProb, and calibrates overall prob to nsfwProb (0.45).
    let nsfw_only_dto = l4a_protocol::CatalogFilterConfigDto {
        params: l4a_protocol::FilterPresetId::NsfwOnly.default_params(),
        ..enable_dto
    };
    let (cal_code, _) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr.clone(),
        Some(serde_json::to_value(&nsfw_only_dto).unwrap()),
    )
    .await;
    assert_eq!(cal_code, StatusCode::OK);

    let reloaded_cfg: CatalogConfig =
        toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(reloaded_cfg.filter.gore_weight, 0.0);
    assert_eq!(reloaded_cfg.filter.nsfw_weight, 1.0);

    let (p3_code, p3_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(req_body),
    )
    .await;
    assert_eq!(p3_code, StatusCode::OK);
    let p3: l4a_protocol::FilterProbabilities = serde_json::from_slice(&p3_bytes).unwrap();
    assert_eq!(predict_calls.load(Ordering::SeqCst), 2);
    assert_eq!(p3.gore_prob, 0.0);
    assert!((p3.nsfw_prob - 0.45).abs() < 1e-5);
    assert!((p3.prob - 0.45).abs() < 1e-5);
    assert_eq!(p3.text_gore_prob, Some(0.0));
    assert_eq!(p3.text_prob, Some(0.35));

    // 5. Admin restart and sandbox test endpoints + audit logs.
    let (rst_code, _) = json_request(
        &app,
        "POST",
        "/api/v1/admin/filter/restart",
        admin_hdr.clone(),
        None,
    )
    .await;
    assert_eq!(rst_code, StatusCode::OK);

    let (test_code, test_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/admin/filter/test",
        admin_hdr.clone(),
        Some(json!({ "title": "Admin Sandbox Check" })),
    )
    .await;
    assert_eq!(test_code, StatusCode::OK);
    let test_probs: l4a_protocol::FilterProbabilities =
        serde_json::from_slice(&test_bytes).unwrap();
    assert_eq!(test_probs.gore_prob, 0.0);

    let (audit_code, audit_bytes) =
        json_request(&app, "GET", "/api/v1/admin/audit?limit=20", admin_hdr, None).await;
    assert_eq!(audit_code, StatusCode::OK);
    let audits: Vec<l4a_protocol::AuditLogEntry> = serde_json::from_slice(&audit_bytes).unwrap();
    let actions: Vec<&str> = audits.iter().map(|a| a.action.as_str()).collect();
    assert!(actions.contains(&"filter_config_update"));
    assert!(actions.contains(&"filter_restart"));

    mock_task.abort();
}

#[tokio::test]
async fn filter_workshop_id_lookup_populates_public_items_and_rejects_ssrf_and_restricted_items() {
    use axum::routing::{get, post};
    use base64::Engine;

    let sample_png: &'static [u8] =
        b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x10\x00\x00\x00\x10\x08\x02\x00\x00\x00\x00\x00\x00\x00";
    let captured_requests: Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let reqs_cl = captured_requests.clone();

    let mock_srv = axum::Router::new()
        .route(
            "/health",
            get(|| async {
                axum::Json(json!({ "status": "ok", "model": "MockModel", "version": "1.0" }))
            }),
        )
        .route(
            "/fixture.png",
            get(move || async move { ([(header::CONTENT_TYPE, "image/png")], sample_png) }),
        )
        .route(
            "/evil-redirect",
            get(|| async {
                (
                    StatusCode::FOUND,
                    [(header::LOCATION, "https://evil.example.com/ssrf.png")],
                    "",
                )
            }),
        )
        .route(
            "/predict",
            post(move |axum::Json(payload): axum::Json<serde_json::Value>| {
                let reqs = reqs_cl.clone();
                async move {
                    reqs.lock().unwrap().push(payload);
                    axum::Json(json!({
                        "prob": 0.64,
                        "goreProb": 0.12,
                        "nsfwProb": 0.61,
                        "textProb": 0.22,
                        "imageProb": 0.58
                    }))
                }
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mock_task = tokio::spawn(async move {
        let _ = axum::serve(listener, mock_srv).await;
    });

    let mut cfg = CatalogConfig::default();
    cfg.admin.password = "test-admin-password".to_string();
    cfg.filter.enabled = true;
    cfg.filter.backend = l4a_protocol::CatalogFilterBackendKind::UpstreamHttp;
    cfg.filter.upstream_url = Some(format!("http://{addr}"));
    cfg.filter.max_images = 2;

    let store = Arc::new(CatalogConfigStore::new(None, cfg));
    let repo = Arc::new(ServerRepo::open_in_memory().unwrap());
    let state = ServerState::with_components(store, repo, Arc::new(MockWorkshopUpstream::new()));

    // Insert public active item 55001, private item 55002, and unlisted item 55003.
    let mut public_item = sample_item(55001, "Public Anime Survivor", 500, None);
    public_item.description = Some("Custom survivor model description".to_string());
    public_item.tags = vec!["Survivors".to_string(), "Models".to_string()];
    public_item.preview_url = Some(format!("http://{addr}/fixture.png"));
    public_item.previews = vec![];
    state.repo.upsert_item(&public_item, false, true).unwrap();

    let mut private_item = sample_item(55002, "Hidden Private Title", 500, None);
    private_item.status = WorkshopItemStatus::PrivateOrUnavailable;
    private_item.description = Some("Top secret private description".to_string());
    state.repo.upsert_item(&private_item, false, true).unwrap();

    let mut unlisted_item = sample_item(55003, "Unlisted Secret Item", 500, None);
    unlisted_item.description = Some("Unlisted secret description".to_string());
    state
        .repo
        .apply_upstream_outcomes_with_policy(
            &[WorkshopObservation {
                outcome: WorkshopFetchOutcome::Found {
                    item: unlisted_item,
                },
                listed: false,
                consumer_appid: Some(550),
            }],
            l4a_server::db::CatalogIndexPolicy::CacheOnly,
        )
        .unwrap();

    let app = build_router(state.clone());

    // 1. Default SSRF enforcement (production mode: loopback HTTP and arbitrary hosts rejected with 400).
    for disallowed_url in [
        "https://evil.example.com/a.png",
        "http://169.254.169.254/latest/meta-data/",
        "http://images.steamusercontent.com:8080/ugc/123/custom_http_port.jpg",
        "https://images.steamusercontent.com:8443/ugc/123/custom_port.jpg",
        "https://cloud-3.steampowered.com/non-ugc/not_allowed.jpg",
        &format!("http://{addr}/fixture.png"),
    ] {
        let (code, _) = json_request(
            &app,
            "POST",
            "/api/v1/filter/predict",
            vec![],
            Some(json!({
                "title": "SSRF Probe",
                "imageUrls": [disallowed_url]
            })),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::BAD_REQUEST,
            "expected 400 Bad Request for disallowed imageUrl: {disallowed_url}"
        );
    }
    assert!(captured_requests.lock().unwrap().is_empty());

    // 2. Enable test loopback flag to test local image fetch & cross-host redirect SSRF protection.
    state.filter.set_allow_loopback_image_urls_for_testing(true);

    let (redir_code, _) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({
            "title": "Redirect SSRF Probe",
            "imageUrls": [format!("http://{addr}/evil-redirect")]
        })),
    )
    .await;
    assert_eq!(redir_code, StatusCode::BAD_REQUEST);

    // 3. Public active `workshopId` populates title, description, tags, and preview image bytes.
    let (ok_code, ok_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "workshopId": "55001" })),
    )
    .await;
    assert_eq!(ok_code, StatusCode::OK);
    let ok_probs: l4a_protocol::FilterProbabilities = serde_json::from_slice(&ok_bytes).unwrap();
    assert!((ok_probs.prob - 0.64).abs() < 1e-5);

    {
        let reqs = captured_requests.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0]["title"], "Public Anime Survivor");
        assert_eq!(reqs[0]["description"], "Custom survivor model description");
        assert_eq!(reqs[0]["tags"], json!(["Survivors", "Models"]));
        assert!(reqs[0].get("imagesBase64").is_none());
        let imgs = reqs[0]["images_base64"].as_array().unwrap();
        assert_eq!(imgs.len(), 1);
        let expected_b64 = base64::engine::general_purpose::STANDARD.encode(sample_png);
        assert_eq!(imgs[0].as_str(), Some(expected_b64.as_str()));
    }

    // 4. Private (55002), unlisted (55003), and missing (55999) items return identical 404 Not Found
    //    and never leak restricted metadata into the filter backend.
    for restricted_id in ["55002", "55003", "55999"] {
        let (code, _) = json_request(
            &app,
            "POST",
            "/api/v1/filter/predict",
            vec![],
            Some(json!({ "workshopId": restricted_id })),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "workshopId={restricted_id}");
    }
    assert_eq!(captured_requests.lock().unwrap().len(), 1);

    // Even if caller supplies `title` alongside a private `workshopId`, private description is never read.
    let (partial_code, _) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "workshopId": "55002", "title": "Caller Title Only" })),
    )
    .await;
    assert_eq!(partial_code, StatusCode::OK);
    {
        let reqs = captured_requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[1]["title"], "Caller Title Only");
        assert_eq!(reqs[1]["description"], "");
    }

    mock_task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn filter_managed_python_runner_spawns_live_reconfigures_and_terminates_cleanly() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let deploy_dir = dir.path().join("deploy");
    let py_shim_dir = dir.path().join("pyshims");
    std::fs::create_dir_all(&deploy_dir).unwrap();
    std::fs::create_dir_all(py_shim_dir.join("PIL")).unwrap();

    // Minimal numpy + PIL shims sufficient for `FILTER_RUNNER_PY` SDK fallback mode.
    std::fs::write(py_shim_dir.join("numpy.py"), "float32 = float\n").unwrap();
    std::fs::write(
        py_shim_dir.join("PIL").join("Image.py"),
        "MAX_IMAGE_PIXELS = None\n",
    )
    .unwrap();
    std::fs::write(py_shim_dir.join("PIL").join("__init__.py"), "").unwrap();
    std::fs::write(
        deploy_dir.join("l4filter.py"),
        r#"class Left4Filter:
    def __init__(self, deploy_dir, num_threads=4):
        self.config = {"model_name": "ManagedMockONNX", "version": "3.1.0"}
        self.num_threads = num_threads

    def predict(self, title="", description="", tags=None, image=None, images=None):
        return {"prob": 0.80, "gore_prob": 0.75, "nsfw_prob": 0.30}
"#,
    )
    .unwrap();

    let py_wrapper = dir.path().join("python_wrapper.sh");
    std::fs::write(
        &py_wrapper,
        format!(
            "#!/usr/bin/env bash\nexport PYTHONPATH=\"{}:${{PYTHONPATH:-}}\"\nexec python3 \"$@\"\n",
            py_shim_dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&py_wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();

    let state = ServerState::in_memory("managed-secret").unwrap();
    let app = build_router(state);
    let admin_hdr = vec![("x-admin-token", "managed-secret")];

    // 1. Start ManagedPython runner via admin config PATCH.
    let cfg_v1 = l4a_protocol::CatalogFilterConfigDto {
        enabled: true,
        backend: l4a_protocol::CatalogFilterBackendKind::ManagedPython,
        deploy_dir: Some(deploy_dir.display().to_string()),
        python_bin: Some(py_wrapper.display().to_string()),
        upstream_url: None,
        cache_ttl_secs: 3600,
        params: l4a_protocol::FilterPresetId::Recommended.default_params(),
    };
    let (code1, bytes1) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr.clone(),
        Some(serde_json::to_value(&cfg_v1).unwrap()),
    )
    .await;
    assert_eq!(code1, StatusCode::OK);
    let st1: l4a_protocol::CatalogFilterStatus = serde_json::from_slice(&bytes1).unwrap();
    assert!(st1.ready, "last_error={:?}", st1.last_error);
    let pid1 = st1
        .running_pid
        .expect("ManagedPython should report running_pid");
    assert_eq!(st1.model_name.as_deref(), Some("ManagedMockONNX"));
    assert_eq!(st1.model_version.as_deref(), Some("3.1.0"));

    // 2. Run prediction (unadjusted Recommended preset).
    let (p1_code, p1_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "title": "Managed Test" })),
    )
    .await;
    assert_eq!(p1_code, StatusCode::OK);
    let p1: l4a_protocol::FilterProbabilities = serde_json::from_slice(&p1_bytes).unwrap();
    assert!((p1.gore_prob - 0.75).abs() < 1e-5);
    assert!((p1.nsfw_prob - 0.30).abs() < 1e-5);

    // 3. Reconfigure to NsfwOnly (goreWeight = 0) with same deploy_dir & python_bin:
    //    pushes live `POST /configure` with bearer token without restarting child PID,
    //    and ensures ManagedPython response is NOT double-adjusted!
    let cfg_v2 = l4a_protocol::CatalogFilterConfigDto {
        params: l4a_protocol::FilterPresetId::NsfwOnly.default_params(),
        ..cfg_v1.clone()
    };
    let (code2, bytes2) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr.clone(),
        Some(serde_json::to_value(&cfg_v2).unwrap()),
    )
    .await;
    assert_eq!(code2, StatusCode::OK);
    let st2: l4a_protocol::CatalogFilterStatus = serde_json::from_slice(&bytes2).unwrap();
    assert!(st2.ready);
    assert_eq!(
        st2.running_pid,
        Some(pid1),
        "live /configure should preserve running child PID"
    );

    let (p2_code, p2_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "title": "Managed Test" })),
    )
    .await;
    assert_eq!(p2_code, StatusCode::OK);
    let p2: l4a_protocol::FilterProbabilities = serde_json::from_slice(&p2_bytes).unwrap();
    assert_eq!(p2.gore_prob, 0.0);
    assert!((p2.nsfw_prob - 0.30).abs() < 1e-5);
    assert!((p2.prob - 0.30).abs() < 1e-5);

    // 4. Disable filter and verify running_pid is cleared.
    let cfg_off = l4a_protocol::CatalogFilterConfigDto {
        enabled: false,
        ..cfg_v2
    };
    let (code3, bytes3) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr,
        Some(serde_json::to_value(&cfg_off).unwrap()),
    )
    .await;
    assert_eq!(code3, StatusCode::OK);
    let st3: l4a_protocol::CatalogFilterStatus = serde_json::from_slice(&bytes3).unwrap();
    assert!(!st3.ready);
    assert_eq!(st3.running_pid, None);
}

#[tokio::test]
async fn filter_hardening_oversized_dimensions_input_bounds_sanitized_503_and_legacy_steam_cdn_upgrade(
) {
    use axum::body::Body;
    use axum::routing::{get, post};
    use base64::Engine;
    use tower::ServiceExt;

    // 1. Early disabled middleware rejects `/api/v1/filter/predict` BEFORE buffering body!
    let disabled_state = ServerState::in_memory("secret-admin-pw").unwrap();
    let disabled_app = build_router(disabled_state);
    let body_read_attempted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag_cl = body_read_attempted.clone();
    let unbuffered_stream = futures_util::stream::poll_fn(move |_| {
        flag_cl.store(true, std::sync::atomic::Ordering::SeqCst);
        std::task::Poll::Ready(Some(Ok::<_, std::io::Error>(bytes::Bytes::from_static(
            b"{\"title\":\"huge\"}",
        ))))
    });
    let early_req = Request::builder()
        .method("POST")
        .uri("/api/v1/filter/predict")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from_stream(unbuffered_stream))
        .unwrap();
    let early_resp = disabled_app.clone().oneshot(early_req).await.unwrap();
    assert_eq!(early_resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        !body_read_attempted.load(std::sync::atomic::Ordering::SeqCst),
        "disabled route middleware must reject before reading request body"
    );
    let early_json: serde_json::Value = serde_json::from_slice(
        &http_body_util::BodyExt::collect(early_resp.into_body())
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert_eq!(early_json["error"], "内容过滤服务未启用");

    // 2. Legacy L4D2 Steam CDN HTTP URLs upgrade cleanly to HTTPS while preserving SSRF rejections.
    for (legacy_http, expected_https) in [
        (
            "http://cloud-3.steamusercontent.com/ugc/8829102/preview_legacy.jpg",
            "https://cloud-3.steamusercontent.com/ugc/8829102/preview_legacy.jpg",
        ),
        (
            "http://steamuserimages-a.akamaihd.net/ugc/991234/shot.png",
            "https://steamuserimages-a.akamaihd.net/ugc/991234/shot.png",
        ),
        (
            "http://media.steampowered.com/steamcommunity/public/images/items/550/a.jpg",
            "https://media.steampowered.com/steamcommunity/public/images/items/550/a.jpg",
        ),
        (
            "http://cloud-4.steampowered.com/ugc/112233/old_l4d2.jpg",
            "https://cloud-4.steampowered.com/ugc/112233/old_l4d2.jpg",
        ),
    ] {
        let upgraded = l4a_server::filter::validate_allowed_image_url(legacy_http, false).unwrap();
        assert_eq!(upgraded.as_str(), expected_https);
    }

    // 3. Reject cloud metadata IP `169.254.169.254` in `PATCH /api/v1/admin/config/filter`.
    let admin_hdr = vec![("x-admin-token", "secret-admin-pw")];
    let bad_upstream_dto = l4a_protocol::CatalogFilterConfigDto {
        enabled: true,
        backend: l4a_protocol::CatalogFilterBackendKind::UpstreamHttp,
        deploy_dir: None,
        python_bin: None,
        upstream_url: Some("http://169.254.169.254/latest/meta-data".to_string()),
        cache_ttl_secs: 60,
        params: l4a_protocol::FilterModelParams::default(),
    };
    let (meta_code, _) = json_request(
        &disabled_app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr.clone(),
        Some(serde_json::to_value(&bad_upstream_dto).unwrap()),
    )
    .await;
    assert_eq!(meta_code, StatusCode::BAD_REQUEST);

    // 4. Sanitized public `503` vs detailed admin `503` when backend is broken/unreachable.
    let secret_internal_path = "/srv/internal-secret-models/missing-l4filter-dir";
    let broken_managed_dto = l4a_protocol::CatalogFilterConfigDto {
        enabled: true,
        backend: l4a_protocol::CatalogFilterBackendKind::ManagedPython,
        deploy_dir: Some(secret_internal_path.to_string()),
        python_bin: None,
        upstream_url: None,
        cache_ttl_secs: 60,
        params: l4a_protocol::FilterModelParams::default(),
    };
    let (cfg_code, _) = json_request(
        &disabled_app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr.clone(),
        Some(serde_json::to_value(&broken_managed_dto).unwrap()),
    )
    .await;
    assert_eq!(cfg_code, StatusCode::OK);

    let (pub_503_code, pub_503_bytes) = json_request(
        &disabled_app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "title": "Probe Public Error" })),
    )
    .await;
    assert_eq!(pub_503_code, StatusCode::SERVICE_UNAVAILABLE);
    let pub_err: serde_json::Value = serde_json::from_slice(&pub_503_bytes).unwrap();
    assert_eq!(pub_err["error"], "内容过滤服务暂不可用");
    assert!(!pub_err["error"]
        .as_str()
        .unwrap()
        .contains(secret_internal_path));

    let (adm_503_code, adm_503_bytes) = json_request(
        &disabled_app,
        "POST",
        "/api/v1/admin/filter/test",
        admin_hdr.clone(),
        Some(json!({ "title": "Probe Admin Error" })),
    )
    .await;
    assert_eq!(adm_503_code, StatusCode::SERVICE_UNAVAILABLE);
    let adm_err: serde_json::Value = serde_json::from_slice(&adm_503_bytes).unwrap();
    assert!(
        adm_err["error"]
            .as_str()
            .unwrap()
            .contains(secret_internal_path),
        "admin test endpoint should retain diagnostic detail, got: {:?}",
        adm_err
    );

    // 5. Start a healthy mock upstream to test oversized image dimension bomb rejection (400),
    //    strict input bounds (400), and early `imagesBase64` slicing before decoding.
    let predict_hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits_cl = predict_hits.clone();
    let mock_upstream = axum::Router::new()
        .route(
            "/health",
            get(|| async {
                axum::Json(json!({ "status": "ok", "model": "Mock", "version": "1.0" }))
            }),
        )
        .route(
            "/predict",
            post(move |axum::Json(payload): axum::Json<serde_json::Value>| {
                let hits = hits_cl.clone();
                async move {
                    hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let count = payload["images_base64"]
                        .as_array()
                        .map(|a| a.len())
                        .unwrap_or(0);
                    axum::Json(json!({
                        "prob": 0.25,
                        "goreProb": 0.10,
                        "nsfwProb": 0.20,
                        "textProb": 0.15,
                        "imageProb": if count > 0 { Some(0.25) } else { None }
                    }))
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mock_task = tokio::spawn(async move {
        let _ = axum::serve(listener, mock_upstream).await;
    });

    let params_max1 = l4a_protocol::FilterModelParams {
        max_images: 1,
        ..Default::default()
    };
    let healthy_dto = l4a_protocol::CatalogFilterConfigDto {
        enabled: true,
        backend: l4a_protocol::CatalogFilterBackendKind::UpstreamHttp,
        deploy_dir: None,
        python_bin: None,
        upstream_url: Some(format!("http://{addr}")),
        cache_ttl_secs: 0,
        params: params_max1,
    };
    let (up_code, _) = json_request(
        &disabled_app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr,
        Some(serde_json::to_value(&healthy_dto).unwrap()),
    )
    .await;
    assert_eq!(up_code, StatusCode::OK);

    // Oversized 8192x8192 PNG bomb -> 400 Bad Request without hitting backend `/predict`.
    let make_png = |w: u32, h: u32| -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
        v
    };
    let bomb_b64 = base64::engine::general_purpose::STANDARD.encode(make_png(8192, 8192));
    let (bomb_code, _) = json_request(
        &disabled_app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "title": "Bomb Test", "imageBase64": bomb_b64 })),
    )
    .await;
    assert_eq!(bomb_code, StatusCode::BAD_REQUEST);
    assert_eq!(predict_hits.load(std::sync::atomic::Ordering::SeqCst), 0);

    // Oversized title (> 4096B), description (> 65536B), and tags (> 64) -> 400 Bad Request.
    for invalid_payload in [
        json!({ "title": "x".repeat(4097) }),
        json!({ "description": "y".repeat(65_537) }),
        json!({ "tags": (0..65).map(|i| format!("t{i}")).collect::<Vec<_>>() }),
        json!({ "tags": ["z".repeat(257)] }),
    ] {
        let (code, _) = json_request(
            &disabled_app,
            "POST",
            "/api/v1/filter/predict",
            vec![],
            Some(invalid_payload),
        )
        .await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
    }
    assert_eq!(predict_hits.load(std::sync::atomic::Ordering::SeqCst), 0);

    // Early slicing of `imagesBase64` to `max_images = 1` BEFORE base64 decode:
    // second item is invalid base64, yet ignored because slice happens upfront!
    let valid_b64 = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(make_png(32, 32))
    );
    let (slice_code, _) = json_request(
        &disabled_app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({
            "title": "Early Slice Check",
            "imagesBase64": [valid_b64, "%%%corrupt-unused-second-image%%%"]
        })),
    )
    .await;
    assert_eq!(slice_code, StatusCode::OK);
    assert_eq!(predict_hits.load(std::sync::atomic::Ordering::SeqCst), 1);

    mock_task.abort();
}

#[tokio::test]
async fn filter_upstream_unimodal_backfill_and_fail_open_db_image_cache_guard() {
    use axum::routing::{get, post};
    use base64::Engine;
    use std::sync::atomic::{AtomicBool, Ordering};

    let serve_image_ok = Arc::new(AtomicBool::new(false));
    let img_flag = serve_image_ok.clone();
    let sample_png: &'static [u8] =
        b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x10\x00\x00\x00\x10\x08\x02\x00\x00\x00\x00\x00\x00\x00";

    // Vanilla `deploy/server.py` mock: returns ONLY `prob`, `gore_prob`, `nsfw_prob` (no unimodal fields).
    let vanilla_srv = axum::Router::new()
        .route(
            "/health",
            get(|| async { axum::Json(json!({ "status": "ok" })) }),
        )
        .route(
            "/flaky-preview.png",
            get(move || {
                let flag = img_flag.clone();
                async move {
                    if flag.load(Ordering::SeqCst) {
                        (
                            StatusCode::OK,
                            [(header::CONTENT_TYPE, "image/png")],
                            sample_png,
                        )
                    } else {
                        (
                            StatusCode::SERVICE_UNAVAILABLE,
                            [(header::CONTENT_TYPE, "text/plain")],
                            &b"cdn error"[..],
                        )
                    }
                }
            }),
        )
        .route(
            "/predict",
            post(
                |axum::Json(payload): axum::Json<serde_json::Value>| async move {
                    let has_txt = !payload["title"].as_str().unwrap_or_default().is_empty()
                        || !payload["description"]
                            .as_str()
                            .unwrap_or_default()
                            .is_empty();
                    let has_img = payload["images_base64"]
                        .as_array()
                        .is_some_and(|arr| !arr.is_empty());
                    match (has_txt, has_img) {
                        (true, true) => axum::Json(json!({
                            "prob": 0.90,
                            "gore_prob": 0.15,
                            "nsfw_prob": 0.88
                        })),
                        (true, false) => axum::Json(json!({
                            "prob": 0.20,
                            "gore_prob": 0.05,
                            "nsfw_prob": 0.18
                        })),
                        (false, true) => axum::Json(json!({
                            "prob": 0.86,
                            "gore_prob": 0.12,
                            "nsfw_prob": 0.85
                        })),
                        (false, false) => axum::Json(json!({
                            "prob": 0.0,
                            "gore_prob": 0.0,
                            "nsfw_prob": 0.0
                        })),
                    }
                },
            ),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv_task = tokio::spawn(async move {
        let _ = axum::serve(listener, vanilla_srv).await;
    });

    let mut cfg = CatalogConfig::default();
    cfg.admin.password = "admin-pw".to_string();
    cfg.filter.enabled = true;
    cfg.filter.backend = l4a_protocol::CatalogFilterBackendKind::UpstreamHttp;
    cfg.filter.upstream_url = Some(format!("http://{addr}"));
    cfg.filter.cache_ttl_secs = 3600;
    let store = Arc::new(CatalogConfigStore::new(None, cfg));
    let repo = Arc::new(ServerRepo::open_in_memory().unwrap());
    let state = ServerState::with_components(store, repo, Arc::new(MockWorkshopUpstream::new()));
    state.filter.set_allow_loopback_image_urls_for_testing(true);

    let mut item = sample_item(77001, "Neutral Addon Title", 600, None);
    item.description = Some("Simple survivor skin".to_string());
    item.preview_url = Some(format!("http://{addr}/flaky-preview.png"));
    state.repo.upsert_item(&item, false, true).unwrap();

    let app = build_router(state.clone());

    // 1. First call while DB preview URL returns 503: degrades to text-only (0.20) AND skips caching!
    let (c1, b1) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "workshopId": "77001" })),
    )
    .await;
    assert_eq!(c1, StatusCode::OK);
    let p1: l4a_protocol::FilterProbabilities = serde_json::from_slice(&b1).unwrap();
    assert!((p1.prob - 0.20).abs() < 1e-5);
    assert_eq!(p1.image_prob, None);

    // 2. Preview CDN recovers: next call MUST NOT serve stale text-only cache; it fetches the image AND
    //    backfills both `textProb` (0.20) and `imageProb` (0.86) from vanilla upstream!
    serve_image_ok.store(true, Ordering::SeqCst);
    let (c2, b2) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "workshopId": "77001" })),
    )
    .await;
    assert_eq!(c2, StatusCode::OK);
    let p2: l4a_protocol::FilterProbabilities = serde_json::from_slice(&b2).unwrap();
    assert!((p2.prob - 0.90).abs() < 1e-5);
    assert_eq!(p2.text_prob, Some(0.20));
    assert_eq!(p2.image_prob, Some(0.86));
    assert_eq!(p2.per_image_probs, vec![0.86]);

    // 3. Direct base64 + text call also populates unimodal breakdown.
    let img_b64 = base64::engine::general_purpose::STANDARD.encode(sample_png);
    let (c3, b3) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "title": "Direct Multimodal", "imageBase64": img_b64 })),
    )
    .await;
    assert_eq!(c3, StatusCode::OK);
    let p3: l4a_protocol::FilterProbabilities = serde_json::from_slice(&b3).unwrap();
    assert_eq!(p3.text_prob, Some(0.20));
    assert_eq!(p3.image_prob, Some(0.86));

    srv_task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn filter_managed_python_auto_recovers_after_child_kill_and_handles_non_utf8_pipes() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let deploy_dir = dir.path().join("deploy");
    let py_shim_dir = dir.path().join("pyshims");
    std::fs::create_dir_all(&deploy_dir).unwrap();
    std::fs::create_dir_all(py_shim_dir.join("PIL")).unwrap();

    std::fs::write(py_shim_dir.join("numpy.py"), "float32 = float\n").unwrap();
    std::fs::write(
        py_shim_dir.join("PIL").join("Image.py"),
        "MAX_IMAGE_PIXELS = None\n",
    )
    .unwrap();
    std::fs::write(py_shim_dir.join("PIL").join("__init__.py"), "").unwrap();

    // Emit raw non-UTF-8 bytes to both stdout (before ready) and stderr (during predict)
    // to verify pipe drainers never drop pipes or trigger BrokenPipeError/SIGPIPE!
    std::fs::write(
        deploy_dir.join("l4filter.py"),
        r#"import sys

class Left4Filter:
    def __init__(self, deploy_dir, num_threads=4):
        sys.stdout.buffer.write(b"pre-ready-non-utf8:\xff\xfe\n")
        sys.stdout.buffer.flush()
        sys.stderr.buffer.write(b"init-stderr-non-utf8:\xff\xfe\n")
        sys.stderr.buffer.flush()
        self.config = {"model_name": "CrashRecoverMock", "version": "1.0.0"}

    def predict(self, title="", description="", tags=None, image=None, images=None):
        sys.stdout.buffer.write(b"predict-stdout-non-utf8:\x80\xff\n")
        sys.stdout.buffer.flush()
        sys.stderr.buffer.write(b"predict-stderr-non-utf8:\x80\xff\n")
        sys.stderr.buffer.flush()
        return {"prob": 0.55, "gore_prob": 0.25, "nsfw_prob": 0.50}
"#,
    )
    .unwrap();

    let py_wrapper = dir.path().join("python_wrapper.sh");
    std::fs::write(
        &py_wrapper,
        format!(
            "#!/usr/bin/env bash\nexport PYTHONPATH=\"{}:${{PYTHONPATH:-}}\"\nexec python3 \"$@\"\n",
            py_shim_dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&py_wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();

    let state = ServerState::in_memory("recover-secret").unwrap();
    let app = build_router(state.clone());
    let admin_hdr = vec![("x-admin-token", "recover-secret")];

    let dto = l4a_protocol::CatalogFilterConfigDto {
        enabled: true,
        backend: l4a_protocol::CatalogFilterBackendKind::ManagedPython,
        deploy_dir: Some(deploy_dir.display().to_string()),
        python_bin: Some(py_wrapper.display().to_string()),
        upstream_url: None,
        cache_ttl_secs: 0,
        params: l4a_protocol::FilterModelParams::default(),
    };
    let (c1, b1) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr.clone(),
        Some(serde_json::to_value(&dto).unwrap()),
    )
    .await;
    assert_eq!(c1, StatusCode::OK);
    let st1: l4a_protocol::CatalogFilterStatus = serde_json::from_slice(&b1).unwrap();
    assert!(st1.ready, "last_error={:?}", st1.last_error);
    let pid1 = st1.running_pid.unwrap();

    // Multiple predictions succeed without BrokenPipeError despite non-UTF-8 stdout/stderr writes.
    for i in 0..3 {
        let (pc, _) = json_request(
            &app,
            "POST",
            "/api/v1/filter/predict",
            vec![],
            Some(json!({ "title": format!("NonUtf8 Check {i}") })),
        )
        .await;
        assert_eq!(pc, StatusCode::OK);
    }

    // Force-kill the managed Python child process (`SIGKILL`) and wait briefly for OS reap readiness.
    let kill_status = std::process::Command::new("kill")
        .args(["-9", &pid1.to_string()])
        .status()
        .unwrap();
    assert!(kill_status.success());
    for _ in 0..40 {
        // Poll until child becomes zombie/reapable so try_wait() observes exit.
        let (st_code, st_bytes) = json_request(
            &app,
            "GET",
            "/api/v1/admin/filter/status",
            admin_hdr.clone(),
            None,
        )
        .await;
        assert_eq!(st_code, StatusCode::OK);
        let st_now: l4a_protocol::CatalogFilterStatus = serde_json::from_slice(&st_bytes).unwrap();
        if st_now.running_pid != Some(pid1) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // Next `POST /api/v1/filter/predict` automatically recovers with a fresh PID!
    let (rec_code, rec_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "title": "Recovered After Crash" })),
    )
    .await;
    assert_eq!(rec_code, StatusCode::OK);
    let rec_probs: l4a_protocol::FilterProbabilities = serde_json::from_slice(&rec_bytes).unwrap();
    assert!((rec_probs.prob - 0.55).abs() < 1e-5);

    let (_, st2_bytes) =
        json_request(&app, "GET", "/api/v1/admin/filter/status", admin_hdr, None).await;
    let st2: l4a_protocol::CatalogFilterStatus = serde_json::from_slice(&st2_bytes).unwrap();
    assert!(st2.ready);
    let pid2 = st2.running_pid.unwrap();
    assert_ne!(pid1, pid2, "crashed child must be replaced by a new PID");

    state.filter.shutdown().await;
}

#[tokio::test]
async fn filter_stock_left4filters_wire_format_gallery_aggregation_unicode_and_scoring_revision() {
    use axum::routing::{get, post};
    use base64::Engine;

    let make_png = |w: u32, h: u32| -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
        v
    };
    let png_hi = make_png(16, 16);
    let png_lo = make_png(32, 32);
    let png_mid = make_png(48, 48);

    let b64_hi = base64::engine::general_purpose::STANDARD.encode(&png_hi);
    let b64_lo = base64::engine::general_purpose::STANDARD.encode(&png_lo);
    let b64_mid = base64::engine::general_purpose::STANDARD.encode(&png_mid);

    let hi_cl = b64_hi.clone();
    let lo_cl = b64_lo.clone();
    let mid_cl = b64_mid.clone();

    // Stock `Left4Filters/deploy/server.py` mock:
    // - Rejects camelCase `imagesBase64` / `imageBase64` with 400!
    // - ONLY reads snake_case `images_base64`!
    // - Returns raw `prob`, `gore_prob`, `nsfw_prob` only.
    let stock_srv = axum::Router::new()
        .route(
            "/health",
            get(|| async {
                axum::Json(json!({
                    "status": "ok",
                    "model": "StockLeft4Filters",
                    "version": "1.2.0"
                }))
            }),
        )
        .route(
            "/predict",
            post(move |axum::Json(payload): axum::Json<serde_json::Value>| {
                let hi = hi_cl.clone();
                let lo = lo_cl.clone();
                let mid = mid_cl.clone();
                async move {
                    if payload.get("imagesBase64").is_some()
                        || payload.get("imageBase64").is_some()
                    {
                        return (
                            StatusCode::BAD_REQUEST,
                            axum::Json(json!({ "error": "camelCase image field forbidden on stock Left4Filters" })),
                        );
                    }
                    let has_txt = !payload["title"].as_str().unwrap_or_default().is_empty()
                        || !payload["description"]
                            .as_str()
                            .unwrap_or_default()
                            .is_empty();
                    let imgs: Vec<&str> = payload["images_base64"]
                        .as_array()
                        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                        .unwrap_or_default();

                    let (prob, gore, nsfw) = if imgs.is_empty() {
                        if has_txt {
                            (0.15, 0.05, 0.12)
                        } else {
                            (0.0, 0.0, 0.0)
                        }
                    } else {
                        // Stock server.py default max behavior if multiple images sent at once,
                        // or exact per-image score when 1 image is sent.
                        let mut p_max = 0.0f32;
                        let mut g_max = 0.0f32;
                        let mut n_max = 0.0f32;
                        for im in imgs {
                            let (ip, ig, in_) = if im == hi {
                                if has_txt {
                                    (0.94, 0.84, 0.30)
                                } else {
                                    (0.90, 0.80, 0.20)
                                }
                            } else if im == lo {
                                if has_txt {
                                    (0.20, 0.10, 0.16)
                                } else {
                                    (0.10, 0.04, 0.08)
                                }
                            } else if im == mid {
                                if has_txt {
                                    (0.66, 0.56, 0.24)
                                } else {
                                    (0.60, 0.50, 0.20)
                                }
                            } else {
                                (0.40, 0.20, 0.30)
                            };
                            p_max = p_max.max(ip);
                            g_max = g_max.max(ig);
                            n_max = n_max.max(in_);
                        }
                        (p_max, g_max, n_max)
                    };

                    (
                        StatusCode::OK,
                        axum::Json(json!({
                            "prob": prob,
                            "gore_prob": gore,
                            "nsfw_prob": nsfw
                        })),
                    )
                }
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv_task = tokio::spawn(async move {
        let _ = axum::serve(listener, stock_srv).await;
    });

    let state = ServerState::in_memory("stock-admin-pw").unwrap();
    let app = build_router(state);
    let admin_hdr = vec![("x-admin-token", "stock-admin-pw")];

    // Before enabling: `scoringRevision` is None.
    let (_, off_info_bytes) = json_request(&app, "GET", "/api/v1/filter/info", vec![], None).await;
    let off_info: l4a_protocol::CatalogFilterPublicInfo =
        serde_json::from_slice(&off_info_bytes).unwrap();
    assert!(!off_info.enabled);
    assert_eq!(off_info.scoring_revision, None);

    // 1. Enable UpstreamHttp with `gallery_aggregation = Max`, `max_images = 4`.
    let cfg_max = l4a_protocol::CatalogFilterConfigDto {
        enabled: true,
        backend: l4a_protocol::CatalogFilterBackendKind::UpstreamHttp,
        deploy_dir: None,
        python_bin: None,
        upstream_url: Some(format!("http://{addr}")),
        cache_ttl_secs: 3600,
        params: l4a_protocol::FilterModelParams {
            max_images: 4,
            gallery_aggregation: l4a_protocol::FilterGalleryAggregation::Max,
            ..Default::default()
        },
    };
    let (c_cfg1, _) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr.clone(),
        Some(serde_json::to_value(&cfg_max).unwrap()),
    )
    .await;
    assert_eq!(c_cfg1, StatusCode::OK);

    let (_, info1_bytes) = json_request(&app, "GET", "/api/v1/filter/info", vec![], None).await;
    let info1: l4a_protocol::CatalogFilterPublicInfo =
        serde_json::from_slice(&info1_bytes).unwrap();
    let rev1 = info1
        .scoring_revision
        .expect("enabled filter/info must include scoringRevision");
    assert_eq!(rev1.len(), 16);

    // 2. Malformed Unicode base64 image inputs return 400 Bad Request without panicking!
    for bad_img in ["data:中文;base64,AAAA", "中文中文"] {
        let (bad_code, _) = json_request(
            &app,
            "POST",
            "/api/v1/filter/predict",
            vec![],
            Some(json!({ "imageBase64": bad_img })),
        )
        .await;
        assert_eq!(bad_code, StatusCode::BAD_REQUEST, "input={bad_img}");
    }

    // 3. Single image-only & 2-image Max requests on stock `images_base64` server score non-zero!
    let (c_single, b_single) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "imageBase64": b64_hi })),
    )
    .await;
    assert_eq!(c_single, StatusCode::OK);
    let p_single: l4a_protocol::FilterProbabilities = serde_json::from_slice(&b_single).unwrap();
    assert!((p_single.prob - 0.90).abs() < 1e-5);
    assert_eq!(p_single.image_prob, Some(0.90));
    assert_eq!(p_single.scoring_revision.as_deref(), Some(rev1.as_str()));

    let (c_max_img, b_max_img) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "imagesBase64": [b64_hi, b64_lo] })),
    )
    .await;
    assert_eq!(c_max_img, StatusCode::OK);
    let p_max_img: l4a_protocol::FilterProbabilities = serde_json::from_slice(&b_max_img).unwrap();
    assert!((p_max_img.prob - 0.90).abs() < 1e-5);
    assert!((p_max_img.gore_prob - 0.80).abs() < 1e-5);
    assert_eq!(p_max_img.image_prob, Some(0.90));
    assert_eq!(p_max_img.per_image_probs, vec![0.90, 0.10]);
    assert_eq!(p_max_img.scoring_revision.as_deref(), Some(rev1.as_str()));

    let (c_max_mm, b_max_mm) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({
            "title": "Gallery Multimodal",
            "imagesBase64": [b64_hi, b64_lo]
        })),
    )
    .await;
    assert_eq!(c_max_mm, StatusCode::OK);
    let p_max_mm: l4a_protocol::FilterProbabilities = serde_json::from_slice(&b_max_mm).unwrap();
    assert!((p_max_mm.prob - 0.94).abs() < 1e-5);
    assert_eq!(p_max_mm.text_prob, Some(0.15));
    assert_eq!(p_max_mm.image_prob, Some(0.90));
    assert_eq!(p_max_mm.per_image_probs, vec![0.90, 0.10]);

    // 4. Switch to `gallery_aggregation = Mean`: `scoringRevision` changes AND 2-image scores average!
    let cfg_mean = l4a_protocol::CatalogFilterConfigDto {
        params: l4a_protocol::FilterModelParams {
            max_images: 4,
            gallery_aggregation: l4a_protocol::FilterGalleryAggregation::Mean,
            ..Default::default()
        },
        ..cfg_max.clone()
    };
    let (c_cfg2, _) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr.clone(),
        Some(serde_json::to_value(&cfg_mean).unwrap()),
    )
    .await;
    assert_eq!(c_cfg2, StatusCode::OK);

    let (_, info2_bytes) = json_request(&app, "GET", "/api/v1/filter/info", vec![], None).await;
    let info2: l4a_protocol::CatalogFilterPublicInfo =
        serde_json::from_slice(&info2_bytes).unwrap();
    let rev2 = info2.scoring_revision.unwrap();
    assert_ne!(
        rev1, rev2,
        "scoringRevision must change when gallery_aggregation changes"
    );

    let (c_mean_img, b_mean_img) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "imagesBase64": [b64_hi, b64_lo] })),
    )
    .await;
    assert_eq!(c_mean_img, StatusCode::OK);
    let p_mean_img: l4a_protocol::FilterProbabilities =
        serde_json::from_slice(&b_mean_img).unwrap();
    assert!((p_mean_img.prob - 0.50).abs() < 1e-5);
    assert!((p_mean_img.gore_prob - 0.42).abs() < 1e-5);
    assert!((p_mean_img.nsfw_prob - 0.14).abs() < 1e-5);
    assert_eq!(p_mean_img.image_prob, Some(0.50));
    assert_eq!(p_mean_img.per_image_probs, vec![0.90, 0.10]);
    assert_eq!(p_mean_img.scoring_revision.as_deref(), Some(rev2.as_str()));

    let (c_mean_mm, b_mean_mm) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({
            "title": "Gallery Multimodal",
            "imagesBase64": [b64_hi, b64_lo]
        })),
    )
    .await;
    assert_eq!(c_mean_mm, StatusCode::OK);
    let p_mean_mm: l4a_protocol::FilterProbabilities = serde_json::from_slice(&b_mean_mm).unwrap();
    assert!((p_mean_mm.prob - 0.57).abs() < 1e-5);
    assert!((p_mean_mm.gore_prob - 0.47).abs() < 1e-5);
    assert!((p_mean_mm.nsfw_prob - 0.23).abs() < 1e-5);
    assert_eq!(p_mean_mm.text_prob, Some(0.15));
    assert_eq!(p_mean_mm.image_prob, Some(0.50));
    assert_eq!(p_mean_mm.per_image_probs, vec![0.90, 0.10]);

    // 5. Switch to `Top2Mean` + calibration `bias` change: `scoringRevision` updates again & Top2Mean works across 3 images!
    let cfg_top2 = l4a_protocol::CatalogFilterConfigDto {
        params: l4a_protocol::FilterModelParams {
            max_images: 4,
            gallery_aggregation: l4a_protocol::FilterGalleryAggregation::Top2Mean,
            ..Default::default()
        },
        ..cfg_max.clone()
    };
    let (c_cfg3, _) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr.clone(),
        Some(serde_json::to_value(&cfg_top2).unwrap()),
    )
    .await;
    assert_eq!(c_cfg3, StatusCode::OK);

    let (c_top2, b_top2) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "imagesBase64": [b64_hi, b64_lo, b64_mid] })),
    )
    .await;
    assert_eq!(c_top2, StatusCode::OK);
    let p_top2: l4a_protocol::FilterProbabilities = serde_json::from_slice(&b_top2).unwrap();
    // Top 2 of [0.90, 0.10, 0.60] -> (0.90 + 0.60) / 2 = 0.75
    assert!((p_top2.prob - 0.75).abs() < 1e-5);
    assert_eq!(p_top2.image_prob, Some(0.75));
    assert_eq!(p_top2.per_image_probs, vec![0.90, 0.10, 0.60]);
    let rev3 = p_top2.scoring_revision.unwrap();
    assert_ne!(rev2, rev3);

    let cfg_calibrated = l4a_protocol::CatalogFilterConfigDto {
        params: l4a_protocol::FilterModelParams {
            bias: 0.4,
            ..cfg_top2.params
        },
        ..cfg_top2
    };
    let (c_cfg4, _) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr,
        Some(serde_json::to_value(&cfg_calibrated).unwrap()),
    )
    .await;
    assert_eq!(c_cfg4, StatusCode::OK);
    let (_, info4_bytes) = json_request(&app, "GET", "/api/v1/filter/info", vec![], None).await;
    let info4: l4a_protocol::CatalogFilterPublicInfo =
        serde_json::from_slice(&info4_bytes).unwrap();
    assert_ne!(info4.scoring_revision.as_deref(), Some(rev3.as_str()));

    srv_task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn filter_managed_python_preserves_symlinked_virtualenv_site_packages() {
    use std::os::unix::fs::symlink;

    // Query system python3 canonical path and major.minor version to build a genuine
    // symlink-based `pyvenv.cfg` virtualenv WITHOUT any `PYTHONPATH` wrapper!
    let py_probe = std::process::Command::new("python3")
        .args([
            "-c",
            "import os, sys; print(os.path.realpath(sys.executable)); print(f'{sys.version_info.major}.{sys.version_info.minor}')",
        ])
        .output()
        .expect("python3 must be available");
    assert!(py_probe.status.success());
    let probe_lines: Vec<String> = String::from_utf8(py_probe.stdout)
        .unwrap()
        .lines()
        .map(str::trim)
        .map(str::to_owned)
        .collect();
    let real_sys_python = std::path::PathBuf::from(&probe_lines[0]);
    let py_ver = &probe_lines[1];
    let real_home = real_sys_python.parent().unwrap();

    let dir = tempfile::tempdir().unwrap();
    let repo_root = dir.path().join("Left4Filters");
    let deploy_dir = repo_root.join("deploy");
    let venv_dir = repo_root.join("venv");
    let venv_bin = venv_dir.join("bin");
    let site_packages = venv_dir
        .join("lib")
        .join(format!("python{py_ver}"))
        .join("site-packages");

    std::fs::create_dir_all(&deploy_dir).unwrap();
    std::fs::create_dir_all(&venv_bin).unwrap();
    std::fs::create_dir_all(site_packages.join("PIL")).unwrap();

    // Symlink `venv/bin/python -> /usr/bin/python3.x` + `pyvenv.cfg`.
    let venv_python = venv_bin.join("python");
    symlink(&real_sys_python, &venv_python).unwrap();
    std::fs::write(
        venv_dir.join("pyvenv.cfg"),
        format!(
            "home = {}\ninclude-system-site-packages = false\nversion = {py_ver}.0\n",
            real_home.display()
        ),
    )
    .unwrap();

    // Place `l4a_venv_marker` + `numpy` + `PIL` ONLY inside `venv/lib/pythonX.Y/site-packages`!
    // If `venv/bin/python` were canonicalized to `/usr/bin/python3.x`, Python would ignore `pyvenv.cfg`
    // and `import l4a_venv_marker` would immediately fail with `ModuleNotFoundError`!
    std::fs::write(
        site_packages.join("l4a_venv_marker.py"),
        "ACTIVE_FROM_VENV = True\n",
    )
    .unwrap();
    std::fs::write(site_packages.join("numpy.py"), "float32 = float\n").unwrap();
    std::fs::write(
        site_packages.join("PIL").join("Image.py"),
        "MAX_IMAGE_PIXELS = None\n",
    )
    .unwrap();
    std::fs::write(site_packages.join("PIL").join("__init__.py"), "").unwrap();

    let expected_venv_prefix = std::fs::canonicalize(&venv_dir).unwrap();
    std::fs::write(
        deploy_dir.join("l4filter.py"),
        format!(
            r#"import os
import sys
import l4a_venv_marker

assert l4a_venv_marker.ACTIVE_FROM_VENV is True
assert os.path.realpath(sys.prefix) == {expected_prefix:?}, f"unexpected sys.prefix={{sys.prefix}}"
assert sys.executable.endswith("venv/bin/python"), f"expected symlink path in sys.executable, got {{sys.executable}}"

class Left4Filter:
    def __init__(self, deploy_dir, num_threads=4):
        self.config = {{"model_name": "VenvSymlinkMock", "version": "1.0.0"}}

    def predict(self, title="", description="", tags=None, image=None, images=None):
        return {{"prob": 0.77, "gore_prob": 0.44, "nsfw_prob": 0.66}}
"#,
            expected_prefix = expected_venv_prefix.display().to_string(),
        ),
    )
    .unwrap();

    let state = ServerState::in_memory("venv-secret").unwrap();
    let app = build_router(state.clone());
    let admin_hdr = vec![("x-admin-token", "venv-secret")];

    // 1. Auto-discovered `<deploy_dir>/../venv/bin/python` (`python_bin = None`).
    let dto_auto = l4a_protocol::CatalogFilterConfigDto {
        enabled: true,
        backend: l4a_protocol::CatalogFilterBackendKind::ManagedPython,
        deploy_dir: Some(deploy_dir.display().to_string()),
        python_bin: None,
        upstream_url: None,
        cache_ttl_secs: 0,
        params: l4a_protocol::FilterModelParams::default(),
    };
    let (c1, b1) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr.clone(),
        Some(serde_json::to_value(&dto_auto).unwrap()),
    )
    .await;
    assert_eq!(c1, StatusCode::OK);
    let st1: l4a_protocol::CatalogFilterStatus = serde_json::from_slice(&b1).unwrap();
    assert!(st1.ready, "auto venv failed: {:?}", st1.last_error);
    assert_eq!(st1.model_name.as_deref(), Some("VenvSymlinkMock"));

    let (pc1, pb1) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "title": "Venv Auto Check" })),
    )
    .await;
    assert_eq!(pc1, StatusCode::OK);
    let pr1: l4a_protocol::FilterProbabilities = serde_json::from_slice(&pb1).unwrap();
    assert!((pr1.prob - 0.77).abs() < 1e-5);

    // 2. Explicit `python_bin = Some(.../venv/bin/python)` also preserves symlink!
    let dto_explicit = l4a_protocol::CatalogFilterConfigDto {
        python_bin: Some(venv_python.display().to_string()),
        ..dto_auto
    };
    let (c2, b2) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr,
        Some(serde_json::to_value(&dto_explicit).unwrap()),
    )
    .await;
    assert_eq!(c2, StatusCode::OK);
    let st2: l4a_protocol::CatalogFilterStatus = serde_json::from_slice(&b2).unwrap();
    assert!(st2.ready, "explicit venv failed: {:?}", st2.last_error);

    state.filter.shutdown().await;
}

#[tokio::test]
async fn filter_public_admission_limit_returns_429_before_buffering_request_body() {
    use axum::routing::get;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let sample_png: &'static [u8] =
        b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x10\x00\x00\x00\x10\x08\x02\x00\x00\x00\x00\x00\x00\x00";
    let in_flight_images = Arc::new(AtomicUsize::new(0));
    let all_admitted = Arc::new(tokio::sync::Notify::new());
    let release_gate = Arc::new(tokio::sync::Notify::new());

    let in_flight_cl = in_flight_images.clone();
    let admitted_cl = all_admitted.clone();
    let gate_cl = release_gate.clone();

    let max_admissions = l4a_server::filter::MAX_PUBLIC_PREDICT_ADMISSIONS;
    let slow_srv = axum::Router::new()
        .route(
            "/health",
            get(|| async { axum::Json(json!({ "status": "ok" })) }),
        )
        .route(
            "/slow.png",
            get(move || {
                let count = in_flight_cl.clone();
                let admitted = admitted_cl.clone();
                let gate = gate_cl.clone();
                async move {
                    let prev = count.fetch_add(1, Ordering::SeqCst);
                    if prev + 1 == max_admissions {
                        admitted.notify_waiters();
                    }
                    gate.notified().await;
                    ([(header::CONTENT_TYPE, "image/png")], sample_png)
                }
            }),
        )
        .route(
            "/predict",
            axum::routing::post(|| async {
                axum::Json(json!({
                    "prob": 0.30,
                    "gore_prob": 0.10,
                    "nsfw_prob": 0.25
                }))
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv_task = tokio::spawn(async move {
        let _ = axum::serve(listener, slow_srv).await;
    });

    let mut cfg = CatalogConfig::default();
    cfg.admin.password = "adm-pw".to_string();
    cfg.filter.enabled = true;
    cfg.filter.backend = l4a_protocol::CatalogFilterBackendKind::UpstreamHttp;
    cfg.filter.upstream_url = Some(format!("http://{addr}"));
    cfg.filter.cache_ttl_secs = 0;

    let store = Arc::new(CatalogConfigStore::new(None, cfg));
    let repo = Arc::new(ServerRepo::open_in_memory().unwrap());
    let state = ServerState::with_components(store, repo, Arc::new(MockWorkshopUpstream::new()));
    state.filter.set_allow_loopback_image_urls_for_testing(true);
    state
        .filter
        .set_public_admission_timeout_for_testing(std::time::Duration::from_millis(60));

    let app = build_router(state);

    // Warm up backend health initialization first.
    let (info_code, _) = json_request(&app, "GET", "/api/v1/filter/info", vec![], None).await;
    assert_eq!(info_code, StatusCode::OK);

    // Launch `MAX_PUBLIC_PREDICT_ADMISSIONS` concurrent requests and wait until all 8 hold permits.
    let admitted_wait = all_admitted.notified();
    let mut handles = Vec::with_capacity(max_admissions);
    for idx in 0..max_admissions {
        let app_cl = app.clone();
        let img_url = format!("http://{addr}/slow.png?slot={idx}");
        handles.push(tokio::spawn(async move {
            json_request(
                &app_cl,
                "POST",
                "/api/v1/filter/predict",
                vec![],
                Some(json!({
                    "title": format!("Concurrent {idx}"),
                    "imageUrls": [img_url]
                })),
            )
            .await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), admitted_wait)
        .await
        .expect("all 8 admission slots should enter slow image fetch");

    // 9th concurrent request with an unbuffered stream MUST be rejected with 429 BEFORE reading body!
    let body_read_attempted = Arc::new(AtomicBool::new(false));
    let flag_cl = body_read_attempted.clone();
    let unbuffered_stream = futures_util::stream::poll_fn(move |_| {
        flag_cl.store(true, Ordering::SeqCst);
        std::task::Poll::Ready(Some(Ok::<_, std::io::Error>(bytes::Bytes::from_static(
            b"{\"title\":\"overflow\"}",
        ))))
    });
    let overload_req = Request::builder()
        .method("POST")
        .uri("/api/v1/filter/predict")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from_stream(unbuffered_stream))
        .unwrap();
    let overload_resp = app.clone().oneshot(overload_req).await.unwrap();
    assert_eq!(overload_resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        !body_read_attempted.load(Ordering::SeqCst),
        "overloaded request must be rejected with 429 BEFORE buffering request body"
    );
    let overload_json: serde_json::Value = serde_json::from_slice(
        &overload_resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert_eq!(overload_json["error"], "内容过滤请求繁忙，请稍后重试");

    // Release the 8 in-flight requests and verify they all finish cleanly, restoring capacity!
    release_gate.notify_waiters();
    for h in handles {
        let (code, _) = h.await.unwrap();
        assert_eq!(code, StatusCode::OK);
    }

    let (after_code, _) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({ "title": "After Drain" })),
    )
    .await;
    assert_eq!(after_code, StatusCode::OK);

    srv_task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn filter_large_multi_image_requests_split_into_subbatches_under_runner_limit() {
    use axum::extract::Path as AxumPath;
    use axum::routing::get;
    use std::os::unix::fs::PermissionsExt;

    // Build 4 valid 12 MiB PNGs (`4 * 12 MiB = 48 MiB` raw > `MAX_RUNNER_BATCH_RAW_IMAGE_BYTES` (30 MiB),
    // and ~64+ MiB base64 JSON which would exceed `filter_runner.py`'s 64 MiB request body cap if unsplit!).
    let make_12mb_png = |marker: u8| -> bytes::Bytes {
        let total = 12 * 1024 * 1024;
        let mut buf = Vec::with_capacity(total);
        buf.extend_from_slice(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR");
        buf.extend_from_slice(&32u32.to_be_bytes());
        buf.extend_from_slice(&32u32.to_be_bytes());
        buf.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
        buf.push(marker);
        buf.resize(total, 0u8);
        bytes::Bytes::from(buf)
    };
    let imgs: Arc<Vec<bytes::Bytes>> = Arc::new(vec![
        make_12mb_png(1),
        make_12mb_png(2),
        make_12mb_png(3),
        make_12mb_png(4),
    ]);
    assert!(
        imgs.iter().map(|b| b.len()).sum::<usize>()
            > l4a_server::filter::MAX_RUNNER_BATCH_RAW_IMAGE_BYTES
    );

    let imgs_srv = imgs.clone();
    let img_cdn = axum::Router::new().route(
        "/large/{idx}",
        get(move |AxumPath(idx): AxumPath<usize>| {
            let store = imgs_srv.clone();
            async move { ([(header::CONTENT_TYPE, "image/png")], store[idx].clone()) }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cdn_addr = listener.local_addr().unwrap();
    let cdn_task = tokio::spawn(async move {
        let _ = axum::serve(listener, img_cdn).await;
    });

    let dir = tempfile::tempdir().unwrap();
    let deploy_dir = dir.path().join("deploy");
    let py_shim_dir = dir.path().join("pyshims");
    let batch_log = dir.path().join("batches.log");
    std::fs::create_dir_all(&deploy_dir).unwrap();
    std::fs::create_dir_all(py_shim_dir.join("PIL")).unwrap();

    // Minimal numpy shim supporting `np.asarray` + `.mean(axis=0)` / `.max(axis=0)` for SDK fallback gallery aggregation!
    std::fs::write(
        py_shim_dir.join("numpy.py"),
        r#"float32 = float

class _FakeMatrix(list):
    @property
    def shape(self):
        return (len(self), len(self[0]) if self else 0)

    def mean(self, axis=0):
        cols = len(self[0])
        return [sum(r[c] for r in self) / len(self) for c in range(cols)]

    def max(self, axis=0):
        cols = len(self[0])
        return [max(r[c] for r in self) for c in range(cols)]

def asarray(rows, dtype=None):
    return _FakeMatrix([[float(x) for x in r] for r in rows])
"#,
    )
    .unwrap();
    std::fs::write(
        py_shim_dir.join("PIL").join("Image.py"),
        "MAX_IMAGE_PIXELS = None\n",
    )
    .unwrap();
    std::fs::write(py_shim_dir.join("PIL").join("__init__.py"), "").unwrap();

    std::fs::write(
        deploy_dir.join("l4filter.py"),
        format!(
            r#"class Left4Filter:
    def __init__(self, deploy_dir, num_threads=4):
        self.config = {{"model_name": "SubBatchMock", "version": "1.0.0"}}

    def _score_one(self, im):
        marker = im[33]
        table = {{1: 0.80, 2: 0.60, 3: 0.40, 4: 0.20}}
        p = table.get(marker, 0.10)
        return {{"prob": p, "gore_prob": p * 0.5, "nsfw_prob": p}}

    def predict(self, title="", description="", tags=None, image=None, images=None):
        if image is not None:
            return self._score_one(image)
        if images is not None:
            with open({log_path:?}, "a", encoding="utf-8") as f:
                f.write(f"{{len(images)}}\n")
            scores = [self._score_one(im)["prob"] for im in images]
            avg = sum(scores) / len(scores)
            return {{"prob": avg, "gore_prob": avg * 0.5, "nsfw_prob": avg}}
        return {{"prob": 0.10, "gore_prob": 0.05, "nsfw_prob": 0.10}}
"#,
            log_path = batch_log.display().to_string(),
        ),
    )
    .unwrap();

    let py_wrapper = dir.path().join("python_wrapper.sh");
    std::fs::write(
        &py_wrapper,
        format!(
            "#!/usr/bin/env bash\nexport PYTHONPATH=\"{}:${{PYTHONPATH:-}}\"\nexec python3 \"$@\"\n",
            py_shim_dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&py_wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();

    let state = ServerState::in_memory("batch-secret").unwrap();
    state.filter.set_allow_loopback_image_urls_for_testing(true);
    let app = build_router(state.clone());
    let admin_hdr = vec![("x-admin-token", "batch-secret")];

    let dto = l4a_protocol::CatalogFilterConfigDto {
        enabled: true,
        backend: l4a_protocol::CatalogFilterBackendKind::ManagedPython,
        deploy_dir: Some(deploy_dir.display().to_string()),
        python_bin: Some(py_wrapper.display().to_string()),
        upstream_url: None,
        cache_ttl_secs: 0,
        params: l4a_protocol::FilterModelParams {
            max_images: 4,
            gallery_aggregation: l4a_protocol::FilterGalleryAggregation::Mean,
            ..Default::default()
        },
    };
    let (c_cfg, b_cfg) = json_request(
        &app,
        "PATCH",
        "/api/v1/admin/config/filter",
        admin_hdr,
        Some(serde_json::to_value(&dto).unwrap()),
    )
    .await;
    assert_eq!(c_cfg, StatusCode::OK);
    let st: l4a_protocol::CatalogFilterStatus = serde_json::from_slice(&b_cfg).unwrap();
    assert!(st.ready, "last_error={:?}", st.last_error);

    let urls: Vec<String> = (0..4)
        .map(|i| format!("http://{cdn_addr}/large/{i}"))
        .collect();
    let (p_code, p_bytes) = json_request(
        &app,
        "POST",
        "/api/v1/filter/predict",
        vec![],
        Some(json!({
            "title": "Oversized Gallery SubBatch Test",
            "imageUrls": urls
        })),
    )
    .await;
    assert_eq!(
        p_code,
        StatusCode::OK,
        "resp={}",
        String::from_utf8_lossy(&p_bytes)
    );
    let probs: l4a_protocol::FilterProbabilities = serde_json::from_slice(&p_bytes).unwrap();
    assert_eq!(probs.per_image_probs, vec![0.80, 0.60, 0.40, 0.20]);
    // Mean of [0.80, 0.60, 0.40, 0.20] == 0.50
    assert!((probs.prob - 0.50).abs() < 1e-5);
    assert_eq!(probs.image_prob, Some(0.50));
    assert!(probs.scoring_revision.is_some());

    // Verify the 4 x 12 MiB images were split into two 2-image sub-batches (24 MiB <= 30 MiB each).
    let logged_batches: Vec<usize> = std::fs::read_to_string(&batch_log)
        .unwrap()
        .lines()
        .map(|l| l.trim().parse::<usize>().unwrap())
        .collect();
    assert_eq!(logged_batches, vec![2, 2]);

    state.filter.shutdown().await;
    cdn_task.abort();
}
