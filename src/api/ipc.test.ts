import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  applyProfile,
  applyRenames,
  cancelDownload,
  checkLocalFilterUpdates,
  clearFilterCache,
  createManualBundle,
  deleteAddons,
  deleteBundle,
  deleteFilterExclusion,
  deleteProfile,
  deleteUserGroup,
  deployLocalFilter,
  detectGameAddonsPath,
  downloadWorkshopItem,
  evaluateFilterItems,
  fetchWorkshopItem,
  getDashboardSnapshot,
  getFilterTokenStatus,
  getLocalFilterStatus,
  getSteamCapabilities,
  getSubscribedWorkshopIds,
  IpcError,
  isTauriRuntime,
  moveAddons,
  openExternalUrl,
  openInFileManager,
  previewRename,
  readCachedImage,
  rebuildBundles,
  resolveReconcileIssue,
  resolveReconcileIssuesBatch,
  safeDeleteObsoleteDependency,
  saveConfig,
  saveProfile,
  saveUserGroup,
  scanWorkspace,
  searchByMapCode,
  searchWorkshop,
  setBundleLocked,
  setFilterLlmToken,
  startLocalFilterService,
  stopLocalFilterService,
  subscribeCoreEvents,
  syncWorkshop,
  toggleAddons,
  trackCollection,
  untrackCollection,
  updateWorkshopItem,
  upsertFilterExclusion,
  type CoreEvent,
} from './ipc';
import { createDefaultFilterConfig } from '../utils/contentFilter';

const invokeMock = vi.fn();
let listenCallback: ((event: { payload: CoreEvent }) => void) | null = null;
const listenMock = vi.fn(async (_event: string, cb: (event: { payload: CoreEvent }) => void) => {
  listenCallback = cb;
  return () => {
    listenCallback = null;
  };
});

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: (...args: unknown[]) =>
    listenMock(...(args as [string, (event: { payload: CoreEvent }) => void])),
}));

describe('v2 IPC client wrapper', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    listenMock.mockClear();
    listenCallback = null;
  });

  it('detects non-tauri browser preview safely', () => {
    expect(isTauriRuntime()).toBe(false);
  });

  it('forwards core v2 commands with exact snake/camel payload mapping', async () => {
    invokeMock.mockResolvedValue({});

    await getDashboardSnapshot();
    expect(invokeMock).toHaveBeenCalledWith('get_dashboard_snapshot');

    await scanWorkspace();
    expect(invokeMock).toHaveBeenCalledWith('scan_workspace');

    await detectGameAddonsPath();
    expect(invokeMock).toHaveBeenCalledWith('detect_game_addons_path');

    await getSteamCapabilities();
    expect(invokeMock).toHaveBeenCalledWith('get_steam_capabilities');

    await syncWorkshop();
    expect(invokeMock).toHaveBeenCalledWith('sync_workshop');

    await resolveReconcileIssue('overwritten_888', 'adopt_workshop_update');
    expect(invokeMock).toHaveBeenCalledWith('resolve_reconcile_issue', {
      issueId: 'overwritten_888',
      action: 'adopt_workshop_update',
    });

    await resolveReconcileIssuesBatch([
      { issueId: 'overwritten_888', action: 'adopt_workshop_update' },
    ]);
    expect(invokeMock).toHaveBeenCalledWith('resolve_reconcile_issues_batch', {
      items: [{ issueId: 'overwritten_888', action: 'adopt_workshop_update' }],
    });

    await searchWorkshop({ query: 'yama', tag: 'Campaigns', page: 2, pageSize: 24 });
    expect(invokeMock).toHaveBeenCalledWith('search_workshop', {
      query: { query: 'yama', tag: 'Campaigns', page: 2, pageSize: 24 },
    });

    await fetchWorkshopItem('2814104912');
    expect(invokeMock).toHaveBeenCalledWith('fetch_workshop_item', {
      workshopId: '2814104912',
    });

    await getSubscribedWorkshopIds();
    expect(invokeMock).toHaveBeenCalledWith('get_subscribed_workshop_ids');

    await downloadWorkshopItem('2814104912');
    expect(invokeMock).toHaveBeenCalledWith('download_workshop_item', {
      workshopId: '2814104912',
    });

    await downloadWorkshopItem('2814104912', 'Chernobyl: Chapter One');
    expect(invokeMock).toHaveBeenLastCalledWith('download_workshop_item', {
      workshopId: '2814104912',
      titleHint: 'Chernobyl: Chapter One',
    });

    await updateWorkshopItem('2814104912', 'Chernobyl: Chapter One');
    expect(invokeMock).toHaveBeenCalledWith('update_workshop_item', {
      workshopId: '2814104912',
      titleHint: 'Chernobyl: Chapter One',
    });

    await cancelDownload('2814104912');
    expect(invokeMock).toHaveBeenCalledWith('cancel_download', {
      workshopId: '2814104912',
    });

    await searchByMapCode('c1m1');
    expect(invokeMock).toHaveBeenCalledWith('search_by_map_code', { query: 'c1m1' });

    await readCachedImage('abc123_cover.jpg');
    expect(invokeMock).toHaveBeenCalledWith('read_cached_image', {
      cacheKey: 'abc123_cover.jpg',
    });

    await openExternalUrl('https://steamcommunity.com/sharedfiles/filedetails/?id=42');
    expect(invokeMock).toHaveBeenCalledWith('open_external_url', {
      url: 'https://steamcommunity.com/sharedfiles/filedetails/?id=42',
    });

    await openInFileManager('/addons/test.vpk');
    expect(invokeMock).toHaveBeenCalledWith('open_in_file_manager', {
      path: '/addons/test.vpk',
    });
  });

  it('forwards organize, safe uninstall, and config commands accurately', async () => {
    invokeMock.mockResolvedValue({});

    await rebuildBundles();
    expect(invokeMock).toHaveBeenCalledWith('rebuild_bundles');

    await createManualBundle('Pack A', ['ws:1', 'ws:2']);
    expect(invokeMock).toHaveBeenCalledWith('create_manual_bundle', {
      name: 'Pack A',
      memberAddonIds: ['ws:1', 'ws:2'],
    });

    await deleteBundle('bundle-1');
    expect(invokeMock).toHaveBeenCalledWith('delete_bundle', {
      bundleId: 'bundle-1',
    });

    await setBundleLocked('bundle-1', true);
    expect(invokeMock).toHaveBeenCalledWith('set_bundle_locked', {
      bundleId: 'bundle-1',
      locked: true,
    });

    await trackCollection('3499001245', true);
    expect(invokeMock).toHaveBeenCalledWith('track_collection', {
      collectionId: '3499001245',
      autoSync: true,
    });

    await untrackCollection('3499001245');
    expect(invokeMock).toHaveBeenCalledWith('untrack_collection', {
      collectionId: '3499001245',
    });

    const group = { id: 'g1', name: 'Coop', tags: ['maps'], addonIds: ['ws:1'] };
    await saveUserGroup(group);
    expect(invokeMock).toHaveBeenCalledWith('save_user_group', { group });

    await deleteUserGroup('g1');
    expect(invokeMock).toHaveBeenCalledWith('delete_user_group', { groupId: 'g1' });

    const profile = { id: 'p1', name: 'Friday', description: '', enabledAddonIds: ['ws:1'] };
    await saveProfile(profile);
    expect(invokeMock).toHaveBeenCalledWith('save_profile', { profile });

    await deleteProfile('p1');
    expect(invokeMock).toHaveBeenCalledWith('delete_profile', { profileId: 'p1' });

    await safeDeleteObsoleteDependency('2814105100', true, false);
    expect(invokeMock).toHaveBeenCalledWith('safe_delete_obsolete_dependency', {
      workshopId: '2814105100',
      deleteDiskFile: true,
      purgeKnownRecord: false,
    });

    const rules = {
      enableWorkshopIdPrefix: true,
      enableGroupPrefix: true,
      cleanSpecialChars: false,
      invalidCharReplace: 'underscore' as const,
      maxFilenameLength: 0,
      enableTrim: true,
      enableRemoveDoubleSpaces: true,
    };
    await previewRename(rules, ['ws:1']);
    expect(invokeMock).toHaveBeenCalledWith('preview_rename', { rules, addonIds: ['ws:1'] });

    const config = {
      loadingDir: '/addons',
      workshopDir: '/addons/workshop',
      downloadConcurrency: 2,
      maxDownloadRetries: 3,
      enableDummyBypass: true,
      autoAdoptNewWorkshopDownloads: false,
      gameUpdateDummyPolicy: 'ask' as const,
      disableSteamworksSdk: false,
      forceSteamworksSdkDownload: false,
      httpsOnly: true,
      sourceOrder: ['catalog-server'],
      renameRules: rules,
      filter: createDefaultFilterConfig(),
    };
    await saveConfig(config);
    expect(invokeMock).toHaveBeenCalledWith('save_config', { config });
  });

  it('forwards all 11 content filter, deployment, token, evaluation, and exclusion IPC commands', async () => {
    invokeMock.mockResolvedValue({});

    await getLocalFilterStatus();
    expect(invokeMock).toHaveBeenCalledWith('get_local_filter_status');

    await checkLocalFilterUpdates();
    expect(invokeMock).toHaveBeenCalledWith('check_local_filter_updates');

    const deployRequest = {
      repoUrl: 'https://github.com/FallenCrystal/Left4Filters.git',
      trackLatest: false,
      targetCommit: 'a94f18e',
      startAfterDeploy: true,
    };
    await deployLocalFilter(deployRequest);
    expect(invokeMock).toHaveBeenCalledWith('deploy_local_filter', {
      request: deployRequest,
    });

    await startLocalFilterService();
    expect(invokeMock).toHaveBeenCalledWith('start_local_filter_service');

    await stopLocalFilterService();
    expect(invokeMock).toHaveBeenCalledWith('stop_local_filter_service');

    await getFilterTokenStatus();
    expect(invokeMock).toHaveBeenCalledWith('get_filter_token_status');

    await setFilterLlmToken('sk-test-secret');
    expect(invokeMock).toHaveBeenCalledWith('set_filter_llm_token', {
      token: 'sk-test-secret',
    });

    await setFilterLlmToken(null);
    expect(invokeMock).toHaveBeenCalledWith('set_filter_llm_token', {
      token: null,
    });

    const targets = [
      {
        targetKey: 'addon:ws:2814104912',
        addonId: 'ws:2814104912',
        workshopId: '2814104912',
        title: 'Sample Addon',
        tags: ['Campaign'],
        imageSources: ['https://example.com/cover.jpg'],
        revisionHint: 'rev-1',
        bypassCache: true,
      },
      {
        targetKey: 'ws:999999999',
        workshopId: '999999999',
        title: 'Broken Endpoint Target',
        tags: [],
        imageSources: [],
      },
    ];
    const batchResponse = {
      policySignature: 'pol_0123456789abcdef0123456789abcdef',
      items: [
        {
          targetKey: 'addon:ws:2814104912',
          status: 'completed' as const,
          complete: false,
          verdict: {
            targetKey: 'addon:ws:2814104912',
            revisionHint: 'rev-1',
            flagged: false,
            decisionSource: 'model_safe' as const,
            evaluatedScore: 0.02,
            probabilities: {
              prob: 0.02,
              goreProb: 0.01,
              nsfwProb: 0.01,
              scoringRevision: 'cat_r1',
            },
            textSensitive: false,
            titleSensitive: false,
            descriptionSensitive: false,
            imageSensitive: false,
            evaluatedImageIndices: [],
            sensitiveImageIndices: [],
            complete: false,
            evaluatedAt: '2026-11-25T00:00:00Z',
          },
        },
        {
          targetKey: 'ws:999999999',
          status: 'failed' as const,
          error: 'Inference upstream timeout',
        },
      ],
    };
    invokeMock.mockResolvedValueOnce(batchResponse);
    expect(await evaluateFilterItems(targets)).toEqual(batchResponse);
    expect(invokeMock).toHaveBeenCalledWith('evaluate_filter_items', { targets });

    await clearFilterCache();
    expect(invokeMock).toHaveBeenCalledWith('clear_filter_cache');

    const exclusionReq = {
      addonId: 'ws:2814104912',
      displayLevel: 'lazy_load' as const,
      bindFingerprint: true,
    };
    await upsertFilterExclusion(exclusionReq);
    expect(invokeMock).toHaveBeenCalledWith('upsert_filter_exclusion', {
      request: exclusionReq,
    });

    await deleteFilterExclusion('ws:2814104912');
    expect(invokeMock).toHaveBeenCalledWith('delete_filter_exclusion', {
      addonId: 'ws:2814104912',
    });
  });

  it('normalizes serialized CoreEvent payloads including nullable optional fields', async () => {
    const received: CoreEvent[] = [];
    const unlisten = await subscribeCoreEvents((ev) => received.push(ev));

    expect(listenMock).toHaveBeenCalledWith('l4a-core-event', expect.any(Function));
    listenCallback?.({
      payload: {
        type: 'download_progress',
        taskId: 't-1',
        workshopId: '2814104912',
        title: null,
        downloaded: 1024,
        total: null,
        percent: 42,
        speedBps: null,
      },
    });
    listenCallback?.({
      payload: {
        type: 'download_progress',
        taskId: 't-2',
        workshopId: '3011942811',
        title: 'Yama Campaign Remastered',
        downloaded: 2048,
        total: 4096,
        percent: 50,
        speedBps: 1048576,
      },
    });
    listenCallback?.({
      payload: {
        type: 'filter_verdicts_updated',
        policySignature: null,
        verdicts: [
          {
            targetKey: 'ws:3410887766',
            revisionHint: '1763900000:Crimson:imgs=1:https://example.com/0.jpg',
            flagged: true,
            decisionSource: 'model_direct_flag',
            evaluatedScore: 0.88,
            probabilities: { prob: 0.88, goreProb: 0.87, nsfwProb: 0.01 },
            textSensitive: true,
            titleSensitive: true,
            descriptionSensitive: false,
            imageSensitive: true,
            evaluatedImageIndices: [0],
            sensitiveImageIndices: [0],
            complete: true,
            evaluatedAt: '2026-11-25T00:00:00Z',
          },
        ],
      },
    });
    listenCallback?.({
      payload: {
        type: 'filter_verdicts_updated',
        policySignature: 'pol_abcdef0123456789abcdef0123456789',
        verdicts: [],
      },
    });
    expect(received).toHaveLength(4);
    expect(received[0]).toEqual({
      type: 'download_progress',
      taskId: 't-1',
      workshopId: '2814104912',
      title: undefined,
      downloaded: 1024,
      total: undefined,
      percent: 42,
      speedBps: undefined,
    });
    expect(received[1]).toEqual({
      type: 'download_progress',
      taskId: 't-2',
      workshopId: '3011942811',
      title: 'Yama Campaign Remastered',
      downloaded: 2048,
      total: 4096,
      percent: 50,
      speedBps: 1048576,
    });
    expect(received[2]).toMatchObject({
      type: 'filter_verdicts_updated',
      policySignature: undefined,
      verdicts: [{ targetKey: 'ws:3410887766', flagged: true, complete: true }],
    });
    expect(received[3]).toEqual({
      type: 'filter_verdicts_updated',
      policySignature: 'pol_abcdef0123456789abcdef0123456789',
      verdicts: [],
    });
    unlisten();
  });

  it('preserves results from a partially completed batch', async () => {
    const report = {
      items: [
        { addonId: 'first', status: 'applied' },
        {
          addonId: 'second',
          status: 'failed',
          error: { code: 'conflict', message: '目标文件已存在' },
        },
        { addonId: 'third', status: 'not_attempted' },
      ],
    };
    invokeMock.mockResolvedValue(report);
    const addonIds = ['first', 'second', 'third'];

    expect(await moveAddons(addonIds, 'loading')).toEqual(report);
    expect(invokeMock).toHaveBeenLastCalledWith('move_addons', {
      addonIds,
      targetDirType: 'loading',
    });

    await toggleAddons(addonIds, false);
    expect(invokeMock).toHaveBeenLastCalledWith('toggle_addons', {
      addonIds,
      enabled: false,
      allowWorkshopDir: false,
    });

    await applyRenames([['first', 'New name.vpk']]);
    expect(invokeMock).toHaveBeenLastCalledWith('apply_renames', {
      renames: [['first', 'New name.vpk']],
    });

    await deleteAddons(addonIds, true, false);
    expect(invokeMock).toHaveBeenLastCalledWith('delete_addons', {
      addonIds,
      deleteDiskFile: true,
      purgeKnownRecord: false,
    });

    await applyProfile('profile');
    expect(invokeMock).toHaveBeenLastCalledWith('apply_profile', { profileId: 'profile' });
  });

  it.each([
    [{ code: 'conflict', message: '对账问题已变化' }, 'conflict', '对账问题已变化'],
    ['invalid invoke arguments', 'ipc', 'invalid invoke arguments'],
  ])('exposes an IPC error code and message', async (rejection, code, message) => {
    invokeMock.mockRejectedValue(rejection);
    const result = scanWorkspace();
    await expect(result).rejects.toBeInstanceOf(IpcError);
    await expect(result).rejects.toMatchObject({ code, message });
  });
});
