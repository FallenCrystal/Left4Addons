import React, { useState, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Search, X, AlertTriangle, Package, Check } from 'lucide-react';
import { Addon } from '../types/addon';
import { CacheImage } from './CacheImage';

export interface DependencyDownloadItem {
  workshopId: string;
  title?: string;
  imagePath?: string;
}

interface DependencyPromptModalProps {
  open: boolean;
  missingDependencies: Addon[];
  isScanning: boolean;
  onDownload: (items: DependencyDownloadItem[]) => void;
  onCancel: () => void;
  onGoToSettings: () => void;
}

export const DependencyPromptModal: React.FC<DependencyPromptModalProps> = ({
  open,
  missingDependencies,
  isScanning,
  onDownload,
  onCancel,
  onGoToSettings,
}) => {
  const { t } = useTranslation();
  const [searchQuery, setSearchQuery] = useState('');
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [showSecondaryWarning, setShowSecondaryWarning] = useState(false);

  // Initialize selectedIds when modal opens or missingDependencies change
  React.useEffect(() => {
    if (open) {
      setSelectedIds(new Set(missingDependencies.map(a => a.workshopId || a.id)));
      setShowSecondaryWarning(false);
      setSearchQuery('');
    }
  }, [open, missingDependencies]);

  const filteredDependencies = useMemo(() => {
    if (!searchQuery.trim()) return missingDependencies;
    const lowerQuery = searchQuery.toLowerCase();
    return missingDependencies.filter(addon => {
      const title = addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName || addon.id;
      const author = addon.steamDetails?.creator_name || addon.addonInfo?.addonauthor || addon.workshopDetails?.authorName || '';
      const id = addon.workshopId || addon.id;
      return title.toLowerCase().includes(lowerQuery) || 
             author.toLowerCase().includes(lowerQuery) ||
             id.includes(lowerQuery);
    });
  }, [missingDependencies, searchQuery]);

  if (!open) return null;

  const handleSelectAll = (checked: boolean) => {
    if (checked) {
      setSelectedIds(new Set(filteredDependencies.map(a => a.workshopId || a.id)));
    } else {
      setSelectedIds(new Set());
    }
  };

  const handleSelect = (id: string, checked: boolean) => {
    const next = new Set(selectedIds);
    if (checked) {
      next.add(id);
    } else {
      next.delete(id);
    }
    setSelectedIds(next);
  };

  const allFilteredSelected = filteredDependencies.length > 0 && filteredDependencies.every(a => selectedIds.has(a.workshopId || a.id));
  const someFilteredSelected = filteredDependencies.some(a => selectedIds.has(a.workshopId || a.id)) && !allFilteredSelected;

  const handleDownload = () => {
    if (selectedIds.size > 0) {
      onDownload(
        missingDependencies
          .filter((addon) => selectedIds.has(addon.workshopId || addon.id))
          .map((addon) => ({
            workshopId: addon.workshopId || addon.id,
            title: addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName,
            imagePath: addon.imagePath || addon.workshopDetails?.previewUrl,
          })),
      );
    }
  };

  if (showSecondaryWarning) {
    return (
      <div className="modal-overlay">
        <div className="modal-content" style={{ maxWidth: '500px', padding: '24px' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: '12px', marginBottom: '16px', color: 'var(--md-sys-color-error)' }}>
            <AlertTriangle size={24} />
            <h3 style={{ margin: 0, fontSize: '18px', fontWeight: 600 }}>{t('dependencyPrompt.warningTitle', '您确定吗?')}</h3>
          </div>
          <p style={{ fontSize: '14px', lineHeight: '1.6', color: 'var(--md-sys-color-on-surface-variant)', marginBottom: '24px' }}>
            {t('dependencyPrompt.warningDesc', '虽然一些少数附件确实有一些可选的依赖组件. 但请确认您有充足的理由不会下载它们其中任何一个组件, 和这么做对组件还有游戏的影响.')}
          </p>
          <div className="modal-actions" style={{ marginTop: 0 }}>
            <button className="btn btn-secondary" onClick={() => setShowSecondaryWarning(false)}>
              {t('dependencyPrompt.goBack', '返回')}
            </button>
            <button className="btn btn-primary" style={{ background: 'var(--md-sys-color-error)', color: 'var(--md-sys-color-on-error)' }} onClick={onCancel}>
              {t('dependencyPrompt.confirmCancel', '确定')}
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="modal-overlay">
      <div className="modal-content" style={{ maxWidth: '640px', width: '100%', display: 'flex', flexDirection: 'column', maxHeight: '85vh' }}>
        <div className="modal-header">
          <h2 className="modal-title">{t('dependencyPrompt.title', '发现缺失的依赖项')}</h2>
          <div style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginTop: '4px' }}>
            {t('dependencyPrompt.desc', '这些组件是您已安装或正在下载的附件所需的依赖。')}
            <span 
              onClick={onGoToSettings}
              style={{ color: 'var(--md-sys-color-primary)', cursor: 'pointer', marginLeft: '8px', fontWeight: 500 }}
            >
              {t('dependencyPrompt.goToSettings', '前往设置修改默认行为')}
            </span>
          </div>
        </div>

        <div style={{ padding: '16px 20px 0', display: 'flex', alignItems: 'center', gap: '12px' }}>
          <div className="search-container" style={{ flex: 1, margin: 0 }}>
            <Search size={18} className="text-secondary" />
            <input
              type="text"
              className="search-input"
              placeholder={t('dependencyPrompt.search', '搜索依赖项...')}
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
            />
            {searchQuery && (
              <X size={16} style={{ cursor: 'pointer', color: 'var(--md-sys-color-outline)', marginLeft: '8px' }} onClick={() => setSearchQuery('')} />
            )}
          </div>
          <div style={{ fontSize: '13px', color: 'var(--md-sys-color-primary)', fontWeight: 500 }}>
            {t('dependencyPrompt.selectedCount', '已选: {{count}}', { count: selectedIds.size })}
          </div>
        </div>

        <div style={{ padding: '12px 20px', borderBottom: '1px solid var(--md-sys-color-outline-variant)' }}>
          <div 
            style={{ display: 'flex', alignItems: 'center', gap: '8px', cursor: 'pointer', fontSize: '14px', fontWeight: 500, width: 'fit-content' }}
            onClick={() => handleSelectAll(!allFilteredSelected)}
          >
            <div style={{ 
              width: '18px', height: '18px', borderRadius: '4px', 
              border: `2px solid ${allFilteredSelected || someFilteredSelected ? 'var(--md-sys-color-primary)' : 'var(--md-sys-color-outline)'}`,
              background: allFilteredSelected ? 'var(--md-sys-color-primary)' : 'transparent',
              display: 'flex', alignItems: 'center', justifyContent: 'center'
            }}>
              {(allFilteredSelected || someFilteredSelected) && <Check size={14} color={allFilteredSelected ? 'var(--md-sys-color-on-primary)' : 'var(--md-sys-color-primary)'} />}
            </div>
            {t('dependencyPrompt.selectAll', '全选 (本页)')}
          </div>
        </div>

        <div style={{ flex: 1, overflowY: 'auto', padding: '0 20px', minHeight: '200px' }}>
          {filteredDependencies.length === 0 ? (
            <div style={{ padding: '40px 0', textAlign: 'center', color: 'var(--md-sys-color-outline)', fontSize: '14px' }}>
              {t('dependencyPrompt.noResults', '没有找到匹配的依赖项')}
            </div>
          ) : (
            <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', padding: '12px 0' }}>
              {filteredDependencies.map(addon => {
                const id = addon.workshopId || addon.id;
                const title = addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName || id;
                const author = addon.steamDetails?.creator_name || addon.addonInfo?.addonauthor || addon.workshopDetails?.authorName || t('common.unknownAuthor');
                const imageSrc = addon.imagePath || addon.workshopDetails?.previewUrl;
                const isSelected = selectedIds.has(id);
                
                return (
                  <div 
                    key={id} 
                    onClick={() => handleSelect(id, !isSelected)}
                    style={{ 
                      display: 'flex', alignItems: 'center', gap: '12px', padding: '10px 12px', 
                      borderRadius: '8px', 
                      background: isSelected ? 'var(--md-sys-color-secondary-container)' : 'var(--md-sys-color-surface-container)', 
                      outline: isSelected ? '2px solid var(--md-sys-color-primary)' : '1px solid transparent',
                      cursor: 'pointer', transition: 'all 0.2s' 
                    }}
                  >
                    <div style={{ width: '48px', height: '48px', borderRadius: '6px', overflow: 'hidden', flexShrink: 0, background: 'var(--md-sys-color-surface-container-high)', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                      {imageSrc ? (
                        <CacheImage srcPath={imageSrc} cacheRemote alt="" style={{ width: '100%', height: '100%', objectFit: 'cover' }} />
                      ) : (
                        <Package size={24} color="var(--md-sys-color-outline)" />
                      )}
                    </div>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ fontSize: '14px', fontWeight: 500, color: 'var(--md-sys-color-on-surface)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                        {title}
                      </div>
                      <div style={{ fontSize: '12px', color: 'var(--md-sys-color-outline)', display: 'flex', gap: '8px', marginTop: '4px' }}>
                        <span style={{ whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{author}</span>
                        <span>•</span>
                        <span>ID: {id}</span>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>

        <div style={{ padding: '12px 20px', background: 'var(--md-sys-color-surface-container-low)', display: 'flex', alignItems: 'center', justifyContent: 'flex-end', gap: '12px', borderTop: '1px solid var(--md-sys-color-outline-variant)', borderBottomLeftRadius: '16px', borderBottomRightRadius: '16px' }}>
          {isScanning ? (
            <button className="btn" style={{ background: 'var(--md-sys-color-surface-variant)', color: 'var(--md-sys-color-on-surface-variant)' }} onClick={onCancel}>
              {t('dependencyPrompt.remindLater', '稍后提醒我')}
            </button>
          ) : (
            <button className="btn" style={{ background: 'var(--md-sys-color-surface-variant)', color: 'var(--md-sys-color-on-surface-variant)' }} onClick={() => setShowSecondaryWarning(true)}>
              {t('dependencyPrompt.cancelAll', '我都不想下载')}
            </button>
          )}
          
          {selectedIds.size > 0 && (
            <button className="btn btn-primary" onClick={handleDownload}>
              {isScanning ? t('dependencyPrompt.downloadFirst', '先下载这些') : t('dependencyPrompt.download', '下载选中项')}
            </button>
          )}
        </div>
      </div>
    </div>
  );
};
