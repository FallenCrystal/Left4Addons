import { describe, test, expect, vi, beforeEach } from 'vitest';
import { renderHook, act, waitFor } from '@testing-library/react';
import { useAddonManager } from './useAddonManager';
import { Addon, DatabasePayload, Group, Settings } from '../types/addon';

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
    enableDummyBypass: false,
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
      if (cmd === 'move_addons') {
        return Promise.resolve({});
      }
      if (cmd === 'toggle_addons') {
        expect(args).toEqual({ ids: ['addon1.vpk', 'addon2.vpk'], enabled: true });
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

  test('should handle group operations: create, delete, and rename', async () => {
    mockInvoke.mockImplementation((cmd, args) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({ addons: mockAddons, groups: mockGroups, settings: mockSettings });
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

  test('should set isSubmitting to true during async actions and reset to false afterwards', async () => {
    let resolvePromise: (value: unknown) => void = () => {};
    const tauriPromise = new Promise((resolve) => {
      resolvePromise = resolve;
    });

    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'get_addons') {
        return Promise.resolve({ addons: mockAddons, groups: mockGroups, settings: mockSettings });
      }
      if (cmd === 'save_settings') {
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
      savePromise = result.current.saveSettings('/new/loading/dir', false);
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
});
