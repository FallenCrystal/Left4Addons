import type { DependencyRefreshMode, WorkshopSdkHtmlScope, WorkshopSourceSettings } from '../types/addon';

export const DEFAULT_WORKSHOP_SOURCE_SETTINGS: WorkshopSourceSettings = {
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
};

export function resolveWorkshopSdkHtmlScope(
  scope: WorkshopSdkHtmlScope | string | null | undefined,
  preset?: string | null,
  allowSdkHtmlHybrid?: boolean | null,
): WorkshopSdkHtmlScope {
  if (scope === 'disabled' || scope === 'search' || scope === 'navigation' || scope === 'all') {
    return scope;
  }
  if (preset === 'hybrid' || allowSdkHtmlHybrid) {
    return 'all';
  }
  return 'search';
}

function resolveDependencyRefreshMode(value: DependencyRefreshMode | string | null | undefined, fallback: DependencyRefreshMode): DependencyRefreshMode {
  return value === 'always' || value === 'cache-missing' ? value : fallback;
}

export function normalizeWorkshopSourceSettings(
  settings?: Partial<WorkshopSourceSettings> | null,
): WorkshopSourceSettings {
  const normalizedPreset = (settings as any)?.preset === 'sdkOnly' ? 'sdk-only' : (settings?.preset || DEFAULT_WORKSHOP_SOURCE_SETTINGS.preset);
  const sdkHtmlScope = resolveWorkshopSdkHtmlScope(
    settings?.sdkHtmlScope,
    normalizedPreset,
    settings?.allowSdkHtmlHybrid,
  );

  return {
    ...DEFAULT_WORKSHOP_SOURCE_SETTINGS,
    ...(settings || {}),
    preset: normalizedPreset as WorkshopSourceSettings['preset'],
    allowSdkHtmlHybrid: sdkHtmlScope === 'all',
    sdkHtmlScope,
    dependencySdkRefresh: resolveDependencyRefreshMode(
      settings?.dependencySdkRefresh,
      DEFAULT_WORKSHOP_SOURCE_SETTINGS.dependencySdkRefresh,
    ),
    dependencyHtmlRefresh: resolveDependencyRefreshMode(
      settings?.dependencyHtmlRefresh,
      DEFAULT_WORKSHOP_SOURCE_SETTINGS.dependencyHtmlRefresh,
    ),
    sourceOrder: settings?.sourceOrder?.length
      ? settings.sourceOrder
      : DEFAULT_WORKSHOP_SOURCE_SETTINGS.sourceOrder,
    cacheRetention: 'keep',
  };
}

export function shouldAllowSteamCommunityHtmlSource(
  settings: WorkshopSourceSettings,
  source: string,
  sdkAvailable: boolean,
): boolean {
  if (settings.preset === 'offline' || !settings.allowSteamCommunityHtml) {
    return false;
  }

  if (!sdkAvailable) {
    return true;
  }

  if (source === 'addon-detail' || source === 'workshop-detail' || source === 'dependency-check') {
    return true;
  }

  if (source === 'workshop-search' || source === 'workshop-creator') {
    return settings.sdkHtmlScope === 'search'
      || settings.sdkHtmlScope === 'navigation'
      || settings.sdkHtmlScope === 'all';
  }

  if (source === 'workshop-home' || source === 'workshop-browse') {
    return settings.sdkHtmlScope === 'navigation' || settings.sdkHtmlScope === 'all';
  }

  if (source === 'startup-auto' || source === 'background-refresh') {
    return settings.sdkHtmlScope === 'all';
  }

  return false;
}
