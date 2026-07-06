import React, { useState } from 'react';
import { Download, Trash2, Globe, Search, RefreshCw, AlertCircle } from 'lucide-react';
import { Addon } from '../types/addon';
import { getAddonAuthor, getAddonCategories, formatBytes } from '../utils/addonHelpers';

interface KnownUninstalledViewProps {
  knownUninstalledAddons: Record<string, Addon>;
  downloadProgress: Record<string, number>;
  onDownload: (workshopId: string) => void;
  onDelete: (ids: string[], deleteFile: boolean, removeFromKnown: boolean) => void;
  isSubmitting: boolean;
  onOpenLink: (url: string) => void;
}

export const KnownUninstalledView: React.FC<KnownUninstalledViewProps> = ({
  knownUninstalledAddons,
  downloadProgress,
  onDownload,
  onDelete,
  isSubmitting,
  onOpenLink,
}) => {
  const [searchQuery, setSearchQuery] = useState('');
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [isSelectMode, setIsSelectMode] = useState(false);

  const addonsList = Object.values(knownUninstalledAddons);

  // Search filter
  const filteredAddons = addonsList.filter((addon) => {
    const q = searchQuery.toLowerCase();
    const title = (addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName || '').toLowerCase();
    const desc = (addon.steamDetails?.description || addon.addonInfo?.addondescription || '').toLowerCase();
    const author = getAddonAuthor(addon).toLowerCase();
    const wId = (addon.workshopId || '');
    return title.includes(q) || desc.includes(q) || author.includes(q) || wId.includes(q);
  });

  const handleSelectToggle = (id: string) => {
    setSelectedIds((prev) => {
      const next = prev.includes(id) ? prev.filter((item) => item !== id) : [...prev, id];
      if (next.length > 0 && !isSelectMode) {
        setIsSelectMode(true);
      }
      return next;
    });
  };

  const handleSelectAll = () => {
    const allIds = filteredAddons.map((ad) => ad.vpkName);
    const allSelected = allIds.every((id) => selectedIds.includes(id));
    if (allSelected) {
      setSelectedIds((prev) => prev.filter((id) => !allIds.includes(id)));
    } else {
      setSelectedIds((prev) => {
        const next = [...prev];
        allIds.forEach((id) => {
          if (!next.includes(id)) next.push(id);
        });
        return next;
      });
    }
  };

  const handleClearSelection = () => {
    setSelectedIds([]);
    setIsSelectMode(false);
  };

  const handleBatchDownload = async () => {
    const idsToDownload = selectedIds
      .map((id) => knownUninstalledAddons[id])
      .filter((addon) => addon !== undefined)
      .map((addon) => addon.workshopId)
      .filter(Boolean) as string[];

    if (idsToDownload.length === 0) return;

    for (const wId of idsToDownload) {
      onDownload(wId);
    }
    handleClearSelection();
  };

  const handleBatchDelete = () => {
    onDelete(selectedIds, false, true);
    handleClearSelection();
  };

  return (
    <div className="known-uninstalled-view" style={{ padding: '24px', display: 'flex', flexDirection: 'column', height: '100%', overflowY: 'auto' }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '24px' }}>
        <div>
          <h1 style={{ margin: 0, fontSize: '24px', fontWeight: 600, color: 'var(--md-sys-color-on-surface)' }}>
            未安装但已知的 Addon
          </h1>
          <p style={{ margin: '4px 0 0 0', fontSize: '14px', color: 'var(--md-sys-color-outline)' }}>
            管理之前扫描过、在合集中或手动加入但当前未在本地文件夹中的组件。
          </p>
        </div>

        <div style={{ display: 'flex', gap: '8px' }}>
          <button 
            className="btn btn-outline" 
            onClick={() => setIsSelectMode(!isSelectMode)}
            style={{ borderRadius: '100px' }}
          >
            {isSelectMode ? '取消选择' : '批量管理'}
          </button>
        </div>
      </div>

      {/* Action Bar / Search */}
      <div style={{ display: 'flex', gap: '16px', marginBottom: '24px', alignItems: 'center', flexWrap: 'wrap' }}>
        <div style={{ position: 'relative', flex: 1, minWidth: '240px' }}>
          <Search size={18} style={{ position: 'absolute', left: '16px', top: '50%', transform: 'translateY(-50%)', color: 'var(--md-sys-color-outline)' }} />
          <input
            type="text"
            placeholder="搜索已知组件..."
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

      {/* Floating Batch Actions */}
      {isSelectMode && selectedIds.length > 0 && (
        <div style={{
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
          padding: '16px 24px',
          backgroundColor: 'var(--md-sys-color-primary-container)',
          color: 'var(--md-sys-color-on-primary-container)',
          borderRadius: '16px',
          marginBottom: '24px',
          boxShadow: 'var(--md-sys-elevation-2)'
        }}>
          <span style={{ fontWeight: 500 }}>已选中 {selectedIds.length} 个项目</span>
          <div style={{ display: 'flex', gap: '8px' }}>
            <button className="btn btn-primary" onClick={handleSelectAll} style={{ borderRadius: '100px' }}>
              {selectedIds.length === filteredAddons.length ? '取消全选' : '全选'}
            </button>
            <button className="btn btn-primary" onClick={handleBatchDownload} style={{ borderRadius: '100px' }}>
              <Download size={16} style={{ marginRight: '8px' }} />
              批量下载
            </button>
            <button 
              className="btn btn-outline" 
              onClick={handleBatchDelete} 
              style={{ borderRadius: '100px', borderColor: 'var(--md-sys-color-error)', color: 'var(--md-sys-color-error)' }}
            >
              <Trash2 size={16} style={{ marginRight: '8px' }} />
              从已知列表删除
            </button>
            <button className="btn btn-outline" onClick={handleClearSelection} style={{ borderRadius: '100px' }}>
              清除选择
            </button>
          </div>
        </div>
      )}

      {/* Grid of Addons */}
      {filteredAddons.length === 0 ? (
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', flex: 1, padding: '48px 0', color: 'var(--md-sys-color-outline)' }}>
          <AlertCircle size={48} style={{ marginBottom: '16px' }} />
          <p style={{ margin: 0, fontSize: '16px', fontWeight: 500 }}>没有找到匹配 of 已知组件</p>
        </div>
      ) : (
        <div className="addons-grid" style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(280px, 1fr))', gap: '20px' }}>
          {filteredAddons.map((addon) => {
            const isDownloading = addon.workshopId ? downloadProgress[addon.workshopId] !== undefined : false;
            const progress = addon.workshopId ? (downloadProgress[addon.workshopId] || 0) : 0;
            const title = addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName;
            const author = getAddonAuthor(addon);
            const categories = getAddonCategories(addon);

            return (
              <div 
                key={addon.id} 
                className={`addon-card ${selectedIds.includes(addon.id) ? 'selected' : ''}`}
                style={{
                  display: 'flex',
                  flexDirection: 'column',
                  borderRadius: '16px',
                  border: selectedIds.includes(addon.id) 
                    ? '2px solid var(--md-sys-color-primary)' 
                    : '1px solid var(--md-sys-color-outline-variant)',
                  backgroundColor: 'var(--md-sys-color-surface-container-low)',
                  overflow: 'hidden',
                  cursor: isSelectMode ? 'pointer' : 'default',
                  position: 'relative'
                }}
                onClick={() => isSelectMode && handleSelectToggle(addon.vpkName)}
              >
                {/* Checkbox for selection */}
                {isSelectMode && (
                  <div style={{ position: 'absolute', top: '12px', left: '12px', zIndex: 10 }}>
                    <input
                      type="checkbox"
                      checked={selectedIds.includes(addon.id)}
                      onChange={() => {}} // handled by card onClick
                      style={{ width: '20px', height: '20px', cursor: 'pointer' }}
                    />
                  </div>
                )}

                {/* Card Thumbnail */}
                <div style={{ position: 'relative', width: '100%', height: '160px', backgroundColor: '#111' }}>
                  {addon.imagePath ? (
                    <img 
                      src={addon.imagePath} 
                      alt={title} 
                      style={{ width: '100%', height: '100%', objectFit: 'cover' }}
                      onError={(e) => {
                        (e.target as HTMLImageElement).src = '';
                      }}
                    />
                  ) : (
                    <div style={{ width: '100%', height: '100%', display: 'flex', alignItems: 'center', justifyContent: 'center', color: '#666' }}>
                      暂无图片
                    </div>
                  )}
                  {addon.workshopId && (
                    <div style={{ position: 'absolute', top: '12px', right: '12px', padding: '4px 8px', borderRadius: '4px', fontSize: '12px', fontWeight: 500, backgroundColor: 'rgba(0,0,0,0.6)', color: '#fff' }}>
                      ID: {addon.workshopId}
                    </div>
                  )}
                </div>

                {/* Card Content */}
                <div style={{ padding: '16px', display: 'flex', flexDirection: 'column', flex: 1 }}>
                  <h3 style={{ margin: '0 0 8px 0', fontSize: '16px', fontWeight: 600, color: 'var(--md-sys-color-on-surface)', display: '-webkit-box', WebkitLineClamp: 2, WebkitBoxOrient: 'vertical', overflow: 'hidden', height: '40px', lineHeight: '20px' }}>
                    {title}
                  </h3>

                  <p style={{ margin: '0 0 12px 0', fontSize: '13px', color: 'var(--md-sys-color-outline)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                    作者: {author}
                  </p>

                  {/* Chips */}
                  <div style={{ display: 'flex', gap: '6px', flexWrap: 'wrap', marginBottom: '16px' }}>
                    {categories.map((c) => (
                      <span key={c} style={{ padding: '2px 8px', borderRadius: '8px', fontSize: '11px', fontWeight: 500, backgroundColor: 'var(--md-sys-color-secondary-container)', color: 'var(--md-sys-color-on-secondary-container)' }}>
                        {c}
                      </span>
                    ))}
                    {addon.steamDetails?.file_size && (
                      <span style={{ padding: '2px 8px', borderRadius: '8px', fontSize: '11px', fontWeight: 500, backgroundColor: 'var(--md-sys-color-surface-container-highest)', color: 'var(--md-sys-color-on-surface-variant)' }}>
                        大小: {formatBytes(parseInt(addon.steamDetails.file_size))}
                      </span>
                    )}
                  </div>

                  {/* Description summary */}
                  {addon.steamDetails?.description && (
                    <p style={{ margin: '0 0 16px 0', fontSize: '12px', color: 'var(--md-sys-color-on-surface-variant)', display: '-webkit-box', WebkitLineClamp: 3, WebkitBoxOrient: 'vertical', overflow: 'hidden', lineHeight: '18px', height: '54px' }}>
                      {addon.steamDetails.description}
                    </p>
                  )}

                  {/* Actions */}
                  {!isSelectMode && (
                    <div style={{ marginTop: 'auto', display: 'flex', gap: '8px', width: '100%' }}>
                      {addon.workshopId ? (
                        <button 
                          className="btn btn-primary" 
                          onClick={() => onDownload(addon.workshopId!)}
                          disabled={isDownloading || isSubmitting}
                          style={{ flex: 1, borderRadius: '100px', display: 'flex', alignItems: 'center', justifyContent: 'center', gap: '6px' }}
                        >
                          {isDownloading ? (
                            <>
                              <RefreshCw size={14} className="animate-spin" />
                              下载中 {progress}%
                            </>
                          ) : (
                            <>
                              <Download size={14} />
                              下载
                            </>
                          )}
                        </button>
                      ) : (
                        <div style={{ flex: 1, padding: '8px', textAlign: 'center', fontSize: '12px', color: 'var(--md-sys-color-outline)', backgroundColor: 'var(--md-sys-color-surface-container-high)', borderRadius: '100px' }}>
                          本地组件无法下载
                        </div>
                      )}

                      {addon.workshopId && (
                        <button 
                          className="btn btn-outline" 
                          onClick={() => onOpenLink(`steam://url/CommunityFilePage/${addon.workshopId}`)}
                          style={{ borderRadius: '50%', padding: '8px', minWidth: '38px', width: '38px', height: '38px', display: 'flex', alignItems: 'center', justifyContent: 'center' }}
                          title="在 Steam 中查看"
                        >
                          <Globe size={14} />
                        </button>
                      )}

                      <button 
                        className="btn btn-outline" 
                        onClick={() => onDelete([addon.vpkName], false, true)}
                        disabled={isSubmitting}
                        style={{ borderRadius: '50%', padding: '8px', minWidth: '38px', width: '38px', height: '38px', display: 'flex', alignItems: 'center', justifyContent: 'center', borderColor: 'var(--md-sys-color-error)', color: 'var(--md-sys-color-error)' }}
                        title="从已知列表删除"
                      >
                        <Trash2 size={14} />
                      </button>
                    </div>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};
