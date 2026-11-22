use crate::ids::{AddonId, BundleId, GroupId, IssueId, ProfileId, TaskId, WorkshopId};
use crate::workshop::{MetadataSource, WorkshopItemStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddonDirType {
    Loading,
    Workshop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    Installed,
    UninstalledKnown,
    MissingFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    RealAddon,
    DummyV1,
    DummyV2,
    Corrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapEntry {
    pub code: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalFileRecord {
    pub path: String,
    pub dir_type: AddonDirType,
    pub file_name: String,
    pub enabled: bool,
    pub role: FileRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop_id: Option<WorkshopId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dummy_original_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dummy_original_time_updated: Option<i64>,
    pub file_size: u64,
    pub modified_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonSummary {
    pub id: AddonId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop_id: Option<WorkshopId>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub install_state: InstallState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_dir: Option<AddonDirType>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_file_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub file_size: u64,
    pub files_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_image: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub maps: Vec<MapEntry>,
    #[serde(default)]
    pub has_dummy_bypass: bool,
    #[serde(default)]
    pub remote_status: WorkshopItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_time_updated: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_time_updated: Option<i64>,
    #[serde(default)]
    pub update_available: bool,
    #[serde(default)]
    pub addon_info: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileIssueKind {
    /// `loading/` 存在托管 VPK，且 `workshop/` 出现新的真实 VPK（原 Dummy 被游戏更新覆写）。
    DummyOverwrittenByGameUpdate,
    /// `workshop/` 出现全新未托管的真实 VPK（游戏启动或联机自动下载了新组件）。
    NewWorkshopAutoDownload,
    /// 两个目录存在完全相同指纹的真实 VPK。
    DuplicateIdenticalVpk,
    /// `workshop/` 存在孤儿 Dummy，但 `loading/` 对应真实 VPK 已不在。
    OrphanDummy,
    /// Dummy 仍为旧版 v1 或元信息指纹与 `loading/` 真实 VPK 不一致。
    StaleDummyMetadata,
    /// `workshop/` 目录内存在 `.vpk.disabled` 文件。
    DisabledWorkshopConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileAction {
    /// 采用 `workshop/` 更新覆盖 `loading/`（保留自定义文件名、启用状态和所有分组），并重建 v2 Dummy。
    AdoptWorkshopUpdate,
    /// 丢弃 `workshop/` 更新，保留当前 `loading/` 版本并重建 v2 Dummy。
    KeepLocalAndRegenerateDummy,
    /// 删除 `loading/` 旧副本，回归由 `workshop/` 直接托管。
    KeepWorkshopDeleteLoading,
    /// 将 `workshop/` 新组件移入 `loading/`（按重命名规则）并在 `workshop/` 生成 v2 Dummy。
    ImportToLoadingAndCreateDummy,
    /// 保留在 `workshop/` 作为普通工坊模组管理。
    LeaveInWorkshop,
    /// 删除孤儿 Dummy 文件。
    RemoveOrphanDummy,
    /// 基于当前 `loading/` 真实 VPK 重新生成或升级为 v2 Dummy。
    RefreshDummy,
    /// 将 `workshop/` 中被禁用的文件恢复为 `.vpk`。
    EnableWorkshopFile,
    /// 暂时忽略此对账提示。
    Ignore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileBatchItem {
    pub issue_id: IssueId,
    pub action: ReconcileAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileItemResult {
    pub issue_id: IssueId,
    pub action: ReconcileAction,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<DesktopError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileBatchReport {
    pub items: Vec<ReconcileItemResult>,
    pub snapshot: DashboardSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileIssue {
    pub id: IssueId,
    pub kind: ReconcileIssueKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop_id: Option<WorkshopId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addon_id: Option<AddonId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loading_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop_path: Option<String>,
    pub summary: String,
    pub recommended_action: ReconcileAction,
    pub available_actions: Vec<ReconcileAction>,
    pub resolved: bool,
    pub detected_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAvailableItem {
    pub addon_id: AddonId,
    pub workshop_id: WorkshopId,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_time_updated: Option<i64>,
    pub remote_time_updated: i64,
    pub source: MetadataSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedUpstreamItem {
    pub addon_id: AddonId,
    pub workshop_id: WorkshopId,
    pub title: String,
    pub status: WorkshopItemStatus,
    pub local_file_preserved: bool,
    pub detected_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObsoleteDependencyHint {
    pub dependency_workshop_id: WorkshopId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_addon_id: Option<AddonId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_title: Option<String>,
    pub removed_from_parents: Vec<WorkshopId>,
    pub remaining_active_parents: Vec<WorkshopId>,
    pub can_safely_remove: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyDiff {
    pub parent_workshop_id: WorkshopId,
    pub parent_title: String,
    pub added_dependencies: Vec<WorkshopId>,
    pub removed_dependencies: Vec<WorkshopId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionSyncDiff {
    pub collection_id: WorkshopId,
    pub title: String,
    pub added_items: Vec<WorkshopId>,
    pub removed_items: Vec<WorkshopId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub checked_items: usize,
    pub updates_available: Vec<UpdateAvailableItem>,
    pub removed_upstream: Vec<RemovedUpstreamItem>,
    pub dependency_diffs: Vec<DependencyDiff>,
    pub obsolete_dependencies: Vec<ObsoleteDependencyHint>,
    pub collection_diffs: Vec<CollectionSyncDiff>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleKind {
    DependencyGraph,
    CampaignParts,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleRecord {
    pub id: BundleId,
    pub name: String,
    pub kind: BundleKind,
    pub locked: bool,
    pub member_addon_ids: Vec<AddonId>,
    #[serde(default)]
    pub maps: Vec<MapEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_image: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedCollectionRecord {
    pub collection_id: WorkshopId,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub auto_sync: bool,
    #[serde(default)]
    pub member_workshop_ids: Vec<WorkshopId>,
    #[serde(default)]
    pub removed_upstream_ids: Vec<WorkshopId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserGroup {
    pub id: GroupId,
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub addon_ids: Vec<AddonId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadoutProfile {
    pub id: ProfileId,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled_addon_ids: Vec<AddonId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileApplyReport {
    pub profile_id: ProfileId,
    pub enabled_count: usize,
    pub disabled_count: usize,
    pub skipped_workshop_only: Vec<AddonId>,
    pub missing_addons: Vec<AddonId>,
    pub batch: AddonBatchReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InvalidCharReplace {
    Space,
    #[default]
    Underscore,
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameRuleConfig {
    #[serde(default = "default_true_flag")]
    pub enable_workshop_id_prefix: bool,
    #[serde(default = "default_true_flag")]
    pub enable_group_prefix: bool,
    #[serde(default)]
    pub clean_special_chars: bool,
    #[serde(default)]
    pub invalid_char_replace: InvalidCharReplace,
    #[serde(default)]
    pub max_filename_length: usize,
    #[serde(default = "default_true_flag")]
    pub enable_trim: bool,
    #[serde(default = "default_true_flag")]
    pub enable_remove_double_spaces: bool,
}

fn default_true_flag() -> bool {
    true
}

impl Default for RenameRuleConfig {
    fn default() -> Self {
        Self {
            enable_workshop_id_prefix: true,
            enable_group_prefix: true,
            clean_special_chars: false,
            invalid_char_replace: InvalidCharReplace::Underscore,
            max_filename_length: 0,
            enable_trim: true,
            enable_remove_double_spaces: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamePreviewItem {
    pub addon_id: AddonId,
    pub current_file_name: String,
    pub target_file_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopErrorCode {
    InvalidInput,
    NotFound,
    Conflict,
    Io,
    Database,
    Config,
    Vpk,
    Network,
    Unsupported,
    Cancelled,
    Workshop,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopError {
    pub code: DesktopErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AddonOperationOutcome {
    Applied,
    Unchanged,
    Failed { error: DesktopError },
    NotAttempted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonOperationResult {
    pub addon_id: AddonId,
    #[serde(flatten)]
    pub outcome: AddonOperationOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonBatchReport {
    pub items: Vec<AddonOperationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_error: Option<DesktopError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GameUpdateDummyPolicy {
    #[default]
    Ask,
    AutoAdoptKeepCustomizations,
    AutoKeepLocalRegenerateDummy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub loading_dir: String,
    pub workshop_dir: String,
    #[serde(default = "default_concurrency")]
    pub download_concurrency: usize,
    #[serde(default = "default_retries")]
    pub max_download_retries: usize,
    #[serde(default)]
    pub enable_dummy_bypass: bool,
    #[serde(default)]
    pub auto_adopt_new_workshop_downloads: bool,
    #[serde(default)]
    pub game_update_dummy_policy: GameUpdateDummyPolicy,
    #[serde(default)]
    pub disable_steamworks_sdk: bool,
    #[serde(default)]
    pub force_steamworks_sdk_download: bool,
    #[serde(default = "default_true_flag")]
    pub https_only: bool,
    #[serde(default)]
    pub catalog_server_url: Option<String>,
    #[serde(default = "default_provider_order")]
    pub source_order: Vec<String>,
    #[serde(default)]
    pub rename_rules: RenameRuleConfig,
}

fn default_concurrency() -> usize {
    2
}

fn default_retries() -> usize {
    3
}

fn default_provider_order() -> Vec<String> {
    vec![
        "catalog-server".to_string(),
        "steamworks-sdk".to_string(),
        "steam-web-api".to_string(),
    ]
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            loading_dir: String::new(),
            workshop_dir: String::new(),
            download_concurrency: default_concurrency(),
            max_download_retries: default_retries(),
            enable_dummy_bypass: false,
            auto_adopt_new_workshop_downloads: false,
            game_update_dummy_policy: GameUpdateDummyPolicy::default(),
            disable_steamworks_sdk: false,
            force_steamworks_sdk_download: false,
            https_only: true,
            catalog_server_url: None,
            source_order: default_provider_order(),
            rename_rules: RenameRuleConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Download,
    Sync,
    Scan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    pub id: TaskId,
    pub kind: TaskKind,
    pub status: TaskStatus,
    pub target_id: String,
    pub progress_percent: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSnapshot {
    pub config: AppConfig,
    pub addons: Vec<AddonSummary>,
    pub bundles: Vec<BundleRecord>,
    pub collections: Vec<TrackedCollectionRecord>,
    pub user_groups: Vec<UserGroup>,
    pub profiles: Vec<LoadoutProfile>,
    pub open_issues: Vec<ReconcileIssue>,
    pub tasks: Vec<TaskRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CoreEvent {
    SnapshotUpdated {
        external_change: bool,
    },
    DownloadProgress {
        task_id: TaskId,
        workshop_id: WorkshopId,
        downloaded: u64,
        total: Option<u64>,
        percent: u8,
    },
    ReconcileIssuesDetected {
        count: usize,
    },
    WatchError {
        message: String,
    },
}
