import { useTranslation } from 'react-i18next';
import { Bookmark, Download, ExternalLink, Trash2 } from 'lucide-react';
import type {
  AddonNamePreference,
  AddonSummary,
  TrackedCollectionRecord,
} from '../../../api/ipc';
import { useContentFilterContext } from '../../../state/ContentFilterContext';
import {
  formatFilteredAddonTitle,
  isAddonTitleRedacted,
  resolveLibraryAddonCardFilter,
} from '../../../utils/contentFilter';
import { formatTimestamp } from '../../../utils/format';
import { Button, EmptyState, IconButton, LinearProgress } from '../../m3/primitives';
import type { ExternalAddonFilter } from '../LibraryView';

export interface CollectionsTabProps {
  collections: TrackedCollectionRecord[];
  filteredCollections: TrackedCollectionRecord[];
  addonsByWorkshopId: Map<string, AddonSummary>;
  namePreference: AddonNamePreference;
  busy: boolean;
  onInspectCollection: (collectionId: string) => void;
  onDownloadWorkshopItem: (workshopId: string, title?: string) => void;
  onOpenFilterInLibrary: (filter: ExternalAddonFilter) => void;
  onUntrackCollection: (collectionId: string) => void;
}

export function CollectionsTab({
  collections,
  filteredCollections,
  addonsByWorkshopId,
  namePreference,
  busy,
  onInspectCollection,
  onDownloadWorkshopItem,
  onOpenFilterInLibrary,
  onUntrackCollection,
}: CollectionsTabProps) {
  const { t } = useTranslation();
  const filterCtx = useContentFilterContext();

  if (filteredCollections.length === 0) {
    return (
      <EmptyState
        icon={<Bookmark size={26} />}
        title={collections.length === 0 ? t('common.empty') : t('common.noResults')}
      />
    );
  }

  return (
    <div className="l4a-card-grid-wide">
      {filteredCollections.map((col) => {
        const visibleWids = col.memberWorkshopIds.filter((wid) => {
          const found = addonsByWorkshopId.get(wid);
          return !found || !resolveLibraryAddonCardFilter(found, filterCtx).hiddenCard;
        });
        const total = visibleWids.length;
        const missing = visibleWids.filter(
          (id) => addonsByWorkshopId.get(id)?.installState !== 'installed',
        );
        const installed = total - missing.length;
        const percent = total > 0 ? Math.round((installed / total) * 100) : 0;
        const previewTitles = visibleWids.slice(0, 3).map((wid) => {
          const found = addonsByWorkshopId.get(wid);
          return found ? formatFilteredAddonTitle(found, namePreference, filterCtx, t) : `#${wid}`;
        });

        return (
          <article
            key={col.collectionId}
            className="md-card"
            data-clickable="true"
            style={{ padding: 16, gap: 10 }}
          >
            <button
              type="button"
              className="l4a-card-trigger"
              onClick={() => onInspectCollection(col.collectionId)}
            >
              <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                <span className="md-title-medium l4a-truncate" style={{ flex: 1 }}>
                  {col.title}
                </span>
                <span className="md-pill md-mono" data-tone="surface">
                  #{col.collectionId}
                </span>
              </div>

              <div className="md-chip-row" style={{ marginTop: 8 }}>
                {col.autoSync && (
                  <span className="md-pill" data-tone="primary">
                    {t('organize.autoSync')}
                  </span>
                )}
                <span className="md-pill" data-tone="secondary">
                  {t('organize.installedRatio', { installed, total })}
                </span>
                {col.removedUpstreamIds.length > 0 && (
                  <span className="md-pill" data-tone="error">
                    {t('organize.upstreamRemoved', {
                      count: col.removedUpstreamIds.length,
                    })}
                  </span>
                )}
              </div>

              <div style={{ marginTop: 10 }}>
                <LinearProgress value={percent} label={col.title} />
              </div>

              {previewTitles.length > 0 && (
                <div className="l4a-member-preview">
                  {previewTitles.map((label, idx) => (
                    <span
                      key={`${col.collectionId}-${idx}`}
                      className="l4a-member-chip l4a-truncate"
                    >
                      {label}
                    </span>
                  ))}
                  {total > 3 && (
                    <span className="l4a-member-chip">
                      {t('organize.moreMembers', { count: total - 3 })}
                    </span>
                  )}
                </div>
              )}
            </button>

            <div className="l4a-addon-card-footer">
              <span
                className="md-body-small"
                style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
              >
                {col.lastSyncedAt ? formatTimestamp(col.lastSyncedAt) : '—'}
              </span>
              <div className="l4a-row-wrap" style={{ gap: 4 }}>
                {missing.length > 0 && (
                  <Button
                    variant="tonal"
                    size="sm"
                    icon={<Download size={13} />}
                    onClick={() => {
                      for (const id of missing) {
                        const found = addonsByWorkshopId.get(id);
                        const safeTitle =
                          found && !isAddonTitleRedacted(found, filterCtx)
                            ? found.title
                            : undefined;
                        onDownloadWorkshopItem(id, safeTitle);
                      }
                    }}
                  >
                    {t('organize.downloadMissing', { count: missing.length })}
                  </Button>
                )}
                <IconButton
                  size="sm"
                  label={t('organize.filterInLibrary')}
                  onClick={() =>
                    onOpenFilterInLibrary({
                      label: col.title,
                      workshopIds: visibleWids,
                    })
                  }
                >
                  <ExternalLink size={15} />
                </IconButton>
                <IconButton
                  size="sm"
                  label={t('organize.untrack')}
                  disabled={busy}
                  onClick={() => onUntrackCollection(col.collectionId)}
                >
                  <Trash2 size={15} />
                </IconButton>
              </div>
            </div>
          </article>
        );
      })}
    </div>
  );
}
