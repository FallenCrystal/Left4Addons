import { useTranslation } from 'react-i18next';
import {
  AlertTriangle,
  ArrowUpCircle,
  CloudOff,
  Download,
  Eye,
  FolderOpen,
  Layers,
  Map as MapIcon,
  ShieldCheck,
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
import { Button, Checkbox, IconButton, Switch } from '../../m3/primitives';
import {
  isRemoteUnavailable,
  resolveLibraryMetadataTitle,
  summarizeBundle,
  type LibraryDisplayItem,
} from './helpers';

export interface LibraryListProps {
  displayItems: LibraryDisplayItem[];
  namePreference: AddonNamePreference;
  validSelectedIds: Set<string>;
  batchMode: boolean;
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

interface BundleListRowProps {
  bundle: BundleRecord;
  allMembers: AddonSummary[];
  visibleMembers: AddonSummary[];
  validSelectedIds: Set<string>;
  batchMode: boolean;
  busy: boolean;
  forceShowHiddenCards: boolean;
  onToggleSelectMany: (ids: string[]) => void;
  onInspectBundle: (bundleId: string) => void;
  onBatchToggle: (addonIds: string[], enabled: boolean) => void;
  onOpenInFileManager: (path: string) => void;
}

function BundleListRow({
  bundle,
  allMembers,
  visibleMembers,
  validSelectedIds,
  batchMode,
  busy,
  forceShowHiddenCards,
  onToggleSelectMany,
  onInspectBundle,
  onBatchToggle,
  onOpenInFileManager,
}: BundleListRowProps) {
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
    bundleCoverImage: bundle.coverImage,
    forceShowHiddenCards,
  });
  const bundleDisplayName = coverPres.redactTitle
    ? formatRedactedAddonLabel(visibleSummary.firstWorkshopId ?? summary.firstWorkshopId, t)
    : bundle.name;
  const authorText = coverPres.redactAuthor
    ? t('filter.redactedAuthor')
    : summary.firstAuthor || t('common.unknown');

  return (
    <div
      className="l4a-list-row"
      data-selected={selected ? 'true' : 'false'}
      data-muted={muted ? 'true' : 'false'}
      data-blur-card={coverPres.blurWholeCard ? 'true' : 'false'}
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
        className="l4a-blur-row-inner"
        inert={coverPres.blurWholeCard || undefined}
        aria-hidden={coverPres.blurWholeCard || undefined}
      >
        <Checkbox
          checked={selected}
          label={bundleDisplayName}
          onChange={() => onToggleSelectMany(visibleIds)}
        />
        <div className="l4a-list-thumb">
          <AddonCover
            compact
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
        <button
          type="button"
          className="l4a-card-trigger l4a-list-main"
          onClick={() => {
            if (batchMode) onToggleSelectMany(visibleIds);
            else onInspectBundle(bundle.id);
          }}
        >
          <div className="l4a-row-wrap" style={{ gap: 6 }}>
            <span className="md-title-small l4a-truncate">{bundleDisplayName}</span>
            <span className="md-pill" data-tone="secondary">
              <Layers size={11} />
              {t('library.bundleParts', { count: allMembers.length })}
            </span>
            {summary.hasDummyBypass && (
              <span className="md-pill" data-tone="primary">
                <ShieldCheck size={11} />
              </span>
            )}
            {summary.updateAvailable && (
              <span className="md-pill" data-tone="tertiary">
                <ArrowUpCircle size={11} />
              </span>
            )}
            {summary.removedUpstream && (
              <span className="md-pill" data-tone="error">
                <CloudOff size={11} />
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
          <div className="l4a-addon-meta">
            <span className="l4a-truncate">{authorText}</span>
            <span>·</span>
            <span>{formatBytes(summary.totalSize)}</span>
            {bundle.maps.length > 0 && (
              <>
                <span>·</span>
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 3 }}>
                  <MapIcon size={11} />
                  {bundle.maps.length}
                </span>
              </>
            )}
          </div>
        </button>
        <div className="l4a-list-trailing">
          {visibleSummary.primaryDir && (
            <span className="md-pill" data-tone="surface">
              {visibleSummary.primaryDir === 'loading'
                ? t('library.loadingDir')
                : t('library.workshopDir')}
            </span>
          )}
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
      </div>
    </div>
  );
}

interface AddonListRowProps {
  addon: AddonSummary;
  namePreference: AddonNamePreference;
  selected: boolean;
  batchMode: boolean;
  dl?: LiveDownloadState;
  busy: boolean;
  forceShowHiddenCards: boolean;
  onToggleSelect: (id: string) => void;
  onSelectAddon: (addon: AddonSummary) => void;
  onToggleSingle: (addon: AddonSummary, enabled: boolean) => void;
  onDownloadItem: (workshopId: string, title?: string) => void;
  onOpenInFileManager: (path: string) => void;
}

function AddonListRow({
  addon,
  namePreference,
  selected,
  batchMode,
  dl,
  busy,
  forceShowHiddenCards,
  onToggleSelect,
  onSelectAddon,
  onToggleSingle,
  onDownloadItem,
  onOpenInFileManager,
}: AddonListRowProps) {
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

  return (
    <div
      className="l4a-list-row"
      data-selected={selected ? 'true' : 'false'}
      data-muted={installed && !addon.enabled ? 'true' : 'false'}
      data-blur-card={cardFilter.blurWholeCard ? 'true' : 'false'}
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
        className="l4a-blur-row-inner"
        inert={cardFilter.blurWholeCard || undefined}
        aria-hidden={cardFilter.blurWholeCard || undefined}
      >
        <Checkbox
          checked={selected}
          label={displayName}
          onChange={() => onToggleSelect(addon.id)}
        />
        <div className="l4a-list-thumb">
          <AddonCover
            compact
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
        <button
          type="button"
          className="l4a-card-trigger l4a-list-main"
          onClick={() => {
            if (batchMode) onToggleSelect(addon.id);
            else onSelectAddon(addon);
          }}
        >
          <div className="l4a-row-wrap" style={{ gap: 6 }}>
            <span className="md-title-small l4a-truncate">{displayName}</span>
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
                <AlertTriangle size={11} />
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
                <ShieldCheck size={11} />
              </span>
            )}
            {addon.updateAvailable && (
              <span className="md-pill" data-tone="tertiary">
                <ArrowUpCircle size={11} />
              </span>
            )}
            {remoteUnavailable && (
              <span className="md-pill" data-tone="error">
                <CloudOff size={11} />
              </span>
            )}
            {addon.installState === 'missing_file' && (
              <span className="md-pill" data-tone="error">
                {t('library.missingFile')}
              </span>
            )}
            {installed && !addon.enabled && (
              <span className="md-pill" data-tone="surface">
                {t('common.disabled')}
              </span>
            )}
          </div>
          <div className="l4a-addon-meta">
            <span className="l4a-truncate">{authorText}</span>
            <span>·</span>
            <span>{formatBytes(addon.fileSize)}</span>
            {addon.workshopId && (
              <>
                <span>·</span>
                <span className="md-mono">#{addon.workshopId}</span>
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
        </button>
        <div className="l4a-list-trailing">
          {cardFilter.isBlurCardRevealed && (
            <Button variant="text" size="sm" onClick={() => onSelectAddon(addon)}>
              {t('common.inspect')}
            </Button>
          )}
          {addon.primaryDir && (
            <span className="md-pill" data-tone="surface">
              {addon.primaryDir === 'loading'
                ? t('library.loadingDir')
                : t('library.workshopDir')}
            </span>
          )}
          {installed ? (
            <>
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
            </>
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
  );
}

export function LibraryList({
  displayItems,
  namePreference,
  validSelectedIds,
  batchMode,
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
}: LibraryListProps) {
  return (
    <div className="l4a-list">
      {displayItems.map((entry) => {
        if (entry.kind === 'bundle') {
          return (
            <BundleListRow
              key={`bundle:${entry.bundle.id}`}
              bundle={entry.bundle}
              allMembers={entry.allMembers}
              visibleMembers={entry.visibleMembers}
              validSelectedIds={validSelectedIds}
              batchMode={batchMode}
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
          <AddonListRow
            key={addon.id}
            addon={addon}
            namePreference={namePreference}
            selected={validSelectedIds.has(addon.id)}
            batchMode={batchMode}
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
