import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';
import { DependencyPromptContainer } from './DependencyPromptContainer';
import type { BackgroundTask, Settings } from '../../types/addon';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

const settings: Settings = {
  workshopDir: '',
  loadingDir: '',
  downloadConcurrency: 2,
  enableDummyBypass: false,
  suppressSdkUnavailableWarning: false,
  disableSteamworksSdk: false,
  dependencyMissingBehavior: 'ask',
};

const runningCheck: BackgroundTask = {
  id: 'dependency-check-root',
  kind: 'dependency-check',
  status: 'running',
  targetIds: ['root'],
  progress: 50,
  createdAt: '2026-07-12T00:00:00Z',
  dependencyCheck: {
    rootIds: ['root'],
    discoveredCount: 2,
    completedCount: 1,
    failedNodes: [],
    discoveredDependencies: [],
  },
};

describe('DependencyPromptContainer', () => {
  test('prompts for dependencies discovered for an unknown workshop item', async () => {
    const onDownload = vi.fn();
    const { rerender } = render(
      <DependencyPromptContainer
        addons={{
          'root': {
            id: 'root',
            vpkName: 'root.vpk',
            dirType: 'workshop',
            isEnabled: true,
            fileSize: 1234,
            filesCount: 1,
            workshopId: 'root',
          },
        }}
        knownUninstalledAddons={{}}
        backgroundTasks={[runningCheck]}
        settings={settings}
        onDownload={onDownload}
        onGoToSettings={vi.fn()}
      />,
    );

    rerender(
      <DependencyPromptContainer
        addons={{
          'root': {
            id: 'root',
            vpkName: 'root.vpk',
            dirType: 'workshop',
            isEnabled: true,
            fileSize: 1234,
            filesCount: 1,
            workshopId: 'root',
          },
        }}
        knownUninstalledAddons={{}}
        backgroundTasks={[{
          ...runningCheck,
          status: 'completed',
          finishedAt: '2026-07-12T00:00:01Z',
          dependencyCheck: {
            ...runningCheck.dependencyCheck!,
            completedCount: 2,
            discoveredDependencies: [{
              workshopId: 'dependency',
              title: 'Dependency addon',
              previewUrl: 'https://example.com/dependency.jpg',
              creatorName: 'Dependency author',
            }],
          },
        }]}
        settings={settings}
        onDownload={onDownload}
        onGoToSettings={vi.fn()}
      />,
    );

    await waitFor(() => {
      expect(screen.getByText('Dependency addon')).toBeDefined();
      expect(screen.getByText('Dependency author')).toBeDefined();
    });

    fireEvent.click(screen.getByText('下载选中项'));
    expect(onDownload).toHaveBeenCalledWith([{
      workshopId: 'dependency',
      title: 'Dependency addon',
      imagePath: 'https://example.com/dependency.jpg',
    }]);
  });

  test('does not prompt for dependencies of an addon that has been uninstalled or deleted', async () => {
    const onDownload = vi.fn();
    const { rerender } = render(
      <DependencyPromptContainer
        addons={{
          'root': {
            id: 'root',
            vpkName: 'root.vpk',
            dirType: 'workshop',
            isEnabled: true,
            fileSize: 1234,
            filesCount: 1,
            workshopId: 'root',
          },
        }}
        knownUninstalledAddons={{}}
        backgroundTasks={[runningCheck]}
        settings={settings}
        onDownload={onDownload}
        onGoToSettings={vi.fn()}
      />,
    );

    const completedTask: BackgroundTask = {
      ...runningCheck,
      status: 'completed',
      finishedAt: '2026-07-12T00:00:01Z',
      dependencyCheck: {
        ...runningCheck.dependencyCheck!,
        completedCount: 2,
        discoveredDependencies: [{
          workshopId: 'dependency',
          title: 'Dependency addon',
          previewUrl: 'https://example.com/dependency.jpg',
          creatorName: 'Dependency author',
        }],
      },
    };

    rerender(
      <DependencyPromptContainer
        addons={{
          'root': {
            id: 'root',
            vpkName: 'root.vpk',
            dirType: 'workshop',
            isEnabled: true,
            fileSize: 1234,
            filesCount: 1,
            workshopId: 'root',
          },
        }}
        knownUninstalledAddons={{}}
        backgroundTasks={[completedTask]}
        settings={settings}
        onDownload={onDownload}
        onGoToSettings={vi.fn()}
      />,
    );

    await waitFor(() => {
      expect(screen.getByText('Dependency addon')).toBeDefined();
    });

    // Now uninstall the parent addon (root) by changing dirType to 'none'
    rerender(
      <DependencyPromptContainer
        addons={{
          'root': {
            id: 'root',
            vpkName: 'root.vpk',
            dirType: 'none',
            isEnabled: false,
            fileSize: 0,
            filesCount: 0,
            workshopId: 'root',
          },
        }}
        knownUninstalledAddons={{}}
        backgroundTasks={[completedTask]}
        settings={settings}
        onDownload={onDownload}
        onGoToSettings={vi.fn()}
      />,
    );

    await waitFor(() => {
      expect(screen.queryByText('Dependency addon')).toBeNull();
    });

    // 3. Start a new scan task for a different, active addon (root2)
    const runningCheck2: BackgroundTask = {
      id: 'dependency-check-root2',
      kind: 'dependency-check',
      status: 'running',
      targetIds: ['root2'],
      progress: 50,
      createdAt: '2026-07-12T00:01:00Z',
      dependencyCheck: {
        rootIds: ['root2'],
        discoveredCount: 2,
        completedCount: 1,
        failedNodes: [],
        discoveredDependencies: [],
      },
    };

    rerender(
      <DependencyPromptContainer
        addons={{
          'root2': {
            id: 'root2',
            vpkName: 'root2.vpk',
            dirType: 'workshop',
            isEnabled: true,
            fileSize: 1234,
            filesCount: 1,
            workshopId: 'root2',
          },
        }}
        knownUninstalledAddons={{}}
        backgroundTasks={[completedTask, runningCheck2]}
        settings={settings}
        onDownload={onDownload}
        onGoToSettings={vi.fn()}
      />,
    );

    // Complete the second scan task
    const completedTask2: BackgroundTask = {
      ...runningCheck2,
      status: 'completed',
      finishedAt: '2026-07-12T00:01:01Z',
      dependencyCheck: {
        ...runningCheck2.dependencyCheck!,
        completedCount: 2,
        discoveredDependencies: [{
          workshopId: 'dependency2',
          title: 'Dependency addon 2',
          previewUrl: 'https://example.com/dependency2.jpg',
          creatorName: 'Dependency author 2',
        }],
      },
    };

    rerender(
      <DependencyPromptContainer
        addons={{
          'root2': {
            id: 'root2',
            vpkName: 'root2.vpk',
            dirType: 'workshop',
            isEnabled: true,
            fileSize: 1234,
            filesCount: 1,
            workshopId: 'root2',
          },
        }}
        knownUninstalledAddons={{}}
        backgroundTasks={[completedTask, completedTask2]}
        settings={settings}
        onDownload={onDownload}
        onGoToSettings={vi.fn()}
      />,
    );

    // Prompt should open again, but ONLY contain "Dependency addon 2", not "Dependency addon"
    await waitFor(() => {
      expect(screen.getByText('Dependency addon 2')).toBeDefined();
    });

    expect(screen.queryByText('Dependency addon')).toBeNull();
  });
});

