use crate::config::CatalogConfigStore;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use l4a_protocol::{
    CatalogSteamStatus, MetadataSource, SteamAuthFlowKind, SteamAuthFlowState, SteamAuthFlowStatus,
    SteamAuthMode, SteamConnectionState, WorkshopDependency, WorkshopFetchOutcome,
    WorkshopFileType, WorkshopId, WorkshopItemRecord, WorkshopItemStatus, WorkshopPreview,
};
use qrcode::{render::svg, QrCode};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use steam_vent::auth::{
    AuthConfirmationHandler, DeviceConfirmationHandler, FileGuardDataStore,
    UserProvidedAuthConfirmationHandler,
};
use steam_vent::{Connection, ConnectionTrait, NetworkError, ServerList};
use steam_vent_proto_common::protobuf::{EnumOrUnknown, MessageField};
use steam_vent_proto_steam::steammessages_auth_steamclient::{
    CAuthentication_BeginAuthSessionViaQR_Request, CAuthentication_DeviceDetails,
    CAuthentication_PollAuthSessionStatus_Request, EAuthTokenPlatformType,
};
use steam_vent_proto_steam::steammessages_player_steamclient::CPlayer_GetPlayerLinkDetails_Request;
use steam_vent_proto_steam::steammessages_publishedfile_steamclient::{
    CPublishedFile_GetDetails_Request, CPublishedFile_QueryFiles_Request, PublishedFileDetails,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, WriteHalf};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::task::AbortHandle;

pub const QUERY_TYPE_RANKED_BY_PUBLICATION_DATE: u32 = 1;
pub const QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE: u32 = 21;
pub const MATCHING_FILE_TYPE_ITEMS: u32 = 0;
pub const MATCHING_FILE_TYPE_COLLECTIONS: u32 = 1;
const WORKSHOP_FILE_TYPE_COLLECTION: u32 = 2;

#[derive(Debug, Clone)]
pub struct WorkshopQueryPage {
    pub total: u64,
    pub next_cursor: Option<String>,
    pub items: Vec<WorkshopItemRecord>,
}

#[async_trait]
pub trait WorkshopUpstream: Send + Sync {
    async fn fetch_details(&self, ids: &[WorkshopId]) -> anyhow::Result<Vec<WorkshopFetchOutcome>>;

    async fn query_files(
        &self,
        query_type: u32,
        file_type: u32,
        cursor: &str,
        num_per_page: u32,
    ) -> anyhow::Result<WorkshopQueryPage>;

    async fn status(&self) -> CatalogSteamStatus;

    async fn reconnect(&self) -> anyhow::Result<CatalogSteamStatus>;

    async fn start_qr_auth(&self) -> anyhow::Result<SteamAuthFlowStatus>;

    async fn start_credentials_auth(
        &self,
        account_name: String,
        password: String,
    ) -> anyhow::Result<SteamAuthFlowStatus>;

    async fn submit_guard_code(
        &self,
        flow_id: &str,
        code: &str,
    ) -> anyhow::Result<SteamAuthFlowStatus>;

    async fn set_refresh_token(
        &self,
        account_name: String,
        refresh_token: String,
    ) -> anyhow::Result<CatalogSteamStatus>;

    async fn switch_anonymous(&self) -> anyhow::Result<CatalogSteamStatus>;
}

struct ActiveFlowHandle {
    status: SteamAuthFlowStatus,
    abort_handle: Option<AbortHandle>,
    code_writer: Option<WriteHalf<tokio::io::DuplexStream>>,
}

#[derive(Default)]
struct ConnectionMeta {
    state: SteamConnectionState,
    steam_id: Option<String>,
    cell_id: Option<u32>,
    country_code: Option<String>,
    connected_since: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

struct SteamCmInner {
    config_store: Arc<CatalogConfigStore>,
    server_list: Mutex<Option<ServerList>>,
    connection: Mutex<Option<Connection>>,
    meta: RwLock<ConnectionMeta>,
    active_flow: Mutex<Option<ActiveFlowHandle>>,
    persona_cache: RwLock<HashMap<u64, String>>,
    request_gate: Arc<Semaphore>,
    total_requests: AtomicU64,
    failed_requests: AtomicU64,
}

#[derive(Clone)]
pub struct SteamCmClient {
    inner: Arc<SteamCmInner>,
}

impl SteamCmClient {
    pub fn new(config_store: Arc<CatalogConfigStore>) -> Self {
        let concurrency = config_store.get().steam.request_concurrency.clamp(1, 16);
        Self {
            inner: Arc::new(SteamCmInner {
                config_store,
                server_list: Mutex::new(None),
                connection: Mutex::new(None),
                meta: RwLock::new(ConnectionMeta {
                    state: SteamConnectionState::Disconnected,
                    ..Default::default()
                }),
                active_flow: Mutex::new(None),
                persona_cache: RwLock::new(HashMap::new()),
                request_gate: Arc::new(Semaphore::new(concurrency)),
                total_requests: AtomicU64::new(0),
                failed_requests: AtomicU64::new(0),
            }),
        }
    }
}

impl SteamCmInner {
    async fn ensure_server_list(&self) -> anyhow::Result<ServerList> {
        let mut guard = self.server_list.lock().await;
        if let Some(ref list) = *guard {
            return Ok(list.clone());
        }
        let discovered = ServerList::discover()
            .await
            .map_err(|e| anyhow::anyhow!("发现 Steam CM 节点列表失败: {e}"))?;
        *guard = Some(discovered.clone());
        Ok(discovered)
    }

    async fn connect_fresh(&self) -> anyhow::Result<Connection> {
        {
            let mut m = self.meta.write().await;
            m.state = SteamConnectionState::Connecting;
        }
        let server_list = match self.ensure_server_list().await {
            Ok(s) => s,
            Err(err) => {
                let mut m = self.meta.write().await;
                m.state = SteamConnectionState::Error;
                m.last_error = Some(err.to_string());
                return Err(err);
            }
        };

        let steam_cfg = self.config_store.get().steam;
        let conn_res = match (
            steam_cfg.mode,
            steam_cfg.account_name.as_deref(),
            steam_cfg.refresh_token.as_deref(),
        ) {
            (SteamAuthMode::Account, Some(acc), Some(tok))
                if !acc.trim().is_empty() && !tok.trim().is_empty() =>
            {
                Connection::access(&server_list, acc.trim(), tok.trim())
                    .await
                    .map_err(|e| anyhow::anyhow!("使用 Refresh Token 登录 Steam CM 失败: {e}"))
            }
            _ => Connection::anonymous(&server_list)
                .await
                .map_err(|e| anyhow::anyhow!("建立匿名 Steam CM 会话失败: {e}")),
        };

        match conn_res {
            Ok(mut conn) => {
                conn.set_timeout(Duration::from_secs(20));
                self.record_connected(&conn).await;
                Ok(conn)
            }
            Err(err) => {
                let mut m = self.meta.write().await;
                m.state = SteamConnectionState::Error;
                m.last_error = Some(err.to_string());
                Err(err)
            }
        }
    }

    async fn record_connected(&self, conn: &Connection) {
        let mut m = self.meta.write().await;
        m.state = SteamConnectionState::Connected;
        m.steam_id = Some(u64::from(conn.steam_id()).to_string());
        m.cell_id = Some(conn.cell_id());
        m.country_code = conn.ip_country_code();
        m.connected_since = Some(Utc::now());
        m.last_error = None;
    }

    async fn get_or_connect(&self) -> anyhow::Result<Connection> {
        let mut guard = self.connection.lock().await;
        if let Some(ref conn) = *guard {
            return Ok(conn.clone());
        }
        let fresh = self.connect_fresh().await?;
        *guard = Some(fresh.clone());
        Ok(fresh)
    }

    async fn invalidate_and_reconnect(&self) -> anyhow::Result<Connection> {
        {
            let mut m = self.meta.write().await;
            m.state = SteamConnectionState::Reconnecting;
        }
        let mut guard = self.connection.lock().await;
        *guard = None;
        let fresh = self.connect_fresh().await?;
        *guard = Some(fresh.clone());
        Ok(fresh)
    }

    async fn install_authenticated_connection(
        &self,
        mut conn: Connection,
        account_name: String,
        refresh_token: String,
    ) -> anyhow::Result<()> {
        conn.set_timeout(Duration::from_secs(20));
        self.config_store.update(|cfg| {
            cfg.steam.mode = SteamAuthMode::Account;
            cfg.steam.account_name = Some(account_name.clone());
            cfg.steam.refresh_token = Some(refresh_token.clone());
        })?;
        self.record_connected(&conn).await;
        *self.connection.lock().await = Some(conn);
        Ok(())
    }

    async fn resolve_persona_names(&self, conn: &Connection, steam_ids: &[u64]) {
        if self.config_store.get().steam.mode != SteamAuthMode::Account || steam_ids.is_empty() {
            return;
        }
        let missing: Vec<u64> = {
            let cache = self.persona_cache.read().await;
            steam_ids
                .iter()
                .copied()
                .filter(|id| *id > 0 && !cache.contains_key(id))
                .collect()
        };
        if missing.is_empty() {
            return;
        }

        for chunk in missing.chunks(50) {
            let req = CPlayer_GetPlayerLinkDetails_Request {
                steamids: chunk.to_vec(),
                ..Default::default()
            };
            if let Ok(resp) = conn.service_method(req).await {
                let mut cache = self.persona_cache.write().await;
                for acc in resp.accounts {
                    let sid = acc.public_data.steamid();
                    if let Some(ref name) = acc.public_data.persona_name {
                        let trimmed = name.trim();
                        if sid > 0 && !trimmed.is_empty() {
                            cache.insert(sid, trimmed.to_string());
                        }
                    }
                }
            }
        }
    }

    async fn map_found_details(
        &self,
        conn: &Connection,
        details: &[PublishedFileDetails],
    ) -> Vec<WorkshopFetchOutcome> {
        let now = Utc::now();
        let creators: Vec<u64> = details
            .iter()
            .filter_map(|d| d.creator.filter(|c| *c > 0))
            .collect();
        self.resolve_persona_names(conn, &creators).await;
        let persona_map = self.persona_cache.read().await;

        let mut outcomes = Vec::with_capacity(details.len());
        for d in details {
            let Some(wid) = d.publishedfileid.and_then(|id| WorkshopId::new(id).ok()) else {
                continue;
            };
            match d.result.unwrap_or(0) {
                1 => {
                    if d.banned.unwrap_or(false) {
                        outcomes.push(WorkshopFetchOutcome::Removed {
                            workshop_id: wid,
                            source: MetadataSource::SteamKit,
                            observed_at: now,
                            reason: d
                                .ban_reason
                                .clone()
                                .or_else(|| Some("Banned on Steam Workshop".into())),
                        });
                        continue;
                    }
                    let creator_name = d.creator.and_then(|cid| persona_map.get(&cid).cloned());
                    outcomes.push(WorkshopFetchOutcome::Found {
                        item: details_to_record(wid, d, creator_name, now),
                    });
                }
                9 => {
                    outcomes.push(WorkshopFetchOutcome::Removed {
                        workshop_id: wid,
                        source: MetadataSource::SteamKit,
                        observed_at: now,
                        reason: Some("Steam CM result=9 (FileNotFound/Removed)".into()),
                    });
                }
                code => {
                    outcomes.push(WorkshopFetchOutcome::PrivateOrUnavailable {
                        workshop_id: wid,
                        source: MetadataSource::SteamKit,
                        observed_at: now,
                        reason: Some(format!("Steam CM result={code}")),
                    });
                }
            }
        }
        outcomes
    }

    async fn cancel_active_flow(&self) {
        let mut guard = self.active_flow.lock().await;
        if let Some(prev) = guard.take() {
            if let Some(abort) = prev.abort_handle {
                abort.abort();
            }
        }
    }

    async fn status(&self) -> CatalogSteamStatus {
        let cfg = self.config_store.get().steam;
        let m = self.meta.read().await;
        let flow = self
            .active_flow
            .lock()
            .await
            .as_ref()
            .map(|f| f.status.clone());

        CatalogSteamStatus {
            provider_name: "steam-vent (SteamKit CM)".to_string(),
            mode: cfg.mode,
            connection_state: m.state,
            steam_id: m.steam_id.clone(),
            account_name: cfg.account_name,
            cell_id: m.cell_id,
            country_code: m.country_code.clone(),
            connected_since: m.connected_since,
            total_requests: self.total_requests.load(Ordering::Relaxed),
            failed_requests: self.failed_requests.load(Ordering::Relaxed),
            last_error: m.last_error.clone(),
            active_auth_flow: flow,
        }
    }
}

fn should_retry_network(err: &NetworkError) -> bool {
    !matches!(err, NetworkError::ApiError(_))
}

pub fn render_qr_svg(url: &str) -> Option<String> {
    let code = QrCode::new(url.as_bytes()).ok()?;
    Some(
        code.render::<svg::Color<'_>>()
            .min_dimensions(200, 200)
            .dark_color(svg::Color("#111815"))
            .light_color(svg::Color("#ffffff"))
            .build(),
    )
}

fn details_to_record(
    wid: WorkshopId,
    d: &PublishedFileDetails,
    creator_name: Option<String>,
    fetched_at: DateTime<Utc>,
) -> WorkshopItemRecord {
    let is_collection = d.file_type == Some(WORKSHOP_FILE_TYPE_COLLECTION);
    let file_type = if is_collection {
        WorkshopFileType::Collection
    } else {
        WorkshopFileType::Item
    };

    let mut dependencies = Vec::new();
    let mut collection_children = Vec::new();
    for child in &d.children {
        if let Some(cid) = child
            .publishedfileid
            .and_then(|raw| WorkshopId::new(raw).ok())
        {
            if is_collection {
                collection_children.push(cid);
            } else {
                dependencies.push(WorkshopDependency {
                    workshop_id: cid,
                    title: None,
                    required: true,
                });
            }
        }
    }

    let tags: Vec<String> = d
        .tags
        .iter()
        .filter_map(|t| {
            t.display_name
                .as_deref()
                .or(t.tag.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
        })
        .collect();

    let previews: Vec<WorkshopPreview> = d
        .previews
        .iter()
        .map(|p| WorkshopPreview {
            preview_id: p.previewid.filter(|id| *id > 0).map(|id| id.to_string()),
            url: p
                .url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string),
            youtube_video_id: p
                .youtubevideoid
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string),
            preview_type: p.preview_type.unwrap_or(0),
        })
        .collect();

    let title = d
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("Workshop #{wid}"));

    let description = d
        .file_description
        .as_deref()
        .or(d.short_description.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let preview_url = d
        .preview_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let file_url = d
        .file_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let status = match d.visibility.unwrap_or(0) {
        0 | 3 => WorkshopItemStatus::Active,
        _ => WorkshopItemStatus::PrivateOrUnavailable,
    };

    WorkshopItemRecord {
        workshop_id: wid,
        file_type,
        status,
        title,
        description,
        creator_steam_id: d.creator.filter(|c| *c > 0).map(|c| c.to_string()),
        creator_name,
        preview_url,
        previews,
        file_url,
        file_size: d.file_size.filter(|s| *s > 0),
        time_created: d.time_created.filter(|t| *t > 0).map(i64::from),
        time_updated: d.time_updated.filter(|t| *t > 0).map(i64::from),
        tags,
        dependencies,
        collection_children,
        subscriptions: d.subscriptions.map(u64::from),
        favorites: d.favorited.map(u64::from),
        source: MetadataSource::SteamKit,
        fetched_at,
    }
}

#[async_trait]
impl WorkshopUpstream for SteamCmClient {
    async fn fetch_details(&self, ids: &[WorkshopId]) -> anyhow::Result<Vec<WorkshopFetchOutcome>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let _permit = self.inner.request_gate.acquire().await?;
        let app_id = self.inner.config_store.get().steam.app_id;
        let mut conn = self.inner.get_or_connect().await?;
        let mut all_outcomes = Vec::with_capacity(ids.len());

        for chunk in ids.chunks(100) {
            self.inner.total_requests.fetch_add(1, Ordering::Relaxed);
            let make_req = || CPublishedFile_GetDetails_Request {
                publishedfileids: chunk.iter().map(|id| id.as_u64()).collect(),
                includetags: Some(true),
                includeadditionalpreviews: Some(true),
                includechildren: Some(true),
                includevotes: Some(true),
                appid: Some(app_id),
                ..Default::default()
            };

            let resp = match conn.service_method(make_req()).await {
                Ok(r) => r,
                Err(err) if should_retry_network(&err) => {
                    conn = self.inner.invalidate_and_reconnect().await?;
                    conn.service_method(make_req()).await.map_err(|e| {
                        self.inner.failed_requests.fetch_add(1, Ordering::Relaxed);
                        anyhow::anyhow!("Steam CM GetDetails 重试失败: {e}")
                    })?
                }
                Err(err) => {
                    self.inner.failed_requests.fetch_add(1, Ordering::Relaxed);
                    let mut m = self.inner.meta.write().await;
                    m.last_error = Some(err.to_string());
                    return Err(anyhow::anyhow!("Steam CM GetDetails 调用失败: {err}"));
                }
            };

            let mapped = self
                .inner
                .map_found_details(&conn, &resp.publishedfiledetails)
                .await;
            all_outcomes.extend(mapped);
        }

        Ok(all_outcomes)
    }

    async fn query_files(
        &self,
        query_type: u32,
        file_type: u32,
        cursor: &str,
        num_per_page: u32,
    ) -> anyhow::Result<WorkshopQueryPage> {
        let _permit = self.inner.request_gate.acquire().await?;
        let app_id = self.inner.config_store.get().steam.app_id;
        let mut conn = self.inner.get_or_connect().await?;
        self.inner.total_requests.fetch_add(1, Ordering::Relaxed);

        let cursor_str = if cursor.trim().is_empty() {
            "*".to_string()
        } else {
            cursor.trim().to_string()
        };

        let make_req = || CPublishedFile_QueryFiles_Request {
            query_type: Some(query_type),
            cursor: Some(cursor_str.clone()),
            numperpage: Some(num_per_page.clamp(1, 100)),
            appid: Some(app_id),
            creator_appid: Some(app_id),
            filetype: Some(file_type),
            return_vote_data: Some(true),
            return_tags: Some(true),
            return_previews: Some(true),
            return_children: Some(true),
            return_short_description: Some(true),
            return_details: Some(true),
            ..Default::default()
        };

        let resp = match conn.service_method(make_req()).await {
            Ok(r) => r,
            Err(err) if should_retry_network(&err) => {
                conn = self.inner.invalidate_and_reconnect().await?;
                conn.service_method(make_req()).await.map_err(|e| {
                    self.inner.failed_requests.fetch_add(1, Ordering::Relaxed);
                    anyhow::anyhow!("Steam CM QueryFiles 重试失败: {e}")
                })?
            }
            Err(err) => {
                self.inner.failed_requests.fetch_add(1, Ordering::Relaxed);
                let mut m = self.inner.meta.write().await;
                m.last_error = Some(err.to_string());
                return Err(anyhow::anyhow!("Steam CM QueryFiles 调用失败: {err}"));
            }
        };

        let outcomes = self
            .inner
            .map_found_details(&conn, &resp.publishedfiledetails)
            .await;
        let items = outcomes
            .into_iter()
            .filter_map(|o| match o {
                WorkshopFetchOutcome::Found { item } => Some(item),
                _ => None,
            })
            .collect();

        Ok(WorkshopQueryPage {
            total: u64::from(resp.total.unwrap_or(0)),
            next_cursor: resp.next_cursor.filter(|s| !s.trim().is_empty()),
            items,
        })
    }

    async fn status(&self) -> CatalogSteamStatus {
        self.inner.status().await
    }

    async fn reconnect(&self) -> anyhow::Result<CatalogSteamStatus> {
        self.inner.invalidate_and_reconnect().await?;
        Ok(self.inner.status().await)
    }

    async fn start_qr_auth(&self) -> anyhow::Result<SteamAuthFlowStatus> {
        self.inner.cancel_active_flow().await;
        let server_list = self.inner.ensure_server_list().await?;
        let mut anon_conn = Connection::anonymous(&server_list)
            .await
            .map_err(|e| anyhow::anyhow!("创建 QR 认证会话失败: {e}"))?;
        anon_conn.set_timeout(Duration::from_secs(20));

        let qr_req = CAuthentication_BeginAuthSessionViaQR_Request {
            device_friendly_name: Some("Left4Addons Catalog Server".to_string()),
            platform_type: Some(EnumOrUnknown::new(
                EAuthTokenPlatformType::k_EAuthTokenPlatformType_SteamClient,
            )),
            device_details: MessageField::some(CAuthentication_DeviceDetails {
                device_friendly_name: Some("Left4Addons Catalog Server".to_string()),
                platform_type: Some(EnumOrUnknown::new(
                    EAuthTokenPlatformType::k_EAuthTokenPlatformType_SteamClient,
                )),
                os_type: Some(1),
                ..Default::default()
            }),
            website_id: Some("Client".to_string()),
            ..Default::default()
        };

        let started = anon_conn
            .service_method(qr_req)
            .await
            .map_err(|e| anyhow::anyhow!("发起 Steam QR 登录请求失败: {e}"))?;

        let flow_id = format!("qr_{}", uuid::Uuid::new_v4().simple());
        let challenge_url = started.challenge_url().to_string();
        let initial_status = SteamAuthFlowStatus {
            flow_id: flow_id.clone(),
            kind: SteamAuthFlowKind::Qr,
            state: SteamAuthFlowState::WaitingForScan,
            challenge_url: Some(challenge_url.clone()),
            qr_svg: render_qr_svg(&challenge_url),
            code_prompt: None,
            account_name: None,
            error: None,
            updated_at: Utc::now(),
        };

        let mut client_id = started.client_id();
        let request_id = started.request_id().to_vec();
        let poll_interval = Duration::from_secs_f32(started.interval().clamp(1.5, 5.0));

        let inner = self.inner.clone();
        let flow_id_bg = flow_id.clone();
        let join = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(poll_interval).await;
                let poll_req = CAuthentication_PollAuthSessionStatus_Request {
                    client_id: Some(client_id),
                    request_id: Some(request_id.clone()),
                    ..Default::default()
                };
                match anon_conn.service_method(poll_req).await {
                    Ok(resp) => {
                        if resp.has_new_client_id() {
                            client_id = resp.new_client_id();
                        }
                        if resp.has_new_challenge_url() {
                            let next_url = resp.new_challenge_url().to_string();
                            let mut fg = inner.active_flow.lock().await;
                            if let Some(ref mut f) = *fg {
                                if f.status.flow_id == flow_id_bg {
                                    f.status.challenge_url = Some(next_url.clone());
                                    f.status.qr_svg = render_qr_svg(&next_url);
                                    f.status.updated_at = Utc::now();
                                }
                            }
                        }
                        if resp.has_refresh_token() && resp.has_account_name() {
                            let account = resp.account_name().to_string();
                            let refresh_tok = resp.refresh_token().to_string();
                            let connect_res =
                                Connection::access(&server_list, &account, &refresh_tok).await;
                            match connect_res {
                                Ok(auth_conn) => {
                                    if let Err(err) = inner
                                        .install_authenticated_connection(
                                            auth_conn,
                                            account.clone(),
                                            refresh_tok,
                                        )
                                        .await
                                    {
                                        update_flow_failed(&inner, &flow_id_bg, err.to_string())
                                            .await;
                                        return;
                                    }
                                    let mut fg = inner.active_flow.lock().await;
                                    if let Some(ref mut f) = *fg {
                                        if f.status.flow_id == flow_id_bg {
                                            f.status.state = SteamAuthFlowState::Succeeded;
                                            f.status.account_name = Some(account);
                                            f.status.qr_svg = None;
                                            f.status.updated_at = Utc::now();
                                        }
                                    }
                                    return;
                                }
                                Err(err) => {
                                    update_flow_failed(
                                        &inner,
                                        &flow_id_bg,
                                        format!("使用扫码 Token 建立会话失败: {err}"),
                                    )
                                    .await;
                                    return;
                                }
                            }
                        }
                    }
                    Err(err) => {
                        update_flow_failed(&inner, &flow_id_bg, format!("轮询扫码状态失败: {err}"))
                            .await;
                        return;
                    }
                }
            }
            update_flow_failed(&inner, &flow_id_bg, "二维码已超时过期，请重新生成".into()).await;
        });

        *self.inner.active_flow.lock().await = Some(ActiveFlowHandle {
            status: initial_status.clone(),
            abort_handle: Some(join.abort_handle()),
            code_writer: None,
        });

        Ok(initial_status)
    }

    async fn start_credentials_auth(
        &self,
        account_name: String,
        password: String,
    ) -> anyhow::Result<SteamAuthFlowStatus> {
        let account = account_name.trim().to_string();
        if account.is_empty() || password.is_empty() {
            anyhow::bail!("账号名与密码不能为空");
        }

        self.inner.cancel_active_flow().await;
        let server_list = self.inner.ensure_server_list().await?;
        let guard_path = self.inner.config_store.get().steam.guard_data_path;

        let (code_stream_handler, code_stream_controller) = tokio::io::duplex(256);
        let (handler_read, _) = tokio::io::split(code_stream_handler);
        let (_, controller_write) = tokio::io::split(code_stream_controller);

        let (mut prompt_read, prompt_write) = tokio::io::duplex(512);

        let flow_id = format!("cred_{}", uuid::Uuid::new_v4().simple());
        let initial_status = SteamAuthFlowStatus {
            flow_id: flow_id.clone(),
            kind: SteamAuthFlowKind::Credentials,
            state: SteamAuthFlowState::Authenticating,
            challenge_url: None,
            qr_svg: None,
            code_prompt: None,
            account_name: Some(account.clone()),
            error: None,
            updated_at: Utc::now(),
        };

        let inner_prompt = self.inner.clone();
        let flow_id_prompt = flow_id.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 256];
            if let Ok(n) = prompt_read.read(&mut buf).await {
                if n > 0 {
                    let prompt = String::from_utf8_lossy(&buf[..n]).trim().to_string();
                    let mut fg = inner_prompt.active_flow.lock().await;
                    if let Some(ref mut f) = *fg {
                        if f.status.flow_id == flow_id_prompt
                            && f.status.state == SteamAuthFlowState::Authenticating
                        {
                            f.status.state = SteamAuthFlowState::WaitingForCode;
                            f.status.code_prompt = Some(prompt);
                            f.status.updated_at = Utc::now();
                        }
                    }
                }
            }
        });

        let inner_login = self.inner.clone();
        let flow_id_login = flow_id.clone();
        let account_login = account.clone();
        let join = tokio::spawn(async move {
            let confirmation = UserProvidedAuthConfirmationHandler::new(handler_read, prompt_write)
                .or(DeviceConfirmationHandler);
            let guard_store = FileGuardDataStore::new(guard_path);
            let login_res = Connection::login(
                &server_list,
                &account_login,
                &password,
                guard_store,
                confirmation,
            )
            .await;

            match login_res {
                Ok(conn) => {
                    let Some(refresh_token) = conn.access_token().map(ToString::to_string) else {
                        update_flow_failed(
                            &inner_login,
                            &flow_id_login,
                            "Steam CM 未返回持久化 Refresh Token".into(),
                        )
                        .await;
                        return;
                    };
                    if let Err(err) = inner_login
                        .install_authenticated_connection(
                            conn,
                            account_login.clone(),
                            refresh_token,
                        )
                        .await
                    {
                        update_flow_failed(&inner_login, &flow_id_login, err.to_string()).await;
                        return;
                    }
                    let mut fg = inner_login.active_flow.lock().await;
                    if let Some(ref mut f) = *fg {
                        if f.status.flow_id == flow_id_login {
                            f.status.state = SteamAuthFlowState::Succeeded;
                            f.status.code_prompt = None;
                            f.status.updated_at = Utc::now();
                            f.code_writer = None;
                        }
                    }
                }
                Err(err) => {
                    update_flow_failed(
                        &inner_login,
                        &flow_id_login,
                        format!("Steam 账号认证失败: {err}"),
                    )
                    .await;
                }
            }
        });

        *self.inner.active_flow.lock().await = Some(ActiveFlowHandle {
            status: initial_status.clone(),
            abort_handle: Some(join.abort_handle()),
            code_writer: Some(controller_write),
        });

        // 稍作等待以便若需要 Steam Guard 验证码可直接返回 waiting_for_code 状态
        tokio::time::sleep(Duration::from_millis(250)).await;
        let current = self
            .inner
            .active_flow
            .lock()
            .await
            .as_ref()
            .map(|f| f.status.clone())
            .unwrap_or(initial_status);
        Ok(current)
    }

    async fn submit_guard_code(
        &self,
        flow_id: &str,
        code: &str,
    ) -> anyhow::Result<SteamAuthFlowStatus> {
        let trimmed = code.trim();
        if trimmed.is_empty() {
            anyhow::bail!("验证码不能为空");
        }
        let mut guard = self.inner.active_flow.lock().await;
        let Some(ref mut handle) = *guard else {
            anyhow::bail!("当前没有进行中的 Steam 登录流程");
        };
        if handle.status.flow_id != flow_id {
            anyhow::bail!("登录流程 ID 不匹配或已过期");
        }
        let Some(mut writer) = handle.code_writer.take() else {
            anyhow::bail!("当前流程不接受验证码或验证码已提交");
        };
        writer
            .write_all(format!("{trimmed}\n").as_bytes())
            .await
            .map_err(|e| anyhow::anyhow!("发送 Steam Guard 验证码失败: {e}"))?;
        writer.flush().await.ok();

        handle.status.state = SteamAuthFlowState::Authenticating;
        handle.status.updated_at = Utc::now();
        Ok(handle.status.clone())
    }

    async fn set_refresh_token(
        &self,
        account_name: String,
        refresh_token: String,
    ) -> anyhow::Result<CatalogSteamStatus> {
        let account = account_name.trim().to_string();
        let token = refresh_token.trim().to_string();
        if account.is_empty() || token.is_empty() {
            anyhow::bail!("账号名与 Refresh Token 不能为空");
        }
        self.inner.cancel_active_flow().await;
        let server_list = self.inner.ensure_server_list().await?;
        let conn = Connection::access(&server_list, &account, &token)
            .await
            .map_err(|e| anyhow::anyhow!("验证 Refresh Token 失败: {e}"))?;
        self.inner
            .install_authenticated_connection(conn, account, token)
            .await?;
        Ok(self.inner.status().await)
    }

    async fn switch_anonymous(&self) -> anyhow::Result<CatalogSteamStatus> {
        self.inner.cancel_active_flow().await;
        self.inner.config_store.update(|cfg| {
            cfg.steam.mode = SteamAuthMode::Anonymous;
            cfg.steam.account_name = None;
            cfg.steam.refresh_token = None;
        })?;
        self.inner.persona_cache.write().await.clear();
        self.inner.invalidate_and_reconnect().await?;
        Ok(self.inner.status().await)
    }
}

async fn update_flow_failed(inner: &SteamCmInner, flow_id: &str, message: String) {
    let mut fg = inner.active_flow.lock().await;
    if let Some(ref mut f) = *fg {
        if f.status.flow_id == flow_id {
            f.status.state = SteamAuthFlowState::Failed;
            f.status.error = Some(message);
            f.status.updated_at = Utc::now();
            f.code_writer = None;
        }
    }
}

#[derive(Default)]
struct MockUpstreamState {
    outcomes: HashMap<WorkshopId, WorkshopFetchOutcome>,
    query_pages: HashMap<(u32, u32, String), WorkshopQueryPage>,
    fetch_counts: HashMap<WorkshopId, u64>,
    fail_fetches: bool,
    mode: SteamAuthMode,
    account_name: Option<String>,
    active_flow: Option<SteamAuthFlowStatus>,
    total_requests: u64,
}

/// 供离线单元与集成测试使用的确定性 SteamKit CM Mock 上游。
#[derive(Clone, Default)]
pub struct MockWorkshopUpstream {
    inner: Arc<StdMutex<MockUpstreamState>>,
}

impl MockWorkshopUpstream {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_item(&self, mut item: WorkshopItemRecord) {
        item.source = MetadataSource::SteamKit;
        let wid = item.workshop_id;
        self.inner
            .lock()
            .unwrap()
            .outcomes
            .insert(wid, WorkshopFetchOutcome::Found { item });
    }

    pub fn insert_outcome(&self, wid: WorkshopId, outcome: WorkshopFetchOutcome) {
        self.inner.lock().unwrap().outcomes.insert(wid, outcome);
    }

    pub fn insert_query_page(
        &self,
        query_type: u32,
        file_type: u32,
        cursor: impl Into<String>,
        page: WorkshopQueryPage,
    ) {
        self.inner
            .lock()
            .unwrap()
            .query_pages
            .insert((query_type, file_type, cursor.into()), page);
    }

    pub fn set_fail_fetches(&self, fail: bool) {
        self.inner.lock().unwrap().fail_fetches = fail;
    }

    pub fn fetch_count(&self, wid: WorkshopId) -> u64 {
        self.inner
            .lock()
            .unwrap()
            .fetch_counts
            .get(&wid)
            .copied()
            .unwrap_or(0)
    }
}

#[async_trait]
impl WorkshopUpstream for MockWorkshopUpstream {
    async fn fetch_details(&self, ids: &[WorkshopId]) -> anyhow::Result<Vec<WorkshopFetchOutcome>> {
        let mut guard = self.inner.lock().unwrap();
        guard.total_requests += 1;
        if guard.fail_fetches {
            anyhow::bail!("Simulated Steam CM network failure");
        }
        let now = Utc::now();
        let mut out = Vec::with_capacity(ids.len());
        for wid in ids {
            *guard.fetch_counts.entry(*wid).or_insert(0) += 1;
            if let Some(outcome) = guard.outcomes.get(wid).cloned() {
                out.push(outcome);
            } else {
                out.push(WorkshopFetchOutcome::Removed {
                    workshop_id: *wid,
                    source: MetadataSource::SteamKit,
                    observed_at: now,
                    reason: Some("Mock item not registered (result=9)".into()),
                });
            }
        }
        Ok(out)
    }

    async fn query_files(
        &self,
        query_type: u32,
        file_type: u32,
        cursor: &str,
        _num_per_page: u32,
    ) -> anyhow::Result<WorkshopQueryPage> {
        let mut guard = self.inner.lock().unwrap();
        guard.total_requests += 1;
        if guard.fail_fetches {
            anyhow::bail!("Simulated Steam CM query_files failure");
        }
        let key = (query_type, file_type, cursor.to_string());
        Ok(guard
            .query_pages
            .get(&key)
            .cloned()
            .unwrap_or(WorkshopQueryPage {
                total: 0,
                next_cursor: None,
                items: Vec::new(),
            }))
    }

    async fn status(&self) -> CatalogSteamStatus {
        let guard = self.inner.lock().unwrap();
        CatalogSteamStatus {
            provider_name: "mock-steamkit-cm".to_string(),
            mode: guard.mode,
            connection_state: if guard.fail_fetches {
                SteamConnectionState::Error
            } else {
                SteamConnectionState::Connected
            },
            steam_id: Some("76561197960265728".to_string()),
            account_name: guard.account_name.clone(),
            cell_id: Some(47),
            country_code: Some("HK".to_string()),
            connected_since: Some(Utc::now()),
            total_requests: guard.total_requests,
            failed_requests: 0,
            last_error: None,
            active_auth_flow: guard.active_flow.clone(),
        }
    }

    async fn reconnect(&self) -> anyhow::Result<CatalogSteamStatus> {
        Ok(self.status().await)
    }

    async fn start_qr_auth(&self) -> anyhow::Result<SteamAuthFlowStatus> {
        let url = "https://s.team/q/1/mock_challenge";
        let flow = SteamAuthFlowStatus {
            flow_id: "qr_mock_1".to_string(),
            kind: SteamAuthFlowKind::Qr,
            state: SteamAuthFlowState::WaitingForScan,
            challenge_url: Some(url.to_string()),
            qr_svg: render_qr_svg(url),
            code_prompt: None,
            account_name: None,
            error: None,
            updated_at: Utc::now(),
        };
        self.inner.lock().unwrap().active_flow = Some(flow.clone());
        Ok(flow)
    }

    async fn start_credentials_auth(
        &self,
        account_name: String,
        _password: String,
    ) -> anyhow::Result<SteamAuthFlowStatus> {
        let flow = SteamAuthFlowStatus {
            flow_id: "cred_mock_1".to_string(),
            kind: SteamAuthFlowKind::Credentials,
            state: SteamAuthFlowState::WaitingForCode,
            challenge_url: None,
            qr_svg: None,
            code_prompt: Some("device code: mock@steam".to_string()),
            account_name: Some(account_name),
            error: None,
            updated_at: Utc::now(),
        };
        self.inner.lock().unwrap().active_flow = Some(flow.clone());
        Ok(flow)
    }

    async fn submit_guard_code(
        &self,
        flow_id: &str,
        code: &str,
    ) -> anyhow::Result<SteamAuthFlowStatus> {
        let mut guard = self.inner.lock().unwrap();
        let Some(ref mut flow) = guard.active_flow else {
            anyhow::bail!("No active auth flow");
        };
        if flow.flow_id != flow_id || code.trim().is_empty() {
            anyhow::bail!("Invalid guard flow or code");
        }
        flow.state = SteamAuthFlowState::Succeeded;
        flow.updated_at = Utc::now();
        let acc = flow
            .account_name
            .clone()
            .unwrap_or_else(|| "mock_user".to_string());
        let cloned = flow.clone();
        guard.mode = SteamAuthMode::Account;
        guard.account_name = Some(acc);
        Ok(cloned)
    }

    async fn set_refresh_token(
        &self,
        account_name: String,
        refresh_token: String,
    ) -> anyhow::Result<CatalogSteamStatus> {
        if account_name.trim().is_empty() || refresh_token.trim().is_empty() {
            anyhow::bail!("accountName and refreshToken required");
        }
        {
            let mut guard = self.inner.lock().unwrap();
            guard.mode = SteamAuthMode::Account;
            guard.account_name = Some(account_name.trim().to_string());
            guard.active_flow = None;
        }
        Ok(self.status().await)
    }

    async fn switch_anonymous(&self) -> anyhow::Result<CatalogSteamStatus> {
        {
            let mut guard = self.inner.lock().unwrap();
            guard.mode = SteamAuthMode::Anonymous;
            guard.account_name = None;
            guard.active_flow = None;
        }
        Ok(self.status().await)
    }
}
