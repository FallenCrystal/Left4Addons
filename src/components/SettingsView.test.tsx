import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { SettingsView } from './SettingsView';
import type { Settings } from '../types/addon';

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
      sourceOrder: ['steamworks-sdk', 'steam-web-api', 'steamcommunity-html'],
      cacheRetention: 'keep',
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
    fireEvent.click(screen.getByText('保存并重新扫描'));

    expect(onConfirm).toHaveBeenCalledWith(
      '/game/addons',
      2,
      false,
      false,
      true,
      false,
      3,
      baseSettings.workshopSourceSettings,
    );
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
    fireEvent.click(screen.getByText('保存并重新扫描'));

    expect(onConfirm).toHaveBeenCalledWith(
      '/game/addons',
      8,
      false,
      false,
      false,
      false,
      3,
      baseSettings.workshopSourceSettings,
    );
  });
});
