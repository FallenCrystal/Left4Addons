import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, test, vi } from 'vitest';
import { useBackgroundTasks } from './useBackgroundTasks';

const { mockInvoke, mockFetchDependencySnapshot } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
  mockFetchDependencySnapshot: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (command: string, args?: Record<string, unknown>) => mockInvoke(command, args),
}));

vi.mock('../services/workshopClient', () => ({
  fetchWorkshopDependencySnapshot: (workshopId: string) => mockFetchDependencySnapshot(workshopId),
  fetchWorkshopItem: vi.fn(),
}));

describe('useBackgroundTasks dependency checks', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockFetchDependencySnapshot.mockReset();
    mockInvoke.mockImplementation((command) => {
      if (command === 'get_background_tasks') return Promise.resolve([]);
      if (command === 'persist_workshop_page_details') {
        return Promise.resolve({ addons: {}, knownUninstalledAddons: {}, groups: [], settings: {} });
      }
      return Promise.resolve();
    });
  });

  test('visits a dependency chain once and terminates a cycle', async () => {
    const relations: Record<string, string[]> = {
      A: ['B'],
      B: ['C'],
      C: ['A'],
    };
    mockFetchDependencySnapshot.mockImplementation(async (workshopId: string) => ({
      imageGallery: [],
      tags: [],
      parentCollections: [],
      requiredItems: relations[workshopId].map((id) => ({ workshopId: id, title: id })),
    }));
    const updateLocalState = vi.fn();
    const { result } = renderHook(() => useBackgroundTasks({
      enabled: true,
      downloadConcurrency: 1,
      addons: {},
      knownUninstalledAddons: {},
      updateLocalState,
      onDownloadSuccess: vi.fn(),
      onDownloadCancelled: vi.fn(),
      onTaskError: vi.fn(),
    }));

    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('get_background_tasks', undefined));
    act(() => {
      result.current.enqueueDependencyCheck(['A'], 'test');
    });

    await waitFor(() => {
      const task = result.current.backgroundTasks.find((candidate) => candidate.kind === 'dependency-check');
      expect(task?.status).toBe('completed');
      expect(task?.dependencyCheck?.completedCount).toBe(3);
    });

    expect(mockFetchDependencySnapshot.mock.calls.map(([id]) => id)).toEqual(['A', 'B', 'C']);
    expect(mockInvoke.mock.calls.filter(([command]) => command === 'persist_workshop_page_details')).toHaveLength(3);
    expect(updateLocalState).toHaveBeenCalledTimes(1);
  });
});
