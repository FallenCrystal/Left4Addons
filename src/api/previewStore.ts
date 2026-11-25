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
  FilterEvaluationTarget,
  FilterItemVerdict,
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
} from './ipc';
import { IpcError } from './ipc';
import {
  deletePreviewFilterExclusion,
  PreviewFilterRuntime,
  reconcilePreviewExclusions,
  upsertPreviewFilterExclusion,
} from './preview/filter';
import { createInitialSnapshot } from './preview/fixtures';
import {
  applyPreviewProfile,
  applyPreviewRenames,
  buildPreviewRenames,
  createPreviewManualBundle,
  deletePreviewAddons,
  movePreviewAddons,
  resolvePreviewReconcileIssue,
  resolvePreviewReconcileIssuesBatch,
  safeDeletePreviewObsoleteDependency,
  togglePreviewAddons,
  trackPreviewCollection,
} from './preview/operations';
import type { PreviewDownloadToken } from './preview/workshop';
import {
  fetchPreviewWorkshopItem,
  getPreviewSubscribedWorkshopIds,
  runPreviewTransfer,
  searchPreviewWorkshop,
  syncPreviewWorkshop,
} from './preview/workshop';

class PreviewEngine {
  private state: DashboardSnapshot = createInitialSnapshot();
  private listeners = new Set<(event: CoreEvent) => void>();
  private activeDownloads = new Map<string, PreviewDownloadToken>();
  private filterRuntime = new PreviewFilterRuntime();

  reset(): void {
    this.state = createInitialSnapshot();
    this.activeDownloads.clear();
    this.filterRuntime.reset();
  }

  private cloneSnapshot(): DashboardSnapshot {
    reconcilePreviewExclusions(this.state);
    this.state.cachedFilterVerdicts = this.filterRuntime.getCachedVerdictsForSnapshot(this.state);
    return structuredClone(this.state);
  }

  private emit = (event: CoreEvent): void => {
    for (const listener of this.listeners) {
      listener(event);
    }
  };

  subscribe(handler: (event: CoreEvent) => void): () => void {
    this.listeners.add(handler);
    return () => {
      this.listeners.delete(handler);
    };
  }

  async getDashboardSnapshot(): Promise<DashboardSnapshot> {
    return this.cloneSnapshot();
  }

  async scanWorkspace(): Promise<DashboardSnapshot> {
    this.emit({ type: 'snapshot_updated', externalChange: false });
    return this.cloneSnapshot();
  }

  async saveConfig(config: AppConfig): Promise<AppConfig> {
    this.state.config = structuredClone(config);
    return structuredClone(this.state.config);
  }

  async detectGameAddonsPath(): Promise<string | null> {
    return '/SteamLibrary/steamapps/common/Left 4 Dead 2/left4dead2/addons';
  }

  async getSteamCapabilities(): Promise<SteamCapabilities> {
    const disabled = this.state.config.disableSteamworksSdk;
    return {
      bridgeAvailable: !disabled,
      bridgeInitialized: !disabled,
      bridgeVersion: disabled ? null : '2.0.0-preview',
      currentUserSteamId: disabled ? null : '76561198000000001',
      lastError: disabled ? 'Disabled in config' : null,
    };
  }

  async syncWorkshop(): Promise<SyncReport> {
    return syncPreviewWorkshop(this.state, this.emit);
  }

  async searchWorkshop(query: SearchQuery): Promise<SearchResponse> {
    return searchPreviewWorkshop(query);
  }

  async fetchWorkshopItem(workshopId: string): Promise<WorkshopItemRecord> {
    return fetchPreviewWorkshopItem(workshopId);
  }

  async getSubscribedWorkshopIds(): Promise<string[]> {
    return getPreviewSubscribedWorkshopIds(this.state);
  }

  async downloadWorkshopItem(workshopId: string, titleHint?: string): Promise<AddonSummary> {
    return runPreviewTransfer(
      this.state,
      this.activeDownloads,
      this.emit,
      workshopId,
      'install',
      titleHint,
    );
  }

  async updateWorkshopItem(workshopId: string, titleHint?: string): Promise<AddonSummary> {
    return runPreviewTransfer(
      this.state,
      this.activeDownloads,
      this.emit,
      workshopId,
      'update',
      titleHint,
    );
  }

  async cancelDownload(workshopId: string): Promise<boolean> {
    const active = this.activeDownloads.get(workshopId);
    if (!active) return false;
    active.cancelled = true;
    return true;
  }

  async resolveReconcileIssue(
    issueId: string,
    action: ReconcileAction,
  ): Promise<DashboardSnapshot> {
    return resolvePreviewReconcileIssue(this.state, this.emit, issueId, action);
  }

  async resolveReconcileIssuesBatch(items: ReconcileBatchItem[]): Promise<ReconcileBatchReport> {
    return resolvePreviewReconcileIssuesBatch(this.state, this.emit, items);
  }

  async moveAddons(
    addonIds: string[],
    targetDirType: AddonDirType,
  ): Promise<AddonBatchReport> {
    return movePreviewAddons(this.state, this.emit, addonIds, targetDirType);
  }

  async toggleAddons(
    addonIds: string[],
    enabled: boolean,
    allowWorkshopDir = false,
  ): Promise<AddonBatchReport> {
    return togglePreviewAddons(this.state, this.emit, addonIds, enabled, allowWorkshopDir);
  }

  async previewRename(
    rules: RenameRuleConfig,
    addonIds: string[] = [],
  ): Promise<RenamePreviewItem[]> {
    return buildPreviewRenames(this.state, rules, addonIds);
  }

  async applyRenames(
    renames: [addonId: string, fileName: string][],
  ): Promise<AddonBatchReport> {
    return applyPreviewRenames(this.state, this.emit, renames);
  }

  async deleteAddons(
    addonIds: string[],
    deleteDiskFile: boolean,
    purgeKnownRecord: boolean,
  ): Promise<AddonBatchReport> {
    return deletePreviewAddons(this.state, this.emit, addonIds, deleteDiskFile, purgeKnownRecord);
  }

  async safeDeleteObsoleteDependency(
    workshopId: string,
    deleteDiskFile: boolean,
    purgeKnownRecord: boolean,
  ): Promise<AddonBatchReport> {
    return safeDeletePreviewObsoleteDependency(
      this.state,
      this.emit,
      workshopId,
      deleteDiskFile,
      purgeKnownRecord,
    );
  }

  async rebuildBundles(): Promise<BundleRecord[]> {
    this.emit({ type: 'snapshot_updated', externalChange: false });
    return structuredClone(this.state.bundles);
  }

  async createManualBundle(name: string, memberAddonIds: string[]): Promise<BundleRecord> {
    return createPreviewManualBundle(this.state, name, memberAddonIds);
  }

  async deleteBundle(bundleId: string): Promise<void> {
    this.state.bundles = this.state.bundles.filter((b) => b.id !== bundleId);
    this.emit({ type: 'snapshot_updated', externalChange: false });
  }

  async setBundleLocked(bundleId: string, locked: boolean): Promise<void> {
    const b = this.state.bundles.find((item) => item.id === bundleId);
    if (b) b.locked = locked;
  }

  async trackCollection(
    collectionId: string,
    autoSync: boolean,
  ): Promise<TrackedCollectionRecord> {
    return trackPreviewCollection(this.state, collectionId, autoSync);
  }

  async untrackCollection(collectionId: string): Promise<void> {
    this.state.collections = this.state.collections.filter(
      (c) => c.collectionId !== collectionId,
    );
  }

  async saveUserGroup(group: UserGroup): Promise<UserGroup> {
    const idx = this.state.userGroups.findIndex((g) => g.id === group.id);
    if (idx >= 0) this.state.userGroups[idx] = structuredClone(group);
    else this.state.userGroups.push(structuredClone(group));
    return structuredClone(group);
  }

  async deleteUserGroup(groupId: string): Promise<void> {
    this.state.userGroups = this.state.userGroups.filter((g) => g.id !== groupId);
  }

  async saveProfile(profile: LoadoutProfile): Promise<LoadoutProfile> {
    const idx = this.state.profiles.findIndex((p) => p.id === profile.id);
    if (idx >= 0) this.state.profiles[idx] = structuredClone(profile);
    else this.state.profiles.push(structuredClone(profile));
    return structuredClone(profile);
  }

  async deleteProfile(profileId: string): Promise<void> {
    this.state.profiles = this.state.profiles.filter((p) => p.id !== profileId);
  }

  async applyProfile(profileId: string): Promise<ProfileApplyReport> {
    return applyPreviewProfile(this.state, this.emit, profileId);
  }

  async searchByMapCode(query: string): Promise<AddonSummary[]> {
    const needle = query.trim().toLowerCase();
    if (!needle) return [];
    return structuredClone(
      this.state.addons.filter((a) =>
        a.maps.some(
          (m) => m.code.toLowerCase().includes(needle) || m.name.toLowerCase().includes(needle),
        ),
      ),
    );
  }

  async readCachedImage(_cacheKey: string): Promise<number[]> {
    throw new IpcError('not_found', 'Preview mode has no local disk image cache');
  }

  async openExternalUrl(url: string): Promise<void> {
    if (typeof window !== 'undefined') {
      window.open(url, '_blank', 'noopener,noreferrer');
    }
  }

  async openInFileManager(_path: string): Promise<void> {
    // Preview mode has no local file manager.
  }

  async getLocalFilterStatus(): Promise<LocalFilterDeploymentStatus> {
    return this.filterRuntime.getStatus();
  }

  async checkLocalFilterUpdates(): Promise<LocalFilterDeploymentStatus> {
    return this.filterRuntime.checkUpdates(this.emit);
  }

  async deployLocalFilter(
    request: DeployLocalFilterRequest,
  ): Promise<LocalFilterDeploymentStatus> {
    const status = this.filterRuntime.deploy(request, this.emit);
    let changed = false;
    const trimmedRepo = request.repoUrl?.trim();
    if (trimmedRepo && this.state.config.filter.repoUrl !== trimmedRepo) {
      this.state.config.filter.repoUrl = trimmedRepo;
      changed = true;
    }
    if (this.state.config.filter.trackLatest !== request.trackLatest) {
      this.state.config.filter.trackLatest = request.trackLatest;
      changed = true;
    }
    const nextPinned = request.trackLatest ? null : (request.targetCommit?.trim() || null);
    if ((this.state.config.filter.pinnedCommit ?? null) !== nextPinned) {
      this.state.config.filter.pinnedCommit = nextPinned;
      changed = true;
    }
    if (changed) {
      this.emit({ type: 'snapshot_updated', externalChange: false });
    }
    return status;
  }

  async startLocalFilterService(): Promise<LocalFilterDeploymentStatus> {
    return this.filterRuntime.startService(this.emit);
  }

  async stopLocalFilterService(): Promise<LocalFilterDeploymentStatus> {
    return this.filterRuntime.stopService(this.emit);
  }

  async getFilterTokenStatus(): Promise<FilterTokenStatus> {
    return this.filterRuntime.getTokenStatus(this.state.config.filter);
  }

  async setFilterLlmToken(token: string | null): Promise<FilterTokenStatus> {
    return this.filterRuntime.setLlmToken(token, this.state.config.filter);
  }

  async evaluateFilterItems(
    targets: FilterEvaluationTarget[],
  ): Promise<FilterItemVerdict[]> {
    return this.filterRuntime.evaluateItems(targets, this.state.config.filter, this.emit, {
      catalogServerUrl: this.state.config.catalogServerUrl,
      httpsOnly: this.state.config.httpsOnly,
    });
  }

  async clearFilterCache(): Promise<number> {
    return this.filterRuntime.clearCache();
  }

  async upsertFilterExclusion(
    request: UpsertFilterExclusionRequest,
  ): Promise<DashboardSnapshot> {
    return upsertPreviewFilterExclusion(this.state, this.emit, request);
  }

  async deleteFilterExclusion(addonId: AddonId): Promise<DashboardSnapshot> {
    return deletePreviewFilterExclusion(this.state, this.emit, addonId);
  }
}

export const previewEngine = new PreviewEngine();
