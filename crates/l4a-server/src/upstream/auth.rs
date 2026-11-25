use super::cm_client::{SteamCmClient, SteamCmInner};
use super::AUTH_FLOW_TIMEOUT;
use crate::credentials::SteamGuardFile;
use chrono::Utc;
use l4a_protocol::{
    CatalogSteamStatus, SteamAuthFlowKind, SteamAuthFlowState, SteamAuthFlowStatus, SteamAuthMode,
};
use qrcode::{render::svg, QrCode};
use std::sync::Arc;
use std::time::Duration;
use steam_vent::auth::{
    AuthConfirmationHandler, ConfirmationAction, ConfirmationMethod, DeviceConfirmationHandler,
    UserProvidedAuthConfirmationHandler,
};
use steam_vent::{Connection, ConnectionTrait, ServerList};
use steam_vent_proto_common::protobuf::{EnumOrUnknown, MessageField};
use steam_vent_proto_steam::steammessages_auth_steamclient::{
    CAuthentication_BeginAuthSessionViaQR_Request, CAuthentication_BeginAuthSessionViaQR_Response,
    CAuthentication_DeviceDetails, CAuthentication_PollAuthSessionStatus_Request,
    EAuthTokenPlatformType,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, WriteHalf};
use tokio::sync::oneshot;
use tokio::task::AbortHandle;

pub(super) struct ActiveFlowHandle {
    pub(super) status: SteamAuthFlowStatus,
    pub(super) abort_handle: Option<AbortHandle>,
    pub(super) code_writer: Option<WriteHalf<tokio::io::DuplexStream>>,
}

#[derive(Default)]
pub(super) struct AuthFlowSlot {
    generation: u64,
    pub(super) active: Option<ActiveFlowHandle>,
}

impl AuthFlowSlot {
    pub(super) fn begin(&mut self) -> u64 {
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

    pub(super) fn ensure_current(&self, generation: u64) -> anyhow::Result<()> {
        anyhow::ensure!(self.generation == generation, "Steam 登录流程已被替换");
        Ok(())
    }
}

pub(super) struct SteamLoginConfirmation<H>(pub(super) H);

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

impl SteamCmInner {
    pub(super) async fn install_authenticated_connection(
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
        connection.generation += 1;
        connection.conn = Some(conn);
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

    pub(super) async fn set_qr_challenge(
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
}

impl SteamCmClient {
    pub(super) async fn start_qr_auth_impl(&self) -> anyhow::Result<SteamAuthFlowStatus> {
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

    pub(super) async fn start_credentials_auth_impl(
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

    pub(super) async fn submit_guard_code_impl(
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

    pub(super) async fn set_refresh_token_impl(
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

    pub(super) async fn switch_anonymous_impl(&self) -> anyhow::Result<CatalogSteamStatus> {
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
            connection.generation += 1;
            connection.conn = None;
        }
        self.inner.persona_cache.write().await.clear();
        self.inner.get_or_connect().await?;
        Ok(self.inner.status().await)
    }
}
