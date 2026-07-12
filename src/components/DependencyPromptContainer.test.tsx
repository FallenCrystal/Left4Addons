import React from 'react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';
import { DependencyPromptContainer } from './DependencyPromptContainer';
import type { BackgroundTask, Settings } from '../types/addon';

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
        addons={{}}
        knownUninstalledAddons={{}}
        backgroundTasks={[runningCheck]}
        settings={settings}
        onDownload={onDownload}
        onGoToSettings={vi.fn()}
      />,
    );

    rerender(
      <DependencyPromptContainer
        addons={{}}
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
});
