use crate::ids::{ContributorId, WorkshopId};
use crate::workshop::{
    MetadataSource, WorkshopDependency, WorkshopFileType, WorkshopItemRecord, WorkshopItemStatus,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    Community,
    TrustedPlayer,
    Maintainer,
    Official,
}

impl TrustTier {
    pub fn default_weight(self) -> i32 {
        match self {
            Self::Official => 100,
            Self::Maintainer => 80,
            Self::TrustedPlayer => 50,
            Self::Community => 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItemsRequest {
    pub ids: Vec<WorkshopId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItemsResponse {
    pub items: Vec<WorkshopItemRecord>,
    pub missing_ids: Vec<WorkshopId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemSubmission {
    pub workshop_id: WorkshopId,
    #[serde(default)]
    pub file_type: WorkshopFileType,
    #[serde(default)]
    pub status: WorkshopItemStatus,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_steam_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_created: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_updated: Option<i64>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<WorkshopDependency>,
    #[serde(default)]
    pub collection_children: Vec<WorkshopId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriptions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favorites: Option<u64>,
    pub observed_source: MetadataSource,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionBatchRequest {
    pub items: Vec<ItemSubmission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictDecision {
    Accepted,
    Merged,
    RejectedLowerTrust,
    RejectedStale,
    FlaggedForReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionReceipt {
    pub workshop_id: WorkshopId,
    pub decision: ConflictDecision,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionBatchResponse {
    pub receipts: Vec<SubmissionReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContributorSummary {
    pub id: ContributorId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_id: Option<String>,
    pub trust_tier: TrustTier,
    pub trust_weight: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContributorRequest {
    pub name: String,
    #[serde(default)]
    pub steam_id: Option<String>,
    pub trust_tier: TrustTier,
    #[serde(default)]
    pub trust_weight: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContributorResponse {
    pub contributor: ContributorSummary,
    /// 明文 Token 仅在创建时返回一次，服务端只持久化 SHA-256 哈希。
    pub token: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateContributorRequest {
    #[serde(default)]
    pub trust_tier: Option<TrustTier>,
    #[serde(default)]
    pub trust_weight: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogEntry {
    pub id: i64,
    pub workshop_id: WorkshopId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contributor_id: Option<ContributorId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contributor_name: Option<String>,
    pub decision: ConflictDecision,
    pub reason: String,
    pub submitted_status: WorkshopItemStatus,
    pub submitted_time_updated: Option<i64>,
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
    pub contributors_count: u64,
    pub flagged_audits_count: u64,
    pub queued_scraper_jobs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScraperPlaceholderStatus {
    pub implemented: bool,
    pub provider_name: String,
    pub queued_jobs: u64,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_tier_ordering_matches_weights() {
        assert!(TrustTier::Official > TrustTier::Maintainer);
        assert!(TrustTier::Maintainer > TrustTier::TrustedPlayer);
        assert!(TrustTier::TrustedPlayer > TrustTier::Community);
        assert!(TrustTier::Official.default_weight() > TrustTier::Community.default_weight());
    }
}
