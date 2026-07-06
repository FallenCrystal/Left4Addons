import React from 'react';
import { FolderPlus, ExternalLink, Move, Edit3, FileText } from 'lucide-react';
import { Addon, Group } from '../types/addon';
import { formatBytes, getAddonCategories, getAddonUrl, getAddonAuthor, getAddonInfoValue } from '../utils/addonHelpers';
import { CacheImage } from './CacheImage';
import { useTranslation } from 'react-i18next';

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
  isSubmitting?: boolean;
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
  isSubmitting = false,
}) => {
  const { t } = useTranslation();
  const categories = getAddonCategories(addon);
  const title = addon.steamDetails?.title || getAddonInfoValue(addon, 'addontitle') || addon.vpkName;
  const author = getAddonAuthor(addon);
  const desc = addon.steamDetails?.description || getAddonInfoValue(addon, 'addondescription') || getAddonInfoValue(addon, 'addontagline') || 'No description provided.';
  
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
        title={isSelected ? t('addonCard.deselect') : t('addonCard.select')}
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
            {addon.isEnabled ? t('addonCard.enabled') : t('addonCard.disabled')}
          </span>
          <span className="badge badge-dir">
            {addon.dirType === 'loading' ? t('addonCard.manualInstall') : t('addonCard.workshop')}
          </span>
        </div>

        <div className="addon-card-info">
          <h3 className="addon-card-title" title={title}>{title}</h3>
          
          <div className="addon-card-author">
            <span>{t('addonCard.author', { author })}</span>
          </div>

          {itemGroup && (
            <div className="group-tag">
              <FolderPlus size={12} />
              <span>{t('addonCard.group', { group: itemGroup.name })}</span>
            </div>
          )}

          <p className="addon-card-desc" title={desc}>{desc}</p>
          
          <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: '11px', color: 'var(--md-sys-color-outline)', marginTop: '8px' }}>
            <span>{t('addonCard.fileSize', { size: formatBytes(addon.fileSize) })}</span>
            {addon.filesCount > 0 && <span>{t('addonCard.containsFiles', { count: addon.filesCount })}</span>}
          </div>

          <div className="addon-card-tags">
            {categories.map(c => (
              <span key={c} className="tag-chip">{t(`categories.${c}`, c)}</span>
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
        <label className="switch" title={addon.isEnabled ? t('addonCard.clickToDisable') : t('addonCard.clickToEnable')}>
          <input 
            type="checkbox" 
            checked={addon.isEnabled} 
            onChange={() => onToggle(addon.vpkName, addon.isEnabled)}
            disabled={isSubmitting}
          />
          <span className="slider"></span>
        </label>

        <div style={{ display: 'flex', gap: '6px' }}>
          {/* Group assign dropdown */}
          <div className="dropdown">
            <button className="btn btn-secondary btn-icon-only" title={t('addonCard.addOrRemoveGroup')} disabled={isSubmitting}>
              <FolderPlus size={14} />
            </button>
            <div className="dropdown-content">
              {groups.map(g => (
                <button 
                  key={g.id} 
                  onClick={() => onAddToGroup(addon.vpkName, g.id)}
                  disabled={isSubmitting || (itemGroup && itemGroup.id === g.id)}
                >
                  {g.name}
                </button>
              ))}
              {itemGroup && (
                <button 
                  onClick={() => onRemoveFromGroup(addon.vpkName, itemGroup.id)}
                  disabled={isSubmitting}
                  style={{ color: 'var(--md-sys-color-error)' }}
                >
                  {t('addonCard.removeFromGroup', { name: itemGroup.name })}
                </button>
              )}
              {groups.length === 0 && !itemGroup && (
                <button disabled style={{ fontStyle: 'italic' }}>{t('addonCard.noGroupsTooltip')}</button>
              )}
            </div>
          </div>

          {/* Open Link with Fallbacks (Dropdown or direct link) */}
          {addon.workshopId ? (
            <div className="dropdown">
              <button className="btn btn-secondary btn-icon-only" title={t('addonCard.openLink')}>
                <ExternalLink size={14} />
              </button>
              <div className="dropdown-content">
                <button onClick={(e) => handleLinkClick(e, `steam://url/CommunityFilePage/${addon.workshopId}`)}>
                  {t('addonCard.openInSteam')}
                </button>
                <button onClick={(e) => handleLinkClick(e, `https://steamcommunity.com/sharedfiles/filedetails/?id=${addon.workshopId}`)}>
                  {t('addonCard.openInBrowser')}
                </button>
                <button onClick={(e) => handleLinkClick(e, `https://steamcommunity.net/sharedfiles/filedetails/?id=${addon.workshopId}`)}>
                  {t('addonCard.openInMirror')}
                </button>
              </div>
            </div>
          ) : addonUrl ? (
            <button 
              className="btn btn-secondary"
              style={{ width: '32px', height: '32px', display: 'flex', alignItems: 'center', justifyContent: 'center', padding: 0 }}
              onClick={(e) => handleLinkClick(e, addonUrl)}
              title={t('addonCard.openBuiltInSource')}
            >
              <ExternalLink size={14} />
            </button>
          ) : null}

          {/* Move between load and workshop dirs (Only from workshop to loading, no moving back) */}
          {addon.dirType === 'workshop' && (
            <button 
              className="btn btn-secondary btn-icon-only"
              onClick={() => onMoveClick(addon)}
              disabled={isSubmitting}
              title={t('addonCard.moveToManual')}
            >
              <Move size={14} />
            </button>
          )}

          {/* Rename */}
          <button 
            className="btn btn-secondary btn-icon-only"
            onClick={() => onRenameClick(addon)}
            disabled={isSubmitting}
            title={t('addonCard.renameAddon')}
          >
            <Edit3 size={14} />
          </button>
        </div>
      </div>
    </div>
  );
};
