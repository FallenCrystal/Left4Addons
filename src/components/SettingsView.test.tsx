import { describe, test, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { SettingsView } from './SettingsView';
import type { Settings } from '../types/addon';

describe('SettingsView', () => {
  const baseSettings: Settings = {
    workshopDir: '/game/addons/workshop',
    loadingDir: '/game/addons',
    enableDummyBypass: false,
    suppressSdkUnavailableWarning: false,
    disableSteamworksSdk: false,
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
      false,
      false,
      true,
      baseSettings.workshopSourceSettings,
    );
  });
});
