import { Edit3, Trash2, Unlock, Lock, FolderOpen } from 'lucide-react';
import { Group } from '../types/addon';

interface GroupHeaderProps {
  currentGroup: Group;
  onRenameGroup: () => void;
  onDeleteGroup: () => void;
  onGroupActionBatch: (actionType: 'enable' | 'disable' | 'move-load') => void;
}

export function GroupHeader({
  currentGroup,
  onRenameGroup,
  onDeleteGroup,
  onGroupActionBatch,
}: GroupHeaderProps) {
  return (
    <div style={{ 
      backgroundColor: 'var(--md-sys-surface-container-high)',
      border: '1px solid var(--md-sys-color-outline-variant)',
      padding: '24px',
      borderRadius: '24px',
      marginBottom: '24px',
      display: 'flex',
      flexDirection: 'column',
      gap: '16px'
    }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <div>
          <h2 style={{ margin: 0, fontSize: '24px', color: '#fff' }}>{currentGroup.name}</h2>
          <p style={{ margin: '4px 0 0 0', fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
            此分组包含 {currentGroup.addons?.length || 0} 个附件文件，可进行批量管理。
          </p>
        </div>
        <div style={{ display: 'flex', gap: '8px' }}>
          <button 
            className="btn btn-secondary"
            onClick={onRenameGroup}
          >
            <Edit3 size={14} />
            <span>重命名分组</span>
          </button>
          <button 
            className="btn btn-secondary" 
            style={{ color: 'var(--md-sys-color-error)' }}
            onClick={onDeleteGroup}
          >
            <Trash2 size={14} />
            <span>解散分组</span>
          </button>
        </div>
      </div>

      <div style={{ display: 'flex', gap: '8px' }}>
        <button className="btn btn-primary" onClick={() => onGroupActionBatch('enable')}>
          <Unlock size={14} />
          <span>全部启用</span>
        </button>
        <button className="btn btn-secondary" onClick={() => onGroupActionBatch('disable')}>
          <Lock size={14} />
          <span>全部禁用</span>
        </button>
        <button className="btn btn-tertiary" onClick={() => onGroupActionBatch('move-load')}>
          <FolderOpen size={14} />
          <span>全部移动至手动安装</span>
        </button>
      </div>
    </div>
  );
}
