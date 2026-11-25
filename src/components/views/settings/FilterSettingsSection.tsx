import { useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AlertCircle,
  AlertTriangle,
  CheckCircle2,
  Cpu,
  Download,
  FlaskConical,
  GitBranch,
  KeyRound,
  Play,
  RefreshCw,
  RotateCcw,
  ShieldAlert,
  Square,
  Trash2,
} from 'lucide-react';
import {
  DEFAULT_LLM_SYSTEM_PROMPT,
  type AddonId,
  type AddonNamePreference,
  type AddonSummary,
  type AppConfig,
  type ClientFilterConfig,
  type ClientFilterSourceMode,
  type DeployLocalFilterRequest,
  type ExclusionDisplayLevel,
  type FilterEvaluationTarget,
  type FilterExclusionRecord,
  type FilterGalleryAggregation,
  type FilterItemVerdict,
  type FilterModelParams,
  type FilterPresetId,
  type FilterScoreTarget,
  type FilterTokenStatus,
  type FlaggedItemAction,
  type LlmTokenSource,
  type LocalFilterDeploymentStatus,
  type UpsertFilterExclusionRequest,
} from '../../../api/ipc';
import {
  getPresetDefaultParams,
  getPresetRecommendedThresholds,
} from '../../../utils/contentFilter';
import { formatAddonDisplayName } from '../../../utils/format';
import {
  Button,
  IconButton,
  SegmentedButton,
  Select,
  Switch,
  TextField,
} from '../../m3/primitives';

export interface FilterSettingsSectionProps {
  effectiveConfig: AppConfig;
  dirty: boolean;
  busy: boolean;
  filterStatusBusy: boolean;
  localStatus: LocalFilterDeploymentStatus | null;
  tokenStatus: FilterTokenStatus | null;
  evalError: string | null;
  addons: AddonSummary[];
  namePreference: AddonNamePreference;
  exclusions: FilterExclusionRecord[];
  onUpdateFilter: (updater: (curr: ClientFilterConfig) => ClientFilterConfig) => void;
  onSaveDraftFirst: () => Promise<boolean>;
  onRefreshLocalStatus: () => Promise<LocalFilterDeploymentStatus | null>;
  onCheckLocalUpdates: () => Promise<LocalFilterDeploymentStatus | null>;
  onDeployLocalFilter: (
    request: DeployLocalFilterRequest,
  ) => Promise<LocalFilterDeploymentStatus | null>;
  onStartLocalService: () => Promise<LocalFilterDeploymentStatus | null>;
  onStopLocalService: () => Promise<LocalFilterDeploymentStatus | null>;
  onRefreshTokenStatus: () => Promise<FilterTokenStatus | null>;
  onSetLlmToken: (token: string | null) => Promise<FilterTokenStatus | null>;
  onClearFilterCache: () => Promise<number | null>;
  onEvaluateSandbox: (target: FilterEvaluationTarget) => Promise<FilterItemVerdict | null>;
  onUpsertExclusion: (request: UpsertFilterExclusionRequest) => Promise<boolean>;
  onDeleteExclusion: (addonId: AddonId) => Promise<boolean>;
  onSelectAddon?: (addon: AddonSummary) => void;
}

const PRESET_OPTIONS: FilterPresetId[] = [
  'recommended',
  'strict',
  'relaxed',
  'nsfw_only',
  'gore_only',
  'low_cpu',
  'custom',
];

const SCORE_TARGET_OPTIONS: FilterScoreTarget[] = [
  'overall',
  'nsfw',
  'gore',
  'max_category',
];

const FLAGGED_ACTION_OPTIONS: FlaggedItemAction[] = [
  'mark_only',
  'hide_image',
  'blur_card',
  'hide_card',
];

const EXCLUSION_LEVEL_OPTIONS: ExclusionDisplayLevel[] = [
  'normal',
  'lazy_load',
  'blur_image',
  'hide_image',
  'hide_text',
];

function parseClampedFloat(raw: string, fallback: number, min: number, max: number): number {
  const parsed = Number.parseFloat(raw);
  if (!Number.isFinite(parsed)) return fallback;
  return Number(Math.min(max, Math.max(min, parsed)).toFixed(4));
}

export function FilterSettingsSection({
  effectiveConfig,
  dirty,
  busy,
  filterStatusBusy,
  localStatus,
  tokenStatus,
  evalError,
  addons,
  namePreference,
  exclusions,
  onUpdateFilter,
  onSaveDraftFirst,
  onRefreshLocalStatus,
  onCheckLocalUpdates,
  onDeployLocalFilter,
  onStartLocalService,
  onStopLocalService,
  onRefreshTokenStatus,
  onSetLlmToken,
  onClearFilterCache,
  onEvaluateSandbox,
  onUpsertExclusion,
  onDeleteExclusion,
  onSelectAddon,
}: FilterSettingsSectionProps) {
  const { t } = useTranslation();
  const systemPromptId = useId();
  const sandboxDescId = useId();

  const filter = effectiveConfig.filter;
  const [applyPresetThresholds, setApplyPresetThresholds] = useState(true);
  const [tokenInput, setTokenInput] = useState('');

  const [sandboxTitle, setSandboxTitle] = useState('');
  const [sandboxDesc, setSandboxDesc] = useState('');
  const [sandboxImage, setSandboxImage] = useState('');
  const [sandboxBusy, setSandboxBusy] = useState(false);
  const [sandboxResult, setSandboxResult] = useState<FilterItemVerdict | null>(null);

  useEffect(() => {
    void onRefreshLocalStatus();
    void onRefreshTokenStatus();
  }, [onRefreshLocalStatus, onRefreshTokenStatus]);

  const addonsById = useMemo(() => {
    const map = new Map<string, AddonSummary>();
    for (const a of addons) map.set(a.id, a);
    return map;
  }, [addons]);

  const updateParams = (updater: (curr: FilterModelParams) => FilterModelParams) => {
    onUpdateFilter((curr) => ({
      ...curr,
      preset: 'custom',
      localParams: updater(curr.localParams),
    }));
  };

  const handleSelectPreset = (nextPreset: FilterPresetId) => {
    onUpdateFilter((curr) => {
      if (nextPreset === 'custom') {
        return { ...curr, preset: 'custom' };
      }
      const nextParams = getPresetDefaultParams(nextPreset);
      if (!applyPresetThresholds) {
        return { ...curr, preset: nextPreset, localParams: nextParams };
      }
      const recommended = getPresetRecommendedThresholds(nextPreset);
      return {
        ...curr,
        preset: nextPreset,
        localParams: nextParams,
        llmThreshold: recommended.llmThreshold,
        flagThreshold: recommended.flagThreshold,
      };
    });
  };

  const recentCommitOptions = useMemo(() => {
    const commits = localStatus?.recentCommits ?? [];
    return [
      ...commits.map((c) => ({
        value: c.sha,
        label: `${c.shortSha} · ${c.subject}`,
      })),
      { value: '__custom__', label: t('filter.settings.customCommitOption') },
    ];
  }, [localStatus?.recentCommits, t]);

  const matchedCommitSha = useMemo(() => {
    const pinned = filter.pinnedCommit?.trim();
    if (!pinned) return '__custom__';
    const hit = localStatus?.recentCommits.find(
      (c) => c.sha === pinned || c.shortSha === pinned,
    );
    return hit ? hit.sha : '__custom__';
  }, [filter.pinnedCommit, localStatus?.recentCommits]);

  const serviceRunning = localStatus?.serviceState === 'running';
  const serviceTransitioning =
    localStatus?.serviceState === 'deploying' || localStatus?.serviceState === 'starting';
  const catalogParamsLocked = filter.mode === 'catalog';

  const handleDeploy = async (forceLatest = false) => {
    if (dirty) {
      const saved = await onSaveDraftFirst();
      if (!saved) return;
    }
    const trackLatest = forceLatest ? true : filter.trackLatest;
    await onDeployLocalFilter({
      repoUrl: filter.repoUrl.trim() || undefined,
      trackLatest,
      targetCommit: trackLatest ? undefined : filter.pinnedCommit?.trim() || undefined,
      startAfterDeploy: filter.autoStartLocalService,
    });
  };

  const handleRunSandbox = async () => {
    if (!sandboxTitle.trim() && !sandboxDesc.trim() && !sandboxImage.trim()) {
      return;
    }
    setSandboxBusy(true);
    try {
      if (dirty) {
        const saved = await onSaveDraftFirst();
        if (!saved) return;
      }
      const verdict = await onEvaluateSandbox({
        targetKey: `sandbox:${Date.now()}`,
        title: sandboxTitle.trim() || 'Untitled Sandbox Sample',
        description: sandboxDesc.trim() || undefined,
        tags: [],
        imageSources: sandboxImage.trim() ? [sandboxImage.trim()] : [],
        bypassCache: true,
      });
      setSandboxResult(verdict);
    } finally {
      setSandboxBusy(false);
    }
  };

  return (
    <>
      {/* 1. Master Switch, Scope & Source */}
      <section className="l4a-section-card">
        <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
          <span className="md-title-medium">{t('filter.settings.scopeTitle')}</span>
          <Switch
            checked={filter.enabled}
            label={t('filter.settings.enabled')}
            onChange={(next) => onUpdateFilter((curr) => ({ ...curr, enabled: next }))}
          />
        </div>

        {evalError && filter.enabled && (
          <div className="l4a-callout" data-tone="error">
            <AlertCircle size={15} style={{ flexShrink: 0, marginTop: 2 }} />
            <span className="md-body-small selectable">{evalError}</span>
          </div>
        )}

        <div className="l4a-divider" />

        <div className="l4a-setting-row">
          <span className="md-title-small">{t('filter.settings.applyToWorkshop')}</span>
          <Switch
            checked={filter.applyToWorkshop}
            label={t('filter.settings.applyToWorkshop')}
            onChange={(next) => onUpdateFilter((curr) => ({ ...curr, applyToWorkshop: next }))}
          />
        </div>

        <div className="l4a-divider" />

        <div className="l4a-setting-row">
          <span className="md-title-small">{t('filter.settings.applyToLibrary')}</span>
          <Switch
            checked={filter.applyToLibrary}
            label={t('filter.settings.applyToLibrary')}
            onChange={(next) => onUpdateFilter((curr) => ({ ...curr, applyToLibrary: next }))}
          />
        </div>

        <div className="l4a-divider" />

        <div className="l4a-setting-row" style={{ flexWrap: 'wrap' }}>
          <span className="md-title-small">{t('filter.settings.sourceMode')}</span>
          <SegmentedButton<ClientFilterSourceMode>
            ariaLabel={t('filter.settings.sourceMode')}
            value={filter.mode}
            onChange={(next) => onUpdateFilter((curr) => ({ ...curr, mode: next }))}
            options={[
              { value: 'local', label: t('filter.settings.modes.local') },
              { value: 'catalog', label: t('filter.settings.modes.catalog') },
              { value: 'custom_endpoint', label: t('filter.settings.modes.custom_endpoint') },
            ]}
          />
        </div>

        {filter.mode === 'custom_endpoint' && (
          <TextField
            label={t('filter.settings.customEndpointUrl')}
            placeholder="http://127.0.0.1:18790/predict"
            value={filter.customEndpointUrl ?? ''}
            onChange={(e) =>
              onUpdateFilter((curr) => ({
                ...curr,
                customEndpointUrl: e.target.value || null,
              }))
            }
          />
        )}

        {filter.mode === 'catalog' && (
          <div className="l4a-callout">
            <Cpu size={15} style={{ flexShrink: 0, marginTop: 2 }} />
            <span className="md-body-small">
              {effectiveConfig.catalogServerUrl?.trim()
                ? `${t('filter.settings.catalogUrlHint')} (${effectiveConfig.catalogServerUrl})`
                : t('filter.settings.catalogUrlMissing')}
            </span>
          </div>
        )}
      </section>

      {/* 2. Local Model One-Click Deployment (Left4Filters) */}
      <section className="l4a-section-card">
        <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
          <div className="l4a-row-wrap" style={{ gap: 8 }}>
            <GitBranch size={17} style={{ color: 'var(--md-sys-color-primary)' }} />
            <span className="md-title-medium">{t('filter.settings.deployTitle')}</span>
          </div>
          <Button
            variant="text"
            size="sm"
            disabled={filterStatusBusy}
            icon={<RefreshCw size={14} className={filterStatusBusy ? 'md-spin' : undefined} />}
            onClick={() => void onRefreshLocalStatus()}
          >
            {t('filter.settings.refreshStatusBtn')}
          </Button>
        </div>

        <div className="md-chip-row">
          <span
            className="md-pill"
            data-tone={localStatus?.env.gitAvailable ? 'primary' : 'error'}
          >
            {localStatus?.env.gitAvailable
              ? t('filter.settings.gitReady', {
                  version: localStatus.env.gitVersion ?? 'OK',
                })
              : t('filter.settings.gitMissing')}
          </span>
          <span
            className="md-pill"
            data-tone={localStatus?.env.pythonAvailable ? 'primary' : 'error'}
          >
            {localStatus?.env.pythonAvailable
              ? t('filter.settings.pythonReady', {
                  version: localStatus.env.pythonVersion ?? 'OK',
                })
              : t('filter.settings.pythonMissing')}
          </span>
          <span
            className="md-pill"
            data-tone={localStatus?.env.venvAvailable ? 'primary' : 'error'}
          >
            {localStatus?.env.venvAvailable
              ? t('filter.settings.venvReady')
              : t('filter.settings.venvMissing')}
          </span>
        </div>

        <span
          className="md-body-small"
          style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
        >
          {t('filter.settings.envPrereqNote')}
        </span>

        {localStatus && (
          <div className="l4a-filter-box">
            <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
              <div className="l4a-row-wrap" style={{ gap: 8 }}>
                <span className="md-label-medium">{t('filter.settings.serviceStateLabel')}</span>
                <span
                  className="md-pill"
                  data-tone={
                    localStatus.serviceState === 'running'
                      ? 'primary'
                      : localStatus.serviceState === 'error'
                        ? 'error'
                        : serviceTransitioning
                          ? 'tertiary'
                          : 'surface'
                  }
                >
                  {t(`filter.settings.serviceState.${localStatus.serviceState}`)}
                </span>
                {!localStatus.deployed && (
                  <span className="md-pill" data-tone="surface">
                    {t('filter.settings.notDeployed')}
                  </span>
                )}
                {localStatus.updateAvailable && (
                  <span className="md-pill" data-tone="tertiary">
                    {t('filter.settings.updateAvailableBadge')}
                  </span>
                )}
              </div>
              {localStatus.serviceEndpoint && (
                <span
                  className="md-body-small md-mono"
                  style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                >
                  {localStatus.serviceEndpoint}
                  {localStatus.runningPid ? ` (PID ${localStatus.runningPid})` : ''}
                </span>
              )}
            </div>

            {(localStatus.currentCommit || localStatus.remoteLatestCommit) && (
              <div className="l4a-kv-grid">
                {localStatus.currentCommit && (
                  <div className="l4a-kv-item">
                    <span className="l4a-kv-label">{t('filter.settings.currentCommit')}</span>
                    <span className="l4a-kv-value md-mono">
                      {localStatus.currentCommit.shortSha} · {localStatus.currentCommit.subject}
                    </span>
                  </div>
                )}
                {localStatus.remoteLatestCommit && (
                  <div className="l4a-kv-item">
                    <span className="l4a-kv-label">{t('filter.settings.remoteLatestCommit')}</span>
                    <span className="l4a-kv-value md-mono">
                      {localStatus.remoteLatestCommit.shortSha} ·{' '}
                      {localStatus.remoteLatestCommit.subject}
                    </span>
                  </div>
                )}
              </div>
            )}

            {localStatus.stageMessage && (
              <span
                className="md-body-small"
                style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
              >
                {localStatus.stageMessage}
              </span>
            )}

            {localStatus.lastError && (
              <div className="l4a-callout" data-tone="error">
                <AlertCircle size={14} style={{ flexShrink: 0, marginTop: 2 }} />
                <span className="md-body-small selectable">{localStatus.lastError}</span>
              </div>
            )}
          </div>
        )}

        <TextField
          label={t('filter.settings.repoUrl')}
          value={filter.repoUrl}
          onChange={(e) => onUpdateFilter((curr) => ({ ...curr, repoUrl: e.target.value }))}
        />

        <div className="l4a-setting-row">
          <span className="md-title-small">{t('filter.settings.autoStartService')}</span>
          <Switch
            checked={filter.autoStartLocalService}
            label={t('filter.settings.autoStartService')}
            onChange={(next) =>
              onUpdateFilter((curr) => ({ ...curr, autoStartLocalService: next }))
            }
          />
        </div>

        <div className="l4a-divider" />

        <div className="l4a-setting-row">
          <span className="md-title-small">{t('filter.settings.trackLatest')}</span>
          <Switch
            checked={filter.trackLatest}
            label={t('filter.settings.trackLatest')}
            onChange={(next) => onUpdateFilter((curr) => ({ ...curr, trackLatest: next }))}
          />
        </div>

        {!filter.trackLatest && (
          <div className="l4a-filter-param-grid">
            <div className="md-field">
              <span className="md-field-label">{t('filter.settings.commitSelect')}</span>
              <Select<string>
                ariaLabel={t('filter.settings.commitSelect')}
                value={matchedCommitSha}
                onChange={(sha) => {
                  if (sha === '__custom__') return;
                  onUpdateFilter((curr) => ({ ...curr, pinnedCommit: sha }));
                }}
                options={recentCommitOptions}
              />
            </div>
            <TextField
              label={t('filter.settings.pinnedCommitInput')}
              placeholder="a94f18e..."
              value={filter.pinnedCommit ?? ''}
              onChange={(e) =>
                onUpdateFilter((curr) => ({
                  ...curr,
                  pinnedCommit: e.target.value || null,
                }))
              }
            />
          </div>
        )}

        <div className="l4a-row-wrap">
          <Button
            variant="filled"
            size="sm"
            disabled={filterStatusBusy || serviceTransitioning}
            icon={<Download size={14} />}
            onClick={() => void handleDeploy(false)}
          >
            {localStatus?.deployed
              ? t('filter.settings.redeployBtn')
              : t('filter.settings.deployBtn')}
          </Button>

          <Button
            variant="tonal"
            size="sm"
            disabled={filterStatusBusy || serviceTransitioning}
            icon={<RefreshCw size={14} className={filterStatusBusy ? 'md-spin' : undefined} />}
            onClick={() => void onCheckLocalUpdates()}
          >
            {t('filter.settings.checkUpdatesBtn')}
          </Button>

          {localStatus?.updateAvailable && (
            <Button
              variant="filled"
              size="sm"
              disabled={filterStatusBusy || serviceTransitioning}
              icon={<RefreshCw size={14} />}
              onClick={() => void handleDeploy(true)}
            >
              {t('filter.settings.updateNowBtn')}
            </Button>
          )}

          {serviceRunning ? (
            <Button
              variant="outlined"
              tone="error"
              size="sm"
              disabled={filterStatusBusy}
              icon={<Square size={13} />}
              onClick={() => void onStopLocalService()}
            >
              {t('filter.settings.stopServiceBtn')}
            </Button>
          ) : (
            <Button
              variant="outlined"
              size="sm"
              disabled={filterStatusBusy || !localStatus?.deployed || serviceTransitioning}
              icon={<Play size={14} />}
              onClick={() => void onStartLocalService()}
            >
              {t('filter.settings.startServiceBtn')}
            </Button>
          )}
        </div>
      </section>

      {/* 3. Local Model Parameters & Calibration Presets */}
      <section className="l4a-section-card">
        <span className="md-title-medium">{t('filter.settings.paramsTitle')}</span>

        {catalogParamsLocked && (
          <div className="l4a-callout">
            <AlertTriangle
              size={16}
              style={{ color: 'var(--md-sys-color-tertiary)', flexShrink: 0, marginTop: 2 }}
            />
            <span className="md-body-small">{t('filter.settings.catalogParamsBanner')}</span>
          </div>
        )}

        <div className="l4a-setting-row" style={{ flexWrap: 'wrap' }}>
          <span className="md-title-small">{t('filter.settings.preset')}</span>
          <Select<FilterPresetId>
            ariaLabel={t('filter.settings.preset')}
            value={filter.preset}
            onChange={handleSelectPreset}
            options={PRESET_OPTIONS.map((p) => ({
              value: p,
              label: t(`filter.settings.presets.${p}`),
            }))}
          />
        </div>

        <div className="l4a-setting-row">
          <span className="md-body-medium">{t('filter.settings.applyPresetThresholds')}</span>
          <Switch
            compact
            checked={applyPresetThresholds}
            label={t('filter.settings.applyPresetThresholds')}
            onChange={setApplyPresetThresholds}
          />
        </div>

        <div className="l4a-divider" />

        <div className="l4a-setting-row">
          <span className="md-title-small">{t('filter.settings.enableText')}</span>
          <Switch
            checked={filter.localParams.enableText}
            disabled={catalogParamsLocked}
            label={t('filter.settings.enableText')}
            onChange={(next) => updateParams((p) => ({ ...p, enableText: next }))}
          />
        </div>

        <div className="l4a-setting-row">
          <span className="md-title-small">{t('filter.settings.enableImage')}</span>
          <Switch
            checked={filter.localParams.enableImage}
            disabled={catalogParamsLocked}
            label={t('filter.settings.enableImage')}
            onChange={(next) => updateParams((p) => ({ ...p, enableImage: next }))}
          />
        </div>

        <div className="l4a-setting-row" style={{ flexWrap: 'wrap' }}>
          <span className="md-title-small">{t('filter.settings.galleryAggregation')}</span>
          <SegmentedButton<FilterGalleryAggregation>
            ariaLabel={t('filter.settings.galleryAggregation')}
            value={filter.localParams.galleryAggregation}
            onChange={(next) => updateParams((p) => ({ ...p, galleryAggregation: next }))}
            options={[
              {
                value: 'max',
                label: t('filter.settings.aggregations.max'),
                disabled: catalogParamsLocked,
              },
              {
                value: 'top2_mean',
                label: t('filter.settings.aggregations.top2_mean'),
                disabled: catalogParamsLocked,
              },
              {
                value: 'mean',
                label: t('filter.settings.aggregations.mean'),
                disabled: catalogParamsLocked,
              },
            ]}
          />
        </div>

        <div className="l4a-filter-param-grid">
          <TextField
            label={t('filter.settings.threads')}
            type="number"
            min={1}
            max={16}
            step={1}
            disabled={catalogParamsLocked}
            value={filter.localParams.threads}
            onChange={(e) =>
              updateParams((p) => ({
                ...p,
                threads: Math.min(16, Math.max(1, Number.parseInt(e.target.value || '4', 10) || 4)),
              }))
            }
          />
          <TextField
            label={t('filter.settings.maxImages')}
            type="number"
            min={1}
            max={8}
            step={1}
            disabled={catalogParamsLocked}
            value={filter.localParams.maxImages}
            onChange={(e) =>
              updateParams((p) => ({
                ...p,
                maxImages: Math.min(
                  8,
                  Math.max(1, Number.parseInt(e.target.value || '2', 10) || 2),
                ),
              }))
            }
          />
          <TextField
            label={t('filter.settings.goreWeight')}
            type="number"
            min={0}
            max={3}
            step={0.05}
            disabled={catalogParamsLocked}
            value={filter.localParams.goreWeight}
            onChange={(e) =>
              updateParams((p) => ({
                ...p,
                goreWeight: parseClampedFloat(e.target.value, p.goreWeight, 0, 3),
              }))
            }
          />
          <TextField
            label={t('filter.settings.nsfwWeight')}
            type="number"
            min={0}
            max={3}
            step={0.05}
            disabled={catalogParamsLocked}
            value={filter.localParams.nsfwWeight}
            onChange={(e) =>
              updateParams((p) => ({
                ...p,
                nsfwWeight: parseClampedFloat(e.target.value, p.nsfwWeight, 0, 3),
              }))
            }
          />
          <TextField
            label={t('filter.settings.temperature')}
            type="number"
            min={0.25}
            max={4}
            step={0.05}
            disabled={catalogParamsLocked}
            value={filter.localParams.temperature}
            onChange={(e) =>
              updateParams((p) => ({
                ...p,
                temperature: parseClampedFloat(e.target.value, p.temperature, 0.25, 4),
              }))
            }
          />
          <TextField
            label={t('filter.settings.bias')}
            type="number"
            min={-5}
            max={5}
            step={0.05}
            disabled={catalogParamsLocked}
            value={filter.localParams.bias}
            onChange={(e) =>
              updateParams((p) => ({
                ...p,
                bias: parseClampedFloat(e.target.value, p.bias, -5, 5),
              }))
            }
          />
        </div>
      </section>

      {/* 4. Threshold Routing & External OpenAI LLM Judge */}
      <section className="l4a-section-card">
        <span className="md-title-medium">{t('filter.settings.llmTitle')}</span>

        <div className="l4a-setting-row" style={{ flexWrap: 'wrap' }}>
          <span className="md-title-small">{t('filter.settings.scoreTarget')}</span>
          <Select<FilterScoreTarget>
            ariaLabel={t('filter.settings.scoreTarget')}
            value={filter.scoreTarget}
            onChange={(next) => onUpdateFilter((curr) => ({ ...curr, scoreTarget: next }))}
            options={SCORE_TARGET_OPTIONS.map((st) => ({
              value: st,
              label: t(`filter.settings.scoreTargets.${st}`),
            }))}
          />
        </div>

        <div className="l4a-filter-param-grid">
          <div className="l4a-filter-box">
            <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
              <span className="md-title-small">{t('filter.settings.enableDirectFlag')}</span>
              <Switch
                compact
                checked={filter.enableDirectFlag}
                label={t('filter.settings.enableDirectFlag')}
                onChange={(next) =>
                  onUpdateFilter((curr) => ({ ...curr, enableDirectFlag: next }))
                }
              />
            </div>
            <TextField
              label={t('filter.settings.flagThreshold')}
              type="number"
              min={0.01}
              max={1}
              step={0.01}
              disabled={!filter.enableDirectFlag}
              value={filter.flagThreshold}
              onChange={(e) =>
                onUpdateFilter((curr) => ({
                  ...curr,
                  flagThreshold: parseClampedFloat(e.target.value, curr.flagThreshold, 0.01, 1),
                }))
              }
            />
          </div>

          <div className="l4a-filter-box">
            <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
              <span className="md-title-small">{t('filter.settings.llmEnabled')}</span>
              <Switch
                compact
                checked={filter.llmEnabled}
                label={t('filter.settings.llmEnabled')}
                onChange={(next) => onUpdateFilter((curr) => ({ ...curr, llmEnabled: next }))}
              />
            </div>
            <TextField
              label={t('filter.settings.llmThreshold')}
              type="number"
              min={0}
              max={1}
              step={0.01}
              disabled={!filter.llmEnabled}
              value={filter.llmThreshold}
              onChange={(e) =>
                onUpdateFilter((curr) => ({
                  ...curr,
                  llmThreshold: parseClampedFloat(e.target.value, curr.llmThreshold, 0, 1),
                }))
              }
            />
          </div>
        </div>

        <span
          className="md-body-small"
          style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
        >
          {t('filter.settings.routingHint')}
        </span>

        <div className="l4a-divider" />

        <div className="l4a-filter-param-grid">
          <TextField
            label={t('filter.settings.llmEndpoint')}
            placeholder="https://api.openai.com/v1"
            value={filter.llmEndpoint}
            onChange={(e) => onUpdateFilter((curr) => ({ ...curr, llmEndpoint: e.target.value }))}
          />
          <TextField
            label={t('filter.settings.llmModel')}
            placeholder="gpt-4o-mini"
            value={filter.llmModel}
            onChange={(e) => onUpdateFilter((curr) => ({ ...curr, llmModel: e.target.value }))}
          />
        </div>

        <div className="l4a-setting-row">
          <span className="md-title-small">{t('filter.settings.llmSendImages')}</span>
          <Switch
            checked={filter.llmSendImages}
            label={t('filter.settings.llmSendImages')}
            onChange={(next) => onUpdateFilter((curr) => ({ ...curr, llmSendImages: next }))}
          />
        </div>

        <div className="l4a-setting-row" style={{ flexWrap: 'wrap' }}>
          <span className="md-title-small">{t('filter.settings.llmTokenSource')}</span>
          <SegmentedButton<LlmTokenSource>
            ariaLabel={t('filter.settings.llmTokenSource')}
            value={filter.llmTokenSource}
            onChange={(next) => onUpdateFilter((curr) => ({ ...curr, llmTokenSource: next }))}
            options={[
              { value: 'env_var', label: t('filter.settings.tokenSources.env_var') },
              {
                value: 'encrypted_local',
                label: t('filter.settings.tokenSources.encrypted_local'),
              },
            ]}
          />
        </div>

        {filter.llmTokenSource === 'env_var' ? (
          <div className="l4a-filter-box">
            <TextField
              label={t('filter.settings.llmTokenEnvVar')}
              placeholder="L4A_LLM_API_KEY"
              value={filter.llmTokenEnvVar}
              onChange={(e) =>
                onUpdateFilter((curr) => ({ ...curr, llmTokenEnvVar: e.target.value }))
              }
              trailing={
                <Button
                  variant="text"
                  size="sm"
                  icon={<RefreshCw size={13} />}
                  onClick={() => void onRefreshTokenStatus()}
                >
                  {t('settings.refreshSdk')}
                </Button>
              }
            />
            <div className="l4a-row-wrap">
              <span
                className="md-pill"
                data-tone={tokenStatus?.envVarPresent ? 'primary' : 'surface'}
              >
                {tokenStatus?.envVarPresent ? (
                  <CheckCircle2 size={12} />
                ) : (
                  <KeyRound size={12} />
                )}
                {tokenStatus?.envVarPresent
                  ? t('filter.settings.envVarPresent')
                  : t('filter.settings.envVarMissing')}
              </span>
            </div>
          </div>
        ) : (
          <div className="l4a-filter-box">
            <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
              <span
                className="md-pill"
                data-tone={tokenStatus?.hasEncryptedToken ? 'primary' : 'surface'}
              >
                <KeyRound size={12} />
                {tokenStatus?.hasEncryptedToken
                  ? t('filter.settings.tokenStoredPill')
                  : t('filter.settings.tokenEmptyPill')}
              </span>
            </div>
            <TextField
              type="password"
              autoComplete="off"
              label={t('filter.settings.encryptedTokenInput')}
              placeholder={t('filter.settings.encryptedTokenPlaceholder')}
              value={tokenInput}
              onChange={(e) => setTokenInput(e.target.value)}
            />
            <div className="l4a-row-wrap">
              <Button
                variant="tonal"
                size="sm"
                disabled={busy || !tokenInput.trim()}
                icon={<KeyRound size={14} />}
                onClick={async () => {
                  const res = await onSetLlmToken(tokenInput.trim());
                  if (res) setTokenInput('');
                }}
              >
                {t('filter.settings.saveTokenBtn')}
              </Button>
              {tokenStatus?.hasEncryptedToken && (
                <Button
                  variant="text"
                  tone="error"
                  size="sm"
                  disabled={busy}
                  icon={<Trash2 size={14} />}
                  onClick={() => void onSetLlmToken(null)}
                >
                  {t('filter.settings.clearTokenBtn')}
                </Button>
              )}
            </div>
            <span
              className="md-body-small"
              style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
            >
              {t('filter.settings.encryptedTokenNote')}
            </span>
          </div>
        )}

        <div className="md-field">
          <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
            <label htmlFor={systemPromptId} className="md-field-label">
              {t('filter.settings.llmSystemPrompt')}
            </label>
            <Button
              variant="text"
              size="sm"
              icon={<RotateCcw size={13} />}
              onClick={() =>
                onUpdateFilter((curr) => ({
                  ...curr,
                  llmSystemPrompt: DEFAULT_LLM_SYSTEM_PROMPT,
                }))
              }
            >
              {t('filter.settings.resetSystemPrompt')}
            </Button>
          </div>
          <textarea
            id={systemPromptId}
            className="md-field-textarea"
            rows={4}
            value={filter.llmSystemPrompt}
            onChange={(e) =>
              onUpdateFilter((curr) => ({ ...curr, llmSystemPrompt: e.target.value }))
            }
          />
        </div>

        <span
          className="md-body-small"
          style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
        >
          {t('filter.settings.builtinUserPromptNote')}
        </span>
      </section>

      {/* 5. Flagged Actions, Modality Subdivision, Sandbox & Exclusions Overview */}
      <section className="l4a-section-card">
        <span className="md-title-medium">{t('filter.settings.actionsTitle')}</span>

        <div className="l4a-setting-row" style={{ flexWrap: 'wrap' }}>
          <span className="md-title-small">{t('filter.settings.flaggedAction')}</span>
          <Select<FlaggedItemAction>
            ariaLabel={t('filter.settings.flaggedAction')}
            value={filter.flaggedAction}
            onChange={(next) => onUpdateFilter((curr) => ({ ...curr, flaggedAction: next }))}
            options={FLAGGED_ACTION_OPTIONS.map((act) => ({
              value: act,
              label: t(`filter.settings.actions.${act}`),
            }))}
          />
        </div>

        <span
          className="md-body-small"
          style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
        >
          {t('filter.settings.detailAlwaysBlurNote')}
        </span>

        <div className="l4a-divider" />

        <div className="l4a-setting-row">
          <span className="md-title-small">{t('filter.settings.subdivideModalities')}</span>
          <Switch
            checked={filter.subdivideModalities}
            label={t('filter.settings.subdivideModalities')}
            onChange={(next) =>
              onUpdateFilter((curr) => ({ ...curr, subdivideModalities: next }))
            }
          />
        </div>

        <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
          <div style={{ flex: '1 1 220px', maxWidth: 280 }}>
            <TextField
              label={t('filter.settings.modalityThreshold')}
              type="number"
              min={0.01}
              max={1}
              step={0.01}
              disabled={!filter.subdivideModalities}
              value={filter.modalityThreshold}
              onChange={(e) =>
                onUpdateFilter((curr) => ({
                  ...curr,
                  modalityThreshold: parseClampedFloat(
                    e.target.value,
                    curr.modalityThreshold,
                    0.01,
                    1,
                  ),
                }))
              }
            />
          </div>
          <Button
            variant="outlined"
            size="sm"
            disabled={busy}
            icon={<Trash2 size={14} />}
            onClick={() => void onClearFilterCache()}
          >
            {t('filter.settings.clearCacheBtn')}
          </Button>
        </div>

        <div className="l4a-divider" />

        {/* Quick Test Sandbox */}
        <div className="l4a-filter-box">
          <div className="l4a-row-wrap" style={{ gap: 8 }}>
            <FlaskConical size={16} style={{ color: 'var(--md-sys-color-primary)' }} />
            <span className="md-title-small">{t('filter.settings.sandboxHeading')}</span>
          </div>

          <div className="l4a-filter-param-grid">
            <TextField
              label={t('filter.settings.sandboxTitleInput')}
              value={sandboxTitle}
              onChange={(e) => setSandboxTitle(e.target.value)}
            />
            <TextField
              label={t('filter.settings.sandboxImageInput')}
              placeholder="https://..."
              value={sandboxImage}
              onChange={(e) => setSandboxImage(e.target.value)}
            />
          </div>

          <div className="md-field">
            <label htmlFor={sandboxDescId} className="md-field-label">
              {t('filter.settings.sandboxDescInput')}
            </label>
            <textarea
              id={sandboxDescId}
              className="md-field-textarea"
              rows={2}
              value={sandboxDesc}
              onChange={(e) => setSandboxDesc(e.target.value)}
            />
          </div>

          <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
            <Button
              variant="tonal"
              size="sm"
              disabled={
                sandboxBusy ||
                (!sandboxTitle.trim() && !sandboxDesc.trim() && !sandboxImage.trim())
              }
              icon={<Play size={14} />}
              onClick={() => void handleRunSandbox()}
            >
              {sandboxBusy
                ? t('filter.settings.sandboxRunning')
                : dirty
                  ? t('filter.settings.sandboxSaveAndRunBtn')
                  : t('filter.settings.sandboxRunBtn')}
            </Button>
          </div>

          {sandboxResult && (
            <div className="l4a-callout">
              <ShieldAlert
                size={15}
                style={{
                  color: sandboxResult.flagged
                    ? 'var(--md-sys-color-error)'
                    : 'var(--md-sys-color-primary)',
                  flexShrink: 0,
                  marginTop: 2,
                }}
              />
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6, flex: 1 }}>
                <div className="l4a-row-wrap" style={{ gap: 6 }}>
                  <span
                    className="md-pill"
                    data-tone={sandboxResult.flagged ? 'error' : 'primary'}
                  >
                    {sandboxResult.flagged
                      ? t('filter.settings.sandboxVerdictFlagged')
                      : t('filter.settings.sandboxVerdictSafe')}
                  </span>
                  <span className="md-pill" data-tone="secondary">
                    {t(`filter.decisionSource.${sandboxResult.decisionSource}`)}
                  </span>
                  {sandboxResult.imageSensitive && (
                    <span className="md-pill" data-tone="tertiary">
                      [{t('filter.modImageShort')}]
                    </span>
                  )}
                  {sandboxResult.textSensitive && (
                    <span className="md-pill" data-tone="tertiary">
                      [{t('filter.modTextShort')}]
                    </span>
                  )}
                </div>
                <div className="md-body-small md-mono selectable">
                  {[
                    `score=${sandboxResult.evaluatedScore.toFixed(3)}`,
                    `prob=${sandboxResult.probabilities.prob.toFixed(3)}`,
                    `nsfw=${sandboxResult.probabilities.nsfwProb.toFixed(3)}`,
                    `gore=${sandboxResult.probabilities.goreProb.toFixed(3)}`,
                    sandboxResult.probabilities.textProb !== undefined
                      ? `text=${sandboxResult.probabilities.textProb.toFixed(3)}`
                      : null,
                    sandboxResult.probabilities.imageProb !== undefined
                      ? `image=${sandboxResult.probabilities.imageProb.toFixed(3)}`
                      : null,
                  ]
                    .filter(Boolean)
                    .join(' · ')}
                </div>
              </div>
            </div>
          )}
        </div>

        <div className="l4a-divider" />

        {/* Library Exclusions Overview */}
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          <span className="md-title-small">
            {t('filter.settings.exclusionsHeading', { count: exclusions.length })}
          </span>

          {exclusions.length === 0 ? (
            <span
              className="md-body-small"
              style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
            >
              {t('filter.settings.exclusionsEmpty')}
            </span>
          ) : (
            <div className="l4a-list">
              {exclusions.map((excl) => {
                const addon = addonsById.get(excl.addonId);
                const label = addon
                  ? formatAddonDisplayName(addon, namePreference)
                  : excl.addonId;

                return (
                  <div
                    key={excl.addonId}
                    className="l4a-list-row l4a-list-row-dense"
                    style={{ cursor: addon && onSelectAddon ? 'pointer' : 'default' }}
                  >
                    <button
                      type="button"
                      className="l4a-card-trigger l4a-list-main"
                      disabled={!addon || !onSelectAddon}
                      onClick={() => {
                        if (addon && onSelectAddon) onSelectAddon(addon);
                      }}
                    >
                      <div className="l4a-row-wrap" style={{ gap: 6 }}>
                        <span className="md-title-small l4a-truncate">{label}</span>
                        {excl.bindFingerprint ? (
                          excl.fingerprintValid ? (
                            <span className="md-pill md-mono" data-tone="primary">
                              {t('filter.settings.fingerprintValidPill', {
                                hash: excl.boundFingerprint?.slice(0, 8) ?? '—',
                              })}
                            </span>
                          ) : (
                            <span className="md-pill" data-tone="error">
                              <AlertTriangle size={11} />
                              {t('filter.settings.fingerprintInvalidBadge')}
                            </span>
                          )
                        ) : (
                          <span className="md-pill" data-tone="surface">
                            {t('filter.settings.fingerprintUnboundPill')}
                          </span>
                        )}
                      </div>
                      <span
                        className="md-body-small md-mono"
                        style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                      >
                        {excl.addonId}
                      </span>
                    </button>

                    <div className="l4a-list-trailing">
                      <Select<ExclusionDisplayLevel>
                        ariaLabel={t('filter.exclusionLevelLabel')}
                        value={excl.displayLevel}
                        onChange={(nextLevel) =>
                          void onUpsertExclusion({
                            addonId: excl.addonId,
                            displayLevel: nextLevel,
                            bindFingerprint: excl.bindFingerprint,
                            note: excl.note,
                          })
                        }
                        options={EXCLUSION_LEVEL_OPTIONS.map((lvl) => ({
                          value: lvl,
                          label: t(`filter.levels.${lvl}`),
                        }))}
                      />

                      {excl.bindFingerprint && !excl.fingerprintValid && addon?.contentHash && (
                        <Button
                          variant="tonal"
                          size="sm"
                          disabled={busy}
                          icon={<RefreshCw size={13} />}
                          onClick={() =>
                            void onUpsertExclusion({
                              addonId: excl.addonId,
                              displayLevel: excl.displayLevel,
                              bindFingerprint: true,
                              note: excl.note,
                            })
                          }
                        >
                          {t('filter.settings.rebindShort')}
                        </Button>
                      )}

                      <IconButton
                        size="sm"
                        label={t('common.remove')}
                        disabled={busy}
                        onClick={() => void onDeleteExclusion(excl.addonId)}
                      >
                        <Trash2 size={15} />
                      </IconButton>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </section>
    </>
  );
}
