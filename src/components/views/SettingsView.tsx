import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AlertCircle,
  ArrowDown,
  ArrowUp,
  Check,
  Cpu,
  Download,
  Filter,
  FolderOpen,
  FolderSearch,
  Palette,
  RefreshCw,
  Shield,
  TextCursorInput,
} from 'lucide-react';
import type {
  AddonId,
  AddonSummary,
  AppConfig,
  DeployLocalFilterRequest,
  FilterEvaluationTarget,
  FilterExclusionRecord,
  FilterItemVerdict,
  FilterTokenStatus,
  GameUpdateDummyPolicy,
  InvalidCharReplace,
  LocalFilterDeploymentStatus,
  SteamCapabilities,
  UpsertFilterExclusionRequest,
} from '../../api/ipc';
import { previewFilenameByRules } from '../../utils/format';
import { THEME_SEEDS, type ThemeMode, type ThemePreference } from '../../utils/theme';
import {
  Button,
  IconButton,
  SegmentedButton,
  Switch,
  TextField,
} from '../m3/primitives';
import { FilterSettingsSection } from './settings/FilterSettingsSection';

export type SettingsSection =
  | 'paths'
  | 'bypass'
  | 'downloads'
  | 'sources'
  | 'filter'
  | 'rename'
  | 'appearance';

export interface SettingsViewProps {
  config: AppConfig;
  section: SettingsSection;
  onSectionChange: (section: SettingsSection) => void;
  steamCaps: SteamCapabilities | null;
  checkingSteamCaps: boolean;
  busy: boolean;
  themePref: ThemePreference;
  language: 'zh' | 'en';
  addons?: AddonSummary[];
  exclusions?: FilterExclusionRecord[];
  localFilterStatus?: LocalFilterDeploymentStatus | null;
  filterStatusBusy?: boolean;
  filterTokenStatus?: FilterTokenStatus | null;
  filterEvalError?: string | null;
  onSaveConfig: (next: AppConfig, forceRescan?: boolean) => Promise<boolean>;
  onDetectPath: () => Promise<string | null>;
  onOpenInFileManager: (path: string) => void;
  onRefreshSteamCaps: () => void;
  onChangeTheme: (pref: ThemePreference) => void;
  onChangeLanguage: (lang: 'zh' | 'en') => void;
  onNotify: (msg: string, tone?: 'default' | 'error') => void;
  onRefreshLocalFilterStatus?: () => Promise<LocalFilterDeploymentStatus | null>;
  onCheckLocalFilterUpdates?: () => Promise<LocalFilterDeploymentStatus | null>;
  onDeployLocalFilter?: (
    request: DeployLocalFilterRequest,
  ) => Promise<LocalFilterDeploymentStatus | null>;
  onStartLocalFilterService?: () => Promise<LocalFilterDeploymentStatus | null>;
  onStopLocalFilterService?: () => Promise<LocalFilterDeploymentStatus | null>;
  onRefreshFilterTokenStatus?: () => Promise<FilterTokenStatus | null>;
  onSetFilterLlmToken?: (token: string | null) => Promise<FilterTokenStatus | null>;
  onClearFilterCache?: () => Promise<number | null>;
  onRetryFailedEvaluations?: () => void;
  onEvaluateSandboxTarget?: (
    target: FilterEvaluationTarget,
  ) => Promise<FilterItemVerdict | null>;
  onUpsertFilterExclusion?: (request: UpsertFilterExclusionRequest) => Promise<boolean>;
  onDeleteFilterExclusion?: (addonId: AddonId) => Promise<boolean>;
  onSelectAddon?: (addon: AddonSummary) => void;
}

export function SettingsView({
  config,
  section,
  onSectionChange,
  steamCaps,
  checkingSteamCaps,
  busy,
  themePref,
  language,
  addons = [],
  exclusions = [],
  localFilterStatus = null,
  filterStatusBusy = false,
  filterTokenStatus = null,
  filterEvalError = null,
  onSaveConfig,
  onDetectPath,
  onOpenInFileManager,
  onRefreshSteamCaps,
  onChangeTheme,
  onChangeLanguage,
  onNotify,
  onRefreshLocalFilterStatus = async () => null,
  onCheckLocalFilterUpdates = async () => null,
  onDeployLocalFilter = async () => null,
  onStartLocalFilterService = async () => null,
  onStopLocalFilterService = async () => null,
  onRefreshFilterTokenStatus = async () => null,
  onSetFilterLlmToken = async () => null,
  onClearFilterCache = async () => null,
  onRetryFailedEvaluations = () => {},
  onEvaluateSandboxTarget = async () => null,
  onUpsertFilterExclusion = async () => false,
  onDeleteFilterExclusion = async () => false,
  onSelectAddon,
}: SettingsViewProps) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<AppConfig | null>(null);
  const [detecting, setDetecting] = useState(false);

  const effective = draft ?? config;

  useEffect(() => {
    if (section === 'sources' && !steamCaps && !checkingSteamCaps) {
      onRefreshSteamCaps();
    }
  }, [section, steamCaps, checkingSteamCaps, onRefreshSteamCaps]);

  const sections: { id: SettingsSection; label: string; icon: typeof FolderSearch }[] = [
    { id: 'paths', label: t('settings.sections.paths'), icon: FolderSearch },
    { id: 'bypass', label: t('settings.sections.bypass'), icon: Shield },
    { id: 'downloads', label: t('settings.sections.downloads'), icon: Download },
    { id: 'sources', label: t('settings.sections.sources'), icon: Cpu },
    { id: 'filter', label: t('settings.sections.filter'), icon: Filter },
    { id: 'rename', label: t('settings.sections.rename'), icon: TextCursorInput },
    { id: 'appearance', label: t('settings.sections.appearance'), icon: Palette },
  ];

  const updateDraft = (updater: (curr: AppConfig) => AppConfig) => {
    setDraft((prev) => updater(prev ?? config));
  };

  const dirty = draft !== null && JSON.stringify(draft) !== JSON.stringify(config);
  const pathsChanged =
    effective.loadingDir !== config.loadingDir || effective.workshopDir !== config.workshopDir;

  const [saveBarMounted, setSaveBarMounted] = useState(dirty);
  const [lastPathsChanged, setLastPathsChanged] = useState(pathsChanged);

  if (dirty && !saveBarMounted) {
    setSaveBarMounted(true);
  }
  if (dirty && pathsChanged !== lastPathsChanged) {
    setLastPathsChanged(pathsChanged);
  }

  useEffect(() => {
    if (dirty || !saveBarMounted) return;
    const timer = window.setTimeout(() => {
      setSaveBarMounted(false);
    }, 180);
    return () => window.clearTimeout(timer);
  }, [dirty, saveBarMounted]);

  const handleSave = useCallback(async () => {
    if (!dirty || busy) return;
    const savedDraft = draft;
    const ok = await onSaveConfig(effective, pathsChanged);
    if (ok) {
      setDraft((curr) => (curr === savedDraft ? null : curr));
    }
  }, [busy, dirty, draft, effective, onSaveConfig, pathsChanged]);

  useEffect(() => {
    if (!dirty) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
        e.preventDefault();
        void handleSave();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [dirty, handleSave]);

  const handleDetect = async () => {
    setDetecting(true);
    try {
      const detected = await onDetectPath();
      if (detected) {
        const sep = detected.includes('\\') ? '\\' : '/';
        const normalized = detected.replace(/[\\/]+$/, '');
        updateDraft((prev) => ({
          ...prev,
          loadingDir: normalized,
          workshopDir: `${normalized}${sep}workshop`,
        }));
        onNotify(t('settings.detected'));
      } else {
        onNotify(t('settings.notDetected'), 'error');
      }
    } finally {
      setDetecting(false);
    }
  };

  const moveProvider = (idx: number, delta: -1 | 1) => {
    const nextIdx = idx + delta;
    if (nextIdx < 0 || nextIdx >= effective.sourceOrder.length) return;
    updateDraft((prev) => {
      const list = [...prev.sourceOrder];
      const [item] = list.splice(idx, 1);
      list.splice(nextIdx, 0, item);
      return { ...prev, sourceOrder: list };
    });
  };

  const renameSample = previewFilenameByRules(effective.renameRules, {
    workshopId: '2814104912',
    groupName: 'Campaigns',
    title: '  Chernobyl:  Chapter One (v2.4)  ',
  });

  return (
    <div className="l4a-view l4a-settings-view">
      <header className="l4a-view-header">
        <h1 className="md-headline-small">{t('nav.settings')}</h1>
      </header>

      <div className="l4a-settings-layout">
        <nav className="l4a-settings-nav" aria-label={t('settings.navLabel')}>
          {sections.map((item) => {
            const Icon = item.icon;
            return (
              <button
                key={item.id}
                type="button"
                className="l4a-settings-nav-item"
                data-active={section === item.id ? 'true' : 'false'}
                onClick={() => onSectionChange(item.id)}
              >
                <Icon size={18} />
                <span>{item.label}</span>
              </button>
            );
          })}
        </nav>

        <div className="l4a-settings-pane">
          {section === 'paths' && (
            <section className="l4a-section-card">
              <TextField
                label={t('settings.loadingDir')}
                value={effective.loadingDir}
                onChange={(e) => updateDraft((d) => ({ ...d, loadingDir: e.target.value }))}
                trailing={
                  <div className="l4a-row-wrap" style={{ gap: 4, flexWrap: 'nowrap' }}>
                    {config.loadingDir && (
                      <IconButton
                        size="sm"
                        label={t('common.openFolder')}
                        onClick={() => onOpenInFileManager(config.loadingDir)}
                      >
                        <FolderOpen size={15} />
                      </IconButton>
                    )}
                    <Button
                      variant="tonal"
                      size="sm"
                      disabled={detecting}
                      onClick={() => void handleDetect()}
                    >
                      {t('settings.autoDetect')}
                    </Button>
                  </div>
                }
              />
              <TextField
                label={t('settings.workshopDir')}
                value={effective.workshopDir}
                onChange={(e) => updateDraft((d) => ({ ...d, workshopDir: e.target.value }))}
                trailing={
                  config.workshopDir ? (
                    <IconButton
                      size="sm"
                      label={t('common.openFolder')}
                      onClick={() => onOpenInFileManager(config.workshopDir)}
                    >
                      <FolderOpen size={15} />
                    </IconButton>
                  ) : undefined
                }
              />
            </section>
          )}

          {section === 'bypass' && (
            <section className="l4a-section-card">
              <div className="l4a-setting-row">
                <span className="md-title-small">{t('settings.enableDummyBypass')}</span>
                <Switch
                  checked={effective.enableDummyBypass}
                  label={t('settings.enableDummyBypass')}
                  onChange={(v) =>
                    updateDraft((d) => ({
                      ...d,
                      enableDummyBypass: v,
                      gameUpdateDummyPolicy: !v ? 'ask' : d.gameUpdateDummyPolicy,
                    }))
                  }
                />
              </div>
              <div className="l4a-divider" />
              <div className="l4a-setting-row">
                <span className="md-title-small">{t('settings.autoAdoptNew')}</span>
                <Switch
                  checked={effective.autoAdoptNewWorkshopDownloads}
                  disabled={!effective.enableDummyBypass}
                  label={t('settings.autoAdoptNew')}
                  onChange={(v) =>
                    updateDraft((d) => ({ ...d, autoAdoptNewWorkshopDownloads: v }))
                  }
                />
              </div>
              <div className="l4a-divider" />
              <div className="l4a-setting-row" style={{ flexWrap: 'wrap' }}>
                <span className="md-title-small">{t('settings.updatePolicy')}</span>
                <SegmentedButton<GameUpdateDummyPolicy>
                  ariaLabel={t('settings.updatePolicy')}
                  value={effective.gameUpdateDummyPolicy}
                  onChange={(v) => updateDraft((d) => ({ ...d, gameUpdateDummyPolicy: v }))}
                  options={[
                    { value: 'ask', label: t('settings.policy.ask') },
                    {
                      value: 'auto_adopt_keep_customizations',
                      label: t('settings.policy.auto_adopt_keep_customizations'),
                      disabled: !effective.enableDummyBypass,
                    },
                    {
                      value: 'auto_keep_local_regenerate_dummy',
                      label: t('settings.policy.auto_keep_local_regenerate_dummy'),
                      disabled: !effective.enableDummyBypass,
                    },
                  ]}
                />
              </div>
            </section>
          )}

          {section === 'downloads' && (
            <section className="l4a-section-card">
              <div className="l4a-setting-row">
                <span className="md-title-small">{t('settings.concurrency')}</span>
                <div className="l4a-row-wrap">
                  <Button
                    variant="outlined"
                    size="sm"
                    disabled={effective.downloadConcurrency <= 1}
                    onClick={() =>
                      updateDraft((d) => ({
                        ...d,
                        downloadConcurrency: Math.max(1, d.downloadConcurrency - 1),
                      }))
                    }
                  >
                    −
                  </Button>
                  <span className="md-title-medium md-mono" style={{ minWidth: 28, textAlign: 'center' }}>
                    {effective.downloadConcurrency}
                  </span>
                  <Button
                    variant="outlined"
                    size="sm"
                    disabled={effective.downloadConcurrency >= 8}
                    onClick={() =>
                      updateDraft((d) => ({
                        ...d,
                        downloadConcurrency: Math.min(8, d.downloadConcurrency + 1),
                      }))
                    }
                  >
                    +
                  </Button>
                </div>
              </div>
              <div className="l4a-divider" />
              <div className="l4a-setting-row">
                <span className="md-title-small">{t('settings.retries')}</span>
                <div className="l4a-row-wrap">
                  <Button
                    variant="outlined"
                    size="sm"
                    disabled={effective.maxDownloadRetries <= 0}
                    onClick={() =>
                      updateDraft((d) => ({
                        ...d,
                        maxDownloadRetries: Math.max(0, d.maxDownloadRetries - 1),
                      }))
                    }
                  >
                    −
                  </Button>
                  <span className="md-title-medium md-mono" style={{ minWidth: 28, textAlign: 'center' }}>
                    {effective.maxDownloadRetries}
                  </span>
                  <Button
                    variant="outlined"
                    size="sm"
                    disabled={effective.maxDownloadRetries >= 10}
                    onClick={() =>
                      updateDraft((d) => ({
                        ...d,
                        maxDownloadRetries: Math.min(10, d.maxDownloadRetries + 1),
                      }))
                    }
                  >
                    +
                  </Button>
                </div>
              </div>
            </section>
          )}

          {section === 'sources' && (
            <>
              <section className="l4a-section-card">
                <span className="md-label-medium" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                  {t('settings.sourceOrder')}
                </span>
                <div className="l4a-list">
                  {effective.sourceOrder.map((src, idx) => (
                    <div
                      key={src}
                      className="l4a-list-row"
                      style={{ minHeight: 50, cursor: 'default' }}
                    >
                      <span className="md-pill md-mono" data-tone="secondary">
                        {idx + 1}
                      </span>
                      <span className="md-title-small" style={{ flex: 1 }}>
                        {t(`settings.provider.${src}`, { defaultValue: src })}
                      </span>
                      <IconButton
                        size="sm"
                        label={t('common.moveUp')}
                        disabled={idx === 0}
                        onClick={() => moveProvider(idx, -1)}
                      >
                        <ArrowUp size={15} />
                      </IconButton>
                      <IconButton
                        size="sm"
                        label={t('common.moveDown')}
                        disabled={idx === effective.sourceOrder.length - 1}
                        onClick={() => moveProvider(idx, 1)}
                      >
                        <ArrowDown size={15} />
                      </IconButton>
                    </div>
                  ))}
                </div>
              </section>

              <section className="l4a-section-card">
                <div className="l4a-setting-row">
                  <span className="md-title-small">{t('settings.httpsOnly')}</span>
                  <Switch
                    checked={effective.httpsOnly}
                    label={t('settings.httpsOnly')}
                    onChange={(v) => updateDraft((d) => ({ ...d, httpsOnly: v }))}
                  />
                </div>
                <TextField
                  label={t('settings.catalogUrl')}
                  placeholder={effective.httpsOnly ? 'https://catalog.example.com' : 'http://127.0.0.1:8787'}
                  value={effective.catalogServerUrl ?? ''}
                  onChange={(e) =>
                    updateDraft((d) => ({
                      ...d,
                      catalogServerUrl: e.target.value || null,
                    }))
                  }
                />
              </section>

              <section className="l4a-section-card">
                <div className="l4a-setting-row">
                  <div className="l4a-row-wrap">
                    <span className="md-title-small">{t('settings.sdkStatus')}</span>
                    <span
                      className="md-pill"
                      data-tone={
                        checkingSteamCaps
                          ? 'tertiary'
                          : steamCaps?.bridgeInitialized
                            ? 'primary'
                            : 'surface'
                      }
                    >
                      {checkingSteamCaps
                        ? t('settings.sdkChecking')
                        : steamCaps?.bridgeInitialized
                          ? t('settings.sdkReady')
                          : t('settings.sdkUnavailable')}
                    </span>
                    {steamCaps?.bridgeVersion && (
                      <span className="md-body-small md-mono" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                        v{steamCaps.bridgeVersion}
                      </span>
                    )}
                    {steamCaps?.currentUserSteamId && (
                      <span className="md-body-small md-mono" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                        {steamCaps.currentUserSteamId}
                      </span>
                    )}
                  </div>
                  <Button
                    variant="tonal"
                    size="sm"
                    disabled={checkingSteamCaps}
                    icon={<RefreshCw size={13} className={checkingSteamCaps ? 'md-spin' : undefined} />}
                    onClick={onRefreshSteamCaps}
                  >
                    {t('settings.refreshSdk')}
                  </Button>
                </div>
                {steamCaps?.lastError && !steamCaps.bridgeInitialized && (
                  <div className="l4a-path-chip">
                    <AlertCircle size={14} style={{ color: 'var(--md-sys-color-error)', flexShrink: 0 }} />
                    <span className="md-body-small selectable">{steamCaps.lastError}</span>
                  </div>
                )}
                <div className="l4a-divider" />
                <div className="l4a-setting-row">
                  <span className="md-title-small">{t('settings.disableSdk')}</span>
                  <Switch
                    checked={effective.disableSteamworksSdk}
                    label={t('settings.disableSdk')}
                    onChange={(v) => updateDraft((d) => ({ ...d, disableSteamworksSdk: v }))}
                  />
                </div>
                <div className="l4a-divider" />
                <div className="l4a-setting-row">
                  <span className="md-title-small">{t('settings.forceSdkDownload')}</span>
                  <Switch
                    checked={effective.forceSteamworksSdkDownload}
                    disabled={effective.disableSteamworksSdk}
                    label={t('settings.forceSdkDownload')}
                    onChange={(v) =>
                      updateDraft((d) => ({ ...d, forceSteamworksSdkDownload: v }))
                    }
                  />
                </div>
              </section>
            </>
          )}

          {section === 'filter' && (
            <FilterSettingsSection
              effectiveConfig={effective}
              dirty={dirty}
              busy={busy}
              filterStatusBusy={filterStatusBusy}
              localStatus={localFilterStatus}
              tokenStatus={filterTokenStatus}
              evalError={filterEvalError}
              addons={addons}
              namePreference={effective.addonNamePreference ?? 'workshop_title'}
              exclusions={exclusions}
              onUpdateFilter={(updater) =>
                updateDraft((d) => ({ ...d, filter: updater(d.filter) }))
              }
              onSaveDraftFirst={async () => {
                if (!dirty) return true;
                const savedDraft = draft;
                const ok = await onSaveConfig(effective, pathsChanged);
                if (ok) {
                  setDraft((curr) => (curr === savedDraft ? null : curr));
                }
                return ok;
              }}
              onRefreshLocalStatus={onRefreshLocalFilterStatus}
              onCheckLocalUpdates={onCheckLocalFilterUpdates}
              onDeployLocalFilter={async (request) => {
                const status = await onDeployLocalFilter(request);
                if (status) {
                  const nextPinned = request.trackLatest
                    ? null
                    : (request.targetCommit?.trim() || null);
                  setDraft((curr) =>
                    curr
                      ? {
                          ...curr,
                          filter: {
                            ...curr.filter,
                            repoUrl: request.repoUrl?.trim() || curr.filter.repoUrl,
                            trackLatest: request.trackLatest,
                            pinnedCommit: nextPinned,
                          },
                        }
                      : null,
                  );
                }
                return status;
              }}
              onStartLocalService={onStartLocalFilterService}
              onStopLocalService={onStopLocalFilterService}
              onRefreshTokenStatus={onRefreshFilterTokenStatus}
              onSetLlmToken={onSetFilterLlmToken}
              onClearFilterCache={onClearFilterCache}
              onRetryFailedEvaluations={onRetryFailedEvaluations}
              onEvaluateSandbox={onEvaluateSandboxTarget}
              onUpsertExclusion={onUpsertFilterExclusion}
              onDeleteExclusion={onDeleteFilterExclusion}
              onSelectAddon={onSelectAddon}
            />
          )}

          {section === 'rename' && (
            <section className="l4a-section-card">
              <div className="md-field">
                <span className="md-field-label">{t('settings.previewLabel')}</span>
                <div className="l4a-preview-box">{renameSample}</div>
              </div>

              <div className="l4a-setting-row">
                <span className="md-title-small">{t('settings.workshopIdPrefix')}</span>
                <Switch
                  checked={effective.renameRules.enableWorkshopIdPrefix}
                  label={t('settings.workshopIdPrefix')}
                  onChange={(v) =>
                    updateDraft((d) => ({
                      ...d,
                      renameRules: { ...d.renameRules, enableWorkshopIdPrefix: v },
                    }))
                  }
                />
              </div>
              <div className="l4a-divider" />
              <div className="l4a-setting-row">
                <span className="md-title-small">{t('settings.groupPrefix')}</span>
                <Switch
                  checked={effective.renameRules.enableGroupPrefix}
                  label={t('settings.groupPrefix')}
                  onChange={(v) =>
                    updateDraft((d) => ({
                      ...d,
                      renameRules: { ...d.renameRules, enableGroupPrefix: v },
                    }))
                  }
                />
              </div>
              <div className="l4a-divider" />
              <div className="l4a-setting-row">
                <span className="md-title-small">{t('settings.cleanSpecialChars')}</span>
                <Switch
                  checked={effective.renameRules.cleanSpecialChars}
                  label={t('settings.cleanSpecialChars')}
                  onChange={(v) =>
                    updateDraft((d) => ({
                      ...d,
                      renameRules: { ...d.renameRules, cleanSpecialChars: v },
                    }))
                  }
                />
              </div>
              {effective.renameRules.cleanSpecialChars && (
                <div className="l4a-setting-row">
                  <span className="md-title-small">{t('settings.invalidCharReplace')}</span>
                  <SegmentedButton<InvalidCharReplace>
                    ariaLabel={t('settings.invalidCharReplace')}
                    value={effective.renameRules.invalidCharReplace}
                    onChange={(v) =>
                      updateDraft((d) => ({
                        ...d,
                        renameRules: { ...d.renameRules, invalidCharReplace: v },
                      }))
                    }
                    options={[
                      { value: 'underscore', label: t('settings.replace.underscore') },
                      { value: 'space', label: t('settings.replace.space') },
                      { value: 'empty', label: t('settings.replace.empty') },
                    ]}
                  />
                </div>
              )}
              <div className="l4a-divider" />
              <div className="l4a-setting-row">
                <span className="md-title-small">{t('settings.enableTrim')}</span>
                <Switch
                  checked={effective.renameRules.enableTrim}
                  label={t('settings.enableTrim')}
                  onChange={(v) =>
                    updateDraft((d) => ({
                      ...d,
                      renameRules: { ...d.renameRules, enableTrim: v },
                    }))
                  }
                />
              </div>
              <div className="l4a-divider" />
              <div className="l4a-setting-row">
                <span className="md-title-small">{t('settings.removeDoubleSpaces')}</span>
                <Switch
                  checked={effective.renameRules.enableRemoveDoubleSpaces}
                  label={t('settings.removeDoubleSpaces')}
                  onChange={(v) =>
                    updateDraft((d) => ({
                      ...d,
                      renameRules: { ...d.renameRules, enableRemoveDoubleSpaces: v },
                    }))
                  }
                />
              </div>
              <div className="l4a-divider" />
              <TextField
                label={t('settings.maxFilenameLength')}
                type="number"
                min={0}
                max={240}
                value={effective.renameRules.maxFilenameLength}
                onChange={(e) =>
                  updateDraft((d) => ({
                    ...d,
                    renameRules: {
                      ...d.renameRules,
                      maxFilenameLength: Math.max(
                        0,
                        Math.min(240, Number.parseInt(e.target.value || '0', 10) || 0),
                      ),
                    },
                  }))
                }
                trailing={
                  <div className="l4a-row-wrap" style={{ gap: 4, flexWrap: 'nowrap' }}>
                    <Button
                      variant="outlined"
                      size="sm"
                      disabled={effective.renameRules.maxFilenameLength <= 0}
                      onClick={() =>
                        updateDraft((d) => ({
                          ...d,
                          renameRules: {
                            ...d.renameRules,
                            maxFilenameLength: Math.max(0, d.renameRules.maxFilenameLength - 10),
                          },
                        }))
                      }
                    >
                      −
                    </Button>
                    <Button
                      variant="outlined"
                      size="sm"
                      disabled={effective.renameRules.maxFilenameLength >= 240}
                      onClick={() =>
                        updateDraft((d) => ({
                          ...d,
                          renameRules: {
                            ...d.renameRules,
                            maxFilenameLength: Math.min(240, d.renameRules.maxFilenameLength + 10),
                          },
                        }))
                      }
                    >
                      +
                    </Button>
                  </div>
                }
              />
            </section>
          )}

          {section === 'appearance' && (
            <>
              <section className="l4a-section-card">
                <div className="l4a-setting-row" style={{ flexWrap: 'wrap' }}>
                  <span className="md-title-small">{t('settings.themeMode')}</span>
                  <SegmentedButton<ThemeMode>
                    ariaLabel={t('settings.themeMode')}
                    value={themePref.mode}
                    onChange={(mode) => onChangeTheme({ ...themePref, mode })}
                    options={[
                      { value: 'system', label: t('settings.theme.system') },
                      { value: 'light', label: t('settings.theme.light') },
                      { value: 'dark', label: t('settings.theme.dark') },
                    ]}
                  />
                </div>
                <div className="l4a-divider" />
                <div className="l4a-setting-row">
                  <span className="md-title-small">{t('settings.seedColor')}</span>
                  <div className="l4a-color-swatches">
                    {THEME_SEEDS.map((s) => (
                      <button
                        key={s.id}
                        type="button"
                        className="l4a-swatch"
                        style={{ backgroundColor: s.swatch }}
                        data-selected={themePref.seed === s.id ? 'true' : 'false'}
                        aria-label={s.id}
                        onClick={() => onChangeTheme({ ...themePref, seed: s.id })}
                      >
                        {themePref.seed === s.id && <Check size={15} strokeWidth={2.8} />}
                      </button>
                    ))}
                  </div>
                </div>
                <div className="l4a-divider" />
                <div className="l4a-setting-row">
                  <span className="md-title-small">{t('settings.language')}</span>
                  <SegmentedButton<'zh' | 'en'>
                    ariaLabel={t('settings.language')}
                    value={language}
                    onChange={onChangeLanguage}
                    options={[
                      { value: 'zh', label: '中文' },
                      { value: 'en', label: 'English' },
                    ]}
                  />
                </div>
              </section>

              <section className="l4a-section-card">
                <div className="l4a-setting-row" style={{ flexWrap: 'wrap' }}>
                  <span className="md-title-small">{t('settings.addonNamePreference')}</span>
                  <SegmentedButton<'workshop_title' | 'addon_title' | 'file_name'>
                    ariaLabel={t('settings.addonNamePreference')}
                    value={effective.addonNamePreference ?? 'workshop_title'}
                    onChange={(v) => updateDraft((d) => ({ ...d, addonNamePreference: v }))}
                    options={[
                      { value: 'workshop_title', label: t('settings.namePref.workshop_title') },
                      { value: 'addon_title', label: t('settings.namePref.addon_title') },
                      { value: 'file_name', label: t('settings.namePref.file_name') },
                    ]}
                  />
                </div>
                <div className="l4a-divider" />
                <div className="l4a-setting-row">
                  <span className="md-title-small">{t('settings.collapseCampaignBundles')}</span>
                  <Switch
                    checked={effective.collapseCampaignBundles ?? true}
                    label={t('settings.collapseCampaignBundles')}
                    onChange={(v) =>
                      updateDraft((d) => ({ ...d, collapseCampaignBundles: v }))
                    }
                  />
                </div>
              </section>
            </>
          )}
        </div>
      </div>

      {saveBarMounted && (
        <div
          className="l4a-settings-save-bar"
          role="region"
          aria-label={t('settings.unsavedChanges')}
          data-state={dirty ? 'open' : 'closing'}
        >
          <span className="l4a-settings-save-dot" aria-hidden="true" />
          <span className="md-label-large">{t('settings.unsavedChanges')}</span>
          <div className="l4a-spacer" />
          <Button variant="text" size="sm" disabled={busy || !dirty} onClick={() => setDraft(null)}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="filled"
            size="sm"
            disabled={busy || !dirty}
            onClick={() => void handleSave()}
          >
            {(dirty ? pathsChanged : lastPathsChanged)
              ? t('settings.saveAndScan')
              : t('common.save')}
          </Button>
        </div>
      )}
    </div>
  );
}
