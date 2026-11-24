use super::auth::AuthFlowSlot;
use super::mapping::{build_search_files_proto, can_resolve_by_id, map_detail};
use super::{
    SteamSearchRequest, WorkshopObservation, WorkshopQueryPage, WorkshopUpstream,
    DETAILS_BATCH_SIZE,
};
use crate::config::CatalogConfigStore;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use l4a_protocol::{
    CatalogSteamStatus, SteamAuthFlowStatus, SteamAuthMode, SteamConnectionState, WorkshopId,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use steam_vent::{Connection, ConnectionTrait, NetworkError, ServerList};
use steam_vent_proto_steam::steammessages_player_steamclient::CPlayer_GetPlayerLinkDetails_Request;
use steam_vent_proto_steam::steammessages_publishedfile_steamclient::{
    CPublishedFile_GetDetails_Request, CPublishedFile_QueryFiles_Request, PublishedFileDetails,
};
use tokio::sync::{Mutex, RwLock, Semaphore};

const MAX_PERSONA_CACHE_ENTRIES: usize = 10_000;

#[derive(Default)]
pub(super) struct ConnectionSlot {
    pub(super) generation: u64,
    pub(super) conn: Option<Connection>,
}

#[derive(Default)]
pub(super) struct ConnectionMeta {
    state: SteamConnectionState,
    steam_id: Option<String>,
    cell_id: Option<u32>,
    country_code: Option<String>,
    connected_since: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

struct ConnectStateGuard {
    meta: Arc<RwLock<ConnectionMeta>>,
    completed: bool,
}

impl ConnectStateGuard {
    fn new(meta: Arc<RwLock<ConnectionMeta>>) -> Self {
        Self {
            meta,
            completed: false,
        }
    }

    fn disarm(&mut self) {
        self.completed = true;
    }
}

impl Drop for ConnectStateGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        if let Ok(mut meta) = self.meta.try_write() {
            if matches!(
                meta.state,
                SteamConnectionState::Connecting | SteamConnectionState::Reconnecting
            ) {
                meta.state = SteamConnectionState::Disconnected;
            }
        } else if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let meta = Arc::clone(&self.meta);
            handle.spawn(async move {
                let mut m = meta.write().await;
                if matches!(
                    m.state,
                    SteamConnectionState::Connecting | SteamConnectionState::Reconnecting
                ) {
                    m.state = SteamConnectionState::Disconnected;
                }
            });
        }
    }
}

pub(super) struct SteamCmInner {
    pub(super) config_store: Arc<CatalogConfigStore>,
    server_list: Mutex<Option<ServerList>>,
    pub(super) connection: Mutex<ConnectionSlot>,
    pub(super) meta: Arc<RwLock<ConnectionMeta>>,
    pub(super) auth_flow: Mutex<AuthFlowSlot>,
    pub(super) guard_file_lock: Arc<StdMutex<()>>,
    pub(super) persona_cache: RwLock<HashMap<u64, String>>,
    request_gate: Arc<Semaphore>,
    total_requests: AtomicU64,
    failed_requests: AtomicU64,
}

#[derive(Clone)]
pub struct SteamCmClient {
    pub(super) inner: Arc<SteamCmInner>,
}

impl SteamCmClient {
    pub fn new(config_store: Arc<CatalogConfigStore>) -> Self {
        let concurrency = config_store.get().steam.request_concurrency.clamp(1, 16);
        Self {
            inner: Arc::new(SteamCmInner {
                config_store,
                server_list: Mutex::new(None),
                connection: Mutex::new(ConnectionSlot::default()),
                meta: Arc::new(RwLock::new(ConnectionMeta {
                    state: SteamConnectionState::Disconnected,
                    ..Default::default()
                })),
                auth_flow: Mutex::new(AuthFlowSlot::default()),
                guard_file_lock: Arc::new(StdMutex::new(())),
                persona_cache: RwLock::new(HashMap::new()),
                request_gate: Arc::new(Semaphore::new(concurrency)),
                total_requests: AtomicU64::new(0),
                failed_requests: AtomicU64::new(0),
            }),
        }
    }

    async fn fetch_details_chunk(
        &self,
        ids: &[WorkshopId],
    ) -> anyhow::Result<Vec<WorkshopObservation>> {
        let _permit = self.inner.request_gate.acquire().await?;
        let app_id = self.inner.config_store.get().steam.app_id;
        let (conn_gen, mut conn) = self.inner.get_or_connect().await?;
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
                let (_, reconnected) = self.inner.invalidate_and_reconnect(Some(conn_gen)).await?;
                conn = reconnected;
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

impl SteamCmInner {
    pub(super) async fn ensure_server_list(&self) -> anyhow::Result<ServerList> {
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

    async fn connect_fresh(&self, reconnecting: bool) -> anyhow::Result<Connection> {
        let mut state_guard = ConnectStateGuard::new(Arc::clone(&self.meta));
        {
            let mut m = self.meta.write().await;
            m.state = if reconnecting {
                SteamConnectionState::Reconnecting
            } else {
                SteamConnectionState::Connecting
            };
        }
        let server_list = match self.ensure_server_list().await {
            Ok(s) => s,
            Err(err) => {
                let mut m = self.meta.write().await;
                m.state = SteamConnectionState::Error;
                m.last_error = Some(err.to_string());
                state_guard.disarm();
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
                state_guard.disarm();
                Ok(conn)
            }
            Err(err) => {
                let mut m = self.meta.write().await;
                m.state = SteamConnectionState::Error;
                m.last_error = Some(err.to_string());
                state_guard.disarm();
                Err(err)
            }
        }
    }

    async fn record_connected(&self, conn: &Connection) {
        Self::set_connected_meta(&mut *self.meta.write().await, conn);
    }

    pub(super) fn set_connected_meta(m: &mut ConnectionMeta, conn: &Connection) {
        m.state = SteamConnectionState::Connected;
        m.steam_id = Some(u64::from(conn.steam_id()).to_string());
        m.cell_id = Some(conn.cell_id());
        m.country_code = conn.ip_country_code();
        m.connected_since = Some(Utc::now());
        m.last_error = None;
    }

    pub(super) async fn get_or_connect(&self) -> anyhow::Result<(u64, Connection)> {
        let mut guard = self.connection.lock().await;
        if let Some(ref conn) = guard.conn {
            return Ok((guard.generation, conn.clone()));
        }
        let fresh = self.connect_fresh(false).await?;
        guard.generation += 1;
        guard.conn = Some(fresh.clone());
        Ok((guard.generation, fresh))
    }

    async fn invalidate_and_reconnect(
        &self,
        failed_gen: Option<u64>,
    ) -> anyhow::Result<(u64, Connection)> {
        let mut guard = self.connection.lock().await;
        if let (Some(failed), Some(ref conn)) = (failed_gen, &guard.conn) {
            if guard.generation != failed {
                return Ok((guard.generation, conn.clone()));
            }
        }
        guard.conn = None;
        let fresh = self.connect_fresh(true).await?;
        guard.generation += 1;
        guard.conn = Some(fresh.clone());
        Ok((guard.generation, fresh))
    }

    async fn record_request_failure(&self, err_msg: String) {
        self.failed_requests.fetch_add(1, Ordering::Relaxed);
        self.meta.write().await.last_error = Some(err_msg);
    }

    async fn resolve_persona_names(&self, conn: &Connection, steam_ids: &[u64]) {
        if self.config_store.get().steam.mode != SteamAuthMode::Account || steam_ids.is_empty() {
            return;
        }
        let missing: Vec<u64> = {
            let cache = self.persona_cache.read().await;
            let mut seen = HashSet::with_capacity(steam_ids.len());
            steam_ids
                .iter()
                .copied()
                .filter(|id| *id > 0 && !cache.contains_key(id) && seen.insert(*id))
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
                            if !cache.contains_key(&sid) && cache.len() >= MAX_PERSONA_CACHE_ENTRIES
                            {
                                let drop_n = (MAX_PERSONA_CACHE_ENTRIES / 4).max(1);
                                let keys: Vec<u64> = cache.keys().take(drop_n).copied().collect();
                                for key in keys {
                                    cache.remove(&key);
                                }
                            }
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

    pub(super) async fn status(&self) -> CatalogSteamStatus {
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
    matches!(
        err,
        NetworkError::IO(_)
            | NetworkError::Ws(_)
            | NetworkError::CryptoHandshakeFailed
            | NetworkError::CryptoError(_)
            | NetworkError::EOF
    )
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
                    self.inner.record_request_failure(err.to_string()).await;
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
        let (conn_gen, mut conn) = self.inner.get_or_connect().await?;
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
                match self.inner.invalidate_and_reconnect(Some(conn_gen)).await {
                    Ok((_, reconnected)) => conn = reconnected,
                    Err(e) => {
                        self.inner.record_request_failure(e.to_string()).await;
                        return Err(e);
                    }
                }
                observed_at = Utc::now();
                match conn.service_method(make_req()).await {
                    Ok(r) => r,
                    Err(e) => {
                        let msg = format!("Steam CM QueryFiles 重试失败: {e}");
                        self.inner.record_request_failure(msg.clone()).await;
                        return Err(anyhow::anyhow!(msg));
                    }
                }
            }
            Err(err) => {
                let msg = format!("Steam CM QueryFiles 调用失败: {err}");
                self.inner.record_request_failure(err.to_string()).await;
                return Err(anyhow::anyhow!(msg));
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
            next_cursor: resp
                .next_cursor
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            outcomes,
        })
    }

    async fn search_files(
        &self,
        request: &SteamSearchRequest,
    ) -> anyhow::Result<WorkshopQueryPage> {
        let _permit = self.inner.request_gate.acquire().await?;
        let app_id = self.inner.config_store.get().steam.app_id;
        let (conn_gen, mut conn) = self.inner.get_or_connect().await?;
        self.inner.total_requests.fetch_add(1, Ordering::Relaxed);

        let mut observed_at = Utc::now();
        let resp = match conn
            .service_method(build_search_files_proto(app_id, request))
            .await
        {
            Ok(r) => r,
            Err(err) if should_retry_network(&err) => {
                match self.inner.invalidate_and_reconnect(Some(conn_gen)).await {
                    Ok((_, reconnected)) => conn = reconnected,
                    Err(e) => {
                        self.inner.record_request_failure(e.to_string()).await;
                        return Err(e);
                    }
                }
                observed_at = Utc::now();
                match conn
                    .service_method(build_search_files_proto(app_id, request))
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        let msg = format!("Steam CM search_files 重试失败: {e}");
                        self.inner.record_request_failure(msg.clone()).await;
                        return Err(anyhow::anyhow!(msg));
                    }
                }
            }
            Err(err) => {
                let msg = format!("Steam CM search_files 调用失败: {err}");
                self.inner.record_request_failure(err.to_string()).await;
                return Err(anyhow::anyhow!(msg));
            }
        };

        let outcomes = self
            .inner
            .map_details(&conn, &resp.publishedfiledetails, observed_at)
            .await;
        if !resp.publishedfiledetails.is_empty() && outcomes.is_empty() {
            let msg = "Steam CM search_files 未核验任何返回的条目".to_string();
            self.inner.record_request_failure(msg.clone()).await;
            return Err(anyhow::anyhow!(msg));
        }
        if outcomes.len() < resp.publishedfiledetails.len() {
            tracing::warn!(
                returned = resp.publishedfiledetails.len(),
                verified = outcomes.len(),
                "Steam CM search_files 部分条目状态未核验"
            );
        }
        Ok(WorkshopQueryPage {
            total: u64::from(resp.total.unwrap_or(0)),
            next_cursor: resp
                .next_cursor
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            outcomes,
        })
    }

    async fn status(&self) -> CatalogSteamStatus {
        self.inner.status().await
    }

    async fn reconnect(&self) -> anyhow::Result<CatalogSteamStatus> {
        self.inner.invalidate_and_reconnect(None).await?;
        Ok(self.inner.status().await)
    }

    async fn start_qr_auth(&self) -> anyhow::Result<SteamAuthFlowStatus> {
        self.start_qr_auth_impl().await
    }

    async fn start_credentials_auth(
        &self,
        account_name: String,
        password: String,
    ) -> anyhow::Result<SteamAuthFlowStatus> {
        self.start_credentials_auth_impl(account_name, password)
            .await
    }

    async fn submit_guard_code(
        &self,
        flow_id: &str,
        code: &str,
    ) -> anyhow::Result<SteamAuthFlowStatus> {
        self.submit_guard_code_impl(flow_id, code).await
    }

    async fn set_refresh_token(
        &self,
        account_name: String,
        refresh_token: String,
    ) -> anyhow::Result<CatalogSteamStatus> {
        self.set_refresh_token_impl(account_name, refresh_token)
            .await
    }

    async fn switch_anonymous(&self) -> anyhow::Result<CatalogSteamStatus> {
        self.switch_anonymous_impl().await
    }
}
