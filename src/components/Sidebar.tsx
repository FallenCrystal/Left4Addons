import React from 'react';
import { Layers, Grid, FolderOpen, Folder, Lock, FolderPlus, Plus, Sparkles, Settings, Download, Globe, Library } from 'lucide-react';
import { Addon, Group, MasterCollection } from '../types/addon';
import { useTranslation } from 'react-i18next';

interface SidebarProps {
  addons: Record<string, Addon>;
  groups: Group[];
  masterCollections: MasterCollection[];
  currentFilterTab: string;
  selectedGroupId: string | null;
  selectedMasterCollectionId: string | null;
  onFilterTabChange: (tab: string, groupId: string | null) => void;
  onOpenGroupModal: () => void;
  onOpenMasterCollectionModal: () => void;
  onAutoGrouping: () => void;
  autoGrouping: boolean;
  disabledCount: number;
  totalAddonsCount: number;
  knownUninstalledCount: number;
}

export const Sidebar: React.FC<SidebarProps> = ({
  addons,
  groups,
  masterCollections,
  currentFilterTab,
  selectedGroupId,
  selectedMasterCollectionId,
  onFilterTabChange,
  onOpenGroupModal,
  onOpenMasterCollectionModal,
  onAutoGrouping,
  autoGrouping,
  disabledCount,
  totalAddonsCount,
  knownUninstalledCount,
}) => {
  const { t } = useTranslation();
  const loadingCount = Object.values(addons).filter(a => a.dirType === 'loading').length;
  const workshopCount = Object.values(addons).filter(a => a.dirType === 'workshop').length;

  const getMasterCollectionDisplayName = (mc: MasterCollection) => {
    if (mc.nameKey) {
      return t(mc.nameKey, mc.name);
    }
    return mc.name;
  };

  return (
    <div className="sidebar">
      <div className="sidebar-header">
        <div className="sidebar-logo">
          <Layers size={28} />
        </div>
        <div>
          <h1 className="sidebar-title">Left 4 Addons</h1>
          <div style={{ fontSize: '11px', color: 'var(--md-sys-color-outline)' }}>{t('sidebar.title')}</div>
        </div>
      </div>

      <div className="sidebar-menu">
        <button
          className={`menu-item ${currentFilterTab === 'all' ? 'active' : ''}`}
          onClick={() => onFilterTabChange('all', null)}
        >
          <div className="menu-item-left">
            <Grid size={18} />
            <span>{t('sidebar.allAddons')}</span>
          </div>
          <span className="menu-item-badge">{totalAddonsCount}</span>
        </button>

        <button
          className={`menu-item ${currentFilterTab === 'loading' ? 'active' : ''}`}
          onClick={() => onFilterTabChange('loading', null)}
        >
          <div className="menu-item-left">
            <FolderOpen size={18} />
            <span>{t('sidebar.manualInstall')}</span>
          </div>
          <span className="menu-item-badge">{loadingCount}</span>
        </button>

        <button
          className={`menu-item ${currentFilterTab === 'workshop' ? 'active' : ''}`}
          onClick={() => onFilterTabChange('workshop', null)}
        >
          <div className="menu-item-left">
            <Folder size={18} />
            <span>{t('sidebar.workshopDir')}</span>
          </div>
          <span className="menu-item-badge">{workshopCount}</span>
        </button>

        <button
          className={`menu-item ${currentFilterTab === 'disabled' ? 'active' : ''}`}
          onClick={() => onFilterTabChange('disabled', null)}
        >
          <div className="menu-item-left">
            <Lock size={18} />
            <span>{t('sidebar.disabledAddons')}</span>
          </div>
          <span className="menu-item-badge">{disabledCount}</span>
        </button>

        <button
          className={`menu-item ${currentFilterTab === 'known-uninstalled' ? 'active' : ''}`}
          onClick={() => onFilterTabChange('known-uninstalled', null)}
        >
          <div className="menu-item-left">
            <Download size={18} />
            <span>{t('sidebar.disabledAddons') === '被禁用的组件 (.disabled)' ? '未安装的已知组件' : 'Known Uninstalled'}</span>
          </div>
          <span className="menu-item-badge">{knownUninstalledCount}</span>
        </button>

        <button
          className={`menu-item ${currentFilterTab === 'workshop-browser' ? 'active' : ''}`}
          onClick={() => onFilterTabChange('workshop-browser', null)}
        >
          <div className="menu-item-left">
            <Globe size={18} />
            <span>{t('sidebar.disabledAddons') === '被禁用的组件 (.disabled)' ? '内置创意工坊' : 'Built-in Workshop'}</span>
          </div>
        </button>

        {/* Master Collections Section */}
        <div className="sidebar-section-title">{t('sidebar.masterCollections')}</div>

        <div className="groups-list">
          {masterCollections.length === 0 ? (
            <div style={{ padding: '8px 16px', fontSize: '12px', color: 'var(--md-sys-color-outline)' }}>
              {t('sidebar.noMasterCollections')}
            </div>
          ) : (
            masterCollections.map(mc => (
              <button
                key={mc.id}
                className={`menu-item ${currentFilterTab === 'master-collection' && selectedMasterCollectionId === mc.id ? 'active' : ''}`}
                onClick={() => onFilterTabChange('master-collection', mc.id)}
              >
                <div className="menu-item-left">
                  <Library size={16} />
                  <span style={{ maxWidth: '160px', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {getMasterCollectionDisplayName(mc)}
                  </span>
                </div>
                <span className="menu-item-badge">{mc.groupIds?.length || 0}</span>
              </button>
            ))
          )}
        </div>

        {/* Groups Section */}
        <div className="sidebar-section-title">{t('sidebar.myGroups')}</div>

        <div className="groups-list">
          {groups.filter(g => !g.masterCollectionIds || g.masterCollectionIds.length === 0).length === 0 ? (
            <div style={{ padding: '8px 16px', fontSize: '12px', color: 'var(--md-sys-color-outline)' }}>
              {t('sidebar.noGroups')}
            </div>
          ) : (
            groups.filter(g => !g.masterCollectionIds || g.masterCollectionIds.length === 0).map(group => (
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
          <span>{t('sidebar.newGroup')}</span>
        </button>

        <button
          className="btn btn-secondary"
          style={{ width: '100%' }}
          onClick={onOpenMasterCollectionModal}
        >
          <Library size={16} />
          <span>{t('sidebar.newMasterCollection')}</span>
        </button>

        <button
          className="btn btn-tertiary"
          style={{ width: '100%' }}
          onClick={onAutoGrouping}
          disabled={autoGrouping}
        >
          <Sparkles size={16} />
          <span>{autoGrouping ? t('sidebar.classifying') : t('sidebar.autoClassify')}</span>
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
            <span>{t('sidebar.settings')}</span>
          </div>
        </button>
      </div>
    </div>
  );
};
