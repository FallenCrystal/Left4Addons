import React from 'react';
import { Folder, FolderOpen } from 'lucide-react';
import { Addon, Group } from '../types/addon';
import { formatBytes, getAddonAuthor } from '../utils/addonHelpers';
import { CacheImage } from './CacheImage';

interface GroupCardProps {
  group: Group;
  addons: Addon[];
  onToggleGroup: (addonsList: Addon[], enabled: boolean) => void;
  onViewGroupDetails: (groupId: string) => void;
  isSelectMode?: boolean;
  isGroupSelected?: boolean;
  onSelectGroupToggle?: (addonsList: Addon[]) => void;
  isSubmitting?: boolean;
}

export const GroupCard: React.FC<GroupCardProps> = ({
  group,
  addons,
  onToggleGroup,
  onViewGroupDetails,
  isSelectMode = false,
  isGroupSelected = false,
  onSelectGroupToggle,
  isSubmitting = false,
}) => {
  const firstAddonWithImage = addons.find(ad => ad.imagePath);
  const imagePath = firstAddonWithImage ? firstAddonWithImage.imagePath : null;
  
  const allEnabled = addons.every(ad => ad.isEnabled);
  const noneEnabled = addons.every(ad => !ad.isEnabled);
  let statusText = '部分启用';
  let badgeClass = 'badge-disabled';
  if (allEnabled) {
    statusText = '已启用';
    badgeClass = 'badge-enabled';
  } else if (noneEnabled) {
    statusText = '已禁用';
    badgeClass = 'badge-disabled';
  }

  const authors = Array.from(new Set(addons.map(ad => getAddonAuthor(ad))));
  const authorText = authors.length === 1 ? authors[0] : `${authors[0]} 等 ${authors.length} 位作者`;
  const groupDesc = `包含 ${addons.length} 个组件:\n` + 
    addons.map(ad => `• ${ad.steamDetails?.title || ad.addonInfo?.addontitle || ad.vpkName}`).join('\n');
  const groupSize = addons.reduce((sum, ad) => sum + (ad.fileSize || 0), 0);

  return (
    <div className={`addon-card group-card ${noneEnabled ? 'disabled' : ''} ${isGroupSelected ? 'card-selected' : ''} ${isSelectMode ? 'select-mode-active' : ''}`}>
      {/* Checkbox Wrapper */}
      {isSelectMode && (
        <div 
          className={`addon-card-checkbox-wrapper ${isGroupSelected ? 'selected' : ''}`}
          onClick={(e) => {
            e.stopPropagation();
            onSelectGroupToggle?.(addons);
          }}
          title={isGroupSelected ? '取消选择整个分组' : '选中整个分组'}
        >
          {isGroupSelected ? (
            <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>
          ) : null}
        </div>
      )}

      <div 
        className="addon-card-clickable-area"
        onClick={() => {
          if (isSelectMode) {
            onSelectGroupToggle?.(addons);
          } else {
            onViewGroupDetails(group.id);
          }
        }}
      >
        <div className="addon-card-image-wrapper">
          {imagePath ? (
            <CacheImage 
              srcPath={imagePath} 
              alt={group.name} 
              className="addon-card-image"
            />
          ) : (
            <div className="addon-placeholder-icon">
              <Folder size={48} />
            </div>
          )}
        </div>

        <div className="addon-card-badges">
          <span className={`badge ${badgeClass}`}>
            {statusText}
          </span>
          <span className="badge badge-dir" style={{ background: 'var(--md-sys-color-primary)', color: 'var(--md-sys-color-on-primary)' }}>
            分组 ({addons.length})
          </span>
        </div>

        <div className="addon-card-info">
          <h3 className="addon-card-title" title={group.name}>
            <Folder 
              size={18} 
              style={{ 
                display: 'inline-block', 
                verticalAlign: 'text-bottom', 
                marginRight: '6px', 
                color: 'var(--md-sys-color-primary)' 
              }} 
            />
            {group.name}
          </h3>
          
          <div className="addon-card-author">
            <span>作者: {authorText}</span>
          </div>

          <p className="addon-card-desc" style={{ whiteSpace: 'pre-line' }} title={groupDesc}>{groupDesc}</p>
          
          <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: '11px', color: 'var(--md-sys-color-outline)', marginTop: '8px' }}>
            <span>总大小: {formatBytes(groupSize)}</span>
          </div>
        </div>
      </div>

      <div className="addon-card-footer">
        <label className="switch" title="启用/禁用此分组内所有组件">
          <input 
            type="checkbox" 
            checked={allEnabled} 
            onChange={() => onToggleGroup(addons, !allEnabled)}
            disabled={isSubmitting}
          />
          <span className="slider"></span>
        </label>

        <div style={{ display: 'flex', gap: '6px' }}>
          <button 
            className="btn btn-secondary btn-icon-only" 
            onClick={() => onViewGroupDetails(group.id)}
            title="查看分组详情"
          >
            <FolderOpen size={14} />
          </button>
        </div>
      </div>
    </div>
  );
};
