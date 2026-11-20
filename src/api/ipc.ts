import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type AddonDirType = 'loading' | 'workshop';
export type InstallState = 'installed' | 'uninstalled_known' | 'missing_file';
export type WorkshopItemStatus = 'active' | 'removed' | 'private_or_unavailable' | 'unknown';
export type BundleKind = 'dependency_graph' | 'campaign_parts' | 'manual';

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

export interface AppConfig {
  loadingDir: string;
  workshopDir: string;
  downloadConcurrency: number;
  maxDownloadRetries: number;
  enableDummyBypass: boolean;
  autoAdoptNewWorkshopDownloads: boolean;
  gameUpdateDummyPolicy: 'ask' | 'auto_adopt_keep_customizations' | 'auto_keep_local_regenerate_dummy';
  disableSteamworksSdk: boolean;
  forceSteamworksSdkDownload: boolean;
  catalogServerUrl?: string | null;
  catalogContributorToken?: string | null;
  sourceOrder: string[];
  renameRules: RenameRuleConfig;
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
}

export interface UpdateAvailableItem {
  addonId: string;
  workshopId: string;
  title: string;
  localTimeUpdated?: number;
  remoteTimeUpdated: number;
  source: string;
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
      downloaded: number;
      total?: number;
      percent: number;
    }
  | { type: 'reconcile_issues_detected'; count: number }
  | { type: 'watch_error'; message: string };

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
      typeof error === 'object' && error !== null &&
      'code' in error && typeof error.code === 'string' &&
      'message' in error && typeof error.message === 'string'
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

export async function syncWorkshop(): Promise<SyncReport> {
  return invokeCommand<SyncReport>('sync_workshop');
}

export async function resolveReconcileIssue(
  issueId: string,
  action: ReconcileAction,
): Promise<DashboardSnapshot> {
  return invokeCommand<DashboardSnapshot>('resolve_reconcile_issue', { issueId, action });
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
  return invokeCommand<AddonBatchReport>('toggle_addons', { addonIds, enabled, allowWorkshopDir });
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
    addonIds, deleteDiskFile, purgeKnownRecord,
  });
}

export async function applyProfile(profileId: string): Promise<ProfileApplyReport> {
  return invokeCommand<ProfileApplyReport>('apply_profile', { profileId });
}

export async function searchByMapCode(query: string): Promise<AddonSummary[]> {
  return invokeCommand<AddonSummary[]>('search_by_map_code', { query });
}

export async function subscribeCoreEvents(
  handler: (payload: CoreEvent) => void,
): Promise<UnlistenFn> {
  return listen<CoreEvent>('l4a-core-event', (event) => {
    handler(event.payload);
  });
}
