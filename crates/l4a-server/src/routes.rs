use crate::admin_ui;
use crate::auth::{generate_token, hash_token, verify_admin, verify_contributor};
use crate::ingest::ingest_submission;
use crate::ServerState;
use axum::{
    extract::{Path, Query, State},
    http::{
        header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS},
        HeaderMap, StatusCode,
    },
    response::IntoResponse,
    routing::{get, patch, post},
    Json, Router,
};
use chrono::Utc;
use l4a_protocol::{
    AdminItemOverrideRequest, BatchItemsRequest, BatchItemsResponse, ConflictDecision,
    ContributorId, CreateContributorRequest, CreateContributorResponse, MetadataSource,
    SearchQuery, SubmissionBatchRequest, SubmissionBatchResponse, UpdateContributorRequest,
    WorkshopFileType, WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::str::FromStr;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

const MAX_BATCH_SIZE: usize = 200;
const ADMIN_OVERRIDE_TRUST_SCORE: i32 = 200;

pub fn router(state: ServerState) -> Router {
    let public_routes = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/items/search", get(search_items))
        .route("/api/v1/items/batch", post(batch_items))
        .route("/api/v1/items/{workshop_id}", get(get_item))
        .route("/api/v1/collections/{collection_id}", get(get_collection))
        .route("/api/v1/submissions", post(submit_batch))
        .layer(CorsLayer::permissive());

    let admin_routes = Router::new()
        .route("/admin", get(serve_admin_ui))
        .route("/api/v1/admin/stats", get(admin_stats))
        .route(
            "/api/v1/admin/contributors",
            get(admin_list_contributors).post(admin_create_contributor),
        )
        .route(
            "/api/v1/admin/contributors/{id}",
            patch(admin_update_contributor),
        )
        .route("/api/v1/admin/audit", get(admin_list_audit))
        .route(
            "/api/v1/admin/items/{workshop_id}/override",
            post(admin_override_item),
        )
        .route("/api/v1/admin/scraper/status", get(admin_scraper_status));

    Router::new()
        .merge(public_routes)
        .merge(admin_routes)
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<Value>)>;

fn err_resp(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.into() })))
}

fn internal_err(err: anyhow::Error) -> (StatusCode, Json<Value>) {
    tracing::error!(error = %err, "l4a-server 内部处理异常");
    err_resp(StatusCode::INTERNAL_SERVER_ERROR, "服务端内部错误")
}

async fn serve_admin_ui() -> impl IntoResponse {
    (
        [
            (CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                CONTENT_SECURITY_POLICY,
                "default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self' https: data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
            ),
            (X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        admin_ui::ADMIN_HTML,
    )
}

async fn health(State(state): State<ServerState>) -> ApiResult<Value> {
    let stats = state.repo.stats().map_err(internal_err)?;
    Ok(Json(json!({
        "status": "ok",
        "service": "l4a-server",
        "version": env!("CARGO_PKG_VERSION"),
        "items": stats.total_items,
    })))
}

async fn get_item(
    State(state): State<ServerState>,
    Path(workshop_id): Path<String>,
) -> ApiResult<WorkshopItemRecord> {
    let wid = WorkshopId::from_str(&workshop_id)
        .map_err(|e| err_resp(StatusCode::BAD_REQUEST, e.to_string()))?;
    let meta = state
        .repo
        .get_item(wid)
        .map_err(internal_err)?
        .ok_or_else(|| err_resp(StatusCode::NOT_FOUND, format!("未缓存条目 {wid}")))?;
    Ok(Json(meta.record))
}

async fn batch_items(
    State(state): State<ServerState>,
    Json(req): Json<BatchItemsRequest>,
) -> ApiResult<BatchItemsResponse> {
    if req.ids.len() > MAX_BATCH_SIZE {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            format!("单次批量查询最多支持 {MAX_BATCH_SIZE} 个条目"),
        ));
    }
    let mut seen = HashSet::with_capacity(req.ids.len());
    let deduped: Vec<WorkshopId> = req
        .ids
        .into_iter()
        .filter(|id| seen.insert(*id))
        .collect();
    let (items, missing_ids) = state.repo.get_batch(&deduped).map_err(internal_err)?;
    Ok(Json(BatchItemsResponse { items, missing_ids }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchParams {
    q: Option<String>,
    tag: Option<String>,
    status: Option<WorkshopItemStatus>,
    sort: Option<String>,
    page: Option<u32>,
    page_size: Option<u32>,
}

async fn search_items(
    State(state): State<ServerState>,
    Query(params): Query<SearchParams>,
) -> ApiResult<l4a_protocol::SearchResponse> {
    let q = SearchQuery {
        query: params.q,
        tag: params.tag,
        status: params.status,
        sort: params.sort,
        page: params.page.unwrap_or(1),
        page_size: params.page_size.unwrap_or(30),
    };
    let res = state.repo.search(&q).map_err(internal_err)?;
    Ok(Json(res))
}

async fn get_collection(
    State(state): State<ServerState>,
    Path(collection_id): Path<String>,
) -> ApiResult<l4a_protocol::WorkshopCollectionRecord> {
    let cid = WorkshopId::from_str(&collection_id)
        .map_err(|e| err_resp(StatusCode::BAD_REQUEST, e.to_string()))?;
    let col = state
        .repo
        .get_collection(cid)
        .map_err(internal_err)?
        .ok_or_else(|| err_resp(StatusCode::NOT_FOUND, format!("未缓存合集 {cid}")))?;
    Ok(Json(col))
}

async fn submit_batch(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(req): Json<SubmissionBatchRequest>,
) -> ApiResult<SubmissionBatchResponse> {
    if req.items.len() > MAX_BATCH_SIZE {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            format!("单次上报最多支持 {MAX_BATCH_SIZE} 个条目"),
        ));
    }
    let contributor = verify_contributor(&headers, &state.repo)
        .map_err(|(code, msg)| err_resp(code, msg))?;

    let mut receipts = Vec::with_capacity(req.items.len());
    for item in &req.items {
        let receipt = ingest_submission(&state.repo, &contributor, item).map_err(internal_err)?;
        receipts.push(receipt);
    }

    Ok(Json(SubmissionBatchResponse { receipts }))
}

async fn admin_stats(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> ApiResult<l4a_protocol::CatalogStats> {
    verify_admin(&headers, &state.admin_token).map_err(|(c, m)| err_resp(c, m))?;
    let stats = state.repo.stats().map_err(internal_err)?;
    Ok(Json(stats))
}

async fn admin_list_contributors(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> ApiResult<Vec<l4a_protocol::ContributorSummary>> {
    verify_admin(&headers, &state.admin_token).map_err(|(c, m)| err_resp(c, m))?;
    let list = state.repo.list_contributors().map_err(internal_err)?;
    Ok(Json(list))
}

async fn admin_create_contributor(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(req): Json<CreateContributorRequest>,
) -> ApiResult<CreateContributorResponse> {
    verify_admin(&headers, &state.admin_token).map_err(|(c, m)| err_resp(c, m))?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(err_resp(StatusCode::BAD_REQUEST, "贡献者名称不能为空"));
    }
    if let Some(w) = req.trust_weight {
        if !(1..=100).contains(&w) {
            return Err(err_resp(
                StatusCode::BAD_REQUEST,
                "trustWeight 必须在 1 到 100 之间",
            ));
        }
    }

    let id = ContributorId::generate();
    let plain_token = generate_token();
    let token_hash = hash_token(&plain_token);
    let weight = req
        .trust_weight
        .unwrap_or_else(|| req.trust_tier.default_weight());

    let contributor = state
        .repo
        .create_contributor(
            &id,
            name,
            req.steam_id.as_deref(),
            &token_hash,
            req.trust_tier,
            weight,
        )
        .map_err(internal_err)?;

    Ok(Json(CreateContributorResponse {
        contributor,
        token: plain_token,
    }))
}

async fn admin_update_contributor(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateContributorRequest>,
) -> ApiResult<l4a_protocol::ContributorSummary> {
    verify_admin(&headers, &state.admin_token).map_err(|(c, m)| err_resp(c, m))?;
    if let Some(w) = req.trust_weight {
        if !(1..=100).contains(&w) {
            return Err(err_resp(
                StatusCode::BAD_REQUEST,
                "trustWeight 必须在 1 到 100 之间",
            ));
        }
    }
    let cid = ContributorId::new(id).map_err(|e| err_resp(StatusCode::BAD_REQUEST, e.to_string()))?;
    let updated = state
        .repo
        .update_contributor(&cid, req.trust_tier, req.trust_weight, req.enabled)
        .map_err(internal_err)?
        .ok_or_else(|| err_resp(StatusCode::NOT_FOUND, "未找到该贡献者"))?;
    Ok(Json(updated))
}

async fn admin_list_audit(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> ApiResult<Vec<l4a_protocol::AuditLogEntry>> {
    verify_admin(&headers, &state.admin_token).map_err(|(c, m)| err_resp(c, m))?;
    let list = state.repo.list_audit_logs(200).map_err(internal_err)?;
    Ok(Json(list))
}

async fn admin_override_item(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(workshop_id): Path<String>,
    Json(req): Json<AdminItemOverrideRequest>,
) -> ApiResult<WorkshopItemRecord> {
    verify_admin(&headers, &state.admin_token).map_err(|(c, m)| err_resp(c, m))?;
    let wid = WorkshopId::from_str(&workshop_id)
        .map_err(|e| err_resp(StatusCode::BAD_REQUEST, e.to_string()))?;

    let existing = state.repo.get_item(wid).map_err(internal_err)?;
    let now = Utc::now();

    let next_record = if let Some(cur) = existing {
        let mut r = cur.record;
        r.status = req.status;
        if let Some(title) = req.title {
            r.title = title;
        }
        if let Some(desc) = req.description {
            r.description = Some(desc);
        }
        if let Some(url) = req.file_url {
            r.file_url = Some(url);
        }
        if let Some(preview) = req.preview_url {
            r.preview_url = Some(preview);
        }
        if let Some(deps) = req.dependencies {
            r.dependencies = deps;
        }
        r.source = MetadataSource::AdminOverride;
        r.fetched_at = now;
        r
    } else {
        WorkshopItemRecord {
            workshop_id: wid,
            file_type: WorkshopFileType::Item,
            status: req.status,
            title: req.title.unwrap_or_else(|| format!("Workshop #{wid}")),
            description: req.description,
            creator_steam_id: None,
            creator_name: None,
            preview_url: req.preview_url,
            file_url: req.file_url,
            file_size: None,
            time_created: None,
            time_updated: Some(now.timestamp()),
            tags: Vec::new(),
            dependencies: req.dependencies.unwrap_or_default(),
            collection_children: Vec::new(),
            subscriptions: None,
            favorites: None,
            source: MetadataSource::AdminOverride,
            fetched_at: now,
        }
    };

    state
        .repo
        .upsert_item(&next_record, ADMIN_OVERRIDE_TRUST_SCORE, true)
        .map_err(internal_err)?;
    let reason = req
        .reason
        .unwrap_or_else(|| "管理员手动强制覆写条目状态".to_string());
    state
        .repo
        .append_audit_log(
            wid,
            None,
            Some("admin"),
            ConflictDecision::Accepted,
            &reason,
            next_record.status,
            next_record.time_updated,
        )
        .map_err(internal_err)?;

    Ok(Json(next_record))
}

async fn admin_scraper_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> ApiResult<l4a_protocol::ScraperPlaceholderStatus> {
    verify_admin(&headers, &state.admin_token).map_err(|(c, m)| err_resp(c, m))?;
    let stats = state.repo.stats().map_err(internal_err)?;
    Ok(Json(state.scraper.status(stats.queued_scraper_jobs).await))
}
