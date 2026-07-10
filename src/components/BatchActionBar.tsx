import { CheckSquare, Square, Unlock, Lock, FolderOpen, Edit3, FolderPlus, Library, X, Download, HardDrive, Globe } from 'lucide-react';
import { Addon, Group, MasterCollection } from '../types/addon';
import { useTranslation } from 'react-i18next';

interface BatchActionBarProps {
  selectedIds: string[];
  filteredItems: Addon[];
  addons: Record<string, Addon>;
  knownUninstalledAddons?: Record<string, Addon>;
  groups: Group[];
  masterCollections?: MasterCollection[];
  selectedGroupIds?: string[];
  onSelectAll: (visibleItems: Addon[]) => void;
  onSelectAllGroups?: (groups: Group[]) => void;
  onBatchToggle: (enabled: boolean) => void;
  onBatchMove: () => void;
  onBatchRename: () => void;
  onBatchAddToGroup: () => void;
  onBatchAddToMasterCollection?: () => void;
  onBatchDownload?: (ids: string[]) => void;
  onClearSelection: () => void;
  isSubmitting?: boolean;
}

export function BatchActionBar({
  selectedIds,
  filteredItems,
  addons,
  knownUninstalledAddons = {},
  groups,
  masterCollections = [],
  selectedGroupIds = [],
  onSelectAll,
  onSelectAllGroups,
  onBatchToggle,
  onBatchMove,
  onBatchRename,
  onBatchAddToGroup,
  onBatchAddToMasterCollection,
  onBatchDownload,
  onClearSelection,
  isSubmitting = false,
}: BatchActionBarProps) {
  const { t } = useTranslation();

  // Determine what's selected
  const hasAddonSelection = selectedIds.length > 0;
  const hasGroupSelection = selectedGroupIds.length > 0;
  const totalSelected = selectedIds.length + selectedGroupIds.length;

  const selectedAddons = selectedIds
    .map((id) => addons[id] || knownUninstalledAddons[id])
    .filter((addon): addon is Addon => Boolean(addon));

  // If any uninstalled addon is selected, certain actions shouldn't be available
  const hasUninstalled = selectedAddons.some((addon) => addon.dirType === 'none');
  const allInstalled = selectedAddons.every((addon) => addon.dirType !== 'none');
  
  const canBatchEnable = allInstalled && selectedAddons.some((addon) => !addon.isEnabled);
  const canBatchDisable = allInstalled && selectedAddons.some((addon) => addon.isEnabled);
  const canBatchMove = allInstalled && selectedAddons.some((addon) => addon.dirType === 'workshop');
  const canBatchRename = allInstalled && selectedAddons.length > 0;
  
  const uninstalledIds = selectedAddons.filter(a => a.dirType === 'none').map(a => a.id);
  const canBatchDownload = uninstalledIds.length > 0;

  // Check if we're in master collection view (only groups selected)
  const isGroupOnlySelection = hasGroupSelection && !hasAddonSelection;

  const hasDownloaded = filteredItems.some((item) => item.dirType !== 'none');
  const hasUndownloaded = filteredItems.some((item) => item.dirType === 'none');
  const showSelectiveSelection = selectedIds.length === 0 && hasDownloaded && hasUndownloaded;

  return (
    <div className="batch-action-bar">
      <div className="batch-action-info">
        {totalSelected === 0 ? t('batchActionBar.noneSelected') : t('batchActionBar.selectedCount', { count: totalSelected })}
      </div>
      <div className="batch-action-buttons">
        {/* Select All button */}
        {!isGroupOnlySelection && (
          <>
            <button
              className="btn btn-secondary"
              style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
              onClick={() => onSelectAll(filteredItems)}
              disabled={isSubmitting}
            >
              {filteredItems.every(item => selectedIds.includes(item.id)) ? <Square size={14} /> : <CheckSquare size={14} />}
              <span>
                {filteredItems.every(item => selectedIds.includes(item.id)) ? t('batchActionBar.deselectAll') : t('batchActionBar.selectAll')}
              </span>
            </button>

            {showSelectiveSelection && (
              <>
                <button
                  className="btn btn-secondary"
                  style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
                  onClick={() => onSelectAll(filteredItems.filter(item => item.dirType !== 'none'))}
                  disabled={isSubmitting}
                >
                  <HardDrive size={14} />
                  <span>{t('batchActionBar.selectDownloaded')}</span>
                </button>

                <button
                  className="btn btn-secondary"
                  style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
                  onClick={() => onSelectAll(filteredItems.filter(item => item.dirType === 'none'))}
                  disabled={isSubmitting}
                >
                  <Globe size={14} />
                  <span>{t('batchActionBar.selectUndownloaded')}</span>
                </button>
              </>
            )}
          </>
        )}

        {isGroupOnlySelection && onSelectAllGroups && (
          <button
            className="btn btn-secondary"
            style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
            onClick={() => onSelectAllGroups(groups)}
            disabled={isSubmitting}
          >
            {groups.every(g => selectedGroupIds.includes(g.id)) ? <Square size={14} /> : <CheckSquare size={14} />}
            <span>
              {groups.every(g => selectedGroupIds.includes(g.id)) ? t('batchActionBar.deselectAll') : t('batchActionBar.selectAll')}
            </span>
          </button>
        )}

        {/* Addon-specific actions */}
        {hasAddonSelection && (
          <>
            {canBatchEnable && (
              <button
                className="btn btn-primary"
                style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
                onClick={() => onBatchToggle(true)}
                disabled={isSubmitting}
              >
                <Unlock size={14} />
                <span>{t('batchActionBar.enable')}</span>
              </button>
            )}

            {canBatchDisable && (
              <button
                className="btn btn-secondary"
                style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
                onClick={() => onBatchToggle(false)}
                disabled={isSubmitting}
              >
                <Lock size={14} />
                <span>{t('batchActionBar.disable')}</span>
              </button>
            )}

            {canBatchMove && (
              <button
                className="btn btn-secondary"
                style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
                onClick={onBatchMove}
                disabled={isSubmitting}
              >
                <FolderOpen size={14} />
                <span>{t('batchActionBar.moveToManual')}</span>
              </button>
            )}

            {canBatchRename && (
              <button
                className="btn btn-secondary"
                style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
                onClick={onBatchRename}
                disabled={isSubmitting}
              >
                <Edit3 size={14} />
                <span>{t('batchActionBar.autoRename')}</span>
              </button>
            )}

            <button
              className="btn btn-secondary"
              style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
              onClick={onBatchAddToGroup}
              disabled={selectedIds.length === 0 || groups.length === 0 || isSubmitting}
            >
              <FolderPlus size={14} />
              <span>{t('batchActionBar.addToGroup')}</span>
            </button>

            {canBatchDownload && onBatchDownload && (
              <button
                className="btn btn-primary"
                style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
                onClick={() => onBatchDownload(uninstalledIds)}
                disabled={isSubmitting}
              >
                <Download size={14} />
                <span>{t('batchActionBar.download')}</span>
              </button>
            )}
          </>
        )}

        {/* Group-specific actions */}
        {hasGroupSelection && onBatchAddToMasterCollection && (
          <button
            className="btn btn-secondary"
            style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
            onClick={onBatchAddToMasterCollection}
            disabled={selectedGroupIds.length === 0 || masterCollections.length === 0 || isSubmitting}
          >
            <Library size={14} />
            <span>{t('batchActionBar.addToMasterCollection')}</span>
          </button>
        )}

        <button
          className="btn btn-secondary"
          style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px', color: 'var(--md-sys-color-error)' }}
          onClick={onClearSelection}
          disabled={isSubmitting}
        >
          <X size={14} />
          <span>{t('batchActionBar.exitBatch')}</span>
        </button>
      </div>
    </div>
  );
}
