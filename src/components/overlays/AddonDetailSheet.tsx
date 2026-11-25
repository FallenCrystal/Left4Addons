import { useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AlertTriangle,
  ArrowRightLeft,
  ArrowUpCircle,
  CloudOff,
  Copy,
  Download,
  ExternalLink,
  Folder,
  FolderOpen,
  Layers,
  Map as MapIcon,
  Pencil,
  Plus,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
  Store,
  Trash2,
} from 'lucide-react';
import type {
  AddonNamePreference,
  AddonSummary,
  BundleRecord,
  ExclusionDisplayLevel,
  UserGroup,
} from '../../api/ipc';
import { useContentFilterContext } from '../../state/ContentFilterContext';
import {
  addonFilterKey,
  collectAddonImageSources,
  formatRedactedAddonLabel,
  isAddonTitleRedacted,
  resolveLibraryAddonDetailFilter,
} from '../../utils/contentFilter';
import {
  buildSteamProtocolUrl,
  buildWorkshopUrl,
  formatAddonDisplayName,
  formatBytes,
  formatTimestamp,
  resolveAddonMetadataTitle,
} from '../../utils/format';
import { AddonCover } from '../m3/AddonCover';
import { Button, FilterChip, IconButton, Select, SideSheet, Switch, Tabs } from '../m3/primitives';

export interface AddonDetailSheetProps {
  addon: AddonSummary | null;
  namePreference?: AddonNamePreference;
  bundles: BundleRecord[];
  userGroups: UserGroup[];
  busy: boolean;
  onClose: () => void;
  onToggle: (addon: AddonSummary, enabled: boolean) => void;
  onMove: (addon: AddonSummary) => void;
  onRename: (addon: AddonSummary) => void;
  onDelete: (addon: AddonSummary) => void;
  onDownload: (workshopId: string, title?: string) => void;
  onUpdate: (workshopId: string, title?: string) => void;
  onAddToGroup: (addon: AddonSummary) => void;
  onRemoveFromGroup: (group: UserGroup, addonId: string) => void;
  onOpenExternal: (url: string) => void;
  onOpenInFileManager: (path: string) => void;
  onCopy: (text: string) => void;
}

function isHttpUrl(value: string): boolean {
  return /^https?:\/\/\S+$/i.test(value.trim());
}

function isZeroContentFlag(key: string, value: string): boolean {
  return key.startsWith('addoncontent_') && value.trim() === '0';
}

const EXCLUSION_LEVELS: ExclusionDisplayLevel[] = [
  'normal',
  'lazy_load',
  'blur_image',
  'hide_image',
  'hide_text',
];

export function AddonDetailSheet({
  addon,
  namePreference = 'workshop_title',
  bundles,
  userGroups,
  busy,
  onClose,
  onToggle,
  onMove,
  onRename,
  onDelete,
  onDownload,
  onUpdate,
  onAddToGroup,
  onRemoveFromGroup,
  onOpenExternal,
  onOpenInFileManager,
  onCopy,
}: AddonDetailSheetProps) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<'info' | 'maps'>('info');
  const [showZeroFlags, setShowZeroFlags] = useState(false);
  const [draftExclAddonId, setDraftExclAddonId] = useState<string | null>(null);
  const [draftLevel, setDraftLevel] = useState<ExclusionDisplayLevel>('lazy_load');
  const [draftBindFingerprint, setDraftBindFingerprint] = useState<boolean>(true);
  const tabsId = useId();

  const filterCtx = useContentFilterContext();
  const {
    verdicts,
    exclusionsByAddonId,
    upsertFilterExclusion,
    deleteFilterExclusion,
  } = filterCtx;

  const { primaryInfoEntries, zeroFlagEntries } = useMemo(() => {
    if (!addon) {
      return {
        primaryInfoEntries: [] as [string, string][],
        zeroFlagEntries: [] as [string, string][],
      };
    }
    const primary: [string, string][] = [];
    const zeros: [string, string][] = [];
    for (const [k, v] of Object.entries(addon.addonInfo)) {
      const trimmed = v.trim();
      if (!trimmed || k === 'addondescription' || k === 'description') continue;
      if (isZeroContentFlag(k, trimmed)) {
        zeros.push([k, trimmed]);
      } else {
        primary.push([k, trimmed]);
      }
    }
    return { primaryInfoEntries: primary, zeroFlagEntries: zeros };
  }, [addon]);

  if (!addon) return null;

  const installed = addon.installState === 'installed';
  const exclusion = exclusionsByAddonId.get(addon.id);
  const verdict = verdicts[addonFilterKey(addon.id)];
  const detailFilter = resolveLibraryAddonDetailFilter(addon, filterCtx);
  const addonImageSources = collectAddonImageSources(addon);

  const canBindFingerprint = installed && Boolean(addon.contentHash);
  const activeFingerprint = installed ? addon.contentHash : exclusion?.currentFingerprint;

  const isSameDraftAddon = draftExclAddonId === addon.id;
  const effectiveDraftLevel: ExclusionDisplayLevel = exclusion
    ? exclusion.displayLevel
    : isSameDraftAddon
      ? draftLevel
      : 'lazy_load';
  const effectiveDraftBind: boolean = exclusion
    ? exclusion.bindFingerprint
    : !canBindFingerprint
      ? false
      : isSameDraftAddon
        ? draftBindFingerprint
        : true;

  const rawDisplayName = formatAddonDisplayName(addon, namePreference);
  const displayName = detailFilter.redactText
    ? formatRedactedAddonLabel(addon.workshopId, t)
    : rawDisplayName;

  const titleRedactedForTasks =
    detailFilter.redactText || isAddonTitleRedacted(addon, filterCtx);
  const metadataTitle = titleRedactedForTasks
    ? undefined
    : resolveAddonMetadataTitle(
        addon,
        namePreference === 'addon_title' ? 'addon_title' : 'workshop_title',
      );
  const workshopTitle = addon.workshopTitle?.trim();
  const addonInfoTitle = addon.addonTitle?.trim();
  const hasMaps = addon.maps.length > 0;
  const activeTab: 'info' | 'maps' = hasMaps ? tab : 'info';
  const memberBundles = bundles.filter((b) => b.memberAddonIds.includes(addon.id));
  const memberGroups = userGroups.filter((g) => g.addonIds.includes(addon.id));
  const description = addon.addonInfo.addondescription ?? addon.addonInfo.description;
  const visibleInfoEntries = showZeroFlags
    ? [...primaryInfoEntries, ...zeroFlagEntries]
    : primaryInfoEntries;
  const remoteUnavailable =
    addon.remoteStatus === 'removed' || addon.remoteStatus === 'private_or_unavailable';

  return (
    <SideSheet
      open={Boolean(addon)}
      title={displayName}
      onClose={onClose}
      headerTrailing={
        installed ? (
          <Switch
            checked={addon.enabled}
            disabled={busy}
            label={addon.enabled ? t('common.enabled') : t('common.disabled')}
            onChange={(next) => onToggle(addon, next)}
          />
        ) : undefined
      }
      footer={
        <>
          {installed && addon.primaryDir === 'loading' && (
            <Button
              variant="text"
              size="sm"
              disabled={busy}
              icon={<Pencil size={15} />}
              onClick={() => onRename(addon)}
            >
              {t('common.rename')}
            </Button>
          )}
          {installed && (
            <Button
              variant="tonal"
              size="sm"
              disabled={busy}
              icon={<ArrowRightLeft size={15} />}
              onClick={() => onMove(addon)}
            >
              {addon.primaryDir === 'loading'
                ? t('library.moveToWorkshop')
                : t('library.moveToLoading')}
            </Button>
          )}
          {addon.workshopId && !installed && !remoteUnavailable && (
            <Button
              variant="filled"
              size="sm"
              disabled={busy}
              icon={<Download size={15} />}
              onClick={() => onDownload(addon.workshopId!, metadataTitle)}
            >
              {t('common.download')}
            </Button>
          )}
          {addon.workshopId && installed && addon.updateAvailable && !remoteUnavailable && (
            <Button
              variant="filled"
              size="sm"
              disabled={busy}
              icon={<ArrowUpCircle size={15} />}
              onClick={() => onUpdate(addon.workshopId!, metadataTitle)}
            >
              {t('common.update')}
            </Button>
          )}
          <Button
            variant="text"
            tone="error"
            size="sm"
            disabled={busy}
            icon={<Trash2 size={15} />}
            onClick={() => onDelete(addon)}
          >
            {t('common.delete')}
          </Button>
        </>
      }
    >
      {hasMaps && (
        <Tabs
          compact
          idPrefix={tabsId}
          value={activeTab}
          onChange={setTab}
          tabs={[
            { value: 'info', label: t('detail.info') },
            { value: 'maps', label: t('detail.maps'), count: addon.maps.length },
          ]}
        />
      )}

      <div
        className="md-sheet-body"
        role={hasMaps ? 'tabpanel' : undefined}
        id={hasMaps ? `${tabsId}-panel-${activeTab}` : undefined}
        aria-labelledby={hasMaps ? `${tabsId}-tab-${activeTab}` : undefined}
      >
        {activeTab === 'info' ? (
          <>
            <div
              className="l4a-addon-cover-wrap"
              style={{ borderRadius: 'var(--md-sys-shape-corner-medium)' }}
            >
              <AddonCover
                src={addon.coverImage}
                fallbackSrc={addon.localCoverImage}
                title={displayName}
                category={addon.categories[0]}
                filterMode={detailFilter.coverMode}
                filterLabel={
                  detailFilter.pendingEvaluation ? t('filter.evaluatingMedia') : undefined
                }
                allowPeek={detailFilter.coverMode === 'blur'}
              />
              <div className="l4a-cover-overlay-top">
                <div className="l4a-status-pills">
                  {addon.primaryDir && (
                    <span className="md-pill" data-tone="surface">
                      {addon.primaryDir === 'loading' ? (
                        <Folder size={12} />
                      ) : (
                        <Store size={12} />
                      )}
                      {addon.primaryDir === 'loading'
                        ? t('library.loadingDir')
                        : t('library.workshopDir')}
                    </span>
                  )}
                  {addon.installState === 'uninstalled_known' && (
                    <span className="md-pill" data-tone="surface">
                      {t('library.uninstalledKnown')}
                    </span>
                  )}
                  {addon.installState === 'missing_file' && (
                    <span className="md-pill" data-tone="error">
                      <AlertTriangle size={12} />
                      {t('library.missingFile')}
                    </span>
                  )}
                  {addon.hasDummyBypass && (
                    <span className="md-pill" data-tone="primary">
                      <ShieldCheck size={12} />
                      {t('library.dummyBypass')}
                    </span>
                  )}
                  {addon.updateAvailable && (
                    <span className="md-pill" data-tone="tertiary">
                      <ArrowUpCircle size={12} />
                      {t('library.updateAvailable')}
                    </span>
                  )}
                  {addon.remoteStatus === 'removed' && (
                    <span className="md-pill" data-tone="error">
                      <CloudOff size={12} />
                      {t('library.removedUpstream')}
                    </span>
                  )}
                  {addon.remoteStatus === 'private_or_unavailable' && (
                    <span className="md-pill" data-tone="error">
                      <CloudOff size={12} />
                      {t('library.privateUpstream')}
                    </span>
                  )}
                </div>
              </div>
            </div>

            {(addon.currentPath || addon.workshopId) && (
              <div className="l4a-row-wrap">
                {addon.currentPath && installed && (
                  <Button
                    variant="tonal"
                    size="sm"
                    icon={<FolderOpen size={14} />}
                    onClick={() => onOpenInFileManager(addon.currentPath!)}
                  >
                    {t('common.openInExplorer')}
                  </Button>
                )}
                {addon.workshopId && (
                  <>
                    <Button
                      variant="outlined"
                      size="sm"
                      icon={<ExternalLink size={14} />}
                      onClick={() => onOpenExternal(buildSteamProtocolUrl(addon.workshopId!))}
                    >
                      {t('detail.openSteam')}
                    </Button>
                    <Button
                      variant="outlined"
                      size="sm"
                      icon={<ExternalLink size={14} />}
                      onClick={() => onOpenExternal(buildWorkshopUrl(addon.workshopId!))}
                    >
                      {t('detail.openWeb')}
                    </Button>
                    <Button
                      variant="text"
                      size="sm"
                      icon={<Copy size={14} />}
                      onClick={() => onCopy(addon.workshopId!)}
                    >
                      {addon.workshopId}
                    </Button>
                  </>
                )}
              </div>
            )}

            <div className="l4a-kv-grid">
              <div className="l4a-kv-item">
                <span className="l4a-kv-label">{t('detail.author')}</span>
                <span className="l4a-kv-value">
                  {detailFilter.redactText
                    ? t('filter.redactedAuthor')
                    : addon.author || t('common.unknown')}
                </span>
              </div>
              <div className="l4a-kv-item">
                <span className="l4a-kv-label">{t('detail.version')}</span>
                <span className="l4a-kv-value">{addon.version || '—'}</span>
              </div>
              <div className="l4a-kv-item">
                <span className="l4a-kv-label">{t('detail.size')}</span>
                <span className="l4a-kv-value">{formatBytes(addon.fileSize)}</span>
              </div>
              <div className="l4a-kv-item">
                <span className="l4a-kv-label">{t('detail.filesCount')}</span>
                <span className="l4a-kv-value">{addon.filesCount}</span>
              </div>
              {!detailFilter.redactText && workshopTitle && workshopTitle !== displayName && (
                <div className="l4a-kv-item" style={{ gridColumn: '1 / -1' }}>
                  <span className="l4a-kv-label">{t('detail.workshopTitle')}</span>
                  <span className="l4a-kv-value selectable">{workshopTitle}</span>
                </div>
              )}
              {!detailFilter.redactText &&
                addonInfoTitle &&
                addonInfoTitle !== displayName &&
                addonInfoTitle !== workshopTitle && (
                  <div className="l4a-kv-item" style={{ gridColumn: '1 / -1' }}>
                    <span className="l4a-kv-label">{t('detail.addonTitle')}</span>
                    <span className="l4a-kv-value selectable">{addonInfoTitle}</span>
                  </div>
                )}
              <div className="l4a-kv-item" style={{ gridColumn: '1 / -1' }}>
                <span className="l4a-kv-label">{t('detail.fileName')}</span>
                <span className="l4a-kv-value md-mono selectable">
                  {detailFilter.redactText
                    ? addon.workshopId
                      ? `#${addon.workshopId}`
                      : '—'
                    : addon.currentFileName || '—'}
                </span>
              </div>
              {activeFingerprint && (
                <div className="l4a-kv-item" style={{ gridColumn: '1 / -1' }}>
                  <span className="l4a-kv-label">{t('detail.hash')}</span>
                  <div className="l4a-row-wrap">
                    <span className="l4a-kv-value md-mono">
                      {activeFingerprint.slice(0, 16)}
                    </span>
                    <IconButton
                      size="sm"
                      label={t('common.copy')}
                      onClick={() => onCopy(activeFingerprint)}
                    >
                      <Copy size={14} />
                    </IconButton>
                  </div>
                </div>
              )}
              {addon.localTimeUpdated && (
                <div className="l4a-kv-item">
                  <span className="l4a-kv-label">{t('detail.localUpdated')}</span>
                  <span className="l4a-kv-value">{formatTimestamp(addon.localTimeUpdated)}</span>
                </div>
              )}
              {addon.remoteTimeUpdated && (
                <div className="l4a-kv-item">
                  <span className="l4a-kv-label">{t('detail.remoteUpdated')}</span>
                  <span className="l4a-kv-value">{formatTimestamp(addon.remoteTimeUpdated)}</span>
                </div>
              )}
            </div>

            {addon.categories.length > 0 && (
              <div className="md-chip-row">
                {addon.categories.map((cat) => (
                  <span key={cat} className="md-pill" data-tone="secondary">
                    {t(`categories.${cat}`, { defaultValue: cat })}
                  </span>
                ))}
              </div>
            )}

            <div className="l4a-divider" />

            {/* Content Filter Verdict & Library Exclusion Controls */}
            <div className="l4a-filter-box">
              <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                <div className="l4a-row-wrap" style={{ gap: 6 }}>
                  <ShieldAlert
                    size={15}
                    style={{ color: 'var(--md-sys-color-primary)', flexShrink: 0 }}
                  />
                  <span className="md-label-large">{t('filter.detailSectionTitle')}</span>
                </div>
                {exclusion && (
                  <span
                    className="md-pill"
                    data-tone={exclusion.fingerprintValid ? 'primary' : 'error'}
                  >
                    {exclusion.fingerprintValid
                      ? t(`filter.levels.${exclusion.displayLevel}`)
                      : t('filter.exclusionInvalidPill')}
                  </span>
                )}
              </div>

              {detailFilter.flagged && verdict && (
                <div className="l4a-callout">
                  <ShieldAlert
                    size={15}
                    style={{ color: 'var(--md-sys-color-tertiary)', flexShrink: 0, marginTop: 2 }}
                  />
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                    <div className="l4a-row-wrap" style={{ gap: 6 }}>
                      <span className="md-label-medium">{t('filter.flaggedBannerTitle')}</span>
                      <span className="md-pill" data-tone="tertiary">
                        {t(`filter.decisionSource.${verdict.decisionSource}`)}
                      </span>
                      {detailFilter.imageSensitive && (
                        <span className="md-pill" data-tone="secondary">
                          [{t('filter.modImageShort')}]
                        </span>
                      )}
                      {detailFilter.textSensitive && (
                        <span className="md-pill" data-tone="secondary">
                          [{t('filter.modTextShort')}]
                        </span>
                      )}
                    </div>
                    <span
                      className="md-body-small"
                      style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                    >
                      {t('filter.scoreSummary', {
                        score: verdict.evaluatedScore.toFixed(2),
                        nsfw: verdict.probabilities.nsfwProb.toFixed(2),
                        gore: verdict.probabilities.goreProb.toFixed(2),
                      })}
                    </span>
                  </div>
                </div>
              )}

              {exclusion && !exclusion.fingerprintValid && (
                <div className="l4a-callout" data-tone="error">
                  <AlertTriangle size={16} style={{ flexShrink: 0, marginTop: 2 }} />
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 6, flex: 1 }}>
                    <span className="md-label-large">{t('filter.fingerprintInvalidBanner')}</span>
                    <span className="md-body-small md-mono">
                      {t('filter.fingerprintChangedDetail', {
                        bound: exclusion.boundFingerprint?.slice(0, 12) ?? '—',
                        current:
                          (exclusion.currentFingerprint ?? activeFingerprint)?.slice(0, 12) ?? '—',
                      })}
                    </span>
                    <div>
                      <Button
                        variant="filled"
                        size="sm"
                        disabled={busy || !canBindFingerprint}
                        icon={<RefreshCw size={14} />}
                        onClick={() =>
                          void upsertFilterExclusion({
                            addonId: addon.id,
                            displayLevel: exclusion.displayLevel,
                            bindFingerprint: canBindFingerprint,
                            note: exclusion.note,
                          })
                        }
                      >
                        {t('filter.rebindFingerprint')}
                      </Button>
                    </div>
                  </div>
                </div>
              )}

              <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                <span className="md-body-small">{t('filter.exclusionLevelLabel')}</span>
                <Select<ExclusionDisplayLevel>
                  ariaLabel={t('filter.exclusionLevelLabel')}
                  value={effectiveDraftLevel}
                  onChange={(nextLevel) => {
                    setDraftExclAddonId(addon.id);
                    setDraftLevel(nextLevel);
                    if (exclusion) {
                      void upsertFilterExclusion({
                        addonId: addon.id,
                        displayLevel: nextLevel,
                        bindFingerprint: exclusion.bindFingerprint && canBindFingerprint,
                        note: exclusion.note,
                      });
                    }
                  }}
                  options={EXCLUSION_LEVELS.map((lvl) => ({
                    value: lvl,
                    label: t(`filter.levels.${lvl}`),
                  }))}
                />
              </div>

              <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                  <span className="md-body-small">{t('filter.bindFingerprint')}</span>
                  <span
                    className="md-body-small md-mono"
                    style={{ color: 'var(--md-sys-color-on-surface-variant)', fontSize: 11 }}
                  >
                    {activeFingerprint
                      ? activeFingerprint.slice(0, 16)
                      : t('filter.noFingerprintHint')}
                  </span>
                </div>
                <Switch
                  compact
                  checked={effectiveDraftBind}
                  disabled={busy || !canBindFingerprint}
                  label={t('filter.bindFingerprint')}
                  onChange={(nextBind) => {
                    setDraftExclAddonId(addon.id);
                    setDraftBindFingerprint(nextBind);
                    if (exclusion) {
                      void upsertFilterExclusion({
                        addonId: addon.id,
                        displayLevel: exclusion.displayLevel,
                        bindFingerprint: nextBind && canBindFingerprint,
                        note: exclusion.note,
                      });
                    }
                  }}
                />
              </div>

              <div className="l4a-row-wrap" style={{ justifyContent: 'flex-end' }}>
                {exclusion ? (
                  <Button
                    variant="text"
                    tone="error"
                    size="sm"
                    disabled={busy}
                    icon={<Trash2 size={14} />}
                    onClick={() => void deleteFilterExclusion(addon.id)}
                  >
                    {t('filter.removeExclusion')}
                  </Button>
                ) : (
                  <Button
                    variant="tonal"
                    size="sm"
                    disabled={busy}
                    icon={<Plus size={14} />}
                    onClick={() =>
                      void upsertFilterExclusion({
                        addonId: addon.id,
                        displayLevel: effectiveDraftLevel,
                        bindFingerprint: effectiveDraftBind && canBindFingerprint,
                      })
                    }
                  >
                    {t('filter.addExclusion')}
                  </Button>
                )}
              </div>
            </div>

            <div className="l4a-divider" />

            <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
              <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                <span
                  className="md-label-medium"
                  style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                >
                  {t('detail.groups')}
                </span>
                <IconButton
                  size="sm"
                  label={t('library.addToGroup')}
                  onClick={() => onAddToGroup(addon)}
                >
                  <Plus size={16} />
                </IconButton>
              </div>
              <div className="md-chip-row">
                {memberGroups.length === 0 ? (
                  <span
                    className="md-body-small"
                    style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                  >
                    {t('detail.noGroups')}
                  </span>
                ) : (
                  memberGroups.map((g) => (
                    <FilterChip
                      key={g.id}
                      label={g.name}
                      tone="surface"
                      icon={
                        g.color ? (
                          <span
                            style={{
                              width: 8,
                              height: 8,
                              borderRadius: 999,
                              backgroundColor: g.color,
                            }}
                          />
                        ) : undefined
                      }
                      onRemove={() => onRemoveFromGroup(g, addon.id)}
                    />
                  ))
                )}
              </div>
            </div>

            {memberBundles.length > 0 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                <span
                  className="md-label-medium"
                  style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                >
                  {t('detail.bundles')}
                </span>
                <div className="md-chip-row">
                  {memberBundles.map((b) => (
                    <span key={b.id} className="md-pill" data-tone="primary">
                      <Layers size={12} />
                      {titleRedactedForTasks ? t('filter.redactedTitle') : b.name}
                    </span>
                  ))}
                </div>
              </div>
            )}

            {description && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                <span
                  className="md-label-medium"
                  style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                >
                  {t('detail.description')}
                </span>
                <div className="md-body-small selectable l4a-description-box">
                  {detailFilter.redactText ? t('filter.redactedDescription') : description}
                </div>
              </div>
            )}

            {detailFilter.redactText ? (
              <div className="md-body-small l4a-description-box">{t('filter.redactedMeta')}</div>
            ) : (
              (visibleInfoEntries.length > 0 || zeroFlagEntries.length > 0) && (
                <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                  <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                    <span
                      className="md-label-medium"
                      style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                    >
                      {t('detail.metadata')}
                    </span>
                    {zeroFlagEntries.length > 0 && (
                      <Button
                        variant="text"
                        size="sm"
                        onClick={() => setShowZeroFlags((v) => !v)}
                      >
                        {showZeroFlags
                          ? t('detail.hideZeroFlags')
                          : t('detail.showAllFields', {
                              count: primaryInfoEntries.length + zeroFlagEntries.length,
                            })}
                      </Button>
                    )}
                  </div>
                  <div className="l4a-list">
                    {visibleInfoEntries.map(([k, v]) => {
                      const urlValue = isHttpUrl(v);
                      const stacked = urlValue || v.length > 28 || v.includes('\n');
                      return (
                        <div
                          key={k}
                          className="l4a-info-row"
                          data-stacked={stacked ? 'true' : 'false'}
                        >
                          <span className="l4a-info-key md-body-small md-mono">{k}</span>
                          {urlValue ? (
                            <div className="l4a-info-url-wrap">
                              <button
                                type="button"
                                className="l4a-info-link md-body-small selectable"
                                onClick={() => onOpenExternal(v.trim())}
                              >
                                <span>{v.trim()}</span>
                                <ExternalLink size={12} style={{ flexShrink: 0 }} />
                              </button>
                              <IconButton
                                size="sm"
                                label={t('common.copy')}
                                onClick={() => onCopy(v.trim())}
                              >
                                <Copy size={13} />
                              </IconButton>
                            </div>
                          ) : (
                            <span className="l4a-info-value md-body-small selectable">{v}</span>
                          )}
                        </div>
                      );
                    })}
                  </div>
                </div>
              )
            )}
          </>
        ) : addon.maps.length === 0 ? (
          <div className="md-empty" style={{ padding: '40px 12px' }}>
            <div className="md-empty-icon">
              <MapIcon size={24} />
            </div>
            <div className="md-title-small">{t('detail.noMaps')}</div>
          </div>
        ) : (
          <div className="l4a-list">
            {addon.maps.map((m) => {
              const trimmedImg = m.imagePath?.trim();
              const imgIdx = trimmedImg ? addonImageSources.indexOf(trimmedImg) : -1;
              const mapImgMode = trimmedImg
                ? imgIdx >= 0
                  ? detailFilter.imageModeAt(imgIdx)
                  : detailFilter.mapCoverMode
                : 'none';
              const mapLabel = detailFilter.redactText ? m.code : m.name;
              return (
                <div
                  key={m.code}
                  className="l4a-list-row"
                  style={{ minHeight: 56, cursor: 'default' }}
                >
                  {m.imagePath && (
                    <div className="l4a-list-thumb">
                      <AddonCover
                        compact
                        src={m.imagePath}
                        title={mapLabel}
                        category="Map"
                        filterMode={mapImgMode}
                        filterLabel={
                          detailFilter.pendingEvaluation
                            ? t('filter.evaluatingMedia')
                            : undefined
                        }
                      />
                    </div>
                  )}
                  <div className="l4a-list-main">
                    <span className="md-title-small">{mapLabel}</span>
                    <span
                      className="md-body-small md-mono"
                      style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                    >
                      {m.code}
                    </span>
                  </div>
                  <Button
                    variant="tonal"
                    size="sm"
                    icon={<Copy size={13} />}
                    title={t('detail.copyMapCode')}
                    aria-label={`${t('detail.copyMapCode')} ${m.code}`}
                    onClick={() => onCopy(m.code)}
                  >
                    {t('common.copy')}
                  </Button>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </SideSheet>
  );
}
