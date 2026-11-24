import { useTranslation } from 'react-i18next';
import {
  ArrowRightLeft,
  ArrowUpCircle,
  Download,
  FolderPlus,
  Layers,
  Pencil,
  Power,
  PowerOff,
  Trash2,
  X,
} from 'lucide-react';
import type { AddonDirType, AddonNamePreference, AddonSummary } from '../../../api/ipc';
import { Button, FilterChip, IconButton } from '../../m3/primitives';
import { isRemoteUnavailable, resolveLibraryMetadataTitle } from './helpers';

export interface LibraryBatchBarProps {
  visibleSelectedAddons: AddonSummary[];
  visibleSelectedIds: string[];
  hiddenSelectedCount: number;
  namePreference: AddonNamePreference;
  busy: boolean;
  onKeepOnlyVisibleSelection: () => void;
  onSelectAllVisible: () => void;
  onClearSelection: () => void;
  onBatchToggle: (addonIds: string[], enabled: boolean) => void;
  onBatchMove: (addonIds: string[], targetDir: AddonDirType) => void;
  onBatchRename: (addonIds: string[]) => void;
  onBatchAddToGroup: (addonIds: string[]) => void;
  onBatchCreateBundle: (addonIds: string[]) => void;
  onBatchDelete: (addons: AddonSummary[]) => void;
  onDownloadItem: (workshopId: string, title?: string) => void;
  onUpdateItem: (workshopId: string, title?: string) => void;
}

export function LibraryBatchBar({
  visibleSelectedAddons,
  visibleSelectedIds,
  hiddenSelectedCount,
  namePreference,
  busy,
  onKeepOnlyVisibleSelection,
  onSelectAllVisible,
  onClearSelection,
  onBatchToggle,
  onBatchMove,
  onBatchRename,
  onBatchAddToGroup,
  onBatchCreateBundle,
  onBatchDelete,
  onDownloadItem,
  onUpdateItem,
}: LibraryBatchBarProps) {
  const { t } = useTranslation();

  const loadingToggleTargets = visibleSelectedAddons
    .filter((a) => a.installState === 'installed' && a.primaryDir === 'loading')
    .map((a) => a.id);
  const workshopToLoadingTargets = visibleSelectedAddons
    .filter((a) => a.installState === 'installed' && a.primaryDir === 'workshop')
    .map((a) => a.id);
  const loadingToWorkshopTargets = loadingToggleTargets;
  const renameTargets = loadingToggleTargets;
  const installableTargets = visibleSelectedAddons.filter(
    (a) => a.workshopId && a.installState !== 'installed' && !isRemoteUnavailable(a),
  );
  const updatableTargets = visibleSelectedAddons.filter(
    (a) =>
      a.workshopId &&
      a.installState === 'installed' &&
      a.updateAvailable &&
      !isRemoteUnavailable(a),
  );

  return (
    <div className="l4a-batch-bar">
      <span className="md-label-large">
        {t('common.selected', { count: visibleSelectedIds.length })}
      </span>
      {hiddenSelectedCount > 0 && (
        <FilterChip
          label={t('library.hiddenSelected', { count: hiddenSelectedCount })}
          tone="surface"
          onRemove={onKeepOnlyVisibleSelection}
          onClick={onKeepOnlyVisibleSelection}
        />
      )}
      <Button variant="text" size="sm" onClick={onSelectAllVisible}>
        {t('library.selectAll')}
      </Button>
      <div className="l4a-spacer" />
      <Button
        variant="text"
        size="sm"
        disabled={busy || loadingToggleTargets.length === 0}
        icon={<Power size={14} />}
        onClick={() => onBatchToggle(loadingToggleTargets, true)}
      >
        {t('common.enable')}
      </Button>
      <Button
        variant="text"
        size="sm"
        disabled={busy || loadingToggleTargets.length === 0}
        icon={<PowerOff size={14} />}
        onClick={() => onBatchToggle(loadingToggleTargets, false)}
      >
        {t('common.disable')}
      </Button>
      {workshopToLoadingTargets.length > 0 && (
        <Button
          variant="text"
          size="sm"
          disabled={busy}
          icon={<ArrowRightLeft size={14} />}
          onClick={() => onBatchMove(workshopToLoadingTargets, 'loading')}
        >
          {t('library.moveToLoading')}
        </Button>
      )}
      {loadingToWorkshopTargets.length > 0 && (
        <Button
          variant="text"
          size="sm"
          disabled={busy}
          icon={<ArrowRightLeft size={14} />}
          onClick={() => onBatchMove(loadingToWorkshopTargets, 'workshop')}
        >
          {t('library.moveToWorkshop')}
        </Button>
      )}
      <Button
        variant="text"
        size="sm"
        disabled={busy || renameTargets.length === 0}
        icon={<Pencil size={14} />}
        onClick={() => onBatchRename(renameTargets)}
      >
        {t('library.autoRename')}
      </Button>
      <Button
        variant="text"
        size="sm"
        disabled={busy || visibleSelectedIds.length === 0}
        icon={<FolderPlus size={14} />}
        onClick={() => onBatchAddToGroup(visibleSelectedIds)}
      >
        {t('library.addToGroup')}
      </Button>
      <Button
        variant="text"
        size="sm"
        disabled={busy || visibleSelectedIds.length === 0}
        icon={<Layers size={14} />}
        onClick={() => onBatchCreateBundle(visibleSelectedIds)}
      >
        {t('library.createBundle')}
      </Button>
      {installableTargets.length > 0 && (
        <Button
          variant="filled"
          size="sm"
          disabled={busy}
          icon={<Download size={14} />}
          onClick={() => {
            for (const item of installableTargets) {
              if (item.workshopId) {
                onDownloadItem(
                  item.workshopId,
                  resolveLibraryMetadataTitle(item, namePreference),
                );
              }
            }
          }}
        >
          {t('common.download')} ({installableTargets.length})
        </Button>
      )}
      {updatableTargets.length > 0 && (
        <Button
          variant="filled"
          size="sm"
          disabled={busy}
          icon={<ArrowUpCircle size={14} />}
          onClick={() => {
            for (const item of updatableTargets) {
              if (item.workshopId) {
                onUpdateItem(item.workshopId, resolveLibraryMetadataTitle(item, namePreference));
              }
            }
          }}
        >
          {t('common.update')} ({updatableTargets.length})
        </Button>
      )}
      <Button
        variant="text"
        tone="error"
        size="sm"
        disabled={busy || visibleSelectedAddons.length === 0}
        icon={<Trash2 size={14} />}
        onClick={() => onBatchDelete(visibleSelectedAddons)}
      >
        {t('common.delete')}
      </Button>
      <IconButton size="sm" label={t('common.close')} onClick={onClearSelection}>
        <X size={16} />
      </IconButton>
    </div>
  );
}
