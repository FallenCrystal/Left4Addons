import React from 'react';
import { FolderPlus, ExternalLink, Move, Edit3, FileText } from 'lucide-react';
import { Addon, Group } from '../types/addon';
import { formatBytes, getAddonCategories, getAddonUrl, getAddonAuthor } from '../utils/addonHelpers';
import { CacheImage } from './CacheImage';

interface AddonCardProps {
  addon: Addon;
  groups: Group[];
  onToggle: (vpkName: string, isEnabled: boolean) => void;
  onAddToGroup: (vpkName: string, groupId: string) => void;
  onRemoveFromGroup: (vpkName: string, groupId: string) => void;
  onOpenLink: (url: string) => void;
  onMoveClick: (addon: Addon) => void;
  onRenameClick: (addon: Addon) => void;
  onDetailClick: (addon: Addon) => void;
  isSelectMode: boolean;
  isSelected: boolean;
  onSelectToggle: (vpkName: string) => void;
}

export const AddonCard: React.FC<AddonCardProps> = ({
  addon,
  groups,
  onToggle,
  onAddToGroup,
  onRemoveFromGroup,
  onOpenLink,
  onMoveClick,
  onRenameClick,
  onDetailClick,
  isSelectMode,
  isSelected,
  onSelectToggle,
}) => {
  const categories = getAddonCategories(addon);
  const title = addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName;
  const author = getAddonAuthor(addon);
  const desc = addon.steamDetails?.description || addon.addonInfo?.addonDescription || addon.addonInfo?.addontagline || 'No description provided.';
  
  const itemGroup = groups.find(g => g.addons.includes(addon.vpkName));
  const addonUrl = getAddonUrl(addon);

  const handleLinkClick = (e: React.MouseEvent, url: string) => {
    e.preventDefault();
    onOpenLink(url);
  };

  return (
    <div className={`addon-card ${!addon.isEnabled ? 'disabled' : ''} ${isSelected ? 'card-selected' : ''} ${isSelectMode ? 'select-mode-active' : ''}`}>
      {/* Checkbox Wrapper */}
      <div 
        className={`addon-card-checkbox-wrapper ${isSelected ? 'selected' : ''}`}
        onClick={(e) => {
          e.stopPropagation();
          onSelectToggle(addon.vpkName);
        }}
        title={isSelected ? '取消选择' : '选中此组件'}
      >
        {isSelected ? (
          <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>
        ) : null}
      </div>

      {/* Clickable Area for Detail Modal or Selection Toggle */}
      <div 
        className="addon-card-clickable-area"
        onClick={(e) => {
          if (isSelectMode) {
            e.stopPropagation();
            onSelectToggle(addon.vpkName);
          } else {
            onDetailClick(addon);
          }
        }}
      >
        <div className="addon-card-image-wrapper">
          {addon.imagePath ? (
            <CacheImage 
              srcPath={addon.imagePath} 
              alt={title} 
              className="addon-card-image"
              fallback={
                <div className="addon-placeholder-icon">
                  <svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="text-secondary"><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M10 9H8"/><path d="M16 13H8"/><path d="M16 17H8"/></svg>
                </div>
              }
            />
          ) : (
            <div className="addon-placeholder-icon">
              <FileText size={48} />
            </div>
          )}
        </div>

        <div className="addon-card-badges">
          <span className={`badge ${addon.isEnabled ? 'badge-enabled' : 'badge-disabled'}`}>
            {addon.isEnabled ? '已启用' : '已禁用'}
          </span>
          <span className="badge badge-dir">
            {addon.dirType === 'loading' ? '手动安装' : '创意工坊'}
          </span>
        </div>

        <div className="addon-card-info">
          <h3 className="addon-card-title" title={title}>{title}</h3>
          
          <div className="addon-card-author">
            <span>作者: {author}</span>
          </div>

          {itemGroup && (
            <div className="group-tag">
              <FolderPlus size={12} />
              <span>分组: {itemGroup.name}</span>
            </div>
          )}

          <p className="addon-card-desc" title={desc}>{desc}</p>
          
          <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: '11px', color: 'var(--md-sys-color-outline)', marginTop: '8px' }}>
            <span>文件大小: {formatBytes(addon.fileSize)}</span>
            {addon.filesCount > 0 && <span>内含 {addon.filesCount} 个文件</span>}
          </div>

          <div className="addon-card-tags">
            {categories.map(c => (
              <span key={c} className="tag-chip">{c}</span>
            ))}
            {addon.workshopId && (
              <span className="tag-chip" style={{ borderColor: 'var(--md-sys-color-primary)', color: 'var(--md-sys-color-primary)' }}>
                ID: {addon.workshopId}
              </span>
            )}
          </div>
        </div>
      </div>

      <div className="addon-card-footer">
        {/* Enable/Disable Toggle */}
        <label className="switch" title={addon.isEnabled ? '点击禁用该附件' : '点击启用该附件'}>
          <input 
            type="checkbox" 
            checked={addon.isEnabled} 
            onChange={() => onToggle(addon.vpkName, addon.isEnabled)}
          />
          <span className="slider"></span>
        </label>

        <div style={{ display: 'flex', gap: '6px' }}>
          {/* Group assign dropdown */}
          <div className="dropdown">
            <button className="btn btn-secondary btn-icon-only" title="加入或移出群组">
              <FolderPlus size={14} />
            </button>
            <div className="dropdown-content">
              {groups.map(g => (
                <button 
                  key={g.id} 
                  onClick={() => onAddToGroup(addon.vpkName, g.id)}
                  disabled={itemGroup && itemGroup.id === g.id}
                >
                  {g.name}
                </button>
              ))}
              {itemGroup && (
                <button 
                  onClick={() => onRemoveFromGroup(addon.vpkName, itemGroup.id)}
                  style={{ color: 'var(--md-sys-color-error)' }}
                >
                  从当前分组移出 ({itemGroup.name})
                </button>
              )}
              {groups.length === 0 && !itemGroup && (
                <button disabled style={{ fontStyle: 'italic' }}>无分组 (在侧栏创建)</button>
              )}
            </div>
          </div>

          {/* Open Link with Fallbacks (Dropdown or direct link) */}
          {addon.workshopId ? (
            <div className="dropdown">
              <button className="btn btn-secondary btn-icon-only" title="打开链接">
                <ExternalLink size={14} />
              </button>
              <div className="dropdown-content">
                <button onClick={(e) => handleLinkClick(e, `steam://url/CommunityFilePage/${addon.workshopId}`)}>
                  在 Steam 客户端打开
                </button>
                <button onClick={(e) => handleLinkClick(e, `https://steamcommunity.com/sharedfiles/filedetails/?id=${addon.workshopId}`)}>
                  在浏览器中打开 (官方)
                </button>
                <button onClick={(e) => handleLinkClick(e, `https://steamcommunity.net/sharedfiles/filedetails/?id=${addon.workshopId}`)}>
                  在浏览器中打开 (国内镜像)
                </button>
              </div>
            </div>
          ) : addonUrl ? (
            <button 
              className="btn btn-secondary"
              style={{ width: '32px', height: '32px', display: 'flex', alignItems: 'center', justifyContent: 'center', padding: 0 }}
              onClick={(e) => handleLinkClick(e, addonUrl)}
              title="打开组件内置来源网页"
            >
              <ExternalLink size={14} />
            </button>
          ) : null}

          {/* Move between load and workshop dirs (Only from workshop to loading, no moving back) */}
          {addon.dirType === 'workshop' && (
            <button 
              className="btn btn-secondary btn-icon-only"
              onClick={() => onMoveClick(addon)}
              title="移动到手动安装目录 (Addons)"
            >
              <Move size={14} />
            </button>
          )}

          {/* Rename */}
          <button 
            className="btn btn-secondary btn-icon-only"
            onClick={() => onRenameClick(addon)}
            title="重命名附件文件"
          >
            <Edit3 size={14} />
          </button>
        </div>
      </div>
    </div>
  );
};
