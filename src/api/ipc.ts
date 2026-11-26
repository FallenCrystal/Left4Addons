import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type AddonDirType = 'loading' | 'workshop';
export type InstallState = 'installed' | 'uninstalled_known' | 'missing_file';
export type WorkshopItemStatus = 'active' | 'removed' | 'private_or_unavailable' | 'unknown';
export type WorkshopFileType = 'item' | 'collection';
export type MetadataSource =
  | 'catalog_server'
  | 'steam_kit'
  | 'steamworks_sdk'
  | 'steam_web_api'
  | 'web_scraper'
  | 'local_vpk'
  | 'admin_override';
export type BundleKind = 'dependency_graph' | 'campaign_parts' | 'manual';
export type AddonNamePreference = 'workshop_title' | 'addon_title' | 'file_name';

export type ReconcileIssueKind =
  | 'dummy_overwritten_by_game_update'
  | 'new_workshop_auto_download'
  | 'duplicate_identical_vpk'
  | 'orphan_dummy'
  | 'stale_dummy_metadata'
  | 'disabled_workshop_conflict';

export type ReconcileAction =
  | 'adopt_workshop_update'
  | 'keep_local_and_regenerate_dummy'
  | 'keep_workshop_delete_loading'
  | 'import_to_loading_and_create_dummy'
  | 'leave_in_workshop'
  | 'remove_orphan_dummy'
  | 'refresh_dummy'
  | 'enable_workshop_file'
  | 'ignore';

export interface MapEntry {
  code: string;
  name: string;
  imagePath?: string;
  imageHint?: string;
}

export interface AddonSummary {
  id: string;
  workshopId?: string;
  title: string;
  addonTitle?: string;
  workshopTitle?: string;
  author?: string;
  version?: string;
  installState: InstallState;
  primaryDir?: AddonDirType;
  enabled: boolean;
  currentFileName?: string;
  currentPath?: string;
  contentHash?: string;
  fileSize: number;
  filesCount: number;
  coverImage?: string;
  localCoverImage?: string;
  categories: string[];
  maps: MapEntry[];
  hasDummyBypass: boolean;
  remoteStatus: WorkshopItemStatus;
  localTimeUpdated?: number;
  remoteTimeUpdated?: number;
  updateAvailable: boolean;
  addonInfo: Record<string, string>;
}

export interface ReconcileIssue {
  id: string;
  kind: ReconcileIssueKind;
  workshopId?: string;
  addonId?: string;
  loadingPath?: string;
  workshopPath?: string;
  summary: string;
  recommendedAction: ReconcileAction;
  availableActions: ReconcileAction[];
  resolved: boolean;
  detectedAt: string;
}

export interface ReconcileBatchItem {
  issueId: string;
  action: ReconcileAction;
}

export interface ReconcileItemResult {
  issueId: string;
  action: ReconcileAction;
  ok: boolean;
  error?: DesktopError;
}

export interface ReconcileBatchReport {
  items: ReconcileItemResult[];
  snapshot: DashboardSnapshot;
}

export interface BundleRecord {
  id: string;
  name: string;
  kind: BundleKind;
  locked: boolean;
  memberAddonIds: string[];
  maps: MapEntry[];
  coverImage?: string;
}

export interface TrackedCollectionRecord {
  collectionId: string;
  title: string;
  description?: string;
  autoSync: boolean;
  memberWorkshopIds: string[];
  removedUpstreamIds: string[];
  lastSyncedAt?: string;
}

export interface UserGroup {
  id: string;
  name: string;
  tags: string[];
  color?: string;
  addonIds: string[];
}

export interface LoadoutProfile {
  id: string;
  name: string;
  description: string;
  enabledAddonIds: string[];
}

export interface ProfileApplyReport {
  profileId: string;
  enabledCount: number;
  disabledCount: number;
  skippedWorkshopOnly: string[];
  missingAddons: string[];
  batch: AddonBatchReport;
}

export interface TaskRecord {
  id: string;
  kind: 'download' | 'sync' | 'scan';
  status: 'queued' | 'running' | 'completed' | 'failed' | 'cancelled';
  targetId: string;
  targetTitle?: string;
  progressPercent: number;
  message?: string;
  error?: string;
  createdAt: string;
  updatedAt: string;
}

export type InvalidCharReplace = 'space' | 'underscore' | 'empty';

export interface RenameRuleConfig {
  enableWorkshopIdPrefix: boolean;
  enableGroupPrefix: boolean;
  cleanSpecialChars: boolean;
  invalidCharReplace: InvalidCharReplace;
  maxFilenameLength: number;
  enableTrim: boolean;
  enableRemoveDoubleSpaces: boolean;
}

export interface RenamePreviewItem {
  addonId: string;
  currentFileName: string;
  targetFileName: string;
  conflict?: string;
}

export type DesktopErrorCode =
  | 'invalid_input'
  | 'not_found'
  | 'conflict'
  | 'io'
  | 'database'
  | 'config'
  | 'vpk'
  | 'network'
  | 'unsupported'
  | 'cancelled'
  | 'workshop'
  | 'recovery_required';

export interface DesktopError {
  code: DesktopErrorCode;
  message: string;
}

export type AddonOperationResult = { addonId: string } & (
  | { status: 'applied' | 'unchanged' | 'not_attempted' }
  | { status: 'failed'; error: DesktopError }
);

export interface AddonBatchReport {
  items: AddonOperationResult[];
  refreshError?: DesktopError;
}

export type AddonId = string;
export type WorkshopId = string;

export type GameUpdateDummyPolicy =
  | 'ask'
  | 'auto_adopt_keep_customizations'
  | 'auto_keep_local_regenerate_dummy';

export const DEFAULT_FILTER_REPO_URL = 'https://github.com/FallenCrystal/Left4Filters.git';
export const DEFAULT_LLM_ENDPOINT = 'https://api.openai.com/v1';
export const DEFAULT_LLM_MODEL = 'gpt-4o-mini';
export const DEFAULT_LLM_TOKEN_ENV_VAR = 'L4A_LLM_API_KEY';
export const DEFAULT_LLM_SYSTEM_PROMPT =
  'You are a strict content moderation reviewer for Left 4 Dead 2 Steam Workshop addons. Flag items that contain explicit NSFW/sexualized content (nudity, lingerie/bikini fetishes, explicit sexual poses, R18 adult mods) or extreme/graphic gore closeup imagery/text beyond ordinary L4D2 gameplay UI, red fonts, maps, scripts, or standard clothed anime/survivor skins. Also flag clearly visible corpses or piles of bodies in preview images, including corpses without visible blood.';

export type FilterGalleryAggregation = 'max' | 'top2_mean' | 'mean';

export type FilterPresetId =
  | 'recommended'
  | 'strict'
  | 'relaxed'
  | 'nsfw_only'
  | 'gore_only'
  | 'low_cpu'
  | 'custom';

export type ClientFilterSourceMode = 'local' | 'catalog' | 'custom_endpoint';
export type FilterScoreTarget = 'overall' | 'gore' | 'nsfw' | 'max_category';
export type LlmTokenSource = 'env_var' | 'encrypted_local';
export type FlaggedItemAction = 'mark_only' | 'hide_image' | 'blur_card' | 'hide_card';
export type FilterFailureDisplay = 'notice' | 'as_flagged';

export type ExclusionDisplayLevel =
  | 'normal'
  | 'lazy_load'
  | 'blur_image'
  | 'hide_image'
  | 'hide_text';

export type LocalFilterServiceState = 'stopped' | 'deploying' | 'starting' | 'running' | 'error';

export type FilterDecisionSource =
  | 'model_safe'
  | 'model_direct_flag'
  | 'llm_flagged_true'
  | 'llm_flagged_false';

export interface FilterModelParams {
  threads: number;
  enableText: boolean;
  enableImage: boolean;
  maxImages: number;
  galleryAggregation: FilterGalleryAggregation;
  goreWeight: number;
  nsfwWeight: number;
  temperature: number;
  bias: number;
}

export interface FilterProbabilities {
  prob: number;
  goreProb: number;
  nsfwProb: number;
  corpseProb?: number;
  corpsePileProb?: number;
  corpseReviewFloor?: number;
  fusionProb?: number;
  corpseImageIndices?: number[];
  textProb?: number;
  textGoreProb?: number;
  textNsfwProb?: number;
  titleProb?: number;
  descriptionProb?: number;
  imageProb?: number;
  imageGoreProb?: number;
  imageNsfwProb?: number;
  perImageProbs?: number[];
  scoringRevision?: string;
}

export interface ClientFilterConfig {
  enabled: boolean;
  mode: ClientFilterSourceMode;
  customEndpointUrl?: string | null;
  applyToWorkshop: boolean;
  applyToLibrary: boolean;
  repoUrl: string;
  trackLatest: boolean;
  pinnedCommit?: string | null;
  autoStartLocalService: boolean;
  preset: FilterPresetId;
  localParams: FilterModelParams;
  scoreTarget: FilterScoreTarget;
  enableDirectFlag: boolean;
  flagThreshold: number;
  llmEnabled: boolean;
  llmThreshold: number;
  llmEndpoint: string;
  llmModel: string;
  llmSendImages: boolean;
  llmSystemPrompt: string;
  llmTokenSource: LlmTokenSource;
  llmTokenEnvVar: string;
  flaggedAction: FlaggedItemAction;
  failureDisplay: FilterFailureDisplay;
  subdivideModalities: boolean;
  modalityThreshold: number;
}

export interface FilterTokenStatus {
  hasEncryptedToken: boolean;
  envVarName: string;
  envVarPresent: boolean;
  cacheGeneration?: number;
}

export interface FilterCacheClearResult {
  count: number;
  cacheGeneration: number;
}

export interface FilterEnvReport {
  gitAvailable: boolean;
  gitVersion?: string;
  gitPath?: string;
  pythonAvailable: boolean;
  pythonVersion?: string;
  pythonPath?: string;
  venvAvailable: boolean;
}

export interface FilterCommitInfo {
  sha: string;
  shortSha: string;
  subject: string;
  committedAt: string;
}

export interface LocalFilterDeploymentStatus {
  env: FilterEnvReport;
  deployed: boolean;
  repoDir: string;
  venvDir: string;
  serviceState: LocalFilterServiceState;
  serviceEndpoint?: string;
  runningPid?: number;
  currentCommit?: FilterCommitInfo;
  remoteLatestCommit?: FilterCommitInfo;
  updateAvailable: boolean;
  recentCommits: FilterCommitInfo[];
  stageMessage?: string;
  lastError?: string;
  cacheGeneration?: number;
}

export interface DeployLocalFilterRequest {
  repoUrl?: string;
  trackLatest: boolean;
  targetCommit?: string;
  startAfterDeploy: boolean;
}

export interface FilterExclusionRecord {
  addonId: AddonId;
  displayLevel: ExclusionDisplayLevel;
  bindFingerprint: boolean;
  boundFingerprint?: string;
  currentFingerprint?: string;
  fingerprintValid: boolean;
  note?: string;
  createdAt: string;
  updatedAt: string;
}

export interface UpsertFilterExclusionRequest {
  addonId: AddonId;
  displayLevel: ExclusionDisplayLevel;
  bindFingerprint?: boolean;
  note?: string;
}

export interface FilterEvaluationTarget {
  targetKey: string;
  addonId?: AddonId;
  workshopId?: WorkshopId;
  title: string;
  description?: string;
  tags: string[];
  imageSources: string[];
  revisionHint?: string;
  bypassCache?: boolean;
}

export interface FilterItemVerdict {
  targetKey: string;
  revisionHint?: string;
  flagged: boolean;
  decisionSource: FilterDecisionSource;
  evaluatedScore: number;
  probabilities: FilterProbabilities;
  textSensitive: boolean;
  titleSensitive: boolean;
  descriptionSensitive: boolean;
  imageSensitive: boolean;
  evaluatedImageIndices: number[];
  sensitiveImageIndices: number[];
  complete: boolean;
  evaluatedAt: string;
}

export type FilterTargetOutcome =
  | { status: 'completed'; verdict: FilterItemVerdict; complete: boolean }
  | { status: 'failed'; error: string };

export type FilterEvaluationItemResult = {
  targetKey: string;
} & FilterTargetOutcome;

export interface FilterEvaluationBatchResponse {
  policySignature: string;
  cacheGeneration?: number;
  items: FilterEvaluationItemResult[];
}

export interface AppConfig {
  loadingDir: string;
  workshopDir: string;
  downloadConcurrency: number;
  maxDownloadRetries: number;
  enableDummyBypass: boolean;
  autoAdoptNewWorkshopDownloads: boolean;
  gameUpdateDummyPolicy: GameUpdateDummyPolicy;
  disableSteamworksSdk: boolean;
  forceSteamworksSdkDownload: boolean;
  httpsOnly: boolean;
  catalogServerUrl?: string | null;
  sourceOrder: string[];
  renameRules: RenameRuleConfig;
  addonNamePreference?: AddonNamePreference;
  collapseCampaignBundles?: boolean;
  filter: ClientFilterConfig;
}

export interface DashboardSnapshot {
  config: AppConfig;
  addons: AddonSummary[];
  bundles: BundleRecord[];
  collections: TrackedCollectionRecord[];
  userGroups: UserGroup[];
  profiles: LoadoutProfile[];
  openIssues: ReconcileIssue[];
  tasks: TaskRecord[];
  filterExclusions: FilterExclusionRecord[];
  cachedFilterVerdicts?: FilterItemVerdict[];
  filterPolicySignature?: string;
  filterCacheGeneration?: number;
}

export interface SteamCapabilities {
  bridgeAvailable: boolean;
  bridgeInitialized: boolean;
  bridgeVersion?: string | null;
  currentUserSteamId?: string | null;
  lastError?: string | null;
}

export interface WorkshopDependency {
  workshopId: string;
  title?: string;
  required: boolean;
}

export interface WorkshopPreview {
  previewId?: string;
  url?: string;
  youtubeVideoId?: string;
  previewType: number;
}

export interface WorkshopItemRecord {
  workshopId: string;
  fileType: WorkshopFileType;
  status: WorkshopItemStatus;
  title: string;
  description?: string;
  creatorSteamId?: string;
  creatorName?: string;
  previewUrl?: string;
  previews?: WorkshopPreview[];
  fileUrl?: string;
  fileSize?: number;
  timeCreated?: number;
  timeUpdated?: number;
  tags: string[];
  dependencies: WorkshopDependency[];
  collectionChildren: string[];
  subscriptions?: number;
  favorites?: number;
  source: MetadataSource;
  fetchedAt: string;
}

export interface SearchQuery {
  query?: string;
  tag?: string;
  status?: WorkshopItemStatus;
  sort?: string;
  page?: number;
  pageSize?: number;
  language?: string;
  searchTarget?: 'all' | 'title' | 'description';
  fileType?: WorkshopFileType;
}

export interface SearchResponse {
  items: WorkshopItemRecord[];
  total: number;
  page: number;
  pageSize: number;
  source: MetadataSource;
}

export interface UpdateAvailableItem {
  addonId: string;
  workshopId: string;
  title: string;
  localTimeUpdated?: number;
  remoteTimeUpdated: number;
  source: MetadataSource;
}

export interface RemovedUpstreamItem {
  addonId: string;
  workshopId: string;
  title: string;
  status: WorkshopItemStatus;
  localFilePreserved: boolean;
  detectedAt: string;
}

export interface DependencyDiff {
  parentWorkshopId: string;
  parentTitle: string;
  addedDependencies: string[];
  removedDependencies: string[];
}

export interface ObsoleteDependencyHint {
  dependencyWorkshopId: string;
  dependencyAddonId?: string;
  dependencyTitle?: string;
  removedFromParents: string[];
  remainingActiveParents: string[];
  canSafelyRemove: boolean;
}

export interface CollectionSyncDiff {
  collectionId: string;
  title: string;
  addedItems: string[];
  removedItems: string[];
}

export interface SyncReport {
  checkedItems: number;
  updatesAvailable: UpdateAvailableItem[];
  removedUpstream: RemovedUpstreamItem[];
  dependencyDiffs: DependencyDiff[];
  obsoleteDependencies: ObsoleteDependencyHint[];
  collectionDiffs: CollectionSyncDiff[];
  warnings: string[];
}

export type CoreEvent =
  | { type: 'snapshot_updated'; externalChange: boolean }
  | {
      type: 'download_progress';
      taskId: string;
      workshopId: string;
      title?: string | null;
      downloaded: number;
      total?: number | null;
      percent: number;
      speedBps?: number | null;
    }
  | { type: 'reconcile_issues_detected'; count: number }
  | { type: 'watch_error'; message: string }
  | {
      type: 'filter_verdicts_updated';
      verdicts: FilterItemVerdict[];
      policySignature?: string | null;
      cacheGeneration?: number;
    }
  | { type: 'local_filter_status_changed'; status: LocalFilterDeploymentStatus };

export class IpcError extends Error {
  constructor(readonly code: string, message: string) {
    super(message);
  }
}

async function invokeCommand<T>(...args: Parameters<typeof invoke>): Promise<T> {
  try {
    return await invoke<T>(...args);
  } catch (error) {
    if (
      typeof error === 'object' &&
      error !== null &&
      'code' in error &&
      typeof error.code === 'string' &&
      'message' in error &&
      typeof error.message === 'string'
    ) {
      throw new IpcError(error.code, error.message);
    }
    throw new IpcError('ipc', error instanceof Error ? error.message : String(error));
  }
}

export function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export async function getDashboardSnapshot(): Promise<DashboardSnapshot> {
  return invokeCommand<DashboardSnapshot>('get_dashboard_snapshot');
}

export async function scanWorkspace(): Promise<DashboardSnapshot> {
  return invokeCommand<DashboardSnapshot>('scan_workspace');
}

export async function saveConfig(config: AppConfig): Promise<AppConfig> {
  return invokeCommand<AppConfig>('save_config', { config });
}

export async function detectGameAddonsPath(): Promise<string | null> {
  return invokeCommand<string | null>('detect_game_addons_path');
}

export async function getSteamCapabilities(): Promise<SteamCapabilities> {
  return invokeCommand<SteamCapabilities>('get_steam_capabilities');
}

export async function syncWorkshop(): Promise<SyncReport> {
  return invokeCommand<SyncReport>('sync_workshop');
}

export async function searchWorkshop(query: SearchQuery): Promise<SearchResponse> {
  return invokeCommand<SearchResponse>('search_workshop', { query });
}

export async function fetchWorkshopItem(workshopId: string): Promise<WorkshopItemRecord> {
  return invokeCommand<WorkshopItemRecord>('fetch_workshop_item', { workshopId });
}

export async function getSubscribedWorkshopIds(): Promise<string[]> {
  return invokeCommand<string[]>('get_subscribed_workshop_ids');
}

export async function downloadWorkshopItem(
  workshopId: string,
  titleHint?: string,
): Promise<AddonSummary> {
  return invokeCommand<AddonSummary>('download_workshop_item', { workshopId, titleHint });
}

export async function updateWorkshopItem(
  workshopId: string,
  titleHint?: string,
): Promise<AddonSummary> {
  return invokeCommand<AddonSummary>('update_workshop_item', { workshopId, titleHint });
}

export async function cancelDownload(workshopId: string): Promise<boolean> {
  return invokeCommand<boolean>('cancel_download', { workshopId });
}

export async function resolveReconcileIssue(
  issueId: string,
  action: ReconcileAction,
): Promise<DashboardSnapshot> {
  return invokeCommand<DashboardSnapshot>('resolve_reconcile_issue', { issueId, action });
}

export async function resolveReconcileIssuesBatch(
  items: ReconcileBatchItem[],
): Promise<ReconcileBatchReport> {
  return invokeCommand<ReconcileBatchReport>('resolve_reconcile_issues_batch', { items });
}

export async function moveAddons(
  addonIds: string[],
  targetDirType: AddonDirType,
): Promise<AddonBatchReport> {
  return invokeCommand<AddonBatchReport>('move_addons', { addonIds, targetDirType });
}

export async function toggleAddons(
  addonIds: string[],
  enabled: boolean,
  allowWorkshopDir = false,
): Promise<AddonBatchReport> {
  return invokeCommand<AddonBatchReport>('toggle_addons', {
    addonIds,
    enabled,
    allowWorkshopDir,
  });
}

export async function previewRename(
  rules: RenameRuleConfig,
  addonIds: string[] = [],
): Promise<RenamePreviewItem[]> {
  return invokeCommand<RenamePreviewItem[]>('preview_rename', { rules, addonIds });
}

export async function applyRenames(
  renames: [addonId: string, fileName: string][],
): Promise<AddonBatchReport> {
  return invokeCommand<AddonBatchReport>('apply_renames', { renames });
}

export async function deleteAddons(
  addonIds: string[],
  deleteDiskFile: boolean,
  purgeKnownRecord: boolean,
): Promise<AddonBatchReport> {
  return invokeCommand<AddonBatchReport>('delete_addons', {
    addonIds,
    deleteDiskFile,
    purgeKnownRecord,
  });
}

export async function safeDeleteObsoleteDependency(
  workshopId: string,
  deleteDiskFile: boolean,
  purgeKnownRecord: boolean,
): Promise<AddonBatchReport> {
  return invokeCommand<AddonBatchReport>('safe_delete_obsolete_dependency', {
    workshopId,
    deleteDiskFile,
    purgeKnownRecord,
  });
}

export async function rebuildBundles(): Promise<BundleRecord[]> {
  return invokeCommand<BundleRecord[]>('rebuild_bundles');
}

export async function createManualBundle(
  name: string,
  memberAddonIds: string[],
): Promise<BundleRecord> {
  return invokeCommand<BundleRecord>('create_manual_bundle', { name, memberAddonIds });
}

export async function deleteBundle(bundleId: string): Promise<void> {
  return invokeCommand<void>('delete_bundle', { bundleId });
}

export async function setBundleLocked(bundleId: string, locked: boolean): Promise<void> {
  return invokeCommand<void>('set_bundle_locked', { bundleId, locked });
}

export async function trackCollection(
  collectionId: string,
  autoSync: boolean,
): Promise<TrackedCollectionRecord> {
  return invokeCommand<TrackedCollectionRecord>('track_collection', {
    collectionId,
    autoSync,
  });
}

export async function untrackCollection(collectionId: string): Promise<void> {
  return invokeCommand<void>('untrack_collection', { collectionId });
}

export async function saveUserGroup(group: UserGroup): Promise<UserGroup> {
  return invokeCommand<UserGroup>('save_user_group', { group });
}

export async function deleteUserGroup(groupId: string): Promise<void> {
  return invokeCommand<void>('delete_user_group', { groupId });
}

export async function saveProfile(profile: LoadoutProfile): Promise<LoadoutProfile> {
  return invokeCommand<LoadoutProfile>('save_profile', { profile });
}

export async function deleteProfile(profileId: string): Promise<void> {
  return invokeCommand<void>('delete_profile', { profileId });
}

export async function applyProfile(profileId: string): Promise<ProfileApplyReport> {
  return invokeCommand<ProfileApplyReport>('apply_profile', { profileId });
}

export async function searchByMapCode(query: string): Promise<AddonSummary[]> {
  return invokeCommand<AddonSummary[]>('search_by_map_code', { query });
}

export async function readCachedImage(cacheKey: string): Promise<number[]> {
  return invokeCommand<number[]>('read_cached_image', { cacheKey });
}

export function workshopCoverUrl(url: string): string {
  const resource = new URL(convertFileSrc('image', 'l4a-cover'));
  resource.searchParams.set('url', url);
  return resource.toString();
}

export async function openExternalUrl(url: string): Promise<void> {
  return invokeCommand<void>('open_external_url', { url });
}

export async function openInFileManager(path: string): Promise<void> {
  return invokeCommand<void>('open_in_file_manager', { path });
}

export async function getLocalFilterStatus(): Promise<LocalFilterDeploymentStatus> {
  return invokeCommand<LocalFilterDeploymentStatus>('get_local_filter_status');
}

export async function checkLocalFilterUpdates(): Promise<LocalFilterDeploymentStatus> {
  return invokeCommand<LocalFilterDeploymentStatus>('check_local_filter_updates');
}

export async function deployLocalFilter(
  request: DeployLocalFilterRequest,
): Promise<LocalFilterDeploymentStatus> {
  return invokeCommand<LocalFilterDeploymentStatus>('deploy_local_filter', { request });
}

export async function startLocalFilterService(): Promise<LocalFilterDeploymentStatus> {
  return invokeCommand<LocalFilterDeploymentStatus>('start_local_filter_service');
}

export async function stopLocalFilterService(): Promise<LocalFilterDeploymentStatus> {
  return invokeCommand<LocalFilterDeploymentStatus>('stop_local_filter_service');
}

export async function getFilterTokenStatus(): Promise<FilterTokenStatus> {
  return invokeCommand<FilterTokenStatus>('get_filter_token_status');
}

export async function setFilterLlmToken(token: string | null): Promise<FilterTokenStatus> {
  return invokeCommand<FilterTokenStatus>('set_filter_llm_token', { token });
}

export async function evaluateFilterItems(
  targets: FilterEvaluationTarget[],
  automatic = false,
): Promise<FilterEvaluationBatchResponse> {
  return invokeCommand<FilterEvaluationBatchResponse>(
    'evaluate_filter_items',
    automatic ? { targets, automatic } : { targets },
  );
}

export async function clearFilterCache(): Promise<FilterCacheClearResult> {
  return invokeCommand<FilterCacheClearResult>('clear_filter_cache');
}

export async function upsertFilterExclusion(
  request: UpsertFilterExclusionRequest,
): Promise<DashboardSnapshot> {
  return invokeCommand<DashboardSnapshot>('upsert_filter_exclusion', { request });
}

export async function deleteFilterExclusion(addonId: AddonId): Promise<DashboardSnapshot> {
  return invokeCommand<DashboardSnapshot>('delete_filter_exclusion', { addonId });
}

export async function subscribeCoreEvents(
  handler: (payload: CoreEvent) => void,
): Promise<UnlistenFn> {
  return listen<CoreEvent>('l4a-core-event', (event) => {
    const payload = event.payload;
    if (payload.type === 'download_progress') {
      handler({
        ...payload,
        title: payload.title ?? undefined,
        total: payload.total ?? undefined,
        speedBps: payload.speedBps ?? undefined,
      });
      return;
    }
    if (payload.type === 'filter_verdicts_updated') {
      handler({
        ...payload,
        policySignature: payload.policySignature ?? undefined,
      });
      return;
    }
    handler(payload);
  });
}
