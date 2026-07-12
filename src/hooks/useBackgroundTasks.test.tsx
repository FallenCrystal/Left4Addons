import React, { StrictMode } from 'react';
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
      expect(task?.dependencyCheck?.discoveredDependencies).toEqual(expect.arrayContaining([
        expect.objectContaining({ workshopId: 'B', title: 'B' }),
        expect.objectContaining({ workshopId: 'C', title: 'C' }),
        expect.objectContaining({ workshopId: 'A', title: 'A' }),
      ]));
    });

    expect(mockFetchDependencySnapshot.mock.calls.map(([id]) => id)).toEqual(['A', 'B', 'C']);
    expect(mockInvoke.mock.calls.filter(([command]) => command === 'persist_workshop_page_details')).toHaveLength(3);
    expect(updateLocalState).toHaveBeenCalledTimes(1);
  });

  test('retries and removes a failed dependency check', async () => {
    let attempts = 0;
    mockFetchDependencySnapshot.mockImplementation(async () => {
      attempts += 1;
      if (attempts === 1) {
        throw new Error('temporary dependency lookup failure');
      }
      return {
        imageGallery: [],
        tags: [],
        parentCollections: [],
        requiredItems: [],
      };
    });

    const { result } = renderHook(() => useBackgroundTasks({
      enabled: true,
      downloadConcurrency: 1,
      addons: {},
      knownUninstalledAddons: {},
      updateLocalState: vi.fn(),
      onDownloadSuccess: vi.fn(),
      onDownloadCancelled: vi.fn(),
      onTaskError: vi.fn(),
    }));

    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('get_background_tasks', undefined));
    act(() => {
      result.current.enqueueDependencyCheck(['A'], 'test');
    });

    let taskId = '';
    await waitFor(() => {
      const task = result.current.backgroundTasks.find((candidate) => candidate.kind === 'dependency-check');
      expect(task?.status).toBe('failed');
      expect(task?.dependencyCheck?.failedNodes).toEqual([
        expect.objectContaining({ workshopId: 'A' }),
      ]);
      taskId = task!.id;
    });

    act(() => {
      result.current.retryTask(taskId);
    });

    await waitFor(() => {
      const task = result.current.backgroundTasks.find((candidate) => candidate.id === taskId);
      expect(task?.status).toBe('completed');
      expect(task?.dependencyCheck?.failedNodes).toEqual([]);
    });
    expect(mockFetchDependencySnapshot).toHaveBeenCalledTimes(2);

    act(() => {
      result.current.removeTask(taskId);
    });

    expect(result.current.backgroundTasks).not.toContainEqual(expect.objectContaining({ id: taskId }));
  });

  test('restores a persisted download only once under StrictMode', async () => {
    let resolveDownload: (() => void) | undefined;
    const downloadPromise = new Promise<void>((resolve) => {
      resolveDownload = resolve;
    });
    const restoredTask = {
      id: 'download_A_restored',
      kind: 'download' as const,
      status: 'running' as const,
      targetIds: ['A'],
      progress: 42,
      createdAt: new Date().toISOString(),
    };

    mockInvoke.mockImplementation((command) => {
      if (command === 'get_background_tasks') return Promise.resolve([restoredTask]);
      if (command === 'download_addon') return downloadPromise;
      return Promise.resolve();
    });

    renderHook(() => useBackgroundTasks({
      enabled: true,
      downloadConcurrency: 2,
      addons: {},
      knownUninstalledAddons: {},
      updateLocalState: vi.fn(),
      onDownloadSuccess: vi.fn(),
      onDownloadCancelled: vi.fn(),
      onTaskError: vi.fn(),
    }), {
      wrapper: ({ children }) => <StrictMode>{children}</StrictMode>,
    });

    await waitFor(() => {
      expect(mockInvoke.mock.calls.filter(([command]) => command === 'download_addon')).toHaveLength(1);
    });

    resolveDownload?.();
  });
});
