mod auth;
mod cm_client;
mod mapping;
mod mock;
#[cfg(test)]
mod tests;

pub use auth::render_qr_svg;
pub use cm_client::SteamCmClient;
pub use mapping::build_search_files_proto;
pub use mock::MockWorkshopUpstream;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use l4a_protocol::{CatalogSteamStatus, SteamAuthFlowStatus, WorkshopFetchOutcome, WorkshopId};
use std::time::Duration;

pub const QUERY_TYPE_RANKED_BY_VOTE: u32 = 0;
pub const QUERY_TYPE_RANKED_BY_PUBLICATION_DATE: u32 = 1;
pub const QUERY_TYPE_RANKED_BY_TREND: u32 = 3;
pub const QUERY_TYPE_RANKED_BY_TOTAL_UNIQUE_SUBSCRIPTIONS: u32 = 9;
pub const QUERY_TYPE_RANKED_BY_TEXT_SEARCH: u32 = 12;
pub const QUERY_TYPE_RANKED_BY_LAST_UPDATED_DATE: u32 = 21;
pub const MATCHING_FILE_TYPE_ITEMS: u32 = 0;
pub const MATCHING_FILE_TYPE_COLLECTIONS: u32 = 1;
const WORKSHOP_FILE_TYPE_COLLECTION: u32 = 2;
const PROTO_FIELD_SEARCH_TEXT_TARGET: u32 = 50;
pub(crate) const DETAILS_BATCH_SIZE: usize = 100;
const AUTH_FLOW_TIMEOUT: Duration = Duration::from_secs(180);

// Listing metadata is specific to the Catalog cache and stays off the public v1 wire format.
#[derive(Debug, Clone)]
pub struct WorkshopObservation {
    pub outcome: WorkshopFetchOutcome,
    pub listed: bool,
    pub consumer_appid: Option<u32>,
}

impl From<WorkshopFetchOutcome> for WorkshopObservation {
    fn from(outcome: WorkshopFetchOutcome) -> Self {
        Self {
            outcome,
            listed: true,
            consumer_appid: None,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum SteamSearchTarget {
    All = 0,
    Title = 1,
    Description = 2,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SteamSearchRequest {
    pub query_type: u32,
    pub file_type: u32,
    pub page: u32,
    pub num_per_page: u32,
    pub search_text: Option<String>,
    pub required_tags: Vec<String>,
    pub language: i32,
    pub days: Option<u32>,
    pub search_text_target: Option<SteamSearchTarget>,
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

    async fn search_files(&self, request: &SteamSearchRequest)
        -> anyhow::Result<WorkshopQueryPage>;

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
