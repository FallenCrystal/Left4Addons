import type { useTranslation } from 'react-i18next';
import * as ipc from '../../api/ipc';
import type {
  AddonBatchReport,
  AddonDirType,
  AddonId,
  AddonSummary,
  AppConfig,
  BundleRecord,
  CoreEvent,
  DashboardSnapshot,
  DeployLocalFilterRequest,
  FilterEvaluationBatchResponse,
  FilterEvaluationTarget,
  FilterTokenStatus,
  LoadoutProfile,
  LocalFilterDeploymentStatus,
  ProfileApplyReport,
  ReconcileAction,
  ReconcileBatchItem,
  ReconcileBatchReport,
  RenamePreviewItem,
  RenameRuleConfig,
  SearchQuery,
  SearchResponse,
  SteamCapabilities,
  SyncReport,
  TrackedCollectionRecord,
  UpsertFilterExclusionRequest,
  UserGroup,
  WorkshopItemRecord,
} from '../../api/ipc';
import { previewEngine } from '../../api/previewStore';

export type TranslateFn = ReturnType<typeof useTranslation>['t'];

export interface LiveDownloadState {
  requestId: number;
  taskId: string;
  workshopId: string;
  title?: string;
  downloaded: number;
  total?: number;
  percent: number;
  speedBps: number;
  hasProgress: boolean;
  cancelling: boolean;
  updatedAt: number;
}

export interface DownloadSpeedTracker {
  latestDownloaded: number;
  lastSampleBytes: number;
  lastSampleAt: number;
  lastEventAt: number;
  backendSpeed: boolean;
}

export interface SnackbarItem {
  id: number;
  message: string;
  tone?: 'default' | 'error';
  actionLabel?: string;
  onAction?: () => void;
}

export interface NotifyOptions {
  tone?: 'default' | 'error';
  actionLabel?: string;
  onAction?: () => void;
}

export type NotifyFn = (message: string, options?: NotifyOptions) => void;

export interface BatchDialogPayload {
  batch: AddonBatchReport;
  profileSummary?: {
    enabledCount: number;
    disabledCount: number;
    skippedWorkshopOnly: string[];
    missingAddons: string[];
  };
}

export interface DesktopClient {
  getDashboardSnapshot: () => Promise<DashboardSnapshot>;
  scanWorkspace: () => Promise<DashboardSnapshot>;
  saveConfig: (config: AppConfig) => Promise<AppConfig>;
  detectGameAddonsPath: () => Promise<string | null>;
  getSteamCapabilities: () => Promise<SteamCapabilities>;
  syncWorkshop: () => Promise<SyncReport>;
  searchWorkshop: (query: SearchQuery) => Promise<SearchResponse>;
  fetchWorkshopItem: (workshopId: string) => Promise<WorkshopItemRecord>;
  getSubscribedWorkshopIds: () => Promise<string[]>;
  downloadWorkshopItem: (workshopId: string, titleHint?: string) => Promise<AddonSummary>;
  updateWorkshopItem: (workshopId: string, titleHint?: string) => Promise<AddonSummary>;
  cancelDownload: (workshopId: string) => Promise<boolean>;
  resolveReconcileIssue: (
    issueId: string,
    action: ReconcileAction,
  ) => Promise<DashboardSnapshot>;
  resolveReconcileIssuesBatch: (
    items: ReconcileBatchItem[],
  ) => Promise<ReconcileBatchReport>;
  moveAddons: (addonIds: string[], targetDirType: AddonDirType) => Promise<AddonBatchReport>;
  toggleAddons: (
    addonIds: string[],
    enabled: boolean,
    allowWorkshopDir?: boolean,
  ) => Promise<AddonBatchReport>;
  previewRename: (rules: RenameRuleConfig, addonIds?: string[]) => Promise<RenamePreviewItem[]>;
  applyRenames: (renames: [string, string][]) => Promise<AddonBatchReport>;
  deleteAddons: (
    addonIds: string[],
    deleteDiskFile: boolean,
    purgeKnownRecord: boolean,
  ) => Promise<AddonBatchReport>;
  safeDeleteObsoleteDependency: (
    workshopId: string,
    deleteDiskFile: boolean,
    purgeKnownRecord: boolean,
  ) => Promise<AddonBatchReport>;
  rebuildBundles: () => Promise<BundleRecord[]>;
  createManualBundle: (name: string, memberAddonIds: string[]) => Promise<BundleRecord>;
  deleteBundle: (bundleId: string) => Promise<void>;
  setBundleLocked: (bundleId: string, locked: boolean) => Promise<void>;
  trackCollection: (collectionId: string, autoSync: boolean) => Promise<TrackedCollectionRecord>;
  untrackCollection: (collectionId: string) => Promise<void>;
  saveUserGroup: (group: UserGroup) => Promise<UserGroup>;
  deleteUserGroup: (groupId: string) => Promise<void>;
  saveProfile: (profile: LoadoutProfile) => Promise<LoadoutProfile>;
  deleteProfile: (profileId: string) => Promise<void>;
  applyProfile: (profileId: string) => Promise<ProfileApplyReport>;
  searchByMapCode: (query: string) => Promise<AddonSummary[]>;
  openExternalUrl: (url: string) => Promise<void>;
  openInFileManager: (path: string) => Promise<void>;
  getLocalFilterStatus: () => Promise<LocalFilterDeploymentStatus>;
  checkLocalFilterUpdates: () => Promise<LocalFilterDeploymentStatus>;
  deployLocalFilter: (request: DeployLocalFilterRequest) => Promise<LocalFilterDeploymentStatus>;
  startLocalFilterService: () => Promise<LocalFilterDeploymentStatus>;
  stopLocalFilterService: () => Promise<LocalFilterDeploymentStatus>;
  getFilterTokenStatus: () => Promise<FilterTokenStatus>;
  setFilterLlmToken: (token: string | null) => Promise<FilterTokenStatus>;
  evaluateFilterItems: (
    targets: FilterEvaluationTarget[],
    automatic?: boolean,
  ) => Promise<FilterEvaluationBatchResponse>;
  clearFilterCache: () => Promise<ipc.FilterCacheClearResult>;
  upsertFilterExclusion: (request: UpsertFilterExclusionRequest) => Promise<DashboardSnapshot>;
  deleteFilterExclusion: (addonId: AddonId) => Promise<DashboardSnapshot>;
  subscribeCoreEvents: (handler: (ev: CoreEvent) => void) => Promise<() => void>;
}

export function getClient(tauri: boolean): DesktopClient {
  if (tauri) {
    return {
      getDashboardSnapshot: ipc.getDashboardSnapshot,
      scanWorkspace: ipc.scanWorkspace,
      saveConfig: ipc.saveConfig,
      detectGameAddonsPath: ipc.detectGameAddonsPath,
      getSteamCapabilities: ipc.getSteamCapabilities,
      syncWorkshop: ipc.syncWorkshop,
      searchWorkshop: ipc.searchWorkshop,
      fetchWorkshopItem: ipc.fetchWorkshopItem,
      getSubscribedWorkshopIds: ipc.getSubscribedWorkshopIds,
      downloadWorkshopItem: ipc.downloadWorkshopItem,
      updateWorkshopItem: ipc.updateWorkshopItem,
      cancelDownload: ipc.cancelDownload,
      resolveReconcileIssue: ipc.resolveReconcileIssue,
      resolveReconcileIssuesBatch: ipc.resolveReconcileIssuesBatch,
      moveAddons: ipc.moveAddons,
      toggleAddons: ipc.toggleAddons,
      previewRename: ipc.previewRename,
      applyRenames: ipc.applyRenames,
      deleteAddons: ipc.deleteAddons,
      safeDeleteObsoleteDependency: ipc.safeDeleteObsoleteDependency,
      rebuildBundles: ipc.rebuildBundles,
      createManualBundle: ipc.createManualBundle,
      deleteBundle: ipc.deleteBundle,
      setBundleLocked: ipc.setBundleLocked,
      trackCollection: ipc.trackCollection,
      untrackCollection: ipc.untrackCollection,
      saveUserGroup: ipc.saveUserGroup,
      deleteUserGroup: ipc.deleteUserGroup,
      saveProfile: ipc.saveProfile,
      deleteProfile: ipc.deleteProfile,
      applyProfile: ipc.applyProfile,
      searchByMapCode: ipc.searchByMapCode,
      openExternalUrl: ipc.openExternalUrl,
      openInFileManager: ipc.openInFileManager,
      getLocalFilterStatus: ipc.getLocalFilterStatus,
      checkLocalFilterUpdates: ipc.checkLocalFilterUpdates,
      deployLocalFilter: ipc.deployLocalFilter,
      startLocalFilterService: ipc.startLocalFilterService,
      stopLocalFilterService: ipc.stopLocalFilterService,
      getFilterTokenStatus: ipc.getFilterTokenStatus,
      setFilterLlmToken: ipc.setFilterLlmToken,
      evaluateFilterItems: ipc.evaluateFilterItems,
      clearFilterCache: ipc.clearFilterCache,
      upsertFilterExclusion: ipc.upsertFilterExclusion,
      deleteFilterExclusion: ipc.deleteFilterExclusion,
      subscribeCoreEvents: ipc.subscribeCoreEvents,
    };
  }
  return {
    getDashboardSnapshot: () => previewEngine.getDashboardSnapshot(),
    scanWorkspace: () => previewEngine.scanWorkspace(),
    saveConfig: (c) => previewEngine.saveConfig(c),
    detectGameAddonsPath: () => previewEngine.detectGameAddonsPath(),
    getSteamCapabilities: () => previewEngine.getSteamCapabilities(),
    syncWorkshop: () => previewEngine.syncWorkshop(),
    searchWorkshop: (q) => previewEngine.searchWorkshop(q),
    fetchWorkshopItem: (id) => previewEngine.fetchWorkshopItem(id),
    getSubscribedWorkshopIds: () => previewEngine.getSubscribedWorkshopIds(),
    downloadWorkshopItem: (id, titleHint) => previewEngine.downloadWorkshopItem(id, titleHint),
    updateWorkshopItem: (id, titleHint) => previewEngine.updateWorkshopItem(id, titleHint),
    cancelDownload: (id) => previewEngine.cancelDownload(id),
    resolveReconcileIssue: (id, act) => previewEngine.resolveReconcileIssue(id, act),
    resolveReconcileIssuesBatch: (items) => previewEngine.resolveReconcileIssuesBatch(items),
    moveAddons: (ids, dir) => previewEngine.moveAddons(ids, dir),
    toggleAddons: (ids, en, allow) => previewEngine.toggleAddons(ids, en, allow),
    previewRename: (rules, ids) => previewEngine.previewRename(rules, ids),
    applyRenames: (r) => previewEngine.applyRenames(r),
    deleteAddons: (ids, d, p) => previewEngine.deleteAddons(ids, d, p),
    safeDeleteObsoleteDependency: (wid, d, p) =>
      previewEngine.safeDeleteObsoleteDependency(wid, d, p),
    rebuildBundles: () => previewEngine.rebuildBundles(),
    createManualBundle: (n, m) => previewEngine.createManualBundle(n, m),
    deleteBundle: (id) => previewEngine.deleteBundle(id),
    setBundleLocked: (id, l) => previewEngine.setBundleLocked(id, l),
    trackCollection: (id, a) => previewEngine.trackCollection(id, a),
    untrackCollection: (id) => previewEngine.untrackCollection(id),
    saveUserGroup: (g) => previewEngine.saveUserGroup(g),
    deleteUserGroup: (id) => previewEngine.deleteUserGroup(id),
    saveProfile: (p) => previewEngine.saveProfile(p),
    deleteProfile: (id) => previewEngine.deleteProfile(id),
    applyProfile: (id) => previewEngine.applyProfile(id),
    searchByMapCode: (q) => previewEngine.searchByMapCode(q),
    openExternalUrl: (u) => previewEngine.openExternalUrl(u),
    openInFileManager: (p) => previewEngine.openInFileManager(p),
    getLocalFilterStatus: () => previewEngine.getLocalFilterStatus(),
    checkLocalFilterUpdates: () => previewEngine.checkLocalFilterUpdates(),
    deployLocalFilter: (req) => previewEngine.deployLocalFilter(req),
    startLocalFilterService: () => previewEngine.startLocalFilterService(),
    stopLocalFilterService: () => previewEngine.stopLocalFilterService(),
    getFilterTokenStatus: () => previewEngine.getFilterTokenStatus(),
    setFilterLlmToken: (tok) => previewEngine.setFilterLlmToken(tok),
    evaluateFilterItems: (targets, automatic) => previewEngine.evaluateFilterItems(targets, automatic),
    clearFilterCache: () => previewEngine.clearFilterCache(),
    upsertFilterExclusion: (req) => previewEngine.upsertFilterExclusion(req),
    deleteFilterExclusion: (id) => previewEngine.deleteFilterExclusion(id),
    subscribeCoreEvents: async (h) => previewEngine.subscribe(h),
  };
}
