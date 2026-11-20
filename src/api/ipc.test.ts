import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  applyProfile,
  applyRenames,
  cancelDownload,
  createManualBundle,
  deleteAddons,
  deleteBundle,
  deleteProfile,
  deleteUserGroup,
  detectGameAddonsPath,
  downloadWorkshopItem,
  fetchWorkshopItem,
  getDashboardSnapshot,
  getSteamCapabilities,
  getSubscribedWorkshopIds,
  IpcError,
  isTauriRuntime,
  moveAddons,
  openExternalUrl,
  previewRename,
  readCachedImage,
  rebuildBundles,
  resolveReconcileIssue,
  safeDeleteObsoleteDependency,
  saveConfig,
  saveProfile,
  saveUserGroup,
  scanWorkspace,
  searchByMapCode,
  searchWorkshop,
  setBundleLocked,
  subscribeCoreEvents,
  syncWorkshop,
  toggleAddons,
  trackCollection,
  untrackCollection,
  updateWorkshopItem,
  type CoreEvent,
} from './ipc';

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

    await updateWorkshopItem('2814104912');
    expect(invokeMock).toHaveBeenCalledWith('update_workshop_item', {
      workshopId: '2814104912',
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
    };
    await saveConfig(config);
    expect(invokeMock).toHaveBeenCalledWith('save_config', { config });
  });

  it('normalizes serialized CoreEvent payloads including nullable total bytes', async () => {
    const received: CoreEvent[] = [];
    const unlisten = await subscribeCoreEvents((ev) => received.push(ev));

    expect(listenMock).toHaveBeenCalledWith('l4a-core-event', expect.any(Function));
    listenCallback?.({
      payload: {
        type: 'download_progress',
        taskId: 't-1',
        workshopId: '2814104912',
        downloaded: 1024,
        total: null,
        percent: 42,
      },
    });
    expect(received).toEqual([
      {
        type: 'download_progress',
        taskId: 't-1',
        workshopId: '2814104912',
        downloaded: 1024,
        total: undefined,
        percent: 42,
      },
    ]);
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
