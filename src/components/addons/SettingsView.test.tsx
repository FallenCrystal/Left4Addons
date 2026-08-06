import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { SettingsView } from './SettingsView';
import type { Settings } from '../../types/addon';

const mockInvoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => mockInvoke(cmd, args),
}));

describe('SettingsView', () => {
  const baseSettings: Settings = {
    workshopDir: '/game/addons/workshop',
    loadingDir: '/game/addons',
    downloadConcurrency: 2,
    enableDummyBypass: false,
    suppressSdkUnavailableWarning: false,
    disableSteamworksSdk: false,
    forceSteamworksSdkDownload: false,
    maxDownloadRetries: 3,
    workshopSourceSettings: {
      preset: 'conservative',
      allowSteamworksSdk: true,
      allowSteamWebApi: true,
      allowSteamCommunityHtml: true,
      allowSdkHtmlHybrid: false,
      sdkHtmlScope: 'search',
      dependencySdkRefresh: 'always',
      dependencyHtmlRefresh: 'cache-missing',
      sourceOrder: ['steamworks-sdk', 'steam-web-api', 'steamcommunity-html'],
      cacheRetention: 'keep',
    },
    renameSettings: {
      enableWorkshopIdPrefix: true,
      enableGroupPrefix: true,
      cleanSpecialChars: false,
      invalidCharReplace: 'underscore',
      maxFilenameLength: 0,
      enableTrim: true,
      enableRemoveDoubleSpaces: true,
    },
  };

  test('submits the Steamworks SDK disable toggle from the SDK tab', async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);

    render(
      <SettingsView
        settings={baseSettings}
        isSubmitting={false}
        onConfirm={onConfirm}
      />
    );

    fireEvent.click(screen.getByText('SDK'));

    const disableSdkRow = screen.getByText('禁用 Steamworks SDK').closest('div')?.parentElement;
    const disableSdkCheckbox = disableSdkRow?.querySelector('input[type="checkbox"]');
    expect(disableSdkCheckbox).toBeTruthy();
    fireEvent.click(disableSdkCheckbox as HTMLInputElement);

    await waitFor(() => {
      expect(onConfirm).toHaveBeenCalledWith(
        '/game/addons',
        2,
        false,
        false,
        true,
        false,
        3,
        'ask',
        baseSettings.workshopSourceSettings,
        expect.any(Object),
      );
    });
  });

  test('submits clamped download concurrency from the download tab', async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);

    render(
      <SettingsView
        settings={baseSettings}
        isSubmitting={false}
        onConfirm={onConfirm}
      />
    );

    fireEvent.click(screen.getByText('下载'));

    const input = screen.getAllByRole('spinbutton')[0] as HTMLInputElement;
    fireEvent.change(input, { target: { value: '99' } });
    fireEvent.blur(input);

    await waitFor(() => {
      expect(onConfirm).toHaveBeenCalledWith(
        '/game/addons',
        8,
        false,
        false,
        false,
        false,
        3,
        'ask',
        baseSettings.workshopSourceSettings,
        expect.any(Object),
      );
    });
  });

  test('submits sdkHtmlScope changes from the sources tab', async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);

    render(
      <SettingsView
        settings={baseSettings}
        isSubmitting={false}
        onConfirm={onConfirm}
      />
    );

    fireEvent.click(screen.getByText('数据来源'));
    fireEvent.click(screen.getByText('创意工坊搜索'));
    fireEvent.click(screen.getByText('允许所有'));

    await waitFor(() => {
      expect(onConfirm).toHaveBeenCalledWith(
        '/game/addons',
        2,
        false,
        false,
        false,
        false,
        3,
        'ask',
        expect.objectContaining({
          preset: 'hybrid',
          sdkHtmlScope: 'all',
        }),
        expect.any(Object),
      );
    });
  });

  test('handles auto detect path successfully', async () => {
    mockInvoke.mockResolvedValueOnce('/auto/detected/l4d2/addons');

    render(
      <SettingsView
        settings={baseSettings}
        isSubmitting={false}
        onConfirm={vi.fn()}
      />
    );

    const autoDetectBtn = screen.getByText('自动查找');
    fireEvent.click(autoDetectBtn);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('auto_detect_addons_path', undefined);
      expect((screen.getByDisplayValue('/auto/detected/l4d2/addons') as HTMLInputElement).value).toBe('/auto/detected/l4d2/addons');
      expect(screen.getByText('已自动找到 Left 4 Dead 2 路径！')).toBeDefined();
    });
  });
});

