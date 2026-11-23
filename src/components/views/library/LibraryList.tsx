import { useTranslation } from 'react-i18next';
import {
  ArrowUpCircle,
  CloudOff,
  Download,
  FolderOpen,
  Layers,
  Map as MapIcon,
  ShieldCheck,
} from 'lucide-react';
import type { AddonNamePreference, AddonSummary, BundleRecord } from '../../../api/ipc';
import type { LiveDownloadState } from '../../../state/useAppController';
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
  onToggleSelectMany,
  onInspectBundle,
  onBatchToggle,
  onOpenInFileManager,
}: BundleListRowProps) {
  const { t } = useTranslation();
  const summary = summarizeBundle(allMembers);
  const visibleIds = visibleMembers.map((m) => m.id);
  const selected = visibleIds.length > 0 && visibleIds.every((id) => validSelectedIds.has(id));
  const loadingIds = summary.loading.map((m) => m.id);
  const muted = summary.installed.length > 0 && summary.enabledInstalled.length === 0;

  return (
    <div
      className="l4a-list-row"
      data-selected={selected ? 'true' : 'false'}
      data-muted={muted ? 'true' : 'false'}
    >
      <Checkbox
        checked={selected}
        label={bundle.name}
        onChange={() => onToggleSelectMany(visibleIds)}
      />
      <div className="l4a-list-thumb">
        <AddonCover
          compact
          src={bundle.coverImage ?? summary.firstCover}
          fallbackSrc={summary.firstLocalCover}
          title={bundle.name}
          category="Campaign"
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
          <span className="md-title-small l4a-truncate">{bundle.name}</span>
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
          {loadingIds.length > 0 && !summary.allLoadingEnabled && (
            <span
              className="md-pill"
              data-tone={summary.noneLoadingEnabled ? 'surface' : 'tertiary'}
            >
              {summary.noneLoadingEnabled
                ? t('common.disabled')
                : t('organize.statusPartial', {
                    enabled: summary.enabledLoading.length,
                    total: summary.loading.length,
                  })}
            </span>
          )}
        </div>
        <div className="l4a-addon-meta">
          <span className="l4a-truncate">{summary.firstAuthor || t('common.unknown')}</span>
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
        {summary.primaryDir && (
          <span className="md-pill" data-tone="surface">
            {summary.primaryDir === 'loading'
              ? t('library.loadingDir')
              : t('library.workshopDir')}
          </span>
        )}
        {summary.firstPath && (
          <IconButton
            size="sm"
            label={t('common.openInExplorer')}
            onClick={(e) => {
              e.stopPropagation();
              onOpenInFileManager(summary.firstPath!);
            }}
          >
            <FolderOpen size={15} />
          </IconButton>
        )}
        {loadingIds.length > 0 && (
          <Switch
            compact
            checked={summary.allLoadingEnabled}
            disabled={busy}
            label={bundle.name}
            onChange={(next) => onBatchToggle(loadingIds, next)}
          />
        )}
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
  onToggleSelect,
  onSelectAddon,
  onToggleSingle,
  onDownloadItem,
  onOpenInFileManager,
}: AddonListRowProps) {
  const { t } = useTranslation();
  const displayName = formatAddonDisplayName(addon, namePreference);
  const installed = addon.installState === 'installed';
  const remoteUnavailable = isRemoteUnavailable(addon);

  return (
    <div
      className="l4a-list-row"
      data-selected={selected ? 'true' : 'false'}
      data-muted={installed && !addon.enabled ? 'true' : 'false'}
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
          <span className="l4a-truncate">{addon.author || t('common.unknown')}</span>
          <span>·</span>
          <span>{formatBytes(addon.fileSize)}</span>
          {addon.workshopId && (
            <>
              <span>·</span>
              <span className="md-mono">#{addon.workshopId}</span>
            </>
          )}
        </div>
      </button>
      <div className="l4a-list-trailing">
        {addon.primaryDir && (
          <span className="md-pill" data-tone="surface">
            {addon.primaryDir === 'loading' ? t('library.loadingDir') : t('library.workshopDir')}
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
              onDownloadItem(addon.workshopId!, resolveLibraryMetadataTitle(addon, namePreference))
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
  );
}

export function LibraryList({
  displayItems,
  namePreference,
  validSelectedIds,
  batchMode,
  liveDownloads,
  busy,
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
