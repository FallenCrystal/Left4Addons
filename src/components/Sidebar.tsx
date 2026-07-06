import React from 'react';
import { Layers, Grid, FolderOpen, Folder, Lock, FolderPlus, Plus, Sparkles, Settings } from 'lucide-react';
import { Addon, Group } from '../types/addon';

interface SidebarProps {
  addons: Record<string, Addon>;
  groups: Group[];
  currentFilterTab: string;
  selectedGroupId: string | null;
  onFilterTabChange: (tab: string, groupId: string | null) => void;
  onOpenGroupModal: () => void;
  onAutoGrouping: () => void;
  autoGrouping: boolean;
  disabledCount: number;
  totalAddonsCount: number;
}

export const Sidebar: React.FC<SidebarProps> = ({
  addons,
  groups,
  currentFilterTab,
  selectedGroupId,
  onFilterTabChange,
  onOpenGroupModal,
  onAutoGrouping,
  autoGrouping,
  disabledCount,
  totalAddonsCount,
}) => {
  const loadingCount = Object.values(addons).filter(a => a.dirType === 'loading').length;
  const workshopCount = Object.values(addons).filter(a => a.dirType === 'workshop').length;

  return (
    <div className="sidebar">
      <div className="sidebar-header">
        <div className="sidebar-logo">
          <Layers size={28} />
        </div>
        <div>
          <h1 className="sidebar-title">Left 4 Addons</h1>
          <div style={{ fontSize: '11px', color: 'var(--md-sys-color-outline)' }}>求生之路2附加组件管理</div>
        </div>
      </div>

      <div className="sidebar-menu">
        <button 
          className={`menu-item ${currentFilterTab === 'all' ? 'active' : ''}`}
          onClick={() => onFilterTabChange('all', null)}
        >
          <div className="menu-item-left">
            <Grid size={18} />
            <span>全部组件</span>
          </div>
          <span className="menu-item-badge">{totalAddonsCount}</span>
        </button>

        <button 
          className={`menu-item ${currentFilterTab === 'loading' ? 'active' : ''}`}
          onClick={() => onFilterTabChange('loading', null)}
        >
          <div className="menu-item-left">
            <FolderOpen size={18} />
            <span>手动安装 (Addons)</span>
          </div>
          <span className="menu-item-badge">{loadingCount}</span>
        </button>

        <button 
          className={`menu-item ${currentFilterTab === 'workshop' ? 'active' : ''}`}
          onClick={() => onFilterTabChange('workshop', null)}
        >
          <div className="menu-item-left">
            <Folder size={18} />
            <span>创意工坊目录 (Workshop)</span>
          </div>
          <span className="menu-item-badge">{workshopCount}</span>
        </button>

        <button 
          className={`menu-item ${currentFilterTab === 'disabled' ? 'active' : ''}`}
          onClick={() => onFilterTabChange('disabled', null)}
        >
          <div className="menu-item-left">
            <Lock size={18} />
            <span>被禁用的组件 (.disabled)</span>
          </div>
          <span className="menu-item-badge">{disabledCount}</span>
        </button>

        <div className="sidebar-section-title">我的分组</div>
        
        <div className="groups-list">
          {groups.length === 0 ? (
            <div style={{ padding: '8px 16px', fontSize: '12px', color: 'var(--md-sys-color-outline)' }}>
              暂无分组，可自动归类或手动创建。
            </div>
          ) : (
            groups.map(group => (
              <button
                key={group.id}
                className={`menu-item ${currentFilterTab === 'groups' && selectedGroupId === group.id ? 'active' : ''}`}
                onClick={() => onFilterTabChange('groups', group.id)}
              >
                <div className="menu-item-left">
                  <FolderPlus size={16} />
                  <span style={{ maxWidth: '160px', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {group.name}
                  </span>
                </div>
                <span className="menu-item-badge">{group.addons?.length || 0}</span>
              </button>
            ))
          )}
        </div>
      </div>

      <div className="sidebar-footer">
        <button 
          className="btn btn-secondary" 
          style={{ width: '100%' }}
          onClick={onOpenGroupModal}
        >
          <Plus size={16} />
          <span>新建手动分组</span>
        </button>
        
        <button 
          className="btn btn-tertiary" 
          style={{ width: '100%' }}
          onClick={onAutoGrouping}
          disabled={autoGrouping}
        >
          <Sparkles size={16} />
          <span>{autoGrouping ? '归类中...' : '自动识别战役并归类'}</span>
        </button>

        <button 
          className={`menu-item ${currentFilterTab === 'settings' ? 'active' : ''}`}
          style={{ 
            width: '100%', 
            marginTop: '8px', 
            borderRadius: '100px', 
            display: 'flex', 
            alignItems: 'center', 
            gap: '12px',
            boxSizing: 'border-box'
          }}
          onClick={() => onFilterTabChange('settings', null)}
        >
          <div className="menu-item-left">
            <Settings size={18} />
            <span>设置</span>
          </div>
        </button>
      </div>
    </div>
  );
};
