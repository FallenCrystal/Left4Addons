import React from 'react';
import { Download, Globe, Trash2, Search, RefreshCw, AlertCircle, FileText } from 'lucide-react';
import { Addon } from '../types/addon';
import { getAddonAuthor, getAddonCategories, formatBytes, getAddonInfoValue } from '../utils/addonHelpers';
import { CacheImage } from './CacheImage';
import { useTranslation } from 'react-i18next';

interface KnownUninstalledViewProps {
  knownUninstalledAddons: Record<string, Addon>;
  downloadProgress: Record<string, number>;
  onDownload: (workshopId: string) => void;
  onDelete: (ids: string[], deleteFile: boolean, removeFromKnown: boolean) => void;
  isSubmitting: boolean;
  onOpenLink: (url: string) => void;
  isSelectMode?: boolean;
  selectedIds?: string[];
  onSelectToggle?: (id: string) => void;
  onSelectAll?: (items: Addon[]) => void;
  onBatchDownload?: (workshopIds: string[]) => void;
  searchQuery?: string;
  onSearchQueryChange?: (query: string) => void;
}

export const KnownUninstalledView: React.FC<KnownUninstalledViewProps> = ({
  knownUninstalledAddons,
  downloadProgress,
  onDownload,
  onDelete,
  isSubmitting,
  onOpenLink,
  isSelectMode = false,
  selectedIds = [],
  onSelectToggle,
  onSelectAll: _onSelectAll,
  onBatchDownload: _onBatchDownload,
  searchQuery: externalSearchQuery,
  onSearchQueryChange,
}) => {
  const { t } = useTranslation();
  const searchQuery = externalSearchQuery || '';
  const setSearchQuery = onSearchQueryChange || (() => {});
  const addonsList = Object.values(knownUninstalledAddons);

  const filteredAddons = addonsList.filter((addon) => {
    const q = searchQuery.toLowerCase();
    const title = (addon.steamDetails?.title || getAddonInfoValue(addon, 'addontitle') || addon.vpkName || '').toLowerCase();
    const desc = (addon.steamDetails?.description || getAddonInfoValue(addon, 'addondescription') || '').toLowerCase();
    const author = getAddonAuthor(addon).toLowerCase();
    const wId = (addon.workshopId || '');
    return title.includes(q) || desc.includes(q) || author.includes(q) || wId.includes(q);
  });

  return (
    <div style={{ padding: '24px', display: 'flex', flexDirection: 'column', height: '100%', overflowY: 'auto' }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '24px' }}>
        <div>
          <h1 style={{ margin: 0, fontSize: '24px', fontWeight: 600, color: 'var(--md-sys-color-on-surface)' }}>
            {t('sidebar.disabledAddons') === '被禁用的组件 (.disabled)' ? '未安装但已知的 Addon' : 'Known Uninstalled Addons'}
          </h1>
          <p style={{ margin: '4px 0 0 0', fontSize: '14px', color: 'var(--md-sys-color-outline)' }}>
            {t('sidebar.disabledAddons') === '被禁用的组件 (.disabled)'
              ? '管理之前扫描过、在合集中或手动加入但当前未在本地文件夹中的组件。'
              : 'Manage addons previously scanned, from collections, or manually added but not currently on disk.'}
          </p>
        </div>
      </div>

      {/* Search */}
      <div style={{ display: 'flex', gap: '16px', marginBottom: '24px', alignItems: 'center', flexWrap: 'wrap' }}>
        <div style={{ position: 'relative', flex: 1, minWidth: '240px' }}>
          <Search size={18} style={{ position: 'absolute', left: '16px', top: '50%', transform: 'translateY(-50%)', color: 'var(--md-sys-color-outline)' }} />
          <input
            type="text"
            placeholder={t('topbar.searchPlaceholder')}
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            style={{
              width: '100%',
              padding: '12px 16px 12px 48px',
              borderRadius: '28px',
              border: '1px solid var(--md-sys-color-outline-variant)',
              backgroundColor: 'var(--md-sys-color-surface-container-high)',
              color: 'var(--md-sys-color-on-surface)',
              outline: 'none',
              fontSize: '14px'
            }}
          />
        </div>
      </div>

      {/* Grid */}
      {filteredAddons.length === 0 ? (
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', flex: 1, padding: '48px 0', color: 'var(--md-sys-color-outline)' }}>
          <AlertCircle size={48} style={{ marginBottom: '16px' }} />
          <p style={{ margin: 0, fontSize: '16px', fontWeight: 500 }}>{t('common.emptyStateTitle')}</p>
        </div>
      ) : (
        <div className="addons-grid" style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(320px, 1fr))', gap: '20px' }}>
          {filteredAddons.map((addon) => {
            const isDownloading = addon.workshopId ? downloadProgress[addon.workshopId] !== undefined : false;
            const progress = addon.workshopId ? (downloadProgress[addon.workshopId] || 0) : 0;
            const title = addon.steamDetails?.title || getAddonInfoValue(addon, 'addontitle') || addon.vpkName;
            const author = getAddonAuthor(addon);
            const categories = getAddonCategories(addon);
            const isSelected = selectedIds.includes(addon.id);

            return (
              <div
                key={addon.id}
                className={`addon-card ${isSelected ? 'card-selected' : ''} ${isSelectMode ? 'select-mode-active' : ''}`}
              >
                {/* Checkbox Wrapper */}
                {isSelectMode && onSelectToggle && (
                  <div
                    className={`addon-card-checkbox-wrapper ${isSelected ? 'selected' : ''}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      onSelectToggle(addon.id);
                    }}
                    title={isSelected ? t('addonCard.deselect') : t('addonCard.select')}
                  >
                    {isSelected ? (
                      <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>
                    ) : null}
                  </div>
                )}

                <div
                  className="addon-card-clickable-area"
                  onClick={() => {
                    if (isSelectMode && onSelectToggle) {
                      onSelectToggle(addon.id);
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
                            <FileText size={48} />
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
                    <span className="badge badge-disabled">{t('addonCard.disabled')}</span>
                    <span className="badge" style={{ background: 'var(--md-sys-color-tertiary)', color: 'var(--md-sys-color-on-tertiary)' }}>
                      {t('sidebar.disabledAddons') === '被禁用的组件 (.disabled)' ? '未安装' : 'Uninstalled'}
                    </span>
                  </div>

                  <div className="addon-card-info">
                    <h3 className="addon-card-title" title={title}>{title}</h3>

                    <div className="addon-card-author">
                      <span>{t('addonCard.author', { author })}</span>
                    </div>

                    <p className="addon-card-desc" title={addon.steamDetails?.description || ''}>
                      {addon.steamDetails?.description || ''}
                    </p>

                    <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: '11px', color: 'var(--md-sys-color-outline)', marginTop: '8px' }}>
                      {addon.steamDetails?.file_size && (
                        <span>{t('addonCard.fileSize', { size: formatBytes(parseInt(addon.steamDetails.file_size)) })}</span>
                      )}
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

                {!isSelectMode && (
                  <div className="addon-card-footer">
                    <div style={{ flex: 1 }} />
                    <div style={{ display: 'flex', gap: '6px' }}>
                      {addon.workshopId ? (
                        <button
                          className="btn btn-primary btn-icon-only"
                          onClick={() => onDownload(addon.workshopId!)}
                          disabled={isDownloading || isSubmitting}
                          title={isDownloading ? t('workshop.detail.downloading', { progress }) : t('workshop.detail.download')}
                        >
                          {isDownloading ? (
                            <RefreshCw size={14} className="animate-spin" />
                          ) : (
                            <Download size={14} />
                          )}
                        </button>
                      ) : null}

                      {addon.workshopId && (
                        <button
                          className="btn btn-secondary btn-icon-only"
                          onClick={() => onOpenLink(`steam://url/CommunityFilePage/${addon.workshopId}`)}
                          title={t('addonCard.openInSteam')}
                        >
                          <Globe size={14} />
                        </button>
                      )}

                      <button
                        className="btn btn-secondary btn-icon-only"
                        onClick={() => onDelete([addon.id], false, true)}
                        disabled={isSubmitting}
                        style={{ color: 'var(--md-sys-color-error)' }}
                        title={t('sidebar.disabledAddons') === '被禁用的组件 (.disabled)' ? '从已知列表删除' : 'Remove from known list'}
                      >
                        <Trash2 size={14} />
                      </button>
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};
