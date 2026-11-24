import { useTranslation } from 'react-i18next';
import {
  ExternalLink,
  FolderOpen,
  Layers,
  Lock,
  Map as MapIcon,
  Trash2,
  Unlock,
} from 'lucide-react';
import type { AddonNamePreference, AddonSummary, BundleRecord } from '../../../api/ipc';
import { formatBundleMemberLabel, formatBytes } from '../../../utils/format';
import { AddonCover } from '../../m3/AddonCover';
import { Button, EmptyState, IconButton, Switch } from '../../m3/primitives';
import type { ExternalAddonFilter } from '../LibraryView';
import { summarizeGroupMembers } from './helpers';
import { GroupStatusPill } from './OrganizeShared';

export interface BundlesTabProps {
  bundles: BundleRecord[];
  filteredBundles: BundleRecord[];
  addonsById: Map<string, AddonSummary>;
  namePreference: AddonNamePreference;
  busy: boolean;
  onInspectBundle: (bundleId: string) => void;
  onToggleBundleLock: (bundleId: string, locked: boolean) => void;
  onBulkToggle: (addonIds: string[], enabled: boolean) => void;
  onOpenInFileManager: (path: string) => void;
  onDeleteBundle: (bundleId: string) => void;
  onOpenFilterInLibrary: (filter: ExternalAddonFilter) => void;
}

export function BundlesTab({
  bundles,
  filteredBundles,
  addonsById,
  namePreference,
  busy,
  onInspectBundle,
  onToggleBundleLock,
  onBulkToggle,
  onOpenInFileManager,
  onDeleteBundle,
  onOpenFilterInLibrary,
}: BundlesTabProps) {
  const { t } = useTranslation();

  if (filteredBundles.length === 0) {
    return (
      <EmptyState
        icon={<Layers size={26} />}
        title={bundles.length === 0 ? t('common.empty') : t('common.noResults')}
      />
    );
  }

  return (
    <div className="l4a-card-grid-wide">
      {filteredBundles.map((bundle) => {
        const summary = summarizeGroupMembers(bundle.memberAddonIds, addonsById);
        const toggleableIds = summary.loading.map((a) => a.id);
        return (
          <article
            key={bundle.id}
            className="md-card"
            data-clickable="true"
            data-muted={summary.loading.length > 0 && summary.noneEnabled ? 'true' : 'false'}
          >
            <div className="l4a-addon-cover-wrap" style={{ aspectRatio: '21 / 9' }}>
              <button
                type="button"
                tabIndex={-1}
                aria-hidden="true"
                className="l4a-card-trigger"
                style={{ height: '100%' }}
                onClick={() => onInspectBundle(bundle.id)}
              >
                <AddonCover
                  src={bundle.coverImage ?? summary.firstCover}
                  fallbackSrc={summary.firstLocalCover}
                  title={bundle.name}
                  category="Campaign"
                />
              </button>
              <div className="l4a-cover-overlay-top">
                <span className="md-pill" data-tone="secondary">
                  {t(`organize.bundleKind.${bundle.kind}`)}
                </span>
                <div className="l4a-row-wrap" style={{ gap: 6 }}>
                  <GroupStatusPill summary={summary} />
                  <IconButton
                    size="sm"
                    variant={bundle.locked ? 'filled' : 'tonal'}
                    label={bundle.locked ? t('organize.unlock') : t('organize.lock')}
                    disabled={busy}
                    onClick={(e) => {
                      e.stopPropagation();
                      onToggleBundleLock(bundle.id, !bundle.locked);
                    }}
                  >
                    {bundle.locked ? <Lock size={14} /> : <Unlock size={14} />}
                  </IconButton>
                </div>
              </div>
            </div>

            <div className="l4a-addon-card-body">
              <button
                type="button"
                className="l4a-card-trigger"
                onClick={() => onInspectBundle(bundle.id)}
              >
                <div className="md-title-medium l4a-truncate">{bundle.name}</div>
                <div className="l4a-addon-meta" style={{ marginTop: 4 }}>
                  <span>{t('common.items', { count: bundle.memberAddonIds.length })}</span>
                  {summary.totalSize > 0 && (
                    <>
                      <span>·</span>
                      <span>{formatBytes(summary.totalSize)}</span>
                    </>
                  )}
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
                {summary.members.length > 0 && (
                  <div className="l4a-member-preview">
                    {summary.members.slice(0, 3).map((m) => (
                      <span key={m.id} className="l4a-member-chip l4a-truncate">
                        {formatBundleMemberLabel(m, summary.members, namePreference, bundle.name)}
                      </span>
                    ))}
                    {summary.members.length > 3 && (
                      <span className="l4a-member-chip">
                        {t('organize.moreMembers', { count: summary.members.length - 3 })}
                      </span>
                    )}
                  </div>
                )}
              </button>

              <div className="l4a-addon-card-footer">
                <div className="l4a-row-wrap" style={{ gap: 6 }}>
                  {toggleableIds.length > 0 && (
                    <Switch
                      compact
                      checked={summary.allEnabled}
                      disabled={busy}
                      label={t('organize.toggleAll')}
                      onChange={(next) => onBulkToggle(toggleableIds, next)}
                    />
                  )}
                  {summary.firstPath && (
                    <IconButton
                      size="sm"
                      label={t('common.openInExplorer')}
                      onClick={() => onOpenInFileManager(summary.firstPath!)}
                    >
                      <FolderOpen size={15} />
                    </IconButton>
                  )}
                  {bundle.kind === 'manual' && (
                    <IconButton
                      size="sm"
                      label={t('common.delete')}
                      disabled={busy}
                      onClick={() => onDeleteBundle(bundle.id)}
                    >
                      <Trash2 size={15} />
                    </IconButton>
                  )}
                </div>
                <Button
                  variant="text"
                  size="sm"
                  icon={<ExternalLink size={14} />}
                  onClick={() =>
                    onOpenFilterInLibrary({
                      label: bundle.name,
                      addonIds: bundle.memberAddonIds,
                    })
                  }
                >
                  {t('organize.filterInLibrary')}
                </Button>
              </div>
            </div>
          </article>
        );
      })}
    </div>
  );
}
