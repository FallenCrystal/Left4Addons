import { describe, test, expect, vi, beforeEach } from 'vitest';
import { renderHook, act, waitFor } from '@testing-library/react';
import { useAddonManager } from './useAddonManager';
import { Addon, DatabasePayload, Group, MasterCollection, Settings } from '../types/addon';

// Mock Tauri invoke function
const mockInvoke = vi.fn();
const mockListen = vi.fn();
const eventListeners = new Map<string, (event: { payload: any }) => void>();

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => mockInvoke(cmd, args),
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: (eventName: string, callback: (event: { payload: any }) => void) => {
    mockListen(eventName, callback);
    eventListeners.set(eventName, callback);
    return Promise.resolve(() => {
      eventListeners.delete(eventName);
    });
  },
}));

describe('useAddonManager', () => {
  const mockAddons: Record<string, Addon> = {
    'addon1.vpk': {
      id: 'addon1.vpk', vpkName: 'addon1.vpk',
      dirType: 'loading',
      isEnabled: true,
      fileSize: 1024 * 1024, // 1 MB
      filesCount: 10,
      addonInfo: { addontitle: 'Addon One', addonAuthor: 'Author One' },
    },
    'addon2.vpk': {
      id: 'addon2.vpk', vpkName: 'addon2.vpk',
      dirType: 'workshop',
      isEnabled: false,
      fileSize: 2 * 1024 * 1024, // 2 MB
      filesCount: 5,
      workshopId: '12345',
      steamDetails: { title: 'Addon Two', creator_name: 'Author Two' },
    },
  };

  const mockGroups: Group[] = [
    { id: 'group-1', name: 'My Group', addons: ['addon1.vpk'] },
  ];

  const mockSettings: Settings = {
    workshopDir: '/path/to/workshop',
    loadingDir: '/path/to/loading',
    downloadConcurrency: 2,
    enableDummyBypass: false,
    suppressSdkUnavailableWarning: false,
    disableSteamworksSdk: false,
    forceSteamworksSdkDownload: false,
    workshopSourceSettings: {
      preset: 'conservative',
      allowSteamworksSdk: true,
      allowSteamWebApi: true,
      allowSteamCommunityHtml: true,
      allowSdkHtmlHybrid: false,
      sdkHtmlScope: 'search',
      sourceOrder: ['steamworks-sdk', 'steam-web-api', 'steamcommunity-html'],
      cacheRetention: 'keep',
    },
  };

  beforeEach(() => {
    vi.clearAllMocks();
    sessionStorage.clear();
    eventListeners.clear();
    
    // Default mock response for get_addons
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({
          addons: mockAddons,
          groups: mockGroups,
          settings: mockSettings,
        });
      }
      if (cmd === 'get_workshop_cache') {
        return Promise.resolve({
          '12345': { lastPageFetchedAt: new Date().toISOString() },
        });
      }
      if (cmd === 'get_background_tasks') {
        return Promise.resolve([]);
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          bridgeLoaded: true,
          bridgeInitialized: true,
          provider: 'steam-sdk',
          canQueryItems: true,
          canQueryHome: true,
          canDownload: true,
          canEnumerateInstalled: true,
        });
      }
      return Promise.resolve({});
    });
  });

  test('should initialize with default states and fetch data on mount', async () => {
    const { result } = renderHook(() => useAddonManager());

    // Initially loading is true
    expect(result.current.loading).toBe(true);

    // Wait for the hook to finish loading
    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.addons).toEqual(mockAddons);
    expect(result.current.groups).toEqual(mockGroups);
    expect(result.current.settings).toEqual(mockSettings);
    expect(mockInvoke).toHaveBeenCalledWith('get_addons', undefined);
  });

  test('should handle categories, query and tab filters correctly', async () => {
    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    // Test initial derived values
    expect(result.current.filteredItems).toHaveLength(2);
    expect(result.current.totalAddonsCount).toBe(2);
    expect(result.current.activeCount).toBe(1);
    expect(result.current.disabledCount).toBe(1);
    expect(result.current.totalStorageSize).toBe(3 * 1024 * 1024);

    // Filter by searchQuery
    act(() => {
      result.current.setSearchQuery('One');
    });
    expect(result.current.filteredItems).toHaveLength(1);
    expect(result.current.filteredItems[0].vpkName).toBe('addon1.vpk');

    // Reset query and filter by tab (loading)
    act(() => {
      result.current.setSearchQuery('');
      result.current.setCurrentFilterTab('loading');
    });
    expect(result.current.filteredItems).toHaveLength(1);
    expect(result.current.filteredItems[0].vpkName).toBe('addon1.vpk');

    // Filter by tab (workshop)
    act(() => {
      result.current.setCurrentFilterTab('workshop');
    });
    expect(result.current.filteredItems).toHaveLength(1);
    expect(result.current.filteredItems[0].vpkName).toBe('addon2.vpk');

    // Filter by tab (disabled)
    act(() => {
      result.current.setCurrentFilterTab('disabled');
    });
    expect(result.current.filteredItems).toHaveLength(1);
    expect(result.current.filteredItems[0].vpkName).toBe('addon2.vpk');
  });

  test('should render empty groups in all view without requiring addons', async () => {
    const emptyGroup: Group = { id: 'group-empty', name: 'Empty Group', addons: [] };
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({
          addons: mockAddons,
          groups: [...mockGroups, emptyGroup],
          settings: mockSettings,
        });
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          bridgeLoaded: true,
          bridgeInitialized: true,
          provider: 'steam-sdk',
          canQueryItems: true,
          canQueryHome: true,
          canDownload: true,
          canEnumerateInstalled: true,
        });
      }
      return Promise.resolve({});
    });

    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.renderedItems).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ type: 'group', id: 'group-empty' }),
      ])
    );

    act(() => {
      result.current.setSearchQuery('addon one');
    });

    expect(result.current.renderedItems).not.toEqual(
      expect.arrayContaining([
        expect.objectContaining({ type: 'group', id: 'group-empty' }),
      ])
    );
  });

  test('should render empty groups inside master collections', async () => {
    const emptyGroup: Group = {
      id: 'group-empty',
      name: 'Empty Group',
      addons: [],
      masterCollectionIds: ['mc-1'],
    };
    const masterCollections: MasterCollection[] = [{
      id: 'mc-1',
      name: 'Master Collection',
      groupIds: ['group-empty'],
      isSystem: false,
    }];

    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({
          addons: mockAddons,
          groups: [...mockGroups, emptyGroup],
          masterCollections,
          settings: mockSettings,
        });
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          bridgeLoaded: true,
          bridgeInitialized: true,
          provider: 'steam-sdk',
          canQueryItems: true,
          canQueryHome: true,
          canDownload: true,
          canEnumerateInstalled: true,
        });
      }
      return Promise.resolve({});
    });

    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    act(() => {
      result.current.handleFilterTabChange('master-collection', 'mc-1');
    });

    expect(result.current.filteredItems).toHaveLength(0);
    expect(result.current.renderedItems).toEqual([
      expect.objectContaining({ type: 'group', id: 'group-empty' }),
    ]);
  });

  test('should toggle selection correctly', async () => {
    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.isSelectMode).toBe(false);
    expect(result.current.selectedIds).toEqual([]);

    // Toggle select addon1
    act(() => {
      result.current.handleSelectToggle('addon1.vpk');
    });
    expect(result.current.isSelectMode).toBe(true);
    expect(result.current.selectedIds).toEqual(['addon1.vpk']);

    // Toggle select addon1 again (deselects it)
    act(() => {
      result.current.handleSelectToggle('addon1.vpk');
    });
    expect(result.current.selectedIds).toEqual([]);
  });

  test('should perform batch enable and disable', async () => {
    sessionStorage.setItem('skipWorkshopWarning', 'true');
    mockInvoke.mockImplementation((cmd, args) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({ addons: mockAddons, groups: mockGroups, settings: mockSettings });
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          bridgeLoaded: true,
          bridgeInitialized: true,
          provider: 'steam-sdk',
          canQueryItems: true,
          canQueryHome: true,
          canDownload: true,
          canEnumerateInstalled: true,
        });
      }
      if (cmd === 'move_addons') {
        return Promise.resolve({});
      }
      if (cmd === 'toggle_addons') {
        expect(args).toEqual({ ids: ['addon2.vpk'], enabled: true });
        const updatedAddons = { ...mockAddons };
        updatedAddons['addon1.vpk'].isEnabled = true;
        updatedAddons['addon2.vpk'].isEnabled = true;
        return Promise.resolve({ addons: updatedAddons, groups: mockGroups });
      }
      return Promise.resolve({});
    });

    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    act(() => {
      result.current.handleSelectToggle('addon1.vpk');
      result.current.handleSelectToggle('addon2.vpk');
    });

    await act(async () => {
      await result.current.handleBatchToggle(true);
    });

    expect(result.current.addons['addon2.vpk'].isEnabled).toBe(true);
    expect(result.current.isSelectMode).toBe(false); // cleared selection
  });

  test('should only move selected workshop addons during batch move', async () => {
    mockInvoke.mockImplementation((cmd, args) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({ addons: mockAddons, groups: mockGroups, settings: mockSettings });
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          bridgeLoaded: true,
          bridgeInitialized: true,
          provider: 'steam-sdk',
          canQueryItems: true,
          canQueryHome: true,
          canDownload: true,
          canEnumerateInstalled: true,
        });
      }
      if (cmd === 'move_addons') {
        expect(args).toEqual({ ids: ['addon2.vpk'], targetDirType: 'loading' });
        return Promise.resolve({ addons: mockAddons, groups: mockGroups, settings: mockSettings });
      }
      return Promise.resolve({});
    });

    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    act(() => {
      result.current.handleSelectToggle('addon1.vpk');
      result.current.handleSelectToggle('addon2.vpk');
    });

    await act(async () => {
      await result.current.handleBatchMove();
    });

    await act(async () => {
      if (result.current.confirmModal.onConfirm) {
        await result.current.confirmModal.onConfirm();
      }
    });

    expect(mockInvoke).toHaveBeenCalledWith('move_addons', { ids: ['addon2.vpk'], targetDirType: 'loading' });
  });

  test('should handle group operations: create, delete, and rename', async () => {
    mockInvoke.mockImplementation((cmd, args) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({ addons: mockAddons, groups: mockGroups, settings: mockSettings });
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          bridgeLoaded: true,
          bridgeInitialized: true,
          provider: 'steam-sdk',
          canQueryItems: true,
          canQueryHome: true,
          canDownload: true,
          canEnumerateInstalled: true,
        });
      }
      if (cmd === 'group_action') {
        if (args?.action === 'create') {
          expect(args.name).toBe('New Group');
          expect(args.ids).toEqual(['addon2.vpk']);
          const newGroups = [...mockGroups, { id: 'group-2', name: 'New Group', addons: ['addon2.vpk'] }];
          return Promise.resolve({ addons: mockAddons, groups: newGroups });
        }
        if (args?.action === 'delete') {
          expect(args.groupId).toBe('group-1');
          return Promise.resolve({ addons: mockAddons, groups: [] });
        }
      }
      return Promise.resolve({});
    });

    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    // Create group
    await act(async () => {
      await result.current.handleCreateGroup('New Group', ['addon2.vpk']);
    });
    expect(result.current.groups).toHaveLength(2);
    expect(result.current.groups[1].name).toBe('New Group');

    // Delete group
    act(() => {
      result.current.handleDeleteGroup('group-1');
    });
    // This opens confirmModal
    expect(result.current.confirmModal.open).toBe(true);
    expect(result.current.confirmModal.title).toBe('确认删除分组');
    
    // Simulate confirm
    await act(async () => {
      if (result.current.confirmModal.onConfirm) {
        await result.current.confirmModal.onConfirm();
      }
    });

    expect(result.current.groups).toHaveLength(0);
  });

  test('should navigate to a newly created empty group', async () => {
    const emptyGroup: Group = { id: 'group-empty', name: 'Empty Group', addons: [] };
    mockInvoke.mockImplementation((cmd, args) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({ addons: mockAddons, groups: mockGroups, settings: mockSettings });
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          bridgeLoaded: true,
          bridgeInitialized: true,
          provider: 'steam-sdk',
          canQueryItems: true,
          canQueryHome: true,
          canDownload: true,
          canEnumerateInstalled: true,
        });
      }
      if (cmd === 'group_action' && args?.action === 'create') {
        expect(args.ids).toEqual([]);
        return Promise.resolve({
          addons: mockAddons,
          groups: [...mockGroups, emptyGroup],
          settings: mockSettings,
        });
      }
      return Promise.resolve({});
    });

    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    act(() => {
      result.current.setSearchQuery('will be cleared');
      result.current.setSelectedCategory('Campaign');
      result.current.setCurrentFilterTab('workshop');
    });

    await act(async () => {
      await result.current.handleCreateGroup('Empty Group');
    });

    expect(result.current.groups).toEqual(expect.arrayContaining([emptyGroup]));
    expect(result.current.currentFilterTab).toBe('groups');
    expect(result.current.selectedGroupId).toBe('group-empty');
    expect(result.current.selectedCategory).toBe('All');
    expect(result.current.searchQuery).toBe('');
    expect(result.current.currentGroup).toEqual(emptyGroup);
  });

  test('should set isSubmitting to true during async actions and reset to false afterwards', async () => {
    let resolvePromise: (value: unknown) => void = () => {};
    const tauriPromise = new Promise((resolve) => {
      resolvePromise = resolve;
    });

    mockInvoke.mockImplementation((cmd, args) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({ addons: mockAddons, groups: mockGroups, settings: mockSettings });
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          bridgeLoaded: true,
          bridgeInitialized: true,
          provider: 'steam-sdk',
          canQueryItems: true,
          canQueryHome: true,
          canDownload: true,
          canEnumerateInstalled: true,
        });
      }
      if (cmd === 'get_workshop_cache') {
        return Promise.resolve({
          '12345': { lastPageFetchedAt: new Date().toISOString() },
        });
      }
      if (cmd === 'get_background_tasks') {
        return Promise.resolve([]);
      }
      if (cmd === 'save_settings') {
        expect(args).toEqual({
          payload: {
            loadingDir: '/new/loading/dir',
            downloadConcurrency: 2,
            enableDummyBypass: false,
            suppressSdkUnavailableWarning: false,
            disableSteamworksSdk: false,
            forceSteamworksSdkDownload: false,
            maxDownloadRetries: 3,
            workshopSourceSettings: undefined,
          },
        });
        return tauriPromise;
      }
      return Promise.resolve({});
    });

    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.isSubmitting).toBe(false);

    // Call saveSettings
    let savePromise: Promise<void> | null = null;
    act(() => {
      savePromise = result.current.saveSettings('/new/loading/dir', 2, false, false, false, false, 3);
    });

    // isSubmitting should be true immediately after invoking
    expect(result.current.isSubmitting).toBe(true);

    // Resolve the promise
    await act(async () => {
      resolvePromise({ addons: mockAddons, groups: mockGroups, settings: { ...mockSettings, loadingDir: '/new/loading/dir' } });
      await savePromise;
    });

    // isSubmitting should reset to false
    expect(result.current.isSubmitting).toBe(false);
  });

  test('should apply watcher database updates and show toast for external changes', async () => {
    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    const nextPayload: DatabasePayload = {
      addons: {
        ...mockAddons,
        'addon3.vpk': {
          id: 'addon3.vpk',
          vpkName: 'addon3.vpk',
          dirType: 'loading',
          isEnabled: true,
          fileSize: 123,
          filesCount: 1,
          addonInfo: { addontitle: 'Addon Three' },
        },
      },
      knownUninstalledAddons: {},
      groups: mockGroups,
      masterCollections: [],
      settings: mockSettings,
    };

    await act(async () => {
      eventListeners.get('addons-db-updated')?.({
        payload: { data: nextPayload, external: true },
      });
    });

    expect(result.current.addons['addon3.vpk']).toBeDefined();
    expect(result.current.toasts.some((toast) => toast.message.includes('自动刷新'))).toBe(true);
  });

  test('should apply internal watcher refreshes without success toast', async () => {
    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    const nextPayload: DatabasePayload = {
      addons: {
        'addon2.vpk': mockAddons['addon2.vpk'],
      },
      knownUninstalledAddons: {},
      groups: mockGroups,
      masterCollections: [],
      settings: mockSettings,
    };

    act(() => {
      result.current.handleSelectToggle('addon1.vpk');
    });

    await act(async () => {
      eventListeners.get('addons-db-updated')?.({
        payload: { data: nextPayload, external: false },
      });
    });

    expect(result.current.addons['addon1.vpk']).toBeUndefined();
    expect(result.current.knownUninstalledAddons['addon1.vpk']).toBeUndefined();
    expect(result.current.selectedIds).toEqual([]);
    expect(result.current.toasts.some((toast) => toast.message.includes('自动刷新'))).toBe(false);
  });

  test('should keep detail modal open for unresolved workshop placeholder entries after database updates', async () => {
    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    act(() => {
      result.current.setDetailModal({
        open: true,
        addon: {
          id: '99999',
          vpkName: '99999',
          workshopId: '99999',
          dirType: 'none',
          isEnabled: false,
          fileSize: 0,
          filesCount: 0,
        },
      });
    });

    const nextPayload: DatabasePayload = {
      addons: mockAddons,
      knownUninstalledAddons: {},
      groups: mockGroups,
      masterCollections: [],
      settings: mockSettings,
    };

    act(() => {
      result.current.applyDatabaseUpdate(nextPayload);
    });

    expect(result.current.detailModal.open).toBe(true);
    expect(result.current.detailModal.addon?.workshopId).toBe('99999');
  });

  test('should resolve detail modal placeholder entries by workshop id after database updates', async () => {
    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    act(() => {
      result.current.setDetailModal({
        open: true,
        addon: {
          id: '99999',
          vpkName: '99999',
          workshopId: '99999',
          dirType: 'none',
          isEnabled: false,
          fileSize: 0,
          filesCount: 0,
        },
      });
    });

    const knownEntry: Addon = {
      id: 'workshop_99999.vpk',
      vpkName: 'workshop_99999.vpk',
      workshopId: '99999',
      dirType: 'none',
      isEnabled: false,
      fileSize: 123,
      filesCount: 1,
      workshopDetails: { title: 'Resolved Entry' },
    };

    const nextPayload: DatabasePayload = {
      addons: mockAddons,
      knownUninstalledAddons: {
        [knownEntry.id]: knownEntry,
      },
      groups: mockGroups,
      masterCollections: [],
      settings: mockSettings,
    };

    act(() => {
      result.current.applyDatabaseUpdate(nextPayload);
    });

    expect(result.current.detailModal.open).toBe(true);
    expect(result.current.detailModal.addon?.id).toBe('workshop_99999.vpk');
    expect(result.current.detailModal.addon?.workshopDetails?.title).toBe('Resolved Entry');
  });

  test('should surface watcher errors through toast notifications', async () => {
    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    await act(async () => {
      eventListeners.get('addons-watch-error')?.({
        payload: { message: 'watcher failed' },
      });
    });

    expect(result.current.toasts.some((toast) => toast.message.includes('watcher failed'))).toBe(true);
  });

  test('should track download progress from progress events', async () => {
    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    act(() => {
      eventListeners.get('download-progress')?.({
        payload: {
          workshopId: '12345',
          percent: 50,
          downloaded: 8 * 1024 * 1024,
          total: 16 * 1024 * 1024,
          source: 'web-fallback',
          phase: 'fallback-download',
        },
      });
    });

    expect(result.current.downloadProgress['12345']).toBe(50);
  });

  test('should add a warning task when sdk-download-warning is emitted', async () => {
    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    act(() => {
      eventListeners.get('sdk-download-warning')?.({
        payload: {
          workshopId: '12345',
          title: 'Addon Two',
          reason: 'web-download-failed',
          source: 'steam-sdk',
        },
      });
    });

    await waitFor(() => {
      expect(result.current.backgroundTasks.some((task) => task.id === 'warning_steam_sdk_download_12345')).toBe(true);
    });

    expect(result.current.backgroundTasks.find((task) => task.id === 'warning_steam_sdk_download_12345')).toMatchObject({
      kind: 'warning',
      status: 'failed',
      source: 'steam-sdk-download-warning',
      targetIds: ['12345'],
      title: 'Addon Two (12345)',
    });
  });

  test('should add a frontend warning task when Steamworks SDK is unavailable', async () => {
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({
          addons: mockAddons,
          groups: mockGroups,
          settings: mockSettings,
        });
      }
      if (cmd === 'get_workshop_cache') {
        return Promise.resolve({
          '12345': { lastPageFetchedAt: new Date().toISOString() },
        });
      }
      if (cmd === 'get_background_tasks') {
        return Promise.resolve([]);
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: false,
          bridgeLoaded: false,
          bridgeInitialized: false,
          provider: 'web-fallback',
          lastError: 'Steam bridge DLL not found',
          canQueryItems: false,
          canQueryHome: false,
          canDownload: false,
          canEnumerateInstalled: false,
        });
      }
      return Promise.resolve({});
    });

    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
      expect(result.current.backgroundTasks).toHaveLength(1);
    });

    expect(result.current.backgroundTasks[0]).toMatchObject({
      id: 'steamworks-sdk-unavailable',
      kind: 'warning',
      status: 'failed',
      source: 'frontend-warning',
      title: 'Steamworks SDK 不可用',
    });
    expect(result.current.backgroundTasks[0].error).toContain('Steam bridge DLL not found');
  });

  test('should suppress the frontend SDK warning task when the setting is enabled', async () => {
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({
          addons: mockAddons,
          groups: mockGroups,
          settings: {
            ...mockSettings,
            suppressSdkUnavailableWarning: true,
          },
        });
      }
      if (cmd === 'get_workshop_cache') {
        return Promise.resolve({
          '12345': { lastPageFetchedAt: new Date().toISOString() },
        });
      }
      if (cmd === 'get_background_tasks') {
        return Promise.resolve([]);
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: false,
          bridgeLoaded: false,
          bridgeInitialized: false,
          provider: 'web-fallback',
          lastError: 'Steam bridge DLL not found',
          canQueryItems: false,
          canQueryHome: false,
          canDownload: false,
          canEnumerateInstalled: false,
        });
      }
      return Promise.resolve({});
    });

    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.backgroundTasks).toHaveLength(0);
  });

  test('should suppress the frontend SDK warning task when Steamworks SDK is manually disabled', async () => {
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({
          addons: mockAddons,
          groups: mockGroups,
          settings: {
            ...mockSettings,
            disableSteamworksSdk: true,
          },
        });
      }
      if (cmd === 'get_workshop_cache') {
        return Promise.resolve({
          '12345': { lastPageFetchedAt: new Date().toISOString() },
        });
      }
      if (cmd === 'get_background_tasks') {
        return Promise.resolve([]);
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: false,
          bridgeLoaded: false,
          bridgeInitialized: false,
          provider: 'web-fallback',
          lastError: 'Steam bridge DLL not found',
          canQueryItems: false,
          canQueryHome: false,
          canDownload: false,
          canEnumerateInstalled: false,
        });
      }
      return Promise.resolve({});
    });

    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.backgroundTasks).toHaveLength(0);
  });

  test('should not probe Steamworks capabilities when SDK source is disabled in source settings', async () => {
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({
          addons: mockAddons,
          groups: mockGroups,
          settings: {
            ...mockSettings,
            workshopSourceSettings: {
              preset: 'conservative',
              allowSteamworksSdk: false,
              allowSteamWebApi: true,
              allowSteamCommunityHtml: true,
              allowSdkHtmlHybrid: false,
              sdkHtmlScope: 'search',
              sourceOrder: ['steam-web-api', 'steamcommunity-html', 'steamworks-sdk'],
              cacheRetention: 'keep',
            },
          },
        });
      }
      if (cmd === 'get_workshop_cache') {
        return Promise.resolve({
          '12345': { lastPageFetchedAt: new Date().toISOString() },
        });
      }
      if (cmd === 'get_background_tasks') {
        return Promise.resolve([]);
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          bridgeLoaded: true,
          bridgeInitialized: true,
          provider: 'steam-sdk',
          canQueryItems: true,
          canQueryHome: true,
          canDownload: true,
          canEnumerateInstalled: true,
        });
      }
      return Promise.resolve({});
    });

    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(mockInvoke).not.toHaveBeenCalledWith('get_workshop_capabilities', undefined);
    expect(result.current.backgroundTasks).toHaveLength(0);
  });

  test('should request backend download cancellation when cancelling a running download task', async () => {
    let resolveDownload: ((value: DatabasePayload) => void) | undefined;
    const downloadPromise = new Promise<DatabasePayload>((resolve) => {
      resolveDownload = resolve;
    });

    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({
          addons: mockAddons,
          groups: mockGroups,
          settings: mockSettings,
        });
      }
      if (cmd === 'get_workshop_capabilities') {
        return Promise.resolve({
          bridgeAvailable: true,
          bridgeLoaded: true,
          bridgeInitialized: true,
          provider: 'steam-sdk',
          canQueryItems: true,
          canQueryHome: true,
          canDownload: true,
          canEnumerateInstalled: true,
        });
      }
      if (cmd === 'get_workshop_cache') {
        return Promise.resolve({
          '12345': { lastPageFetchedAt: new Date().toISOString() },
        });
      }
      if (cmd === 'get_background_tasks') {
        return Promise.resolve([
          {
            id: 'download_12345_test',
            kind: 'download',
            status: 'running',
            targetIds: ['12345'],
            progress: 42,
            createdAt: new Date().toISOString(),
            title: 'Addon Two (12345)',
          },
        ]);
      }
      if (cmd === 'cancel_download') {
        return Promise.resolve(null);
      }
      if (cmd === 'download_addon') {
        return downloadPromise;
      }
      return Promise.resolve({});
    });

    const { result } = renderHook(() => useAddonManager());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
      expect(result.current.backgroundTasks).toHaveLength(1);
      expect(result.current.backgroundTasks[0].status).toBe('running');
    });

    act(() => {
      result.current.cancelTask('download_12345_test');
    });

    expect(mockInvoke).toHaveBeenCalledWith('cancel_download', { workshopId: '12345' });
    expect(result.current.backgroundTasks[0].status).toBe('cancelled');

    resolveDownload?.({
      addons: mockAddons,
      knownUninstalledAddons: {},
      groups: mockGroups,
      masterCollections: [],
      settings: mockSettings,
    });
  });
});
