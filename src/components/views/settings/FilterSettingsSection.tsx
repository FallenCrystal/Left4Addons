import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AlertCircle,
  AlertTriangle,
  CheckCircle2,
  Cpu,
  Download,
  Eye,
  EyeOff,
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
  type FilterFailureDisplay,
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
import { useContentFilterContext } from '../../../state/ContentFilterContext';
import type { FilterEvaluationLogEntry } from '../../../state/controller/useContentFilter';
import {
  getPresetDefaultParams,
  getPresetRecommendedThresholds,
  isAddonTitleRedacted,
  isAutomaticFilterTargetEnabled,
} from '../../../utils/contentFilter';
import { formatAddonDisplayName, formatTimestamp } from '../../../utils/format';
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
  evaluationLog: readonly FilterEvaluationLogEntry[];
  addons: AddonSummary[];
  namePreference: AddonNamePreference;
  exclusions: FilterExclusionRecord[];
  onUpdateFilter: (updater: (curr: ClientFilterConfig) => ClientFilterConfig) => void;
  onSaveDraftFirst: () => Promise<AppConfig | null>;
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
  onRetryFailedEvaluations: () => void;
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

function clampNumber(
  raw: string,
  fallback: number,
  min: number,
  max: number,
  integer: boolean,
): number {
  const trimmed = raw.trim();
  const pattern = integer ? /^-?\d+$/ : /^-?(?:\d+(?:\.\d+)?|\.\d+)$/;
  if (!pattern.test(trimmed)) return fallback;
  const parsed = Number.parseFloat(trimmed);
  if (!Number.isFinite(parsed)) return fallback;
  const clamped = Math.min(max, Math.max(min, parsed));
  if (integer) return Math.round(clamped);
  const rounded = Number(clamped.toFixed(4));
  return Object.is(rounded, -0) ? 0 : rounded;
}

function NumericTextField({
  label,
  value,
  min,
  max,
  integer = false,
  disabled = false,
  onCommit,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  integer?: boolean;
  disabled?: boolean;
  onCommit: (next: number) => void;
}) {
  const [focused, setFocused] = useState(false);
  const [draftText, setDraftText] = useState<string | null>(null);
  const [trackedValue, setTrackedValue] = useState(value);

  if (trackedValue !== value) {
    setTrackedValue(value);
    if (!focused && draftText !== null) {
      setDraftText(null);
    }
  }

  const displayValue = draftText ?? String(value);

  return (
    <TextField
      label={label}
      type="text"
      inputMode={integer ? 'numeric' : 'decimal'}
      disabled={disabled}
      value={displayValue}
      onFocus={() => {
        setFocused(true);
        setDraftText((curr) => curr ?? String(value));
      }}
      onChange={(e) => {
        const raw = e.target.value;
        setDraftText(raw);
        const trimmed = raw.trim();
        const completePattern = integer ? /^-?\d+$/ : /^-?(?:\d+(?:\.\d+)?|\.\d+)$/;
        if (completePattern.test(trimmed)) {
          const parsed = Number.parseFloat(trimmed);
          if (Number.isFinite(parsed) && parsed >= min && parsed <= max) {
            const normalized = integer
              ? Math.round(parsed)
              : Object.is(parsed, -0)
                ? 0
                : Number(parsed.toFixed(4));
            setTrackedValue(normalized);
            if (normalized !== value) {
              onCommit(normalized);
            }
          }
        }
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter' && draftText !== null) {
          const clamped = clampNumber(draftText, value, min, max, integer);
          setDraftText(String(clamped));
          setTrackedValue(clamped);
          if (clamped !== value) {
            onCommit(clamped);
          }
        }
      }}
      onBlur={() => {
        setFocused(false);
        if (draftText !== null) {
          const clamped = clampNumber(draftText, value, min, max, integer);
          setDraftText(null);
          setTrackedValue(clamped);
          if (clamped !== value) {
            onCommit(clamped);
          }
        }
      }}
    />
  );
}

export function FilterSettingsSection({
  effectiveConfig,
  dirty,
  busy,
  filterStatusBusy,
  localStatus,
  tokenStatus,
  evaluationLog,
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
  onRetryFailedEvaluations,
  onEvaluateSandbox,
  onUpsertExclusion,
  onDeleteExclusion,
  onSelectAddon,
}: FilterSettingsSectionProps) {
  const { t } = useTranslation();
  const filterCtx = useContentFilterContext();
  const systemPromptId = useId();
  const sandboxDescId = useId();

  const filter = effectiveConfig.filter;
  const hasFailedTargets = Array.from(filterCtx.failedTargetKeys).some((targetKey) =>
    isAutomaticFilterTargetEnabled(targetKey, filterCtx.filterConfig),
  );
  const filterRef = useRef(filter);
  useEffect(() => {
    filterRef.current = filter;
  }, [filter]);

  const [applyPresetThresholds, setApplyPresetThresholds] = useState(true);
  const [tokenInput, setTokenInput] = useState('');
  const [revealedExclusionIds, setRevealedExclusionIds] = useState<Set<string>>(new Set());

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
  const customEndpoint = filter.mode === 'custom_endpoint';
  const missingPinnedCommit = !filter.trackLatest && !filter.pinnedCommit?.trim();

  const handleDeploy = async (forceLatest = false) => {
    if (!forceLatest && missingPinnedCommit) return;
    let currentFilter = filterRef.current;
    if (dirty) {
      const saved = await onSaveDraftFirst();
      if (!saved) return;
      currentFilter = saved.filter;
    }
    const trackLatest = forceLatest ? true : currentFilter.trackLatest;
    await onDeployLocalFilter({
      repoUrl: currentFilter.repoUrl.trim() || undefined,
      trackLatest,
      targetCommit: trackLatest ? undefined : currentFilter.pinnedCommit?.trim() || undefined,
      startAfterDeploy: currentFilter.autoStartLocalService,
    });
  };

  const handleCheckLocalUpdates = async () => {
    if (dirty) {
      const saved = await onSaveDraftFirst();
      if (!saved) return;
    }
    await onCheckLocalUpdates();
  };

  const handleStartLocalService = async () => {
    if (dirty) {
      const saved = await onSaveDraftFirst();
      if (!saved) return;
    }
    await onStartLocalService();
  };

  const handleRefreshTokenStatus = async () => {
    if (dirty) {
      const saved = await onSaveDraftFirst();
      if (!saved) return;
    }
    await onRefreshTokenStatus();
  };

  const toggleRevealExclusionTitle = (addonId: string) => {
    setRevealedExclusionIds((prev) => {
      const next = new Set(prev);
      if (next.has(addonId)) next.delete(addonId);
      else next.add(addonId);
      return next;
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
        title: sandboxTitle.trim(),
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

        <div className="l4a-divider" />

        <details className="l4a-filter-log">
          <summary className="md-title-small">{t('filter.settings.logsTitle')}</summary>
          <div className="l4a-filter-log-body">
            {evaluationLog.length > 0 ? (
              <ul className="l4a-filter-log-entries">
                {evaluationLog.map((entry) => (
                  <li key={entry.id} className="md-body-small selectable">
                    <div className="l4a-row-wrap md-mono">
                      <time dateTime={entry.at}>{formatTimestamp(entry.at)}</time>
                      <span>{entry.targetKey}</span>
                    </div>
                    {entry.status === 'incomplete'
                      ? t('filter.evalIncompleteDetail')
                      : entry.error || t('filter.evalFailed')}
                  </li>
                ))}
              </ul>
            ) : (
              <span className="md-body-small">{t('filter.settings.logsEmpty')}</span>
            )}
            {hasFailedTargets && (
              <Button
                variant="text"
                size="sm"
                disabled={busy || filterStatusBusy}
                onClick={onRetryFailedEvaluations}
              >
                {t('filter.settings.recheckFailed')}
              </Button>
            )}
          </div>
        </details>
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
          <>
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
            {missingPinnedCommit && (
              <span
                className="md-body-small"
                style={{ color: 'var(--md-sys-color-error)' }}
              >
                {t('filter.settings.pinnedCommitRequiredHint')}
              </span>
            )}
          </>
        )}

        <div className="l4a-row-wrap">
          <Button
            variant="filled"
            size="sm"
            disabled={busy || filterStatusBusy || serviceTransitioning || missingPinnedCommit}
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
            disabled={busy || filterStatusBusy || serviceTransitioning}
            icon={<RefreshCw size={14} className={filterStatusBusy ? 'md-spin' : undefined} />}
            onClick={() => void handleCheckLocalUpdates()}
          >
            {t('filter.settings.checkUpdatesBtn')}
          </Button>

          {localStatus?.updateAvailable && (
            <Button
              variant="filled"
              size="sm"
              disabled={busy || filterStatusBusy || serviceTransitioning}
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
              disabled={busy || filterStatusBusy || serviceTransitioning}
              icon={<Square size={13} />}
              onClick={() => void onStopLocalService()}
            >
              {t('filter.settings.stopServiceBtn')}
            </Button>
          ) : (
            <Button
              variant="outlined"
              size="sm"
              disabled={busy || filterStatusBusy || !localStatus?.deployed || serviceTransitioning}
              icon={<Play size={14} />}
              onClick={() => void handleStartLocalService()}
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

        {!customEndpoint && (
          <>
            <div className="l4a-setting-row" style={{ flexWrap: 'wrap' }}>
              <span className="md-title-small">{t('filter.settings.preset')}</span>
              <Select<FilterPresetId>
                ariaLabel={t('filter.settings.preset')}
                value={filter.preset}
                disabled={catalogParamsLocked}
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
                disabled={catalogParamsLocked}
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
          </>
        )}
        {customEndpoint && (
          <p className="md-body-small">{t('filter.settings.customEndpointParamsHint')}</p>
        )}

        <div className="l4a-filter-param-grid">
          {!customEndpoint && (
            <NumericTextField
              label={t('filter.settings.threads')}
              min={1}
              max={16}
              integer
              disabled={catalogParamsLocked}
              value={filter.localParams.threads}
              onCommit={(threads) => updateParams((p) => ({ ...p, threads }))}
            />
          )}
          <NumericTextField
            label={t('filter.settings.maxImages')}
            min={1}
            max={8}
            integer
            disabled={catalogParamsLocked}
            value={filter.localParams.maxImages}
            onCommit={(maxImages) => updateParams((p) => ({ ...p, maxImages }))}
          />
          {!customEndpoint && (
            <>
              <NumericTextField
                label={t('filter.settings.goreWeight')}
                min={0}
                max={3}
                disabled={catalogParamsLocked}
                value={filter.localParams.goreWeight}
                onCommit={(goreWeight) => updateParams((p) => ({ ...p, goreWeight }))}
              />
              <NumericTextField
                label={t('filter.settings.nsfwWeight')}
                min={0}
                max={3}
                disabled={catalogParamsLocked}
                value={filter.localParams.nsfwWeight}
                onCommit={(nsfwWeight) => updateParams((p) => ({ ...p, nsfwWeight }))}
              />
              <NumericTextField
                label={t('filter.settings.temperature')}
                min={0.25}
                max={4}
                disabled={catalogParamsLocked}
                value={filter.localParams.temperature}
                onCommit={(temperature) => updateParams((p) => ({ ...p, temperature }))}
              />
              <NumericTextField
                label={t('filter.settings.bias')}
                min={-5}
                max={5}
                disabled={catalogParamsLocked}
                value={filter.localParams.bias}
                onCommit={(bias) => updateParams((p) => ({ ...p, bias }))}
              />
            </>
          )}
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
            <NumericTextField
              label={t('filter.settings.flagThreshold')}
              min={0.01}
              max={1}
              disabled={!filter.enableDirectFlag}
              value={filter.flagThreshold}
              onCommit={(flagThreshold) =>
                onUpdateFilter((curr) => ({ ...curr, flagThreshold }))
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
            <NumericTextField
              label={t('filter.settings.llmThreshold')}
              min={0}
              max={1}
              disabled={!filter.llmEnabled}
              value={filter.llmThreshold}
              onCommit={(llmThreshold) =>
                onUpdateFilter((curr) => ({ ...curr, llmThreshold }))
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

        {filter.enableDirectFlag &&
          filter.llmEnabled &&
          filter.llmThreshold >= filter.flagThreshold && (
            <div className="l4a-callout">
              <AlertTriangle
                size={15}
                style={{ color: 'var(--md-sys-color-tertiary)', flexShrink: 0, marginTop: 2 }}
              />
              <span className="md-body-small">
                {t('filter.settings.routingThresholdOverlapWarning')}
              </span>
            </div>
          )}

        {!filter.enableDirectFlag && !filter.llmEnabled && (
          <div className="l4a-callout">
            <AlertTriangle
              size={15}
              style={{ color: 'var(--md-sys-color-tertiary)', flexShrink: 0, marginTop: 2 }}
            />
            <span className="md-body-small">
              {t('filter.settings.routingBothDisabledWarning')}
            </span>
          </div>
        )}

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
                  disabled={busy || filterStatusBusy}
                  icon={<RefreshCw size={13} />}
                  onClick={() => void handleRefreshTokenStatus()}
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
                disabled={busy || filterStatusBusy || !tokenInput.trim()}
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
                  disabled={busy || filterStatusBusy}
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

        <div className="l4a-setting-row" style={{ flexWrap: 'wrap' }}>
          <span className="md-title-small">{t('filter.settings.failureDisplay')}</span>
          <Select<FilterFailureDisplay>
            ariaLabel={t('filter.settings.failureDisplay')}
            value={filter.failureDisplay ?? 'notice'}
            onChange={(next) => onUpdateFilter((curr) => ({ ...curr, failureDisplay: next }))}
            options={[
              { value: 'notice', label: t('filter.settings.failureOptions.notice') },
              { value: 'as_flagged', label: t('filter.settings.failureOptions.as_flagged') },
            ]}
          />
        </div>

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
            <NumericTextField
              label={t('filter.settings.modalityThreshold')}
              min={0.01}
              max={1}
              disabled={!filter.subdivideModalities}
              value={filter.modalityThreshold}
              onCommit={(modalityThreshold) =>
                onUpdateFilter((curr) => ({ ...curr, modalityThreshold }))
              }
            />
          </div>
          <Button
            variant="outlined"
            size="sm"
            disabled={busy || filterStatusBusy}
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
                busy ||
                filterStatusBusy ||
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
                    sandboxResult.probabilities.corpseProb !== undefined
                      ? `corpse=${sandboxResult.probabilities.corpseProb.toFixed(3)}`
                      : null,
                    sandboxResult.probabilities.corpsePileProb !== undefined
                      ? `corpsePile=${sandboxResult.probabilities.corpsePileProb.toFixed(3)}`
                      : null,
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
                const canBindFingerprint =
                  addon?.installState === 'installed' && Boolean(addon.contentHash);
                const shouldRedactTitle =
                  excl.displayLevel === 'hide_text' ||
                  Boolean(addon && isAddonTitleRedacted(addon, filterCtx));
                const isTitleRevealed = revealedExclusionIds.has(excl.addonId);
                const rawLabel = addon
                  ? formatAddonDisplayName(addon, namePreference)
                  : excl.addonId;
                const widFromId =
                  addon?.workshopId ??
                  (excl.addonId.startsWith('ws:') ? excl.addonId.slice(3) : undefined);
                const redactedLabel = widFromId
                  ? t('filter.redactedTitleWithId', { id: widFromId })
                  : `${t('filter.redactedTitle')} · ${excl.addonId.slice(0, 16)}`;
                const label =
                  shouldRedactTitle && !isTitleRevealed ? redactedLabel : rawLabel;

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
                        {widFromId ? `#${widFromId}` : excl.addonId.slice(0, 20)}
                      </span>
                    </button>

                    <div className="l4a-list-trailing">
                      {shouldRedactTitle && Boolean(addon) && (
                        <IconButton
                          size="sm"
                          label={
                            isTitleRevealed ? t('filter.hideTitle') : t('filter.revealTitle')
                          }
                          onClick={() => toggleRevealExclusionTitle(excl.addonId)}
                        >
                          {isTitleRevealed ? <EyeOff size={15} /> : <Eye size={15} />}
                        </IconButton>
                      )}

                      <Select<ExclusionDisplayLevel>
                        ariaLabel={t('filter.exclusionLevelLabel')}
                        value={excl.displayLevel}
                        disabled={busy}
                        onChange={(nextLevel) =>
                          void onUpsertExclusion({
                            addonId: excl.addonId,
                            displayLevel: nextLevel,
                            note: excl.note,
                          })
                        }
                        options={EXCLUSION_LEVEL_OPTIONS.map((lvl) => ({
                          value: lvl,
                          label: t(`filter.levels.${lvl}`),
                        }))}
                      />

                      {excl.bindFingerprint && !excl.fingerprintValid && canBindFingerprint && (
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
