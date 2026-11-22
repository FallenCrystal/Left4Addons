import type {
  AddonBatchReport,
  AddonDirType,
  AddonOperationResult,
  AddonSummary,
  AppConfig,
  BundleRecord,
  CoreEvent,
  DashboardSnapshot,
  LoadoutProfile,
  ProfileApplyReport,
  ReconcileAction,
  ReconcileBatchItem,
  ReconcileBatchReport,
  ReconcileItemResult,
  RenamePreviewItem,
  RenameRuleConfig,
  SearchQuery,
  SearchResponse,
  SteamCapabilities,
  SyncReport,
  TaskRecord,
  TrackedCollectionRecord,
  UserGroup,
  WorkshopItemRecord,
} from './ipc';
import { IpcError } from './ipc';
import { previewFilenameByRules, sanitizeVpkFilename } from '../utils/format';

const PREVIEW_CATALOG: WorkshopItemRecord[] = [
  {
    workshopId: '2814104912',
    fileType: 'item',
    status: 'active',
    title: 'Chernobyl: Chapter One',
    description: 'Custom five-chapter survival campaign set in Pripyat.',
    creatorSteamId: '76561198011223344',
    creatorName: 'PripyatTeam',
    fileSize: 486_539_264,
    timeCreated: 1690000000,
    timeUpdated: 1762000000,
    tags: ['Campaigns', 'Maps'],
    dependencies: [{ workshopId: '2814105100', title: 'Chernobyl Assets Pack', required: true }],
    collectionChildren: [],
    subscriptions: 342_180,
    favorites: 28_400,
    source: 'catalog_server',
    fetchedAt: '2026-11-20T00:00:00Z',
  },
  {
    workshopId: '2814105100',
    fileType: 'item',
    status: 'active',
    title: 'Chernobyl Assets Pack',
    description: 'Shared textures and models required by Chernobyl campaigns.',
    creatorSteamId: '76561198011223344',
    creatorName: 'PripyatTeam',
    fileSize: 218_103_808,
    timeCreated: 1690000000,
    timeUpdated: 1761500000,
    tags: ['Campaigns', 'Maps'],
    dependencies: [],
    collectionChildren: [],
    subscriptions: 310_420,
    favorites: 14_200,
    source: 'catalog_server',
    fetchedAt: '2026-11-20T00:00:00Z',
  },
  {
    workshopId: '3011942811',
    fileType: 'item',
    status: 'active',
    title: 'Yama Campaign Remastered',
    description: 'Japanese mountain temple campaign featuring custom weather and finale.',
    creatorSteamId: '76561198022334455',
    creatorName: 'Kuroda',
    fileSize: 312_475_648,
    timeCreated: 1701000000,
    timeUpdated: 1763100000,
    tags: ['Campaigns', 'Maps'],
    dependencies: [],
    collectionChildren: [],
    subscriptions: 518_920,
    favorites: 49_110,
    source: 'catalog_server',
    fetchedAt: '2026-11-20T00:00:00Z',
  },
  {
    workshopId: '3120481120',
    fileType: 'item',
    status: 'active',
    title: 'Tactical M4A1 SOPMOD',
    description: 'High-fidelity weapon replacement with custom reload animations.',
    creatorSteamId: '76561198033445566',
    creatorName: 'Maverick',
    fileSize: 44_040_192,
    timeCreated: 1712000000,
    timeUpdated: 1762800000,
    tags: ['Weapons', 'Sounds'],
    dependencies: [],
    collectionChildren: [],
    subscriptions: 194_200,
    favorites: 16_300,
    source: 'catalog_server',
    fetchedAt: '2026-11-20T00:00:00Z',
  },
  {
    workshopId: '3209812304',
    fileType: 'item',
    status: 'active',
    title: 'Minimalist HUD v3',
    description: 'Clean vector HUD with readable health bars and teammate indicators.',
    creatorSteamId: '76561198044556677',
    creatorName: 'UICraft',
    fileSize: 12_582_912,
    timeCreated: 1720000000,
    timeUpdated: 1763400000,
    tags: ['UI'],
    dependencies: [],
    collectionChildren: [],
    subscriptions: 267_500,
    favorites: 31_900,
    source: 'catalog_server',
    fetchedAt: '2026-11-20T00:00:00Z',
  },
  {
    workshopId: '3350912881',
    fileType: 'item',
    status: 'active',
    title: 'Improved Bot AI Director',
    description: 'VScript enhancement for survivor bots pathfinding and medkit usage.',
    creatorSteamId: '76561198055667788',
    creatorName: 'ScriptLab',
    fileSize: 4_194_304,
    timeCreated: 1730000000,
    timeUpdated: 1763500000,
    tags: ['Scripts'],
    dependencies: [],
    collectionChildren: [],
    subscriptions: 412_000,
    favorites: 52_000,
    source: 'catalog_server',
    fetchedAt: '2026-11-20T00:00:00Z',
  },
  {
    workshopId: '3499001245',
    fileType: 'collection',
    status: 'active',
    title: 'Friday Coop Essential Pack',
    description: 'Curated co-op campaigns and QoL tweaks.',
    creatorSteamId: '76561198066778899',
    creatorName: 'NightSquad',
    timeCreated: 1738000000,
    timeUpdated: 1763550000,
    tags: ['Campaigns', 'UI'],
    dependencies: [],
    collectionChildren: ['2814104912', '2814105100', '3011942811', '3350912881'],
    subscriptions: 42_100,
    favorites: 5_400,
    source: 'catalog_server',
    fetchedAt: '2026-11-20T00:00:00Z',
  },
];

function createInitialSnapshot(): DashboardSnapshot {
  return {
    config: {
      loadingDir: '/SteamLibrary/steamapps/common/Left 4 Dead 2/left4dead2/addons',
      workshopDir: '/SteamLibrary/steamapps/common/Left 4 Dead 2/left4dead2/addons/workshop',
      downloadConcurrency: 2,
      maxDownloadRetries: 3,
      enableDummyBypass: true,
      autoAdoptNewWorkshopDownloads: false,
      gameUpdateDummyPolicy: 'ask',
      disableSteamworksSdk: false,
      forceSteamworksSdkDownload: false,
      httpsOnly: true,
      catalogServerUrl: null,
      catalogContributorToken: '',
      sourceOrder: ['catalog-server', 'steamworks-sdk', 'steam-web-api'],
      renameRules: {
        enableWorkshopIdPrefix: true,
        enableGroupPrefix: true,
        cleanSpecialChars: false,
        invalidCharReplace: 'underscore',
        maxFilenameLength: 0,
        enableTrim: true,
        enableRemoveDoubleSpaces: true,
      },
    },
    addons: [
      {
        id: 'ws:2814104912',
        workshopId: '2814104912',
        title: 'Chernobyl: Chapter One',
        author: 'PripyatTeam',
        version: '2.4',
        installState: 'installed',
        primaryDir: 'loading',
        enabled: true,
        currentFileName: '[2814104912] Chernobyl_Chapter_One.vpk',
        currentPath:
          '/SteamLibrary/steamapps/common/Left 4 Dead 2/left4dead2/addons/[2814104912] Chernobyl_Chapter_One.vpk',
        contentHash: '9f8e7d6c5b4a3210fedcba9876543210',
        fileSize: 486_539_264,
        filesCount: 842,
        categories: ['Campaign', 'Map'],
        maps: [
          { code: 'l4d2_chernobyl_01', name: 'Outer Exclusion Zone' },
          { code: 'l4d2_chernobyl_02', name: 'Rail Yard' },
          { code: 'l4d2_chernobyl_03', name: 'Pripyat Suburbs' },
          { code: 'l4d2_chernobyl_04', name: 'Reactor Approach' },
          { code: 'l4d2_chernobyl_05', name: 'Sarcophagus Finale' },
        ],
        hasDummyBypass: true,
        remoteStatus: 'active',
        localTimeUpdated: 1760000000,
        remoteTimeUpdated: 1762000000,
        updateAvailable: true,
        addonInfo: {
          addontitle: 'Chernobyl: Chapter One',
          addonauthor: 'PripyatTeam',
          addonversion: '2.4',
          addondescription: 'Five-chapter custom campaign set across the Chernobyl Exclusion Zone.',
          addonurl0: 'https://steamcommunity.com/sharedfiles/filedetails/?id=2814104912',
          addoncontent_campaign: '1',
        },
      },
      {
        id: 'ws:2814105100',
        workshopId: '2814105100',
        title: 'Chernobyl Assets Pack',
        author: 'PripyatTeam',
        version: '2.4',
        installState: 'installed',
        primaryDir: 'loading',
        enabled: true,
        currentFileName: '[2814105100] Chernobyl_Assets_Pack.vpk',
        currentPath:
          '/SteamLibrary/steamapps/common/Left 4 Dead 2/left4dead2/addons/[2814105100] Chernobyl_Assets_Pack.vpk',
        contentHash: '1a2b3c4d5e6f7890abcdef1234567890',
        fileSize: 218_103_808,
        filesCount: 1420,
        categories: ['Campaign'],
        maps: [],
        hasDummyBypass: true,
        remoteStatus: 'active',
        localTimeUpdated: 1761500000,
        remoteTimeUpdated: 1761500000,
        updateAvailable: false,
        addonInfo: {
          addontitle: 'Chernobyl Assets Pack',
          addonauthor: 'PripyatTeam',
          addonversion: '2.4',
        },
      },
      {
        id: 'ws:3011942811',
        workshopId: '3011942811',
        title: 'Yama Campaign Remastered',
        author: 'Kuroda',
        version: '3.1',
        installState: 'installed',
        primaryDir: 'loading',
        enabled: true,
        currentFileName: '[3011942811] Yama_Campaign_Remastered.vpk',
        currentPath:
          '/SteamLibrary/steamapps/common/Left 4 Dead 2/left4dead2/addons/[3011942811] Yama_Campaign_Remastered.vpk',
        contentHash: '4d5e6f7a8b9c0123456789abcdef0123',
        fileSize: 312_475_648,
        filesCount: 615,
        categories: ['Campaign', 'Map'],
        maps: [
          { code: 'l4d_yama_1', name: 'Kiyomizu Temple' },
          { code: 'l4d_yama_2', name: 'Forest Trail' },
          { code: 'l4d_yama_3', name: 'Village Bridge' },
          { code: 'l4d_yama_4', name: 'Cable Car Summit' },
        ],
        hasDummyBypass: true,
        remoteStatus: 'active',
        localTimeUpdated: 1763100000,
        remoteTimeUpdated: 1763100000,
        updateAvailable: false,
        addonInfo: {
          addontitle: 'Yama Campaign Remastered',
          addonauthor: 'Kuroda',
          addonversion: '3.1',
        },
      },
      {
        id: 'ws:3120481120',
        workshopId: '3120481120',
        title: 'Tactical M4A1 SOPMOD',
        author: 'Maverick',
        version: '1.2',
        installState: 'installed',
        primaryDir: 'loading',
        enabled: false,
        currentFileName: '[3120481120] Tactical_M4A1_SOPMOD.vpk',
        currentPath:
          '/SteamLibrary/steamapps/common/Left 4 Dead 2/left4dead2/addons/[3120481120] Tactical_M4A1_SOPMOD.vpk.disabled',
        contentHash: 'aa11bb22cc33dd44ee55ff6677889900',
        fileSize: 44_040_192,
        filesCount: 96,
        categories: ['Weapon', 'Audio'],
        maps: [],
        hasDummyBypass: true,
        remoteStatus: 'active',
        localTimeUpdated: 1762800000,
        remoteTimeUpdated: 1762800000,
        updateAvailable: false,
        addonInfo: {
          addontitle: 'Tactical M4A1 SOPMOD',
          addonauthor: 'Maverick',
          addonversion: '1.2',
        },
      },
      {
        id: 'ws:3209812304',
        workshopId: '3209812304',
        title: 'Minimalist HUD v3',
        author: 'UICraft',
        version: '3.0',
        installState: 'installed',
        primaryDir: 'workshop',
        enabled: true,
        currentFileName: '3209812304.vpk',
        currentPath:
          '/SteamLibrary/steamapps/common/Left 4 Dead 2/left4dead2/addons/workshop/3209812304.vpk',
        contentHash: 'bb22cc33dd44ee55ff66001122334455',
        fileSize: 12_582_912,
        filesCount: 48,
        categories: ['UI'],
        maps: [],
        hasDummyBypass: false,
        remoteStatus: 'active',
        localTimeUpdated: 1763400000,
        remoteTimeUpdated: 1763400000,
        updateAvailable: false,
        addonInfo: {
          addontitle: 'Minimalist HUD v3',
          addonauthor: 'UICraft',
          addonversion: '3.0',
        },
      },
      {
        id: 'ws:2991004512',
        workshopId: '2991004512',
        title: 'Legacy Rain Overhaul',
        author: 'StormFX',
        version: '1.0',
        installState: 'installed',
        primaryDir: 'loading',
        enabled: true,
        currentFileName: '[2991004512] Legacy_Rain_Overhaul.vpk',
        currentPath:
          '/SteamLibrary/steamapps/common/Left 4 Dead 2/left4dead2/addons/[2991004512] Legacy_Rain_Overhaul.vpk',
        contentHash: 'cc33dd44ee55ff6677889900aabbccdd',
        fileSize: 28_311_552,
        filesCount: 64,
        categories: ['Skin', 'Audio'],
        maps: [],
        hasDummyBypass: false,
        remoteStatus: 'removed',
        localTimeUpdated: 1740000000,
        remoteTimeUpdated: undefined,
        updateAvailable: false,
        addonInfo: {
          addontitle: 'Legacy Rain Overhaul',
          addonauthor: 'StormFX',
        },
      },
      {
        id: 'hash:7f6e5d4c3b2a1098',
        title: 'Local Custom Soundboard',
        author: 'Local',
        version: '1.0',
        installState: 'installed',
        primaryDir: 'loading',
        enabled: true,
        currentFileName: 'local_custom_soundboard.vpk',
        currentPath:
          '/SteamLibrary/steamapps/common/Left 4 Dead 2/left4dead2/addons/local_custom_soundboard.vpk',
        contentHash: '7f6e5d4c3b2a10987f6e5d4c3b2a1098',
        fileSize: 9_437_184,
        filesCount: 22,
        categories: ['Audio'],
        maps: [],
        hasDummyBypass: false,
        remoteStatus: 'unknown',
        updateAvailable: false,
        addonInfo: {
          addontitle: 'Local Custom Soundboard',
        },
      },
      {
        id: 'ws:3350912881',
        workshopId: '3350912881',
        title: 'Improved Bot AI Director',
        author: 'ScriptLab',
        version: '4.2',
        installState: 'uninstalled_known',
        enabled: false,
        fileSize: 4_194_304,
        filesCount: 15,
        categories: ['Script'],
        maps: [],
        hasDummyBypass: false,
        remoteStatus: 'active',
        remoteTimeUpdated: 1763500000,
        updateAvailable: false,
        addonInfo: {
          addontitle: 'Improved Bot AI Director',
          addonauthor: 'ScriptLab',
        },
      },
    ],
    bundles: [
      {
        id: 'bundle_dep_2814104912',
        name: 'Chernobyl: Chapter One',
        kind: 'dependency_graph',
        locked: false,
        memberAddonIds: ['ws:2814104912', 'ws:2814105100'],
        maps: [
          { code: 'l4d2_chernobyl_01', name: 'Outer Exclusion Zone' },
          { code: 'l4d2_chernobyl_02', name: 'Rail Yard' },
          { code: 'l4d2_chernobyl_03', name: 'Pripyat Suburbs' },
          { code: 'l4d2_chernobyl_04', name: 'Reactor Approach' },
          { code: 'l4d2_chernobyl_05', name: 'Sarcophagus Finale' },
        ],
      },
    ],
    collections: [
      {
        collectionId: '3499001245',
        title: 'Friday Coop Essential Pack',
        description: 'Curated co-op campaigns and QoL tweaks.',
        autoSync: true,
        memberWorkshopIds: ['2814104912', '2814105100', '3011942811', '3350912881'],
        removedUpstreamIds: ['2991004512'],
        lastSyncedAt: '2026-11-20T00:10:00Z',
      },
    ],
    userGroups: [
      {
        id: 'grp-campaigns',
        name: 'Campaigns',
        tags: ['coop', 'maps'],
        color: '#2D6A4F',
        addonIds: ['ws:2814104912', 'ws:2814105100', 'ws:3011942811'],
      },
      {
        id: 'grp-qol',
        name: 'QoL & Visuals',
        tags: ['hud', 'fx'],
        color: '#006399',
        addonIds: ['ws:3209812304', 'ws:2991004512'],
      },
    ],
    profiles: [
      {
        id: 'prof-coop',
        name: 'Co-op Campaign',
        description: 'Campaigns + QoL',
        enabledAddonIds: ['ws:2814104912', 'ws:2814105100', 'ws:3011942811', 'ws:2991004512'],
      },
      {
        id: 'prof-vanilla',
        name: 'Vanilla+',
        description: 'Minimal audio & visuals',
        enabledAddonIds: ['ws:2991004512', 'hash:7f6e5d4c3b2a1098'],
      },
    ],
    openIssues: [
      {
        id: 'overwritten_2814104912@preview1',
        kind: 'dummy_overwritten_by_game_update',
        workshopId: '2814104912',
        addonId: 'ws:2814104912',
        loadingPath:
          '/SteamLibrary/steamapps/common/Left 4 Dead 2/left4dead2/addons/[2814104912] Chernobyl_Chapter_One.vpk',
        workshopPath:
          '/SteamLibrary/steamapps/common/Left 4 Dead 2/left4dead2/addons/workshop/2814104912.vpk',
        summary: 'Workshop 2814104912 dummy overwritten by game update',
        recommendedAction: 'adopt_workshop_update',
        availableActions: [
          'adopt_workshop_update',
          'keep_local_and_regenerate_dummy',
          'keep_workshop_delete_loading',
          'ignore',
        ],
        resolved: false,
        detectedAt: '2026-11-20T00:15:00Z',
      },
      {
        id: 'new_ws_3209812304@preview2',
        kind: 'new_workshop_auto_download',
        workshopId: '3209812304',
        addonId: 'ws:3209812304',
        workshopPath:
          '/SteamLibrary/steamapps/common/Left 4 Dead 2/left4dead2/addons/workshop/3209812304.vpk',
        summary: 'New workshop addon 3209812304 detected',
        recommendedAction: 'import_to_loading_and_create_dummy',
        availableActions: ['import_to_loading_and_create_dummy', 'leave_in_workshop', 'ignore'],
        resolved: false,
        detectedAt: '2026-11-20T00:16:00Z',
      },
    ],
    tasks: [
      {
        id: 'task-sync-1',
        kind: 'sync',
        status: 'completed',
        targetId: 'workshop-sync',
        progressPercent: 100,
        message: '6 items',
        createdAt: '2026-11-20T00:10:00Z',
        updatedAt: '2026-11-20T00:10:04Z',
      },
    ],
  };
}

class PreviewEngine {
  private state: DashboardSnapshot = createInitialSnapshot();
  private listeners = new Set<(event: CoreEvent) => void>();
  private activeDownloads = new Map<string, { cancelled: boolean }>();

  reset(): void {
    this.state = createInitialSnapshot();
    this.activeDownloads.clear();
  }

  private cloneSnapshot(): DashboardSnapshot {
    return structuredClone(this.state);
  }

  private emit(event: CoreEvent): void {
    for (const listener of this.listeners) {
      listener(event);
    }
  }

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
    const report: SyncReport = {
      checkedItems: this.state.addons.filter((a) => a.workshopId).length,
      updatesAvailable: this.state.addons
        .filter((a) => a.updateAvailable && a.workshopId)
        .map((a) => ({
          addonId: a.id,
          workshopId: a.workshopId!,
          title: a.title,
          localTimeUpdated: a.localTimeUpdated,
          remoteTimeUpdated: a.remoteTimeUpdated ?? Math.floor(Date.now() / 1000),
          source: 'catalog_server',
        })),
      removedUpstream: this.state.addons
        .filter((a) => a.remoteStatus === 'removed' && a.workshopId)
        .map((a) => ({
          addonId: a.id,
          workshopId: a.workshopId!,
          title: a.title,
          status: a.remoteStatus,
          localFilePreserved: a.installState === 'installed',
          detectedAt: new Date().toISOString(),
        })),
      dependencyDiffs: [
        {
          parentWorkshopId: '2814104912',
          parentTitle: 'Chernobyl: Chapter One',
          addedDependencies: ['2814105100'],
          removedDependencies: [],
        },
      ],
      obsoleteDependencies: [],
      collectionDiffs: this.state.collections
        .filter((c) => c.autoSync)
        .map((c) => ({
          collectionId: c.collectionId,
          title: c.title,
          addedItems: ['3350912881'],
          removedItems: c.removedUpstreamIds,
        })),
      warnings: [],
    };
    this.state.tasks.unshift({
      id: `sync-${Date.now()}`,
      kind: 'sync',
      status: 'completed',
      targetId: 'workshop-sync',
      progressPercent: 100,
      message: `${report.checkedItems} items`,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
    });
    this.emit({ type: 'snapshot_updated', externalChange: false });
    return report;
  }

  async searchWorkshop(query: SearchQuery): Promise<SearchResponse> {
    const q = (query.query ?? '').trim().toLowerCase();
    const tag = query.tag?.trim().toLowerCase();
    let items = PREVIEW_CATALOG.filter((item) => {
      if (q) {
        const hit =
          item.title.toLowerCase().includes(q) ||
          item.workshopId.includes(q) ||
          (item.creatorName ?? '').toLowerCase().includes(q) ||
          (item.description ?? '').toLowerCase().includes(q);
        if (!hit) return false;
      }
      if (tag && !item.tags.some((t) => t.toLowerCase() === tag)) {
        return false;
      }
      if (query.status && item.status !== query.status) {
        return false;
      }
      return true;
    });

    if (query.sort === 'updated_asc') {
      items = [...items].sort((a, b) => (a.timeUpdated ?? 0) - (b.timeUpdated ?? 0));
    } else if (query.sort === 'title') {
      items = [...items].sort((a, b) => a.title.localeCompare(b.title));
    } else if (query.sort === 'subscriptions') {
      items = [...items].sort((a, b) => (b.subscriptions ?? 0) - (a.subscriptions ?? 0));
    } else if (query.sort === 'newest') {
      items = [...items].sort((a, b) => (b.timeCreated ?? 0) - (a.timeCreated ?? 0));
    } else {
      items = [...items].sort((a, b) => (b.timeUpdated ?? 0) - (a.timeUpdated ?? 0));
    }

    const page = query.page ?? 1;
    const pageSize = query.pageSize ?? 20;
    const start = (page - 1) * pageSize;

    return {
      items: structuredClone(items.slice(start, start + pageSize)),
      total: items.length,
      page,
      pageSize,
      source: 'catalog_server',
    };
  }

  async fetchWorkshopItem(workshopId: string): Promise<WorkshopItemRecord> {
    const item = PREVIEW_CATALOG.find((i) => i.workshopId === workshopId);
    if (!item) {
      throw new IpcError('not_found', `Workshop item not found: ${workshopId}`);
    }
    return structuredClone(item);
  }

  async getSubscribedWorkshopIds(): Promise<string[]> {
    if (this.state.config.disableSteamworksSdk) {
      throw new IpcError('unsupported', 'Steamworks SDK disabled');
    }
    return ['2814104912', '2814105100', '3350912881'];
  }

  private async runSimulatedTransfer(
    workshopId: string,
    mode: 'install' | 'update',
  ): Promise<AddonSummary> {
    if (!/^\d+$/.test(workshopId)) {
      throw new IpcError('invalid_input', `Invalid Workshop ID: ${workshopId}`);
    }
    if (this.activeDownloads.has(workshopId)) {
      throw new IpcError('conflict', `Already downloading ${workshopId}`);
    }

    const existingIdx = this.state.addons.findIndex((a) => a.workshopId === workshopId);
    const existing = existingIdx >= 0 ? this.state.addons[existingIdx] : undefined;

    if (mode === 'install' && existing?.installState === 'installed') {
      throw new IpcError('conflict', `Addon already installed: ${workshopId}`);
    }
    if (mode === 'update' && (!existing || existing.installState !== 'installed')) {
      throw new IpcError('not_found', `Addon not installed: ${workshopId}`);
    }

    const token = { cancelled: false };
    this.activeDownloads.set(workshopId, token);

    const catalogItem = PREVIEW_CATALOG.find((i) => i.workshopId === workshopId);
    const taskId = `dl-${workshopId}-${Date.now()}`;
    const total = catalogItem?.fileSize ?? 32_000_000;

    const taskRecord: TaskRecord = {
      id: taskId,
      kind: 'download',
      status: 'running',
      targetId: workshopId,
      progressPercent: 0,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
    };
    this.state.tasks.unshift(taskRecord);
    this.emit({ type: 'snapshot_updated', externalChange: false });

    try {
      for (const percent of [25, 60, 90, 100]) {
        await new Promise((r) => setTimeout(r, 100));
        if (token.cancelled) {
          taskRecord.status = 'cancelled';
          taskRecord.updatedAt = new Date().toISOString();
          this.emit({ type: 'snapshot_updated', externalChange: false });
          throw new IpcError('cancelled', `Cancelled ${workshopId}`);
        }
        taskRecord.progressPercent = percent;
        this.emit({
          type: 'download_progress',
          taskId,
          workshopId,
          downloaded: Math.round((total * percent) / 100),
          total,
          percent,
        });
      }

      const nowTs = catalogItem?.timeUpdated ?? Math.floor(Date.now() / 1000);
      if (mode === 'update' && existing) {
        existing.localTimeUpdated = nowTs;
        existing.remoteTimeUpdated = nowTs;
        existing.updateAvailable = false;
        existing.fileSize = total;
        taskRecord.status = 'completed';
        taskRecord.progressPercent = 100;
        taskRecord.updatedAt = new Date().toISOString();
        this.emit({ type: 'snapshot_updated', externalChange: false });
        return structuredClone(existing);
      }

      const id = `ws:${workshopId}`;
      const title = catalogItem?.title ?? `Workshop Addon #${workshopId}`;
      const fileName = previewFilenameByRules(this.state.config.renameRules, {
        workshopId,
        title,
      });
      const nextAddon: AddonSummary = {
        id,
        workshopId,
        title,
        author: catalogItem?.creatorName ?? 'Workshop',
        version: '1.0',
        installState: 'installed',
        primaryDir: 'loading',
        enabled: true,
        currentFileName: fileName,
        currentPath: `${this.state.config.loadingDir}/${fileName}`,
        contentHash: `preview_${workshopId}`,
        fileSize: total,
        filesCount: 42,
        categories:
          catalogItem?.tags.map((t) =>
            t === 'Campaigns'
              ? 'Campaign'
              : t === 'Maps'
                ? 'Map'
                : t === 'Weapons'
                  ? 'Weapon'
                  : t === 'Scripts'
                    ? 'Script'
                    : t === 'Sounds'
                      ? 'Audio'
                      : t,
          ) ?? ['Other'],
        maps: [],
        hasDummyBypass: this.state.config.enableDummyBypass,
        remoteStatus: 'active',
        localTimeUpdated: nowTs,
        remoteTimeUpdated: nowTs,
        updateAvailable: false,
        addonInfo: { addontitle: title },
      };

      if (existingIdx >= 0) {
        this.state.addons[existingIdx] = {
          ...this.state.addons[existingIdx],
          ...nextAddon,
          maps: this.state.addons[existingIdx].maps,
        };
      } else {
        this.state.addons.unshift(nextAddon);
      }
      taskRecord.status = 'completed';
      taskRecord.progressPercent = 100;
      taskRecord.updatedAt = new Date().toISOString();
      this.emit({ type: 'snapshot_updated', externalChange: false });
      return structuredClone(existingIdx >= 0 ? this.state.addons[existingIdx] : nextAddon);
    } finally {
      this.activeDownloads.delete(workshopId);
    }
  }

  async downloadWorkshopItem(workshopId: string): Promise<AddonSummary> {
    return this.runSimulatedTransfer(workshopId, 'install');
  }

  async updateWorkshopItem(workshopId: string): Promise<AddonSummary> {
    return this.runSimulatedTransfer(workshopId, 'update');
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
    const idx = this.state.openIssues.findIndex((i) => i.id === issueId);
    if (idx === -1) {
      throw new IpcError('conflict', 'Issue already resolved');
    }
    const issue = this.state.openIssues[idx];
    if (issue.addonId) {
      const addon = this.state.addons.find((a) => a.id === issue.addonId);
      if (addon) {
        if (action === 'adopt_workshop_update') {
          addon.updateAvailable = false;
          addon.localTimeUpdated = addon.remoteTimeUpdated ?? addon.localTimeUpdated;
          addon.hasDummyBypass = this.state.config.enableDummyBypass;
        } else if (action === 'import_to_loading_and_create_dummy') {
          addon.primaryDir = 'loading';
          addon.hasDummyBypass = this.state.config.enableDummyBypass;
        } else if (action === 'keep_workshop_delete_loading') {
          addon.primaryDir = 'workshop';
          addon.hasDummyBypass = false;
        } else if (action === 'enable_workshop_file') {
          addon.enabled = true;
        }
      }
    }
    this.state.openIssues.splice(idx, 1);
    this.emit({ type: 'snapshot_updated', externalChange: false });
    return this.cloneSnapshot();
  }

  async resolveReconcileIssuesBatch(items: ReconcileBatchItem[]): Promise<ReconcileBatchReport> {
    const results: ReconcileItemResult[] = [];
    for (const item of items) {
      try {
        await this.resolveReconcileIssue(item.issueId, item.action);
        results.push({ issueId: item.issueId, action: item.action, ok: true });
      } catch (err) {
        results.push({
          issueId: item.issueId,
          action: item.action,
          ok: false,
          error: {
            code: err instanceof IpcError ? (err.code as ReconcileItemResult['error'] extends { code: infer C } ? C : 'conflict') : 'conflict',
            message: err instanceof Error ? err.message : String(err),
          },
        });
      }
    }
    return { items: results, snapshot: this.cloneSnapshot() };
  }

  async moveAddons(
    addonIds: string[],
    targetDirType: AddonDirType,
  ): Promise<AddonBatchReport> {
    const items: AddonOperationResult[] = [];
    for (const addonId of addonIds) {
      const addon = this.state.addons.find((a) => a.id === addonId);
      if (!addon || addon.installState !== 'installed') {
        items.push({
          addonId,
          status: 'failed',
          error: { code: 'not_found', message: `Addon not installed: ${addonId}` },
        });
        continue;
      }
      if (addon.primaryDir === targetDirType) {
        items.push({ addonId, status: 'unchanged' });
        continue;
      }
      addon.primaryDir = targetDirType;
      addon.hasDummyBypass =
        targetDirType === 'loading' &&
        Boolean(addon.workshopId) &&
        this.state.config.enableDummyBypass;
      if (targetDirType === 'workshop') {
        addon.enabled = true;
      }
      items.push({ addonId, status: 'applied' });
    }
    this.emit({ type: 'snapshot_updated', externalChange: false });
    return { items };
  }

  async toggleAddons(
    addonIds: string[],
    enabled: boolean,
    allowWorkshopDir = false,
  ): Promise<AddonBatchReport> {
    const items: AddonOperationResult[] = [];
    let failed = false;
    for (const addonId of addonIds) {
      if (failed) {
        items.push({ addonId, status: 'not_attempted' });
        continue;
      }
      const addon = this.state.addons.find((a) => a.id === addonId);
      if (!addon) {
        failed = true;
        items.push({
          addonId,
          status: 'failed',
          error: { code: 'not_found', message: `Not found: ${addonId}` },
        });
        continue;
      }
      if (addon.enabled === enabled) {
        items.push({ addonId, status: 'unchanged' });
        continue;
      }
      if (addon.primaryDir === 'workshop' && !allowWorkshopDir) {
        failed = true;
        items.push({
          addonId,
          status: 'failed',
          error: { code: 'conflict', message: `Addon ${addonId} is in workshop/` },
        });
        continue;
      }
      addon.enabled = enabled;
      items.push({ addonId, status: 'applied' });
    }
    this.emit({ type: 'snapshot_updated', externalChange: false });
    return { items };
  }

  async previewRename(
    rules: RenameRuleConfig,
    addonIds: string[] = [],
  ): Promise<RenamePreviewItem[]> {
    const idSet = new Set(addonIds);
    const groupByAddon = new Map<string, string>();
    for (const group of this.state.userGroups) {
      for (const aid of group.addonIds) {
        if (!groupByAddon.has(aid)) groupByAddon.set(aid, group.name);
      }
    }
    return this.state.addons
      .filter(
        (a) =>
          a.installState === 'installed' &&
          a.currentFileName &&
          (idSet.size === 0 || idSet.has(a.id)),
      )
      .map((a) => ({
        addonId: a.id,
        currentFileName: a.currentFileName!,
        targetFileName: previewFilenameByRules(rules, {
          workshopId: a.workshopId,
          groupName: groupByAddon.get(a.id),
          title: a.title,
        }),
        conflict: a.primaryDir === 'workshop' ? 'workshop/' : undefined,
      }));
  }

  async applyRenames(
    renames: [addonId: string, fileName: string][],
  ): Promise<AddonBatchReport> {
    const items: AddonOperationResult[] = [];
    for (const [addonId, rawName] of renames) {
      const addon = this.state.addons.find((a) => a.id === addonId);
      if (!addon) {
        items.push({
          addonId,
          status: 'failed',
          error: { code: 'not_found', message: addonId },
        });
        continue;
      }
      if (addon.primaryDir === 'workshop') {
        items.push({
          addonId,
          status: 'failed',
          error: { code: 'conflict', message: 'workshop/' },
        });
        continue;
      }
      const clean = sanitizeVpkFilename(rawName, this.state.config.renameRules);
      if (addon.currentFileName === clean) {
        items.push({ addonId, status: 'unchanged' });
      } else {
        addon.currentFileName = clean;
        addon.currentPath = `${this.state.config.loadingDir}/${clean}${addon.enabled ? '' : '.disabled'}`;
        items.push({ addonId, status: 'applied' });
      }
    }
    this.emit({ type: 'snapshot_updated', externalChange: false });
    return { items };
  }

  async deleteAddons(
    addonIds: string[],
    deleteDiskFile: boolean,
    purgeKnownRecord: boolean,
  ): Promise<AddonBatchReport> {
    const items: AddonOperationResult[] = [];
    for (const addonId of addonIds) {
      const idx = this.state.addons.findIndex((a) => a.id === addonId);
      if (idx === -1) {
        items.push({ addonId, status: 'unchanged' });
        continue;
      }
      const addon = this.state.addons[idx];
      if (purgeKnownRecord && !deleteDiskFile && addon.installState === 'installed') {
        items.push({
          addonId,
          status: 'failed',
          error: { code: 'conflict', message: 'Still installed' },
        });
        continue;
      }
      if (purgeKnownRecord) {
        this.state.addons.splice(idx, 1);
        for (const g of this.state.userGroups) {
          g.addonIds = g.addonIds.filter((id) => id !== addonId);
        }
        for (const b of this.state.bundles) {
          b.memberAddonIds = b.memberAddonIds.filter((id) => id !== addonId);
        }
        for (const p of this.state.profiles) {
          p.enabledAddonIds = p.enabledAddonIds.filter((id) => id !== addonId);
        }
        items.push({ addonId, status: 'applied' });
      } else if (deleteDiskFile) {
        addon.installState = 'uninstalled_known';
        addon.primaryDir = undefined;
        addon.currentPath = undefined;
        addon.enabled = false;
        addon.hasDummyBypass = false;
        items.push({ addonId, status: 'applied' });
      } else {
        items.push({ addonId, status: 'unchanged' });
      }
    }
    this.emit({ type: 'snapshot_updated', externalChange: false });
    return { items };
  }

  async safeDeleteObsoleteDependency(
    workshopId: string,
    deleteDiskFile: boolean,
    purgeKnownRecord: boolean,
  ): Promise<AddonBatchReport> {
    const addon = this.state.addons.find((a) => a.workshopId === workshopId);
    if (!addon) {
      throw new IpcError('not_found', `Dependency not found: ${workshopId}`);
    }
    return this.deleteAddons([addon.id], deleteDiskFile, purgeKnownRecord);
  }

  async rebuildBundles(): Promise<BundleRecord[]> {
    this.emit({ type: 'snapshot_updated', externalChange: false });
    return structuredClone(this.state.bundles);
  }

  async createManualBundle(name: string, memberAddonIds: string[]): Promise<BundleRecord> {
    const maps = this.state.addons
      .filter((a) => memberAddonIds.includes(a.id))
      .flatMap((a) => a.maps);
    const bundle: BundleRecord = {
      id: `bundle-manual-${Date.now()}`,
      name,
      kind: 'manual',
      locked: true,
      memberAddonIds,
      maps,
    };
    this.state.bundles.push(bundle);
    return structuredClone(bundle);
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
    const catalog = PREVIEW_CATALOG.find((i) => i.workshopId === collectionId);
    const existing = this.state.collections.find((c) => c.collectionId === collectionId);
    const memberWorkshopIds = catalog?.collectionChildren.length
      ? catalog.collectionChildren
      : ['2814104912', '3011942811'];
    const record: TrackedCollectionRecord = {
      collectionId,
      title: catalog?.title ?? existing?.title ?? `Collection #${collectionId}`,
      description: catalog?.description ?? existing?.description,
      autoSync,
      memberWorkshopIds,
      removedUpstreamIds: (existing?.removedUpstreamIds ?? []).filter(
        (id) => !memberWorkshopIds.includes(id),
      ),
      lastSyncedAt: new Date().toISOString(),
    };
    const idx = this.state.collections.findIndex((c) => c.collectionId === collectionId);
    if (idx >= 0) this.state.collections[idx] = record;
    else this.state.collections.push(record);
    return structuredClone(record);
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
    const profile = this.state.profiles.find((p) => p.id === profileId);
    if (!profile) {
      throw new IpcError('not_found', `Profile not found: ${profileId}`);
    }
    const desired = new Set(profile.enabledAddonIds);
    const items: AddonOperationResult[] = [];
    const skippedWorkshopOnly: string[] = [];
    const missingAddons: string[] = [];
    let enabledCount = 0;
    let disabledCount = 0;

    for (const id of desired) {
      const a = this.state.addons.find((item) => item.id === id);
      if (!a || a.installState !== 'installed') {
        missingAddons.push(id);
      }
    }

    for (const addon of this.state.addons) {
      if (addon.installState !== 'installed') continue;
      if (addon.primaryDir !== 'loading') {
        if (desired.has(addon.id) !== addon.enabled) {
          skippedWorkshopOnly.push(addon.id);
        }
        continue;
      }
      const shouldEnable = desired.has(addon.id);
      if (addon.enabled !== shouldEnable) {
        addon.enabled = shouldEnable;
        items.push({ addonId: addon.id, status: 'applied' });
        if (shouldEnable) enabledCount += 1;
        else disabledCount += 1;
      }
    }

    this.emit({ type: 'snapshot_updated', externalChange: false });
    return {
      profileId,
      enabledCount,
      disabledCount,
      skippedWorkshopOnly,
      missingAddons,
      batch: { items },
    };
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
}

export const previewEngine = new PreviewEngine();
