use crate::config::CatalogConfigStore;
use crate::credentials::SteamGuardFile;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use l4a_protocol::{
    CatalogSteamStatus, MetadataSource, SteamAuthFlowKind, SteamAuthFlowState, SteamAuthFlowStatus,
    SteamAuthMode, SteamConnectionState, WorkshopDependency, WorkshopFetchOutcome,
    WorkshopFileType, WorkshopId, WorkshopItemRecord, WorkshopItemStatus, WorkshopPreview,
};
use qrcode::{render::svg, QrCode};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use steam_vent::auth::{
    AuthConfirmationHandler, ConfirmationAction, ConfirmationMethod, DeviceConfirmationHandler,
    UserProvidedAuthConfirmationHandler,
};
use steam_vent::{Connection, ConnectionTrait, EResult, NetworkError, ServerList};
use steam_vent_proto_common::protobuf::{EnumOrUnknown, MessageField};
use steam_vent_proto_steam::steammessages_auth_steamclient::{
    CAuthentication_BeginAuthSessionViaQR_Request, CAuthentication_BeginAuthSessionViaQR_Response,
    CAuthentication_DeviceDetails, CAuthentication_PollAuthSessionStatus_Request,
    EAuthTokenPlatformType,
};
use steam_vent_proto_steam::steammessages_player_steamclient::CPlayer_GetPlayerLinkDetails_Request;
use steam_vent_proto_steam::steammessages_publishedfile_steamclient::{
    CPublishedFile_GetDetails_Request, CPublishedFile_QueryFiles_Request, PublishedFileDetails,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, WriteHalf};
use tokio::sync::{oneshot, Mutex, RwLock, Semaphore};
use tokio::task::AbortHandle;

pub const QUERY_TYPE_RANKED_BY_PUBLICATION_DATE: u32 = 1;
pub const QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE: u32 = 21;
pub const MATCHING_FILE_TYPE_ITEMS: u32 = 0;
pub const MATCHING_FILE_TYPE_COLLECTIONS: u32 = 1;
const WORKSHOP_FILE_TYPE_COLLECTION: u32 = 2;
pub(crate) const DETAILS_BATCH_SIZE: usize = 100;
const AUTH_FLOW_TIMEOUT: Duration = Duration::from_secs(180);

// Listing metadata is specific to the Catalog cache and stays off the public v1 wire format.
#[derive(Debug, Clone)]
pub struct WorkshopObservation {
    pub outcome: WorkshopFetchOutcome,
    pub listed: bool,
}

impl From<WorkshopFetchOutcome> for WorkshopObservation {
    fn from(outcome: WorkshopFetchOutcome) -> Self {
        Self {
            outcome,
            listed: true,
        }
    }
}

impl WorkshopObservation {
    pub(crate) fn observed_at(&self) -> DateTime<Utc> {
        match &self.outcome {
            WorkshopFetchOutcome::Found { item } => item.fetched_at,
            WorkshopFetchOutcome::Removed { observed_at, .. }
            | WorkshopFetchOutcome::PrivateOrUnavailable { observed_at, .. } => *observed_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkshopQueryPage {
    pub total: u64,
    pub next_cursor: Option<String>,
    pub outcomes: Vec<WorkshopObservation>,
}

#[async_trait]
pub trait WorkshopUpstream: Send + Sync {
    async fn fetch_details(&self, ids: &[WorkshopId]) -> anyhow::Result<Vec<WorkshopObservation>>;

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
struct AuthFlowSlot {
    generation: u64,
    active: Option<ActiveFlowHandle>,
}

impl AuthFlowSlot {
    fn begin(&mut self) -> u64 {
        if let Some(abort) = self
            .active
            .take()
            .and_then(|previous| previous.abort_handle)
        {
            abort.abort();
        }
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    fn ensure_current(&self, generation: u64) -> anyhow::Result<()> {
        anyhow::ensure!(self.generation == generation, "Steam 登录流程已被替换");
        Ok(())
    }
}

struct SteamLoginConfirmation<H>(H);

impl<H: AuthConfirmationHandler + Send> AuthConfirmationHandler for SteamLoginConfirmation<H> {
    async fn handle_confirmation(
        self,
        confirmations: &[ConfirmationMethod],
    ) -> Option<ConfirmationAction> {
        // Connection::login polls for app approval while the code handler waits for input.
        // If no code is offered, preserve the device handler's Some(ConfirmationAction::None).
        match self.0.handle_confirmation(confirmations).await {
            Some(action) => Some(action),
            None => {
                DeviceConfirmationHandler
                    .handle_confirmation(confirmations)
                    .await
            }
        }
    }
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
    auth_flow: Mutex<AuthFlowSlot>,
    guard_file_lock: Arc<StdMutex<()>>,
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
                auth_flow: Mutex::new(AuthFlowSlot::default()),
                guard_file_lock: Arc::new(StdMutex::new(())),
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
        Self::set_connected_meta(&mut *self.meta.write().await, conn);
    }

    fn set_connected_meta(m: &mut ConnectionMeta, conn: &Connection) {
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
        generation: u64,
        mut conn: Connection,
        account_name: String,
        refresh_token: String,
    ) -> anyhow::Result<()> {
        conn.set_timeout(Duration::from_secs(20));
        let mut connection = self.connection.lock().await;
        let mut slot = self.auth_flow.lock().await;
        slot.ensure_current(generation)?;
        let mut meta = self.meta.write().await;

        // After checking the generation, commit config and connection under the same flow lock.
        self.config_store.update(|cfg| {
            cfg.steam.mode = SteamAuthMode::Account;
            cfg.steam.account_name = Some(account_name.clone());
            cfg.steam.refresh_token = Some(refresh_token);
        })?;
        Self::set_connected_meta(&mut meta, &conn);
        *connection = Some(conn);
        if let Some(flow) = &mut slot.active {
            flow.status.state = SteamAuthFlowState::Succeeded;
            flow.status.account_name = Some(account_name);
            flow.status.challenge_url = None;
            flow.status.qr_svg = None;
            flow.status.code_prompt = None;
            flow.status.updated_at = Utc::now();
            flow.code_writer = None;
            flow.abort_handle = None;
        }
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

    async fn map_details(
        &self,
        conn: &Connection,
        details: &[PublishedFileDetails],
        observed_at: DateTime<Utc>,
    ) -> Vec<WorkshopObservation> {
        let creators: Vec<u64> = details
            .iter()
            .filter(|d| d.result == Some(1) && !d.banned.unwrap_or(false) && can_resolve_by_id(d))
            .filter_map(|d| d.creator.filter(|c| *c > 0))
            .collect();
        self.resolve_persona_names(conn, &creators).await;
        let persona_map = self.persona_cache.read().await;

        details
            .iter()
            .filter_map(|d| {
                let creator_name = d.creator.and_then(|cid| persona_map.get(&cid).cloned());
                map_detail(d, creator_name, observed_at)
            })
            .collect()
    }

    async fn create_qr_session(
        &self,
    ) -> anyhow::Result<(
        ServerList,
        Connection,
        CAuthentication_BeginAuthSessionViaQR_Response,
    )> {
        let server_list = self.ensure_server_list().await?;
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
        Ok((server_list, anon_conn, started))
    }

    async fn run_qr_flow(
        &self,
        generation: u64,
        ready: oneshot::Sender<anyhow::Result<SteamAuthFlowStatus>>,
    ) {
        let (server_list, connection, started) = match self.create_qr_session().await {
            Ok(session) => session,
            Err(err) => {
                self.fail_flow(generation, err.to_string()).await;
                let _ = ready.send(Err(err));
                return;
            }
        };
        let status = self
            .set_qr_challenge(generation, started.challenge_url())
            .await;
        let registered = status.is_ok();
        let _ = ready.send(status);
        if !registered {
            return;
        }

        let poll = async {
            let mut client_id = started.client_id();
            let request_id = started.request_id().to_vec();
            let poll_interval = Duration::from_secs_f32(started.interval().clamp(1.5, 5.0));
            loop {
                tokio::time::sleep(poll_interval).await;
                let response = connection
                    .service_method(CAuthentication_PollAuthSessionStatus_Request {
                        client_id: Some(client_id),
                        request_id: Some(request_id.clone()),
                        ..Default::default()
                    })
                    .await
                    .map_err(|err| anyhow::anyhow!("轮询扫码状态失败: {err}"))?;
                if response.has_new_client_id() {
                    client_id = response.new_client_id();
                }
                if response.has_new_challenge_url() {
                    self.set_qr_challenge(generation, response.new_challenge_url())
                        .await?;
                }
                if response.has_refresh_token() && response.has_account_name() {
                    let account = response.account_name().to_string();
                    let token = response.refresh_token().to_string();
                    let conn = Connection::access(&server_list, &account, &token)
                        .await
                        .map_err(|err| anyhow::anyhow!("使用扫码 Token 建立会话失败: {err}"))?;
                    return self
                        .install_authenticated_connection(generation, conn, account, token)
                        .await;
                }
            }
        };
        match tokio::time::timeout(AUTH_FLOW_TIMEOUT, poll).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => self.fail_flow(generation, err.to_string()).await,
            Err(_) => {
                self.fail_flow(generation, "二维码已超时过期，请重新生成".into())
                    .await
            }
        }
    }

    async fn set_qr_challenge(
        &self,
        generation: u64,
        challenge_url: &str,
    ) -> anyhow::Result<SteamAuthFlowStatus> {
        let mut slot = self.auth_flow.lock().await;
        slot.ensure_current(generation)?;
        let flow = slot
            .active
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Steam 扫码流程已结束"))?;
        flow.status.state = SteamAuthFlowState::WaitingForScan;
        flow.status.challenge_url = Some(challenge_url.to_string());
        flow.status.qr_svg = render_qr_svg(challenge_url);
        flow.status.updated_at = Utc::now();
        Ok(flow.status.clone())
    }

    async fn fail_flow(&self, generation: u64, message: String) {
        let mut slot = self.auth_flow.lock().await;
        if slot.ensure_current(generation).is_ok() {
            if let Some(flow) = &mut slot.active {
                flow.status.state = SteamAuthFlowState::Failed;
                flow.status.error = Some(message);
                flow.status.challenge_url = None;
                flow.status.qr_svg = None;
                flow.status.code_prompt = None;
                flow.status.updated_at = Utc::now();
                flow.code_writer = None;
                flow.abort_handle = None;
            }
        }
    }

    async fn status(&self) -> CatalogSteamStatus {
        let slot = self.auth_flow.lock().await;
        let cfg = self.config_store.get().steam;
        let m = self.meta.read().await;
        let flow = slot.active.as_ref().map(|flow| flow.status.clone());

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

fn map_detail(
    d: &PublishedFileDetails,
    creator_name: Option<String>,
    observed_at: DateTime<Utc>,
) -> Option<WorkshopObservation> {
    let workshop_id = WorkshopId::new(d.publishedfileid?).ok()?;
    let source = MetadataSource::SteamKit;
    let result = d
        .result
        .and_then(|code| EResult::try_from(code as i32).ok());
    let outcome = match result {
        Some(EResult::OK) if d.banned.unwrap_or(false) => WorkshopFetchOutcome::Removed {
            workshop_id,
            source,
            observed_at,
            reason: d
                .ban_reason
                .clone()
                .or_else(|| Some("Banned on Steam Workshop".into())),
        },
        Some(EResult::OK) if can_resolve_by_id(d) => WorkshopFetchOutcome::Found {
            item: details_to_record(workshop_id, d, creator_name, observed_at),
        },
        Some(EResult::FileNotFound) => WorkshopFetchOutcome::Removed {
            workshop_id,
            source,
            observed_at,
            reason: Some("Steam CM result=9 (FileNotFound)".into()),
        },
        Some(EResult::OK | EResult::AccessDenied) => WorkshopFetchOutcome::PrivateOrUnavailable {
            workshop_id,
            source,
            observed_at,
            reason: Some(if matches!(result, Some(EResult::OK)) {
                "Steam CM visibility is restricted".into()
            } else {
                "Steam CM result=15 (AccessDenied)".into()
            }),
        },
        _ => {
            tracing::warn!(%workshop_id, result = ?d.result, "Steam CM 未返回可核验的条目状态");
            return None;
        }
    };
    Some(WorkshopObservation {
        outcome,
        listed: d.visibility() == 0,
    })
}

fn can_resolve_by_id(d: &PublishedFileDetails) -> bool {
    matches!(d.visibility(), 0 | 3)
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

    WorkshopItemRecord {
        workshop_id: wid,
        file_type,
        status: WorkshopItemStatus::Active,
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

impl SteamCmClient {
    async fn fetch_details_chunk(
        &self,
        ids: &[WorkshopId],
    ) -> anyhow::Result<Vec<WorkshopObservation>> {
        let _permit = self.inner.request_gate.acquire().await?;
        let app_id = self.inner.config_store.get().steam.app_id;
        let mut conn = self.inner.get_or_connect().await?;
        let make_req = || CPublishedFile_GetDetails_Request {
            publishedfileids: ids.iter().map(|id| id.as_u64()).collect(),
            includetags: Some(true),
            includeadditionalpreviews: Some(true),
            includechildren: Some(true),
            includevotes: Some(true),
            appid: Some(app_id),
            ..Default::default()
        };

        self.inner.total_requests.fetch_add(1, Ordering::Relaxed);
        // Order overlapping requests by the start of the RPC, before any enrichment or database work.
        let mut observed_at = Utc::now();
        let resp = match conn.service_method(make_req()).await {
            Ok(resp) => resp,
            Err(err) if should_retry_network(&err) => {
                conn = self.inner.invalidate_and_reconnect().await?;
                observed_at = Utc::now();
                conn.service_method(make_req())
                    .await
                    .map_err(|err| anyhow::anyhow!("Steam CM GetDetails 重试失败: {err}"))?
            }
            Err(err) => anyhow::bail!("Steam CM GetDetails 调用失败: {err}"),
        };
        let mut outcomes = self
            .inner
            .map_details(&conn, &resp.publishedfiledetails, observed_at)
            .await;
        outcomes.retain(|observation| ids.contains(&observation.outcome.workshop_id()));
        Ok(outcomes)
    }
}

#[async_trait]
impl WorkshopUpstream for SteamCmClient {
    async fn fetch_details(&self, ids: &[WorkshopId]) -> anyhow::Result<Vec<WorkshopObservation>> {
        let mut all_outcomes = Vec::with_capacity(ids.len());
        let mut last_error = None;
        for ids in ids.chunks(DETAILS_BATCH_SIZE) {
            match self.fetch_details_chunk(ids).await {
                Ok(outcomes) => all_outcomes.extend(outcomes),
                Err(err) => {
                    self.inner.failed_requests.fetch_add(1, Ordering::Relaxed);
                    self.inner.meta.write().await.last_error = Some(err.to_string());
                    tracing::warn!(%err, count = ids.len(), "Steam CM 详情分块获取失败");
                    last_error = Some(err);
                }
            }
        }

        if !ids.is_empty() && all_outcomes.is_empty() {
            return Err(
                last_error.unwrap_or_else(|| anyhow::anyhow!("Steam CM 未核验任何请求的条目"))
            );
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
            cache_max_age_seconds: Some(0),
            filetype: Some(file_type),
            return_vote_data: Some(true),
            return_tags: Some(true),
            return_previews: Some(true),
            return_children: Some(true),
            return_short_description: Some(true),
            return_details: Some(true),
            ..Default::default()
        };

        let mut observed_at = Utc::now();
        let resp = match conn.service_method(make_req()).await {
            Ok(r) => r,
            Err(err) if should_retry_network(&err) => {
                conn = self.inner.invalidate_and_reconnect().await?;
                observed_at = Utc::now();
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
            .map_details(&conn, &resp.publishedfiledetails, observed_at)
            .await;
        anyhow::ensure!(
            outcomes.len() == resp.publishedfiledetails.len(),
            "Steam CM QueryFiles 包含未核验的条目"
        );
        Ok(WorkshopQueryPage {
            total: u64::from(resp.total.unwrap_or(0)),
            next_cursor: resp.next_cursor.filter(|s| !s.trim().is_empty()),
            outcomes,
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
        let initial_status = SteamAuthFlowStatus {
            flow_id: format!("qr_{}", uuid::Uuid::new_v4().simple()),
            kind: SteamAuthFlowKind::Qr,
            state: SteamAuthFlowState::Authenticating,
            challenge_url: None,
            qr_svg: None,
            code_prompt: None,
            account_name: None,
            error: None,
            updated_at: Utc::now(),
        };
        let (ready, started) = oneshot::channel();
        {
            let mut slot = self.inner.auth_flow.lock().await;
            let generation = slot.begin();
            let inner = Arc::clone(&self.inner);
            let join = tokio::spawn(async move {
                inner.run_qr_flow(generation, ready).await;
            });
            slot.active = Some(ActiveFlowHandle {
                status: initial_status,
                abort_handle: Some(join.abort_handle()),
                code_writer: None,
            });
        }
        started
            .await
            .map_err(|_| anyhow::anyhow!("Steam 扫码流程已被替换"))?
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
        let initial_status = SteamAuthFlowStatus {
            flow_id: format!("cred_{}", uuid::Uuid::new_v4().simple()),
            kind: SteamAuthFlowKind::Credentials,
            state: SteamAuthFlowState::Authenticating,
            challenge_url: None,
            qr_svg: None,
            code_prompt: None,
            account_name: Some(account.clone()),
            error: None,
            updated_at: Utc::now(),
        };

        let (code_stream_handler, code_stream_controller) = tokio::io::duplex(256);
        let (handler_read, _) = tokio::io::split(code_stream_handler);
        let (_, controller_write) = tokio::io::split(code_stream_controller);
        let (mut prompt_read, prompt_write) = tokio::io::duplex(512);

        let generation = {
            let mut slot = self.inner.auth_flow.lock().await;
            let generation = slot.begin();
            let inner_prompt = Arc::clone(&self.inner);
            tokio::spawn(async move {
                let mut buf = [0u8; 256];
                if let Ok(n) = prompt_read.read(&mut buf).await {
                    if n > 0 {
                        let prompt = String::from_utf8_lossy(&buf[..n]).trim().to_string();
                        let mut slot = inner_prompt.auth_flow.lock().await;
                        if slot.ensure_current(generation).is_ok() {
                            if let Some(flow) = &mut slot.active {
                                if flow.status.state == SteamAuthFlowState::Authenticating {
                                    flow.status.state = SteamAuthFlowState::WaitingForCode;
                                    flow.status.code_prompt = Some(prompt);
                                    flow.status.updated_at = Utc::now();
                                }
                            }
                        }
                    }
                }
            });

            let inner = Arc::clone(&self.inner);
            let join = tokio::spawn(async move {
                let login = async {
                    let server_list = inner.ensure_server_list().await?;
                    let guard_store = SteamGuardFile {
                        path: inner.config_store.get().steam.guard_data_path,
                        lock: Arc::clone(&inner.guard_file_lock),
                    };
                    let confirmation = SteamLoginConfirmation(
                        UserProvidedAuthConfirmationHandler::new(handler_read, prompt_write),
                    );
                    let conn = Connection::login(
                        &server_list,
                        &account,
                        &password,
                        guard_store,
                        confirmation,
                    )
                    .await
                    .map_err(|err| anyhow::anyhow!("Steam 账号认证失败: {err}"))?;
                    let token = conn
                        .access_token()
                        .map(ToString::to_string)
                        .ok_or_else(|| anyhow::anyhow!("Steam CM 未返回持久化 Refresh Token"))?;
                    inner
                        .install_authenticated_connection(generation, conn, account, token)
                        .await
                };
                match tokio::time::timeout(AUTH_FLOW_TIMEOUT, login).await {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => inner.fail_flow(generation, err.to_string()).await,
                    Err(_) => {
                        inner
                            .fail_flow(generation, "Steam 账号认证超时".into())
                            .await
                    }
                }
            });

            slot.active = Some(ActiveFlowHandle {
                status: initial_status,
                abort_handle: Some(join.abort_handle()),
                code_writer: Some(controller_write),
            });
            generation
        };

        // Return the first challenge promptly if Steam has already provided one.
        tokio::time::sleep(Duration::from_millis(250)).await;
        let slot = self.inner.auth_flow.lock().await;
        slot.ensure_current(generation)?;
        slot.active
            .as_ref()
            .map(|flow| flow.status.clone())
            .ok_or_else(|| anyhow::anyhow!("Steam 登录流程已结束"))
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
        let mut slot = self.inner.auth_flow.lock().await;
        let Some(handle) = &mut slot.active else {
            anyhow::bail!("当前没有进行中的 Steam 登录流程");
        };
        if handle.status.flow_id != flow_id {
            anyhow::bail!("登录流程 ID 不匹配或已过期");
        }
        anyhow::ensure!(
            handle.status.state == SteamAuthFlowState::WaitingForCode,
            "当前流程未请求 Steam Guard 验证码"
        );
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
        let generation = self.inner.auth_flow.lock().await.begin();
        let server_list = self.inner.ensure_server_list().await?;
        let conn = Connection::access(&server_list, &account, &token)
            .await
            .map_err(|e| anyhow::anyhow!("验证 Refresh Token 失败: {e}"))?;
        self.inner
            .install_authenticated_connection(generation, conn, account, token)
            .await?;
        Ok(self.inner.status().await)
    }

    async fn switch_anonymous(&self) -> anyhow::Result<CatalogSteamStatus> {
        let generation = self.inner.auth_flow.lock().await.begin();
        {
            let mut connection = self.inner.connection.lock().await;
            let slot = self.inner.auth_flow.lock().await;
            slot.ensure_current(generation)?;
            self.inner.config_store.update(|cfg| {
                cfg.steam.mode = SteamAuthMode::Anonymous;
                cfg.steam.account_name = None;
                cfg.steam.refresh_token = None;
            })?;
            *connection = None;
        }
        self.inner.persona_cache.write().await.clear();
        self.inner.get_or_connect().await?;
        Ok(self.inner.status().await)
    }
}

#[derive(Default)]
struct MockUpstreamState {
    outcomes: HashMap<WorkshopId, WorkshopObservation>,
    query_pages: HashMap<(u32, u32, String), WorkshopQueryPage>,
    fetch_counts: HashMap<WorkshopId, u64>,
    fail_fetches: bool,
    failed_details: HashSet<WorkshopId>,
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
        self.insert_outcome(wid, WorkshopFetchOutcome::Found { item });
    }

    pub fn insert_outcome(&self, wid: WorkshopId, outcome: impl Into<WorkshopObservation>) {
        self.inner
            .lock()
            .unwrap()
            .outcomes
            .insert(wid, outcome.into());
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

    pub fn fail_details_containing(&self, wid: WorkshopId) {
        self.inner.lock().unwrap().failed_details.insert(wid);
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
    async fn fetch_details(&self, ids: &[WorkshopId]) -> anyhow::Result<Vec<WorkshopObservation>> {
        let mut guard = self.inner.lock().unwrap();
        guard.total_requests += 1;
        if guard.fail_fetches || ids.iter().any(|id| guard.failed_details.contains(id)) {
            anyhow::bail!("Simulated Steam CM network failure");
        }
        let now = Utc::now();
        let mut out = Vec::with_capacity(ids.len());
        for wid in ids {
            *guard.fetch_counts.entry(*wid).or_insert(0) += 1;
            let mut observation = guard.outcomes.get(wid).cloned().unwrap_or_else(|| {
                WorkshopFetchOutcome::Removed {
                    workshop_id: *wid,
                    source: MetadataSource::SteamKit,
                    observed_at: now,
                    reason: Some("Mock item not registered (result=9)".into()),
                }
                .into()
            });
            match &mut observation.outcome {
                WorkshopFetchOutcome::Found { item } => item.fetched_at = now,
                WorkshopFetchOutcome::Removed { observed_at, .. }
                | WorkshopFetchOutcome::PrivateOrUnavailable { observed_at, .. } => {
                    *observed_at = now
                }
            }
            out.push(observation);
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
                outcomes: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_account_only_details_become_tombstones() {
        for visibility in [1, 2] {
            let details = PublishedFileDetails {
                publishedfileid: Some(42),
                result: Some(1),
                visibility: Some(visibility),
                title: Some("Account-visible title".into()),
                file_url: Some("https://example.test/private.vpk".into()),
                ..Default::default()
            };
            assert!(matches!(
                map_detail(&details, None, Utc::now()).map(|obs| obs.outcome),
                Some(WorkshopFetchOutcome::PrivateOrUnavailable {
                    workshop_id: WorkshopId(42),
                    ..
                })
            ));
        }
    }

    #[test]
    fn transient_detail_errors_do_not_generate_tombstones() {
        for result in [EResult::Busy, EResult::Timeout, EResult::ServiceUnavailable] {
            let details = PublishedFileDetails {
                publishedfileid: Some(42),
                result: Some(result as u32),
                ..Default::default()
            };
            assert!(map_detail(&details, None, Utc::now()).is_none());
        }
    }

    #[test]
    fn unlisted_details_are_available_by_id() {
        let details = PublishedFileDetails {
            publishedfileid: Some(42),
            result: Some(EResult::OK as u32),
            visibility: Some(3),
            ..Default::default()
        };
        let observation = map_detail(&details, None, Utc::now()).unwrap();
        assert!(!observation.listed);
        assert!(matches!(
            observation.outcome,
            WorkshopFetchOutcome::Found { .. }
        ));
    }

    #[tokio::test]
    async fn app_only_confirmation_keeps_the_polling_action() {
        use steam_vent_proto_steam::steammessages_auth_steamclient::{
            CAuthentication_AllowedConfirmation, EAuthSessionGuardType,
        };
        let method = CAuthentication_AllowedConfirmation {
            confirmation_type: Some(EnumOrUnknown::new(
                EAuthSessionGuardType::k_EAuthSessionGuardType_DeviceConfirmation,
            )),
            ..Default::default()
        };
        let confirmation = SteamLoginConfirmation(UserProvidedAuthConfirmationHandler::new(
            tokio::io::empty(),
            tokio::io::sink(),
        ));
        assert!(matches!(
            confirmation.handle_confirmation(&[method.into()]).await,
            Some(ConfirmationAction::None)
        ));
    }

    #[tokio::test]
    async fn replacing_a_pending_flow_aborts_its_task_and_blocks_stale_updates() {
        let steam = SteamCmClient::new(Arc::new(CatalogConfigStore::new(
            None,
            crate::config::CatalogConfig::default(),
        )));
        let old = tokio::spawn(std::future::pending::<()>());
        let mut slot = steam.inner.auth_flow.lock().await;
        let previous = slot.begin();
        slot.active = Some(ActiveFlowHandle {
            status: SteamAuthFlowStatus {
                flow_id: "old-qr".into(),
                kind: SteamAuthFlowKind::Qr,
                state: SteamAuthFlowState::Authenticating,
                challenge_url: None,
                qr_svg: None,
                code_prompt: None,
                account_name: None,
                error: None,
                updated_at: Utc::now(),
            },
            abort_handle: Some(old.abort_handle()),
            code_writer: None,
        });
        let current = slot.begin();
        assert!(slot.ensure_current(current).is_ok());
        drop(slot);
        assert!(old.await.unwrap_err().is_cancelled());
        assert!(steam
            .inner
            .set_qr_challenge(previous, "https://example.test/old")
            .await
            .is_err());
    }
}
