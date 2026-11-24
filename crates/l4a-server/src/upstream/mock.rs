use super::auth::render_qr_svg;
use super::{WorkshopObservation, WorkshopQueryPage, WorkshopUpstream};
use async_trait::async_trait;
use chrono::Utc;
use l4a_protocol::{
    CatalogSteamStatus, MetadataSource, SteamAuthFlowKind, SteamAuthFlowState, SteamAuthFlowStatus,
    SteamAuthMode, SteamConnectionState, WorkshopFetchOutcome, WorkshopId, WorkshopItemRecord,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};

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
