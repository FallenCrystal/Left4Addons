import { CheckSquare, Unlock, Lock, FolderOpen, Edit3, FolderPlus, X } from 'lucide-react';
import { Addon, Group } from '../types/addon';
import { useTranslation } from 'react-i18next';

interface BatchActionBarProps {
  selectedVpkNames: string[];
  filteredItems: Addon[];
  addons: Record<string, Addon>;
  groups: Group[];
  onSelectAll: (visibleItems: Addon[]) => void;
  onBatchToggle: (enabled: boolean) => void;
  onBatchMove: () => void;
  onBatchRename: () => void;
  onBatchAddToGroup: (groupId: string) => void;
  onClearSelection: () => void;
}

export function BatchActionBar({
  selectedVpkNames,
  filteredItems,
  addons,
  groups,
  onSelectAll,
  onBatchToggle,
  onBatchMove,
  onBatchRename,
  onBatchAddToGroup,
  onClearSelection,
}: BatchActionBarProps) {
  const { t } = useTranslation();
  const isAllSelected = filteredItems.every(item => selectedVpkNames.includes(item.vpkName));
  const hasWorkshopSelected = selectedVpkNames.some(name => addons[name]?.dirType === 'workshop');

  return (
    <div className="batch-action-bar">
      <div className="batch-action-info">
        {t('batchActionBar.selectedCount', { count: selectedVpkNames.length })}
      </div>
      <div className="batch-action-buttons">
        <button 
          className="btn btn-secondary"
          style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
          onClick={() => onSelectAll(filteredItems)}
        >
          <CheckSquare size={14} />
          <span>
            {isAllSelected ? t('batchActionBar.deselectAll') : t('batchActionBar.selectAll')}
          </span>
        </button>

        <button 
          className="btn btn-primary"
          style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
          onClick={() => onBatchToggle(true)}
          disabled={selectedVpkNames.length === 0}
        >
          <Unlock size={14} />
          <span>{t('batchActionBar.enable')}</span>
        </button>

        <button 
          className="btn btn-secondary"
          style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
          onClick={() => onBatchToggle(false)}
          disabled={selectedVpkNames.length === 0}
        >
          <Lock size={14} />
          <span>{t('batchActionBar.disable')}</span>
        </button>

        {hasWorkshopSelected && (
          <button 
            className="btn btn-secondary"
            style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
            onClick={onBatchMove}
            disabled={selectedVpkNames.length === 0}
          >
            <FolderOpen size={14} />
            <span>{t('batchActionBar.moveToManual')}</span>
          </button>
        )}

        <button 
          className="btn btn-secondary"
          style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
          onClick={onBatchRename}
          disabled={selectedVpkNames.length === 0}
        >
          <Edit3 size={14} />
          <span>{t('batchActionBar.autoRename')}</span>
        </button>

        <div className="dropdown">
          <button 
            className="btn btn-secondary"
            style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
            disabled={selectedVpkNames.length === 0}
          >
            <FolderPlus size={14} />
            <span>{t('batchActionBar.addToGroup')}</span>
          </button>
          <div className="dropdown-content">
            {groups.map(g => (
              <button key={g.id} onClick={() => onBatchAddToGroup(g.id)}>
                {g.name}
              </button>
            ))}
            {groups.length === 0 && (
              <button disabled style={{ fontStyle: 'italic' }}>{t('batchActionBar.noGroups')}</button>
            )}
          </div>
        </div>

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
