import React from 'react';
import { Folder, FolderOpen, Download } from 'lucide-react';
import { Addon, Group } from '../types/addon';
import { formatBytes, getAddonAuthor, getAddonInfoValue } from '../utils/addonHelpers';
import { CacheImage } from './CacheImage';
import { useTranslation } from 'react-i18next';

interface GroupCardProps {
  group: Group;
  addons: Addon[];
  allGroupAddons?: Addon[];
  onToggleGroup: (addonsList: Addon[], enabled: boolean) => void;
  onViewGroupDetails: (groupId: string) => void;
  onDownloadUninstalled?: (workshopIds: string[]) => void;
  isSelectMode?: boolean;
  isGroupSelected?: boolean;
  onSelectGroupToggle?: (addonsList: Addon[], groupId?: string) => void;
  isSubmitting?: boolean;
  downloadProgress?: Record<string, number>;
}

export const GroupCard: React.FC<GroupCardProps> = ({
  group,
  addons,
  onToggleGroup,
  onViewGroupDetails,
  onDownloadUninstalled,
  isSelectMode = false,
  isGroupSelected = false,
  onSelectGroupToggle,
  isSubmitting = false,
  downloadProgress = {},
}) => {
  const { t } = useTranslation();
  const isEmptyGroup = group.addons.length === 0 && addons.length === 0;
  const installedAddons = addons.filter(ad => ad.dirType !== 'none');
  const uninstalledAddons = addons.filter(ad => ad.dirType === 'none');
  const displayAddons = installedAddons.length > 0 ? installedAddons : addons;

  const firstAddonWithImage = displayAddons.find(ad => ad.imagePath);
  const imagePath = firstAddonWithImage ? firstAddonWithImage.imagePath : null;

  const allEnabled = installedAddons.length > 0 && installedAddons.every(ad => ad.isEnabled);
  const noneEnabled = installedAddons.length === 0 || installedAddons.every(ad => !ad.isEnabled);
  let statusText = t('groupCard.partiallyEnabled');
  let badgeClass = 'badge-disabled';
  let badgeStyle: React.CSSProperties = {};
  if (isEmptyGroup) {
    statusText = t('groupCard.empty');
    badgeClass = '';
    badgeStyle = { background: 'var(--md-sys-color-surface-container-highest)', color: 'var(--md-sys-color-on-surface-variant)' };
  } else if (installedAddons.length === 0) {
    statusText = t('addonCard.uninstalled');
    badgeClass = '';
    badgeStyle = { background: 'var(--md-sys-color-tertiary)', color: 'var(--md-sys-color-on-tertiary)' };
  } else if (allEnabled) {
    statusText = t('groupCard.enabled');
    badgeClass = 'badge-enabled';
  } else if (noneEnabled) {
    statusText = t('groupCard.disabled');
    badgeClass = 'badge-disabled';
  }

  const authors = Array.from(new Set(displayAddons.map(ad => getAddonAuthor(ad)).filter(a => a !== 'Unknown Author')));
  const authorText = authors.length === 0 ? '' :
    authors.length === 1
      ? t('groupCard.author', { author: authors[0] })
      : t('groupCard.authorMultiple', { first: authors[0], count: authors.length - 1, total: authors.length });

  const groupDesc = t('groupCard.containsAddons', { count: addons.length }) + '\n' +
    addons.map(ad => `• ${ad.steamDetails?.title || getAddonInfoValue(ad, 'addontitle') || ad.vpkName.replace('.vpk', '')}`).join('\n');
  const groupSize = installedAddons.reduce((sum, ad) => sum + (ad.fileSize || 0), 0);

  const hasUninstalled = uninstalledAddons.length > 0;
  const uninstalledWithWorkshopId = uninstalledAddons.filter(ad => ad.workshopId);
  const isAnyDownloading = uninstalledWithWorkshopId.some(ad => downloadProgress[ad.workshopId!] !== undefined);

  return (
    <div className={`addon-card group-card ${noneEnabled && installedAddons.length > 0 ? 'disabled' : ''} ${isGroupSelected ? 'card-selected' : ''} ${isSelectMode ? 'select-mode-active' : ''}`}>
      {isSelectMode && (
        <div
          className={`addon-card-checkbox-wrapper ${isGroupSelected ? 'selected' : ''}`}
          onClick={(e) => {
            e.stopPropagation();
            onSelectGroupToggle?.(addons, group.id);
          }}
          title={isGroupSelected ? t('groupCard.deselectGroup') : t('groupCard.selectGroup')}
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
            onSelectGroupToggle?.(addons, group.id);
          } else {
            onViewGroupDetails(group.id);
          }
        }}
      >
        <div className="addon-card-image-wrapper">
          {imagePath ? (
            <CacheImage
              srcPath={imagePath}
              cacheRemote
              alt={group.name}
              className="addon-card-image"
              fallback={
                <div className="addon-placeholder-icon">
                  <Folder size={48} />
                </div>
              }
            />
          ) : (
            <div className="addon-placeholder-icon">
              <Folder size={48} />
            </div>
          )}
        </div>

        <div className="addon-card-badges">
          <span className={`badge ${badgeClass}`} style={badgeStyle}>{statusText}</span>
          <span className="badge badge-dir" style={{ background: 'var(--md-sys-color-primary)', color: 'var(--md-sys-color-on-primary)' }}>
            {t('groupCard.groupBadge', { count: addons.length })}
          </span>
          {hasUninstalled && installedAddons.length > 0 && (
            <span className="badge" style={{ background: 'var(--md-sys-color-tertiary)', color: 'var(--md-sys-color-on-tertiary)' }}>
              {t('groupHeader.uninstalledCount', { count: uninstalledAddons.length })}
            </span>
          )}
        </div>

        <div className="addon-card-info">
          <h3 className="addon-card-title" title={group.name}>
            <Folder size={18} style={{ display: 'inline-block', verticalAlign: 'text-bottom', marginRight: '6px', color: 'var(--md-sys-color-primary)' }} />
            {group.name}
          </h3>

          <div className="addon-card-author">
            <span>{authorText}</span>
          </div>

          <p className="addon-card-desc" style={{ whiteSpace: 'pre-line' }} title={groupDesc}>{groupDesc}</p>

          {group.tags && group.tags.length > 0 && (
            <div style={{ display: 'flex', gap: '4px', flexWrap: 'wrap', marginTop: '6px', marginBottom: '6px' }}>
              {group.tags.map(tag => (
                <span key={tag} style={{ padding: '2px 8px', borderRadius: '8px', fontSize: '10px', fontWeight: 500, backgroundColor: 'var(--md-sys-color-secondary-container)', color: 'var(--md-sys-color-on-secondary-container)' }}>
                  {tag}
                </span>
              ))}
            </div>
          )}

          <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: '11px', color: 'var(--md-sys-color-outline)', marginTop: '8px' }}>
            <span>{t('groupCard.totalSize', { size: formatBytes(groupSize) })}</span>
          </div>
        </div>
      </div>

      {!isSelectMode && (
        <div className="addon-card-footer">
          {installedAddons.length > 0 ? (
            <label className="switch" title={t('groupCard.toggleAll')}>
              <input
                type="checkbox"
                checked={allEnabled}
                onChange={() => onToggleGroup(installedAddons, !allEnabled)}
                disabled={isSubmitting}
              />
              <span className="slider"></span>
            </label>
          ) : <div />}

          <div style={{ display: 'flex', gap: '6px' }}>
            {hasUninstalled && uninstalledWithWorkshopId.length > 0 && onDownloadUninstalled && (
              <button
                className="btn btn-primary btn-icon-only"
                onClick={(e) => {
                  e.stopPropagation();
                  onDownloadUninstalled(uninstalledWithWorkshopId.map(ad => ad.workshopId!));
                }}
                disabled={isSubmitting || isAnyDownloading}
                title={t('groupHeader.downloadAllUninstalled', { count: uninstalledWithWorkshopId.length })}
              >
                <Download size={14} />
              </button>
            )}
            <button
              className="btn btn-secondary btn-icon-only"
              onClick={() => onViewGroupDetails(group.id)}
              title={t('groupCard.viewDetails')}
            >
              <FolderOpen size={14} />
            </button>
          </div>
        </div>
      )}
    </div>
  );
};
