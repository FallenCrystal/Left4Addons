import { CheckSquare, Unlock, Lock, FolderOpen, Edit3, FolderPlus, Library, X } from 'lucide-react';
import { Addon, Group, MasterCollection } from '../types/addon';
import { useTranslation } from 'react-i18next';

interface BatchActionBarProps {
  selectedIds: string[];
  filteredItems: Addon[];
  addons: Record<string, Addon>;
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
  onClearSelection: () => void;
}

export function BatchActionBar({
  selectedIds,
  filteredItems,
  addons,
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
  onClearSelection,
}: BatchActionBarProps) {
  const { t } = useTranslation();

  // Determine what's selected
  const hasAddonSelection = selectedIds.length > 0;
  const hasGroupSelection = selectedGroupIds.length > 0;
  const totalSelected = selectedIds.length + selectedGroupIds.length;

  const hasWorkshopSelected = selectedIds.some(name => addons[name]?.dirType === 'workshop');

  // Check if we're in master collection view (only groups selected)
  const isGroupOnlySelection = hasGroupSelection && !hasAddonSelection;

  return (
    <div className="batch-action-bar">
      <div className="batch-action-info">
        {t('batchActionBar.selectedCount', { count: totalSelected })}
      </div>
      <div className="batch-action-buttons">
        {/* Select All button */}
        {!isGroupOnlySelection && (
          <button
            className="btn btn-secondary"
            style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
            onClick={() => onSelectAll(filteredItems)}
          >
            <CheckSquare size={14} />
            <span>
              {filteredItems.every(item => selectedIds.includes(item.id)) ? t('batchActionBar.deselectAll') : t('batchActionBar.selectAll')}
            </span>
          </button>
        )}

        {isGroupOnlySelection && onSelectAllGroups && (
          <button
            className="btn btn-secondary"
            style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
            onClick={() => onSelectAllGroups(groups)}
          >
            <CheckSquare size={14} />
            <span>
              {groups.every(g => selectedGroupIds.includes(g.id)) ? t('batchActionBar.deselectAll') : t('batchActionBar.selectAll')}
            </span>
          </button>
        )}

        {/* Addon-specific actions */}
        {hasAddonSelection && (
          <>
            <button
              className="btn btn-primary"
              style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
              onClick={() => onBatchToggle(true)}
              disabled={selectedIds.length === 0}
            >
              <Unlock size={14} />
              <span>{t('batchActionBar.enable')}</span>
            </button>

            <button
              className="btn btn-secondary"
              style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
              onClick={() => onBatchToggle(false)}
              disabled={selectedIds.length === 0}
            >
              <Lock size={14} />
              <span>{t('batchActionBar.disable')}</span>
            </button>

            {hasWorkshopSelected && (
              <button
                className="btn btn-secondary"
                style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
                onClick={onBatchMove}
                disabled={selectedIds.length === 0}
              >
                <FolderOpen size={14} />
                <span>{t('batchActionBar.moveToManual')}</span>
              </button>
            )}

            <button
              className="btn btn-secondary"
              style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
              onClick={onBatchRename}
              disabled={selectedIds.length === 0}
            >
              <Edit3 size={14} />
              <span>{t('batchActionBar.autoRename')}</span>
            </button>

            <button
              className="btn btn-secondary"
              style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
              onClick={onBatchAddToGroup}
              disabled={selectedIds.length === 0 || groups.length === 0}
            >
              <FolderPlus size={14} />
              <span>{t('batchActionBar.addToGroup')}</span>
            </button>
          </>
        )}

        {/* Group-specific actions */}
        {hasGroupSelection && onBatchAddToMasterCollection && (
          <button
            className="btn btn-secondary"
            style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
            onClick={onBatchAddToMasterCollection}
            disabled={selectedGroupIds.length === 0 || masterCollections.length === 0}
          >
            <Library size={14} />
            <span>{t('batchActionBar.addToMasterCollection')}</span>
          </button>
        )}

        <button
          className="btn btn-secondary"
          style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px', color: 'var(--md-sys-color-error)' }}
          onClick={onClearSelection}
        >
          <X size={14} />
          <span>{t('batchActionBar.exitBatch')}</span>
        </button>
      </div>
    </div>
  );
}
