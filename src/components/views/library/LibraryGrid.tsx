import { useTranslation } from 'react-i18next';
import {
  AlertTriangle,
  ArrowUpCircle,
  CloudOff,
  Download,
  Eye,
  Folder,
  FolderOpen,
  Layers,
  Map as MapIcon,
  ShieldCheck,
  Store,
} from 'lucide-react';
import type { AddonNamePreference, AddonSummary, BundleRecord } from '../../../api/ipc';
import { useContentFilterContext } from '../../../state/ContentFilterContext';
import type { LiveDownloadState } from '../../../state/useAppController';
import {
  addonFilterKey,
  formatDiagnosticVerdictBadge,
  formatRedactedAddonLabel,
  resolveBundleCoverPresentation,
  resolveLibraryAddonCardFilter,
} from '../../../utils/contentFilter';
import { formatAddonDisplayName, formatBytes } from '../../../utils/format';
import { AddonCover } from '../../m3/AddonCover';
import { Button, Checkbox, IconButton, LinearProgress, Switch } from '../../m3/primitives';
import {
  isRemoteUnavailable,
  resolveLibraryMetadataTitle,
  summarizeBundle,
  type LibraryDisplayItem,
} from './helpers';

export interface LibraryGridProps {
  displayItems: LibraryDisplayItem[];
  namePreference: AddonNamePreference;
  validSelectedIds: Set<string>;
  batchMode: boolean;
  showBatchBar: boolean;
  liveDownloads: Record<string, LiveDownloadState>;
  busy: boolean;
  forceShowHiddenCards?: boolean;
  onToggleSelect: (id: string) => void;
  onToggleSelectMany: (ids: string[]) => void;
  onSelectAddon: (addon: AddonSummary) => void;
  onInspectBundle: (bundleId: string) => void;
  onToggleSingle: (addon: AddonSummary, enabled: boolean) => void;
  onBatchToggle: (addonIds: string[], enabled: boolean) => void;
  onDownloadItem: (workshopId: string, title?: string) => void;
  onOpenInFileManager: (path: string) => void;
}

interface BundleGridCardProps {
  bundle: BundleRecord;
  allMembers: AddonSummary[];
  visibleMembers: AddonSummary[];
  validSelectedIds: Set<string>;
  batchMode: boolean;
  showBatchBar: boolean;
  busy: boolean;
  forceShowHiddenCards: boolean;
  onToggleSelectMany: (ids: string[]) => void;
  onInspectBundle: (bundleId: string) => void;
  onBatchToggle: (addonIds: string[], enabled: boolean) => void;
  onOpenInFileManager: (path: string) => void;
}

function BundleGridCard({
  bundle,
  allMembers,
  visibleMembers,
  validSelectedIds,
  batchMode,
  showBatchBar,
  busy,
  forceShowHiddenCards,
  onToggleSelectMany,
  onInspectBundle,
  onBatchToggle,
  onOpenInFileManager,
}: BundleGridCardProps) {
  const { t } = useTranslation();
  const filterCtx = useContentFilterContext();
  const { revealCard } = filterCtx;
  const summary = summarizeBundle(allMembers);
  const visibleSummary = summarizeBundle(visibleMembers);
  const visibleIds = visibleMembers.map((m) => m.id);
  const selected = visibleIds.length > 0 && visibleIds.every((id) => validSelectedIds.has(id));
  const loadingIds = visibleSummary.loading.map((m) => m.id);
  const muted = visibleSummary.installed.length > 0 && visibleSummary.enabledInstalled.length === 0;
  const firstPath = visibleSummary.firstPath;

  const coverPres = resolveBundleCoverPresentation(visibleMembers, filterCtx, {
    allMembers,
    bundleCoverImage: bundle.coverImage,
    forceShowHiddenCards,
  });
  const bundleDisplayName = coverPres.redactTitle
    ? formatRedactedAddonLabel(visibleSummary.firstWorkshopId ?? summary.firstWorkshopId, t)
    : bundle.name;
  const authorText = coverPres.redactAuthor
    ? t('filter.redactedAuthor')
    : summary.firstAuthor || t('common.unknown');

  const handleOpenOrSelect = () => {
    if (batchMode) onToggleSelectMany(visibleIds);
    else onInspectBundle(bundle.id);
  };

  return (
    <article
      className="md-card l4a-bundle-card"
      data-clickable="true"
      data-selected={selected ? 'true' : 'false'}
      data-muted={muted ? 'true' : 'false'}
      data-blur-card={coverPres.blurWholeCard ? 'true' : 'false'}
      data-pending-eval={coverPres.pendingEvaluation ? 'true' : undefined}
    >
      {coverPres.blurWholeCard &&
        (coverPres.pendingEvaluation ? (
          <div className="l4a-blur-card-overlay" data-pending="true" aria-hidden="true">
            <span className="l4a-blur-card-chip">
              <span>{t('filter.evaluatingMedia')}</span>
            </span>
          </div>
        ) : (
          <button
            type="button"
            className="l4a-blur-card-overlay"
            onClick={(e) => {
              e.stopPropagation();
              if (batchMode) {
                onToggleSelectMany(visibleIds);
              } else {
                for (const m of visibleMembers) {
                  revealCard(addonFilterKey(m.id));
                }
              }
            }}
          >
            <span className="l4a-blur-card-chip">
              <Eye size={14} />
              <span>{t('filter.blurCardOverlay')}</span>
            </span>
          </button>
        ))}

      <div
        className="l4a-blur-card-inner"
        inert={coverPres.blurWholeCard || undefined}
        aria-hidden={coverPres.blurWholeCard || undefined}
      >
        <div className="l4a-addon-cover-wrap">
          <div
            className="l4a-card-trigger"
            style={{ height: '100%' }}
            onClick={handleOpenOrSelect}
          >
            <AddonCover
              src={coverPres.src}
              fallbackSrc={coverPres.fallbackSrc}
              title={bundleDisplayName}
              category="Campaign"
              filterMode={coverPres.coverMode}
              filterLabel={
                coverPres.pendingEvaluation ? t('filter.evaluatingMedia') : undefined
              }
            />
          </div>
          <div className="l4a-cover-overlay-top">
            <div
              className="l4a-card-check"
              data-visible={showBatchBar || selected ? 'true' : 'false'}
            >
              <Checkbox
                checked={selected}
                label={bundleDisplayName}
                onChange={() => onToggleSelectMany(visibleIds)}
              />
            </div>
            <div className="l4a-status-pills">
              <span className="md-pill" data-tone="secondary">
                <Layers size={12} />
                {t('library.bundleParts', { count: allMembers.length })}
              </span>
              {summary.hasDummyBypass && (
                <span className="md-pill" data-tone="primary">
                  <ShieldCheck size={12} />
                </span>
              )}
              {summary.updateAvailable && (
                <span className="md-pill" data-tone="tertiary">
                  <ArrowUpCircle size={12} />
                </span>
              )}
              {summary.removedUpstream && (
                <span className="md-pill" data-tone="error">
                  <CloudOff size={12} />
                </span>
              )}
            </div>
          </div>
        </div>

        <div className="l4a-addon-card-body">
          <button type="button" className="l4a-card-trigger" onClick={handleOpenOrSelect}>
            <div className="l4a-addon-title">{bundleDisplayName}</div>
          </button>
          <div className="l4a-addon-meta">
            <span className="l4a-truncate">{authorText}</span>
            <span>·</span>
            <span style={{ flexShrink: 0 }}>{formatBytes(summary.totalSize)}</span>
            {bundle.maps.length > 0 && (
              <>
                <span>·</span>
                <span
                  style={{
                    flexShrink: 0,
                    display: 'inline-flex',
                    alignItems: 'center',
                    gap: 3,
                  }}
                >
                  <MapIcon size={12} />
                  {bundle.maps.length}
                </span>
              </>
            )}
          </div>

          <div className="l4a-addon-card-footer">
            <div className="l4a-row-wrap" style={{ gap: 6 }}>
              {visibleSummary.primaryDir && (
                <span className="md-pill" data-tone="surface">
                  {visibleSummary.primaryDir === 'loading' ? (
                    <Folder size={11} />
                  ) : (
                    <Store size={11} />
                  )}
                  {visibleSummary.primaryDir === 'loading'
                    ? t('library.loadingDir')
                    : t('library.workshopDir')}
                </span>
              )}
              {loadingIds.length > 0 && !visibleSummary.allLoadingEnabled && (
                <span
                  className="md-pill"
                  data-tone={visibleSummary.noneLoadingEnabled ? 'surface' : 'tertiary'}
                >
                  {visibleSummary.noneLoadingEnabled
                    ? t('common.disabled')
                    : t('organize.statusPartial', {
                        enabled: visibleSummary.enabledLoading.length,
                        total: loadingIds.length,
                      })}
                </span>
              )}
            </div>

            {(firstPath || loadingIds.length > 0) && (
              <div className="l4a-row-wrap" style={{ gap: 6, flexWrap: 'nowrap' }}>
                {firstPath && (
                  <IconButton
                    size="sm"
                    label={t('common.openInExplorer')}
                    onClick={(e) => {
                      e.stopPropagation();
                      onOpenInFileManager(firstPath);
                    }}
                  >
                    <FolderOpen size={15} />
                  </IconButton>
                )}
                {loadingIds.length > 0 && (
                  <Switch
                    compact
                    checked={visibleSummary.allLoadingEnabled}
                    disabled={busy}
                    label={bundleDisplayName}
                    onChange={(next) => onBatchToggle(loadingIds, next)}
                  />
                )}
              </div>
            )}
          </div>
        </div>
      </div>
    </article>
  );
}

interface AddonGridCardProps {
  addon: AddonSummary;
  namePreference: AddonNamePreference;
  selected: boolean;
  batchMode: boolean;
  showBatchBar: boolean;
  dl?: LiveDownloadState;
  busy: boolean;
  forceShowHiddenCards: boolean;
  onToggleSelect: (id: string) => void;
  onSelectAddon: (addon: AddonSummary) => void;
  onToggleSingle: (addon: AddonSummary, enabled: boolean) => void;
  onDownloadItem: (workshopId: string, title?: string) => void;
  onOpenInFileManager: (path: string) => void;
}

function AddonGridCard({
  addon,
  namePreference,
  selected,
  batchMode,
  showBatchBar,
  dl,
  busy,
  forceShowHiddenCards,
  onToggleSelect,
  onSelectAddon,
  onToggleSingle,
  onDownloadItem,
  onOpenInFileManager,
}: AddonGridCardProps) {
  const { t } = useTranslation();
  const filterCtx = useContentFilterContext();
  const { verdicts, revealCard } = filterCtx;

  const cardKey = addonFilterKey(addon.id);
  const verdict = verdicts[cardKey];
  const cardFilter = resolveLibraryAddonCardFilter(addon, filterCtx, {
    forceShowHiddenCards,
  });

  const rawDisplayName = formatAddonDisplayName(addon, namePreference);
  const displayName = cardFilter.redactTitle
    ? formatRedactedAddonLabel(addon.workshopId, t)
    : rawDisplayName;
  const authorText = cardFilter.redactMeta
    ? t('filter.redactedAuthor')
    : addon.author || t('common.unknown');
  const description = addon.addonInfo.addondescription ?? addon.addonInfo.description;

  const installed = addon.installState === 'installed';
  const remoteUnavailable = isRemoteUnavailable(addon);

  const handleOpenOrSelect = () => {
    if (batchMode) onToggleSelect(addon.id);
    else onSelectAddon(addon);
  };

  return (
    <article
      className="md-card"
      data-clickable="true"
      data-selected={selected ? 'true' : 'false'}
      data-muted={installed && !addon.enabled ? 'true' : 'false'}
      data-blur-card={cardFilter.blurWholeCard ? 'true' : 'false'}
      data-pending-eval={cardFilter.pendingEvaluation ? 'true' : undefined}
    >
      {cardFilter.blurWholeCard &&
        (cardFilter.pendingEvaluation ? (
          <div className="l4a-blur-card-overlay" data-pending="true" aria-hidden="true">
            <span className="l4a-blur-card-chip">
              <span>{t('filter.evaluatingMedia')}</span>
            </span>
          </div>
        ) : (
          <button
            type="button"
            className="l4a-blur-card-overlay"
            onClick={(e) => {
              e.stopPropagation();
              if (batchMode) onToggleSelect(addon.id);
              else revealCard(cardKey);
            }}
          >
            <span className="l4a-blur-card-chip">
              <Eye size={14} />
              <span>{t('filter.blurCardOverlay')}</span>
            </span>
          </button>
        ))}

      <div
        className="l4a-blur-card-inner"
        inert={cardFilter.blurWholeCard || undefined}
        aria-hidden={cardFilter.blurWholeCard || undefined}
      >
        <div className="l4a-addon-cover-wrap">
          <div
            className="l4a-card-trigger"
            style={{ height: '100%' }}
            onClick={handleOpenOrSelect}
          >
            <AddonCover
              src={addon.coverImage}
              fallbackSrc={addon.localCoverImage}
              title={displayName}
              category={addon.categories[0]}
              filterMode={cardFilter.coverMode}
              filterLabel={
                cardFilter.pendingEvaluation ? t('filter.evaluatingMedia') : undefined
              }
            />
          </div>
          <div className="l4a-cover-overlay-top">
            <div
              className="l4a-card-check"
              data-visible={showBatchBar || selected ? 'true' : 'false'}
            >
              <Checkbox
                checked={selected}
                label={displayName}
                onChange={() => onToggleSelect(addon.id)}
              />
            </div>
            <div className="l4a-status-pills">
              {cardFilter.exclusionActive &&
                cardFilter.exclusionDisplayLevel &&
                cardFilter.exclusionDisplayLevel !== 'normal' && (
                  <span className="md-pill" data-tone="secondary">
                    {t('filter.exclusionBadge', {
                      level: t(`filter.levels.${cardFilter.exclusionDisplayLevel}`),
                    })}
                  </span>
                )}
              {cardFilter.exclusionInvalidated && (
                <span className="md-pill" data-tone="error">
                  <AlertTriangle size={12} />
                  {t('filter.exclusionInvalidPill')}
                </span>
              )}
              {cardFilter.showDiagnosticBadge && verdict && (
                <span className="md-pill" data-tone="tertiary">
                  {formatDiagnosticVerdictBadge(verdict, cardFilter, {
                    imageTag: t('filter.modImageShort'),
                    textTag: t('filter.modTextShort'),
                  })}
                </span>
              )}
              {addon.hasDummyBypass && (
                <span className="md-pill" data-tone="primary">
                  <ShieldCheck size={12} />
                </span>
              )}
              {addon.updateAvailable && (
                <span className="md-pill" data-tone="tertiary">
                  <ArrowUpCircle size={12} />
                </span>
              )}
              {addon.remoteStatus === 'removed' && (
                <span className="md-pill" data-tone="error">
                  <CloudOff size={12} />
                </span>
              )}
              {addon.remoteStatus === 'private_or_unavailable' && (
                <span className="md-pill" data-tone="error">
                  <CloudOff size={12} />
                  {t('library.privateUpstream')}
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
            </div>
          </div>
        </div>

        {dl && (
          <LinearProgress
            value={dl.percent}
            indeterminate={dl.cancelling || dl.percent === 0}
            label={displayName}
          />
        )}

        <div className="l4a-addon-card-body">
          <button type="button" className="l4a-card-trigger" onClick={handleOpenOrSelect}>
            <div className="l4a-addon-title">{displayName}</div>
          </button>
          <div className="l4a-addon-meta">
            <span className="l4a-truncate">{authorText}</span>
            <span>·</span>
            <span style={{ flexShrink: 0 }}>{formatBytes(addon.fileSize)}</span>
            {addon.maps.length > 0 && (
              <>
                <span>·</span>
                <span
                  style={{
                    flexShrink: 0,
                    display: 'inline-flex',
                    alignItems: 'center',
                    gap: 3,
                  }}
                >
                  <MapIcon size={12} />
                  {addon.maps.length}
                </span>
              </>
            )}
          </div>

          {cardFilter.isBlurCardRevealed && (
            <div className="l4a-revealed-desc">
              {cardFilter.redactDescription
                ? t('filter.redactedDescription')
                : description?.trim() || t('filter.noDescription')}
            </div>
          )}

          {!cardFilter.isBlurCardRevealed &&
            cardFilter.redactDescription &&
            !cardFilter.redactTitle && (
              <div className="l4a-revealed-desc">{t('filter.redactedDescription')}</div>
            )}

          <div className="l4a-addon-card-footer">
            <div className="l4a-row-wrap" style={{ gap: 6 }}>
              {addon.primaryDir && (
                <span className="md-pill" data-tone="surface">
                  {addon.primaryDir === 'loading' ? <Folder size={11} /> : <Store size={11} />}
                  {addon.primaryDir === 'loading'
                    ? t('library.loadingDir')
                    : t('library.workshopDir')}
                </span>
              )}
              {installed && !addon.enabled && (
                <span className="md-pill" data-tone="surface">
                  {t('common.disabled')}
                </span>
              )}
              {cardFilter.isBlurCardRevealed && (
                <Button variant="text" size="sm" onClick={() => onSelectAddon(addon)}>
                  {t('common.inspect')}
                </Button>
              )}
            </div>

            {installed ? (
              <div className="l4a-row-wrap" style={{ gap: 6, flexWrap: 'nowrap' }}>
                {addon.currentPath && (
                  <IconButton
                    size="sm"
                    label={t('common.openInExplorer')}
                    onClick={(e) => {
                      e.stopPropagation();
                      onOpenInFileManager(addon.currentPath!);
                    }}
                  >
                    <FolderOpen size={15} />
                  </IconButton>
                )}
                <Switch
                  compact
                  checked={addon.enabled}
                  disabled={busy}
                  label={displayName}
                  onChange={(next) => onToggleSingle(addon, next)}
                />
              </div>
            ) : addon.workshopId && !remoteUnavailable ? (
              <Button
                variant="tonal"
                size="sm"
                disabled={Boolean(dl)}
                icon={<Download size={13} />}
                onClick={() =>
                  onDownloadItem(
                    addon.workshopId!,
                    resolveLibraryMetadataTitle(addon, namePreference, filterCtx),
                  )
                }
              >
                {dl
                  ? dl.cancelling
                    ? t('common.cancelling')
                    : `${dl.percent}%`
                  : t('common.download')}
              </Button>
            ) : null}
          </div>
        </div>
      </div>
    </article>
  );
}

export function LibraryGrid({
  displayItems,
  namePreference,
  validSelectedIds,
  batchMode,
  showBatchBar,
  liveDownloads,
  busy,
  forceShowHiddenCards = false,
  onToggleSelect,
  onToggleSelectMany,
  onSelectAddon,
  onInspectBundle,
  onToggleSingle,
  onBatchToggle,
  onDownloadItem,
  onOpenInFileManager,
}: LibraryGridProps) {
  return (
    <div className="l4a-grid">
      {displayItems.map((entry) => {
        if (entry.kind === 'bundle') {
          return (
            <BundleGridCard
              key={`bundle:${entry.bundle.id}`}
              bundle={entry.bundle}
              allMembers={entry.allMembers}
              visibleMembers={entry.visibleMembers}
              validSelectedIds={validSelectedIds}
              batchMode={batchMode}
              showBatchBar={showBatchBar}
              busy={busy}
              forceShowHiddenCards={forceShowHiddenCards}
              onToggleSelectMany={onToggleSelectMany}
              onInspectBundle={onInspectBundle}
              onBatchToggle={onBatchToggle}
              onOpenInFileManager={onOpenInFileManager}
            />
          );
        }

        const { addon } = entry;
        return (
          <AddonGridCard
            key={addon.id}
            addon={addon}
            namePreference={namePreference}
            selected={validSelectedIds.has(addon.id)}
            batchMode={batchMode}
            showBatchBar={showBatchBar}
            dl={addon.workshopId ? liveDownloads[addon.workshopId] : undefined}
            busy={busy}
            forceShowHiddenCards={forceShowHiddenCards}
            onToggleSelect={onToggleSelect}
            onSelectAddon={onSelectAddon}
            onToggleSingle={onToggleSingle}
            onDownloadItem={onDownloadItem}
            onOpenInFileManager={onOpenInFileManager}
          />
        );
      })}
    </div>
  );
}
