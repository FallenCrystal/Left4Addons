use crate::auth::{build_clear_session_cookie, build_session_cookie};
use crate::db::ServerRepo;
use crate::filter::FilterServiceError;
use crate::sync::with_catalog_source;
use crate::web_assets;
use crate::ServerState;
use axum::{
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{
        header::{ACCEPT_LANGUAGE, CACHE_CONTROL, SET_COOKIE, VARY},
        HeaderMap, HeaderName, HeaderValue, Method, StatusCode,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use l4a_protocol::{
    AdminChangePasswordRequest, AdminFetchUpstreamRequest, AdminFetchUpstreamResponse,
    AdminItemDetailResponse, AdminItemOverrideRequest, AdminLoginRequest, AdminSessionInfo,
    AuditLogEntry, BatchItemsRequest, BatchItemsResponse, CatalogFilterConfigDto,
    CatalogFilterPublicInfo, CatalogFilterStatus, CatalogOverviewResponse, CatalogStats,
    CatalogSteamStatus, CatalogSyncConfigDto, CatalogSyncStatus, FilterPredictRequest,
    FilterProbabilities, SearchQuery, SearchResponse, SetRefreshTokenRequest,
    StartCredentialsAuthRequest, SteamAuthFlowStatus, SubmitSteamGuardRequest,
    WorkshopCollectionRecord, WorkshopFileType, WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::str::FromStr;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

const MAX_BATCH_SIZE: usize = 200;
const MAX_FILTER_PREDICT_BODY_BYTES: usize = 48 * 1024 * 1024;

#[derive(Clone)]
struct RouteState {
    server: ServerState,
}

impl RouteState {
    async fn db<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&ServerRepo) -> anyhow::Result<T> + Send + 'static,
    ) -> Result<T, (StatusCode, Json<Value>)> {
        let repo = self.server.repo.clone();
        tokio::task::spawn_blocking(move || operation(&repo))
            .await
            .map_err(|err| internal_err(err.into()))?
            .map_err(internal_err)
    }

    fn require_admin(&self, headers: &HeaderMap) -> Result<String, (StatusCode, Json<Value>)> {
        self.server
            .auth
            .verify_admin(headers)
            .map_err(|(code, msg)| err_resp(code, msg))
    }
}

pub fn router(state: ServerState) -> Router {
    let state = RouteState { server: state };
    let search_route = Router::new()
        .route("/api/v1/items/search", get(search_items))
        .layer(CorsLayer::permissive())
        .layer(middleware::from_fn(append_search_vary));

    let filter_predict_route = post(filter_predict)
        .layer(DefaultBodyLimit::max(MAX_FILTER_PREDICT_BODY_BYTES))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            guard_filter_enabled,
        ));

    let public_routes = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/items/batch", post(batch_items))
        .route("/api/v1/items/{workshop_id}", get(get_item))
        .route("/api/v1/collections/{collection_id}", get(get_collection))
        .route("/api/v1/filter/info", get(filter_info))
        .route("/api/v1/filter/predict", filter_predict_route)
        .layer(CorsLayer::permissive());

    let admin_routes = Router::new()
        .route("/", get(web_assets::redirect_root_to_admin))
        .route("/admin", get(web_assets::serve_admin_index))
        .route("/admin/", get(web_assets::serve_admin_index))
        .route("/admin/{*path}", get(web_assets::serve_admin_asset))
        .route("/api/v1/admin/auth/login", post(admin_login))
        .route("/api/v1/admin/auth/logout", post(admin_logout))
        .route("/api/v1/admin/auth/me", get(admin_session_me))
        .route("/api/v1/admin/auth/password", post(admin_change_password))
        .route("/api/v1/admin/overview", get(admin_overview))
        .route("/api/v1/admin/stats", get(admin_stats))
        .route("/api/v1/admin/audit", get(admin_list_audit))
        .route("/api/v1/admin/sync/status", get(admin_sync_status))
        .route("/api/v1/admin/config/sync", patch(admin_update_sync_config))
        .route("/api/v1/admin/sync/trigger", post(admin_trigger_sync))
        .route(
            "/api/v1/admin/sync/reset-cursor",
            post(admin_reset_backfill_cursor),
        )
        .route("/api/v1/admin/sync/fetch", post(admin_fetch_upstream))
        .route("/api/v1/admin/filter/status", get(admin_filter_status))
        .route(
            "/api/v1/admin/config/filter",
            patch(admin_update_filter_config),
        )
        .route("/api/v1/admin/filter/restart", post(admin_restart_filter))
        .route(
            "/api/v1/admin/filter/test",
            post(admin_test_filter).layer(DefaultBodyLimit::max(MAX_FILTER_PREDICT_BODY_BYTES)),
        )
        .route("/api/v1/admin/steam/status", get(admin_steam_status))
        .route("/api/v1/admin/steam/reconnect", post(admin_steam_reconnect))
        .route(
            "/api/v1/admin/steam/auth/qr/start",
            post(admin_steam_start_qr),
        )
        .route(
            "/api/v1/admin/steam/auth/credentials",
            post(admin_steam_start_credentials),
        )
        .route(
            "/api/v1/admin/steam/auth/guard",
            post(admin_steam_submit_guard),
        )
        .route(
            "/api/v1/admin/steam/auth/token",
            post(admin_steam_set_token),
        )
        .route(
            "/api/v1/admin/steam/auth/anonymous",
            post(admin_steam_switch_anonymous),
        )
        .route("/api/v1/admin/items/search", get(admin_search_items))
        .route(
            "/api/v1/admin/items/{workshop_id}",
            get(admin_get_item_detail),
        )
        .route(
            "/api/v1/admin/items/{workshop_id}/refresh",
            post(admin_refresh_item),
        )
        .route(
            "/api/v1/admin/items/{workshop_id}/override",
            post(admin_override_item).delete(admin_clear_item_override),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            guard_admin_api,
        ));

    Router::new()
        .merge(search_route)
        .merge(public_routes)
        .merge(admin_routes)
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

async fn append_search_vary(request: Request, next: Next) -> Response {
    let should_vary = matches!(request.method(), &Method::GET | &Method::HEAD);
    let mut response = next.run(request).await;
    if !should_vary {
        return response;
    }
    let has_accept_lang = response
        .headers()
        .get_all(VARY)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(','))
        .any(|part| part.trim().eq_ignore_ascii_case("accept-language"));
    if !has_accept_lang {
        response
            .headers_mut()
            .append(VARY, HeaderValue::from_static("Accept-Language"));
    }
    response
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<Value>)>;

async fn guard_admin_api(
    State(state): State<RouteState>,
    request: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let is_admin_api = request.uri().path().starts_with("/api/v1/admin/");
    if is_admin_api
        && !request.method().is_safe()
        && !state.server.auth.allows_admin_write(request.headers())
    {
        return Err(err_resp(
            StatusCode::FORBIDDEN,
            "管理员会话缺少有效的 CSRF 凭证",
        ));
    }
    let mut response = next.run(request).await;
    if is_admin_api {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    Ok(response)
}

fn err_resp(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.into() })))
}

fn internal_err(err: anyhow::Error) -> (StatusCode, Json<Value>) {
    tracing::error!(error = %err, "l4a-server 内部处理异常");
    err_resp(StatusCode::INTERNAL_SERVER_ERROR, "服务端内部错误")
}

fn search_err(err: anyhow::Error) -> (StatusCode, Json<Value>) {
    if let Some(aborted) = err
        .chain()
        .find_map(|cause| cause.downcast_ref::<crate::db::QueryAbortedError>())
    {
        tracing::warn!(error = %aborted, "搜索请求因并发限流或超时中止");
        return match aborted {
            crate::db::QueryAbortedError::Timeout => err_resp(
                StatusCode::SERVICE_UNAVAILABLE,
                "搜索耗时过长，请缩小关键词范围后重试",
            ),
            crate::db::QueryAbortedError::Busy => {
                err_resp(StatusCode::SERVICE_UNAVAILABLE, "搜索服务繁忙，请稍后重试")
            }
            crate::db::QueryAbortedError::Cancelled => {
                err_resp(StatusCode::SERVICE_UNAVAILABLE, "搜索请求已取消")
            }
        };
    }
    internal_err(err)
}

async fn health(State(state): State<RouteState>) -> ApiResult<Value> {
    let total_items = state.db(move |repo| repo.total_items_count()).await?;
    let steam = state.server.upstream.status().await;
    Ok(Json(json!({
        "status": "ok",
        "service": "l4a-server",
        "version": env!("CARGO_PKG_VERSION"),
        "items": total_items,
        "steamState": steam.connection_state,
    })))
}

#[derive(Debug, Default, Deserialize)]
struct RefreshQuery {
    #[serde(default)]
    refresh: bool,
}

async fn get_item(
    State(state): State<RouteState>,
    Path(workshop_id): Path<String>,
    Query(query): Query<RefreshQuery>,
) -> ApiResult<WorkshopItemRecord> {
    let wid = WorkshopId::from_str(&workshop_id)
        .map_err(|e| err_resp(StatusCode::BAD_REQUEST, e.to_string()))?;
    let item = state
        .server
        .sync
        .resolve_single_item(wid, query.refresh)
        .await
        .map_err(internal_err)?
        .ok_or_else(|| err_resp(StatusCode::NOT_FOUND, format!("未找到工坊条目 {wid}")))?;
    Ok(Json(item))
}

async fn batch_items(
    State(state): State<RouteState>,
    Json(req): Json<BatchItemsRequest>,
) -> ApiResult<BatchItemsResponse> {
    if req.ids.len() > MAX_BATCH_SIZE {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            format!("单次批量查询最多支持 {MAX_BATCH_SIZE} 个条目"),
        ));
    }
    let mut seen = HashSet::with_capacity(req.ids.len());
    let deduped: Vec<WorkshopId> = req.ids.into_iter().filter(|id| seen.insert(*id)).collect();
    let resp = state
        .server
        .sync
        .resolve_batch(&deduped, req.force_refresh)
        .await
        .map_err(internal_err)?;
    Ok(Json(resp))
}

const MAX_SEARCH_QUERY_CHARS: usize = 512;
const MAX_SEARCH_TAG_CHARS: usize = 128;

fn empty_string_as_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    let Some(raw) = opt else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    serde_json::from_value(Value::String(trimmed.to_string()))
        .or_else(|_| serde_json::from_str(trimmed))
        .map(Some)
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchParams {
    q: Option<String>,
    tag: Option<String>,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    status: Option<WorkshopItemStatus>,
    sort: Option<String>,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    page: Option<u32>,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    page_size: Option<u32>,
    #[serde(alias = "language")]
    lang: Option<String>,
    #[serde(alias = "search_target", alias = "search_text_target")]
    search_target: Option<String>,
    #[serde(
        default,
        alias = "filetype",
        alias = "file_type",
        deserialize_with = "empty_string_as_none"
    )]
    file_type: Option<WorkshopFileType>,
}

impl From<SearchParams> for SearchQuery {
    fn from(params: SearchParams) -> Self {
        Self {
            query: params.q,
            tag: params.tag,
            status: params.status,
            sort: params.sort,
            page: params.page.unwrap_or(1),
            page_size: params.page_size.unwrap_or(30),
            language: params.lang,
            search_target: params.search_target,
            file_type: params.file_type,
        }
    }
}

fn validate_search_query(q: &SearchQuery) -> Result<(), (StatusCode, Json<Value>)> {
    if q.query
        .as_deref()
        .is_some_and(|s| s.trim().chars().count() > MAX_SEARCH_QUERY_CHARS)
    {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            format!("搜索关键词长度不能超过 {MAX_SEARCH_QUERY_CHARS} 个字符"),
        ));
    }
    if q.tag
        .as_deref()
        .is_some_and(|s| s.trim().chars().count() > MAX_SEARCH_TAG_CHARS)
    {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            format!("标签参数长度不能超过 {MAX_SEARCH_TAG_CHARS} 个字符"),
        ));
    }
    Ok(())
}

async fn search_items(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Query(params): Query<SearchParams>,
) -> Result<(HeaderMap, Json<SearchResponse>), (StatusCode, Json<Value>)> {
    let q = SearchQuery::from(params);
    validate_search_query(&q)?;
    let accept_language = headers
        .get_all(ACCEPT_LANGUAGE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join(",");
    let output = state
        .server
        .search
        .search(&q, Some(&accept_language))
        .await
        .map_err(search_err)?;
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        HeaderName::from_static("x-l4a-search-mode"),
        HeaderValue::from_static(output.mode.as_header_value()),
    );
    Ok((resp_headers, Json(output.response)))
}

async fn admin_search_items(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Query(params): Query<SearchParams>,
) -> ApiResult<SearchResponse> {
    state.require_admin(&headers)?;
    let q = SearchQuery::from(params);
    validate_search_query(&q)?;
    let resp = state
        .server
        .search
        .search_admin(&q)
        .await
        .map_err(search_err)?;
    Ok(Json(resp))
}

async fn get_collection(
    State(state): State<RouteState>,
    Path(collection_id): Path<String>,
    Query(query): Query<RefreshQuery>,
) -> ApiResult<WorkshopCollectionRecord> {
    let cid = WorkshopId::from_str(&collection_id)
        .map_err(|e| err_resp(StatusCode::BAD_REQUEST, e.to_string()))?;
    let mut col = state
        .server
        .sync
        .resolve_collection(cid, query.refresh)
        .await
        .map_err(internal_err)?
        .ok_or_else(|| err_resp(StatusCode::NOT_FOUND, format!("未找到合集 {cid}")))?;
    col.items = col.items.into_iter().map(with_catalog_source).collect();
    Ok(Json(col))
}

async fn admin_login(
    State(state): State<RouteState>,
    Json(req): Json<AdminLoginRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    let session = state
        .server
        .auth
        .login(&req.username, &req.password)
        .map_err(|(c, m)| err_resp(c, m))?;
    let secure = !state.server.config_store.get().admin.allow_insecure_http;
    let cookie = build_session_cookie(&session.token, state.server.auth.session_ttl_secs(), secure);
    let mut headers = HeaderMap::new();
    if let Ok(val) = HeaderValue::from_str(&cookie) {
        headers.insert(SET_COOKIE, val);
    }
    Ok((headers, Json(session)))
}

async fn admin_logout(State(state): State<RouteState>, headers: HeaderMap) -> impl IntoResponse {
    state.server.auth.logout(&headers);
    let mut resp_headers = HeaderMap::new();
    let secure = !state.server.config_store.get().admin.allow_insecure_http;
    if let Ok(val) = HeaderValue::from_str(&build_clear_session_cookie(secure)) {
        resp_headers.insert(SET_COOKIE, val);
    }
    (resp_headers, Json(json!({ "ok": true })))
}

async fn admin_session_me(
    State(state): State<RouteState>,
    headers: HeaderMap,
) -> Json<AdminSessionInfo> {
    Json(state.server.auth.session_info(&headers))
}

async fn admin_change_password(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Json(req): Json<AdminChangePasswordRequest>,
) -> ApiResult<Value> {
    let actor = state.require_admin(&headers)?;
    state
        .server
        .auth
        .change_password(
            &req.current_password,
            &req.new_password,
            &headers,
            |password| {
                state
                    .server
                    .config_store
                    .update(|cfg| cfg.admin.password = password.to_owned())
                    .map(|_| ())
                    .map_err(|err| {
                        tracing::error!(error = %err, "无法保存管理员密码");
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "无法保存管理员密码".into(),
                        )
                    })
            },
        )
        .map_err(|(c, m)| err_resp(c, m))?;

    let _ = state
        .db(move |repo| {
            repo.append_audit_log(
                None,
                &actor,
                "password_change",
                "管理员更新控制台密码并持久化至配置文件",
                None,
                None,
            )
        })
        .await;

    Ok(Json(json!({ "ok": true })))
}

async fn admin_overview(
    State(state): State<RouteState>,
    headers: HeaderMap,
) -> ApiResult<CatalogOverviewResponse> {
    state.require_admin(&headers)?;
    let stats = state.db(move |repo| repo.stats()).await?;
    let steam = state.server.upstream.status().await;
    let sync = state.server.sync.status().await.map_err(internal_err)?;
    let filter = state.server.filter.status().await;
    let cfg = state.server.config_store.get();

    Ok(Json(CatalogOverviewResponse {
        stats,
        steam,
        sync,
        filter,
        config_path: state
            .server
            .config_store
            .path()
            .map(|p| p.display().to_string()),
        bind_addr: cfg.server.bind,
        db_path: cfg.server.db_path.display().to_string(),
    }))
}

async fn admin_stats(
    State(state): State<RouteState>,
    headers: HeaderMap,
) -> ApiResult<CatalogStats> {
    state.require_admin(&headers)?;
    let stats = state.db(move |repo| repo.stats()).await?;
    Ok(Json(stats))
}

#[derive(Debug, Default, Deserialize)]
struct AuditQuery {
    limit: Option<usize>,
}

async fn admin_list_audit(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Query(query): Query<AuditQuery>,
) -> ApiResult<Vec<AuditLogEntry>> {
    state.require_admin(&headers)?;
    let limit = query.limit.unwrap_or(200).clamp(1, 500);
    let list = state.db(move |repo| repo.list_audit_logs(limit)).await?;
    Ok(Json(list))
}

async fn admin_sync_status(
    State(state): State<RouteState>,
    headers: HeaderMap,
) -> ApiResult<CatalogSyncStatus> {
    state.require_admin(&headers)?;
    let status = state.server.sync.status().await.map_err(internal_err)?;
    Ok(Json(status))
}

async fn admin_update_sync_config(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Json(dto): Json<CatalogSyncConfigDto>,
) -> ApiResult<CatalogSyncStatus> {
    let actor = state.require_admin(&headers)?;
    state
        .server
        .config_store
        .update(|cfg| cfg.sync.apply_dto(&dto))
        .map_err(internal_err)?;
    state.server.sync.notify_config_changed();

    let _ = state
        .db(move |repo| {
            repo.append_audit_log(
                None,
                &actor,
                "sync_config_updated",
                &format!(
                    "更新同步策略: read_through={}, ttl={}s, incremental={}, backfill={}",
                    dto.enable_read_through,
                    dto.item_ttl_secs,
                    dto.enable_incremental_sync,
                    dto.enable_backfill
                ),
                None,
                None,
            )
        })
        .await;

    let status = state.server.sync.status().await.map_err(internal_err)?;
    Ok(Json(status))
}

async fn admin_trigger_sync(
    State(state): State<RouteState>,
    headers: HeaderMap,
) -> ApiResult<CatalogSyncStatus> {
    state.require_admin(&headers)?;
    state
        .server
        .sync
        .run_incremental_once()
        .await
        .map_err(|e| err_resp(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let status = state.server.sync.status().await.map_err(internal_err)?;
    Ok(Json(status))
}

async fn admin_reset_backfill_cursor(
    State(state): State<RouteState>,
    headers: HeaderMap,
) -> ApiResult<CatalogSyncStatus> {
    let actor = state.require_admin(&headers)?;
    state
        .server
        .sync
        .reset_backfill_cursor()
        .await
        .map_err(internal_err)?;
    let _ = state
        .db(move |repo| {
            repo.append_audit_log(
                None,
                &actor,
                "backfill_reset",
                "重置全量创意工坊游标为 *",
                None,
                None,
            )
        })
        .await;
    let status = state.server.sync.status().await.map_err(internal_err)?;
    Ok(Json(status))
}

async fn admin_fetch_upstream(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Json(req): Json<AdminFetchUpstreamRequest>,
) -> ApiResult<AdminFetchUpstreamResponse> {
    state.require_admin(&headers)?;
    if req.ids.len() > MAX_BATCH_SIZE {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            format!("单次抓取最多支持 {MAX_BATCH_SIZE} 个 Workshop ID"),
        ));
    }
    let resp = state
        .server
        .sync
        .admin_force_fetch(&req.ids)
        .await
        .map_err(|e| err_resp(StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(resp))
}

async fn admin_steam_status(
    State(state): State<RouteState>,
    headers: HeaderMap,
) -> ApiResult<CatalogSteamStatus> {
    state.require_admin(&headers)?;
    Ok(Json(state.server.upstream.status().await))
}

async fn admin_steam_reconnect(
    State(state): State<RouteState>,
    headers: HeaderMap,
) -> ApiResult<CatalogSteamStatus> {
    state.require_admin(&headers)?;
    let status = state
        .server
        .upstream
        .reconnect()
        .await
        .map_err(|e| err_resp(StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(status))
}

async fn admin_steam_start_qr(
    State(state): State<RouteState>,
    headers: HeaderMap,
) -> ApiResult<SteamAuthFlowStatus> {
    state.require_admin(&headers)?;
    let flow = state
        .server
        .upstream
        .start_qr_auth()
        .await
        .map_err(|e| err_resp(StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(flow))
}

async fn admin_steam_start_credentials(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Json(req): Json<StartCredentialsAuthRequest>,
) -> ApiResult<SteamAuthFlowStatus> {
    state.require_admin(&headers)?;
    let flow = state
        .server
        .upstream
        .start_credentials_auth(req.account_name, req.password)
        .await
        .map_err(|e| err_resp(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(flow))
}

async fn admin_steam_submit_guard(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Json(req): Json<SubmitSteamGuardRequest>,
) -> ApiResult<SteamAuthFlowStatus> {
    state.require_admin(&headers)?;
    let flow = state
        .server
        .upstream
        .submit_guard_code(&req.flow_id, &req.code)
        .await
        .map_err(|e| err_resp(StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(flow))
}

async fn admin_steam_set_token(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Json(req): Json<SetRefreshTokenRequest>,
) -> ApiResult<CatalogSteamStatus> {
    let actor = state.require_admin(&headers)?;
    let account = req.account_name.trim().to_string();
    let status = state
        .server
        .upstream
        .set_refresh_token(req.account_name, req.refresh_token)
        .await
        .map_err(|e| err_resp(StatusCode::BAD_REQUEST, e.to_string()))?;

    let _ = state
        .db(move |repo| {
            repo.append_audit_log(
                None,
                &actor,
                "steam_auth_token",
                &format!("绑定 Steam 账号 {account} Refresh Token"),
                None,
                None,
            )
        })
        .await;
    Ok(Json(status))
}

async fn admin_steam_switch_anonymous(
    State(state): State<RouteState>,
    headers: HeaderMap,
) -> ApiResult<CatalogSteamStatus> {
    let actor = state.require_admin(&headers)?;
    let status = state
        .server
        .upstream
        .switch_anonymous()
        .await
        .map_err(|e| err_resp(StatusCode::BAD_GATEWAY, e.to_string()))?;

    let _ = state
        .db(move |repo| {
            repo.append_audit_log(
                None,
                &actor,
                "steam_auth_anonymous",
                "切换回匿名 SteamKit CM 会话",
                None,
                None,
            )
        })
        .await;
    Ok(Json(status))
}

async fn admin_get_item_detail(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Path(workshop_id): Path<String>,
) -> ApiResult<AdminItemDetailResponse> {
    state.require_admin(&headers)?;
    let wid = WorkshopId::from_str(&workshop_id)
        .map_err(|e| err_resp(StatusCode::BAD_REQUEST, e.to_string()))?;
    let detail = state
        .db(move |repo| repo.get_admin_item_detail(wid))
        .await?
        .ok_or_else(|| err_resp(StatusCode::NOT_FOUND, format!("缓存中不存在条目 {wid}")))?;
    Ok(Json(detail))
}

async fn admin_refresh_item(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Path(workshop_id): Path<String>,
) -> ApiResult<AdminItemDetailResponse> {
    state.require_admin(&headers)?;
    let wid = WorkshopId::from_str(&workshop_id)
        .map_err(|e| err_resp(StatusCode::BAD_REQUEST, e.to_string()))?;

    let existing = state.db(move |repo| repo.get_item(wid)).await?;
    if existing.as_ref().is_some_and(|e| e.is_admin_override) {
        return Err(err_resp(
            StatusCode::CONFLICT,
            "该条目处于管理员覆写锁定状态，请先解除覆写后再执行上游重刷",
        ));
    }

    state
        .server
        .sync
        .admin_refresh_cached(&[wid])
        .await
        .map_err(|e| err_resp(StatusCode::BAD_GATEWAY, e.to_string()))?;

    let detail = state
        .db(move |repo| repo.get_admin_item_detail(wid))
        .await?
        .ok_or_else(|| {
            err_resp(
                StatusCode::NOT_FOUND,
                format!("上游与本地均不存在条目 {wid}"),
            )
        })?;
    Ok(Json(detail))
}

async fn admin_override_item(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Path(workshop_id): Path<String>,
    Json(req): Json<AdminItemOverrideRequest>,
) -> ApiResult<WorkshopItemRecord> {
    let actor = state.require_admin(&headers)?;
    let wid = WorkshopId::from_str(&workshop_id)
        .map_err(|e| err_resp(StatusCode::BAD_REQUEST, e.to_string()))?;
    let updated = state
        .db(move |repo| repo.apply_admin_override(wid, &actor, req))
        .await?;
    Ok(Json(updated))
}

async fn admin_clear_item_override(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Path(workshop_id): Path<String>,
) -> ApiResult<AdminItemDetailResponse> {
    let actor = state.require_admin(&headers)?;
    let wid = WorkshopId::from_str(&workshop_id)
        .map_err(|e| err_resp(StatusCode::BAD_REQUEST, e.to_string()))?;

    let existed = state
        .db(move |repo| repo.clear_admin_override(wid, &actor))
        .await?;
    if !existed {
        return Err(err_resp(StatusCode::NOT_FOUND, format!("未找到条目 {wid}")));
    }

    if state.server.config_store.get().sync.enable_read_through {
        let _ = state.server.sync.admin_refresh_cached(&[wid]).await;
    }

    let detail = state
        .db(move |repo| repo.get_admin_item_detail(wid))
        .await?
        .ok_or_else(|| err_resp(StatusCode::NOT_FOUND, format!("未找到条目 {wid}")))?;
    Ok(Json(detail))
}

async fn guard_filter_enabled(
    State(state): State<RouteState>,
    request: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<Value>)> {
    if !state.server.filter.is_enabled() {
        return Err(err_resp(
            StatusCode::SERVICE_UNAVAILABLE,
            "内容过滤服务未启用",
        ));
    }
    let _admission_permit = state
        .server
        .filter
        .acquire_public_admission()
        .await
        .ok_or_else(|| {
            err_resp(
                StatusCode::TOO_MANY_REQUESTS,
                "内容过滤请求繁忙，请稍后重试",
            )
        })?;
    Ok(next.run(request).await)
}

fn public_filter_err(err: FilterServiceError) -> (StatusCode, Json<Value>) {
    match err {
        FilterServiceError::Disabled(_) => {
            err_resp(StatusCode::SERVICE_UNAVAILABLE, "内容过滤服务未启用")
        }
        FilterServiceError::Unavailable(detail) => {
            tracing::warn!(error = %detail, "公开内容过滤接口后端暂不可用");
            err_resp(StatusCode::SERVICE_UNAVAILABLE, "内容过滤服务暂不可用")
        }
        FilterServiceError::NotFound(msg) => err_resp(StatusCode::NOT_FOUND, msg),
        FilterServiceError::BadRequest(msg) => err_resp(StatusCode::BAD_REQUEST, msg),
        FilterServiceError::Internal(err) => internal_err(err),
    }
}

fn admin_filter_err(err: FilterServiceError) -> (StatusCode, Json<Value>) {
    match err {
        FilterServiceError::Disabled(msg) => err_resp(StatusCode::SERVICE_UNAVAILABLE, msg),
        FilterServiceError::Unavailable(msg) => err_resp(StatusCode::SERVICE_UNAVAILABLE, msg),
        FilterServiceError::NotFound(msg) => err_resp(StatusCode::NOT_FOUND, msg),
        FilterServiceError::BadRequest(msg) => err_resp(StatusCode::BAD_REQUEST, msg),
        FilterServiceError::Internal(err) => internal_err(err),
    }
}

async fn filter_info(State(state): State<RouteState>) -> ApiResult<CatalogFilterPublicInfo> {
    Ok(Json(state.server.filter.public_info().await))
}

async fn filter_predict(
    State(state): State<RouteState>,
    Json(req): Json<FilterPredictRequest>,
) -> ApiResult<FilterProbabilities> {
    let probs = state
        .server
        .filter
        .predict(req)
        .await
        .map_err(public_filter_err)?;
    Ok(Json(probs))
}

async fn admin_filter_status(
    State(state): State<RouteState>,
    headers: HeaderMap,
) -> ApiResult<CatalogFilterStatus> {
    state.require_admin(&headers)?;
    Ok(Json(state.server.filter.status().await))
}

async fn admin_update_filter_config(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Json(dto): Json<CatalogFilterConfigDto>,
) -> ApiResult<CatalogFilterStatus> {
    let actor = state.require_admin(&headers)?;
    if let Some(raw_url) = dto
        .upstream_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        crate::filter::validate_upstream_base_url(raw_url)
            .map_err(|msg| err_resp(StatusCode::BAD_REQUEST, msg))?;
    }
    state
        .server
        .config_store
        .update(|cfg| cfg.filter.apply_dto(&dto))
        .map_err(internal_err)?;
    let status = state.server.filter.reconfigure().await;

    let reason = format!(
        "更新内容过滤模型配置: enabled={}, backend={:?}, ttl={}s, threads={}, max_images={}, gore_w={:.2}, nsfw_w={:.2}, temp={:.2}, bias={:.2}",
        status.config.enabled,
        status.config.backend,
        status.config.cache_ttl_secs,
        status.config.params.threads,
        status.config.params.max_images,
        status.config.params.gore_weight,
        status.config.params.nsfw_weight,
        status.config.params.temperature,
        status.config.params.bias,
    );
    let _ = state
        .db(move |repo| {
            repo.append_audit_log(None, &actor, "filter_config_update", &reason, None, None)
        })
        .await;

    Ok(Json(status))
}

async fn admin_restart_filter(
    State(state): State<RouteState>,
    headers: HeaderMap,
) -> ApiResult<CatalogFilterStatus> {
    let actor = state.require_admin(&headers)?;
    let status = state.server.filter.restart().await;

    let reason = format!(
        "重启/探测内容过滤模型后端: backend={:?}, ready={}",
        status.config.backend, status.ready
    );
    let _ = state
        .db(move |repo| repo.append_audit_log(None, &actor, "filter_restart", &reason, None, None))
        .await;

    Ok(Json(status))
}

async fn admin_test_filter(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Json(req): Json<FilterPredictRequest>,
) -> ApiResult<FilterProbabilities> {
    state.require_admin(&headers)?;
    let probs = state
        .server
        .filter
        .predict(req)
        .await
        .map_err(admin_filter_err)?;
    Ok(Json(probs))
}
