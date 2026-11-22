use crate::ids::WorkshopId;
use crate::workshop::{MetadataSource, WorkshopDependency, WorkshopItemRecord, WorkshopItemStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItemsRequest {
    pub ids: Vec<WorkshopId>,
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItemsResponse {
    pub items: Vec<WorkshopItemRecord>,
    pub missing_ids: Vec<WorkshopId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminLoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminLoginResponse {
    pub token: String,
    pub username: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminSessionInfo {
    pub authenticated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SteamAuthMode {
    #[default]
    Anonymous,
    Account,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SteamConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteamAuthFlowKind {
    Qr,
    Credentials,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteamAuthFlowState {
    WaitingForScan,
    WaitingForCode,
    Authenticating,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamAuthFlowStatus {
    pub flow_id: String,
    pub kind: SteamAuthFlowKind,
    pub state: SteamAuthFlowState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qr_svg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSteamStatus {
    pub provider_name: String,
    pub mode: SteamAuthMode,
    pub connection_state: SteamConnectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connected_since: Option<DateTime<Utc>>,
    pub total_requests: u64,
    pub failed_requests: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_auth_flow: Option<SteamAuthFlowStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartCredentialsAuthRequest {
    pub account_name: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitSteamGuardRequest {
    pub flow_id: String,
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRefreshTokenRequest {
    pub account_name: String,
    pub refresh_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSyncConfigDto {
    pub enable_read_through: bool,
    pub item_ttl_secs: u64,
    pub enable_incremental_sync: bool,
    pub incremental_interval_secs: u64,
    pub incremental_pages_per_tick: u32,
    pub enable_backfill: bool,
    pub backfill_interval_secs: u64,
    pub backfill_batch_size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSyncStatus {
    pub config: CatalogSyncConfigDto,
    pub incremental_running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_incremental_at: Option<DateTime<Utc>>,
    pub last_incremental_upserted: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high_water_time_updated: Option<i64>,
    pub backfill_running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backfill_cursor: Option<String>,
    pub backfill_synced_total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_workshop_total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminFetchUpstreamRequest {
    pub ids: Vec<WorkshopId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminFetchUpstreamResponse {
    pub items: Vec<WorkshopItemRecord>,
    pub removed_ids: Vec<WorkshopId>,
    pub unavailable_ids: Vec<WorkshopId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminItemOverrideRequest {
    pub status: WorkshopItemStatus,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub file_url: Option<String>,
    #[serde(default)]
    pub preview_url: Option<String>,
    #[serde(default)]
    pub dependencies: Option<Vec<WorkshopDependency>>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemRevisionEntry {
    pub id: i64,
    pub status: WorkshopItemStatus,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_updated: Option<i64>,
    #[serde(default)]
    pub dependencies: Vec<WorkshopDependency>,
    pub source: MetadataSource,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminItemDetailResponse {
    pub item: WorkshopItemRecord,
    pub is_admin_override: bool,
    pub last_verified_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<DateTime<Utc>>,
    pub revisions: Vec<ItemRevisionEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogEntry {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop_id: Option<WorkshopId>,
    pub actor: String,
    pub action: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<WorkshopItemStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_updated: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogStats {
    pub total_items: u64,
    pub active_items: u64,
    pub removed_items: u64,
    pub private_items: u64,
    pub collections_count: u64,
    pub overridden_items: u64,
    pub revisions_count: u64,
    pub audit_logs_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogOverviewResponse {
    pub stats: CatalogStats,
    pub steam: CatalogSteamStatus,
    pub sync: CatalogSyncStatus,
    pub config_path: Option<String>,
    pub bind_addr: String,
    pub db_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_items_request_defaults_force_refresh_false() {
        let parsed: BatchItemsRequest = serde_json::from_str(r#"{"ids":["1001","2002"]}"#).unwrap();
        assert_eq!(parsed.ids.len(), 2);
        assert!(!parsed.force_refresh);
    }
}
