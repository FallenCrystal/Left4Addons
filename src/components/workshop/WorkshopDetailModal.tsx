import React from 'react';
import { invoke } from '@tauri-apps/api/core';
import { X, Download, ExternalLink, CheckCircle, PlusCircle, FileText, FolderPlus } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { WorkshopItem, WorkshopBrowserProps } from './types';
import { CacheImage } from '../CacheImage';

interface CollectionData {
  title: string;
  description: string;
  imagePath: string;
  creatorName: string;
  creatorId: string;
  items: WorkshopItem[];
}

interface WorkshopDetailModalProps {
  open: boolean;
  item: WorkshopItem | null;
  collection: CollectionData | null;
  onClose: () => void;
  onDownload: WorkshopBrowserProps['onDownload'];
  onOpenLink: WorkshopBrowserProps['onOpenLink'];
  onImportCollection: WorkshopBrowserProps['onImportCollection'];
  addons: Record<string, any>;
  knownUninstalledAddons: Record<string, any>;
  downloadProgress: Record<string, number>;
  isSubmitting: boolean;
}

export const WorkshopDetailModal: React.FC<WorkshopDetailModalProps> = ({
  open,
  item,
  collection,
  onClose,
  onDownload,
  onOpenLink,
  onImportCollection,
  addons,
  knownUninstalledAddons,
  downloadProgress,
  isSubmitting,
}) => {
  const { t } = useTranslation();
  if (!open || (!item && !collection)) return null;

  // ── Collection detail ────────────────────────────────────────────────────────
  if (collection) {
    const allWorkshopIds = collection.items.map((i) => i.workshopId);
    const allDownloaded = allWorkshopIds.every((id) => addons[id]);
    const allKnown = allWorkshopIds.every((id) => knownUninstalledAddons[id]);

    return (
      <div className="modal-overlay" onClick={onClose}>
        <div className="modal-content modal-content-wide" onClick={(e) => e.stopPropagation()}>
          {/* Header */}
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '16px' }}>
            <h2 className="modal-title" style={{ margin: 0 }}>{t('workshop.detail.collectionTitle')}</h2>
            <button className="btn btn-secondary btn-icon-only" onClick={onClose}>
              <X size={16} />
            </button>
          </div>

          <div className="detail-modal-layout">
            {/* Left: preview */}
            <div className="detail-left">
              <div className="detail-image-box">
                {collection.imagePath ? (
                  <CacheImage srcPath={collection.imagePath} alt={collection.title} className="detail-image" />
                ) : (
                  <FolderPlus size={64} className="text-secondary" />
                )}
              </div>

              <div className="detail-meta-list">
                <div className="detail-meta-item">
                  <span className="detail-meta-label">{t('workshop.detail.collectionAuthor')}</span>
                  <span className="detail-meta-value">{collection.creatorName}</span>
                </div>
                <div className="detail-meta-item">
                  <span className="detail-meta-label">{t('workshop.detail.collectionItems')}</span>
                  <span className="detail-meta-value">{t('workshop.detail.collectionItemsCount', { count: collection.items.length })}</span>
                </div>
              </div>
            </div>

            {/* Right: description + items */}
            <div className="detail-right">
              <h3 style={{ margin: 0, fontSize: '20px', color: '#fff' }}>{collection.title}</h3>
              {collection.description && (
                <>
                  <div style={{ fontWeight: '600', fontSize: '14px', color: '#fff', marginTop: '8px' }}>{t('workshop.detail.descriptionLabel')}</div>
                  <div className="description-block" style={{ maxHeight: '120px', overflowY: 'auto' }}>{collection.description}</div>
                </>
              )}

              <div style={{ fontWeight: '600', fontSize: '14px', color: '#fff', marginTop: '12px' }}>
                {t('workshop.detail.itemsInCollection')}
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: '6px', marginTop: '6px', maxHeight: '200px', overflowY: 'auto' }}>
                {collection.items.map((ci) => {
                  const isKnown = !!knownUninstalledAddons[ci.workshopId];
                  const isDownloaded = !!addons[ci.workshopId];
                  return (
                    <div key={ci.workshopId} style={{
                      display: 'flex', alignItems: 'center', gap: '10px',
                      padding: '6px 10px', borderRadius: '8px',
                      background: 'var(--md-sys-color-surface-container-low)',
                    }}>
                      {ci.imagePath ? (
                        <CacheImage srcPath={ci.imagePath} alt={ci.title}
                          className="addon-thumb" style={{ width: 40, height: 40, borderRadius: 6, objectFit: 'cover' }} />
                      ) : (
                        <div style={{ width: 40, height: 40, borderRadius: 6, background: 'var(--md-sys-color-surface-container-high)', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                          <FileText size={20} className="text-secondary" />
                        </div>
                      )}
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div style={{ fontSize: '13px', fontWeight: 500, color: '#fff', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{ci.title}</div>
                        <div style={{ fontSize: '11px', color: 'var(--md-sys-color-outline)' }}>{ci.authorName}</div>
                      </div>
                      {isDownloaded && <CheckCircle size={16} className="icon-success" />}
                      {!isDownloaded && isKnown && <span style={{ fontSize: '11px', color: 'var(--md-sys-color-tertiary)' }}>{t('workshop.badges.known')}</span>}
                    </div>
                  );
                })}
              </div>
            </div>
          </div>

          {/* Bottom actions */}
          <div className="modal-actions" style={{
            display: 'flex', justifyContent: 'space-between', alignItems: 'center',
            padding: '16px 0 0 0', borderTop: '1px solid var(--md-sys-color-outline-variant)', marginTop: '24px',
          }}>
            <div style={{ display: 'flex', gap: '12px' }}>
              <button className="btn btn-secondary" onClick={() => onImportCollection(collection.title, allWorkshopIds)} disabled={isSubmitting}>
                <FolderPlus size={14} />
                <span>{t('workshop.detail.importAsGroup')}</span>
              </button>
              <button className="btn btn-secondary" onClick={() => allWorkshopIds.forEach((id) => onDownload(id))} disabled={isSubmitting}>
                <Download size={14} />
                <span>{t('workshop.detail.downloadAll', { count: allWorkshopIds.length })}</span>
              </button>
            </div>
            <div style={{ display: 'flex', gap: '12px' }}>
              <a href="#" onClick={(e) => { e.preventDefault(); onOpenLink(`steam://url/CommunityFilePage/${collection.items[0]?.workshopId}`); }} className="btn btn-text" style={{ display: 'inline-flex', gap: '6px' }}>
                <ExternalLink size={14} />
                <span>{t('workshop.detail.openInSteam')}</span>
              </a>
            </div>
          </div>
        </div>
      </div>
    );
  }

  // ── Item detail ──────────────────────────────────────────────────────────────
  if (!item) return null;

  const isKnown = !!knownUninstalledAddons[item.workshopId];
  const isDownloaded = !!addons[item.workshopId];
  const downloading = downloadProgress[item.workshopId] !== undefined;
  const progress = downloadProgress[item.workshopId];

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal-content modal-content-wide" onClick={(e) => e.stopPropagation()}>
        {/* Header */}
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '16px' }}>
          <h2 className="modal-title" style={{ margin: 0 }}>{t('workshop.detail.title')}</h2>
          <button className="btn btn-secondary btn-icon-only" onClick={onClose}>
            <X size={16} />
          </button>
        </div>

        <div className="detail-modal-layout">
          {/* Left: preview + metadata */}
          <div className="detail-left">
            <div className="detail-image-box">
              {item.imagePath ? (
                <CacheImage srcPath={item.imagePath} alt={item.title} className="detail-image" />
              ) : (
                <FileText size={64} className="text-secondary" />
              )}
            </div>

            <div className="detail-meta-list">
              {item.fileSize && (
                <div className="detail-meta-item">
                  <span className="detail-meta-label">{t('workshop.detail.itemSize')}</span>
                  <span className="detail-meta-value">{item.fileSize}</span>
                </div>
              )}
              {item.workshopId && (
                <div className="detail-meta-item">
                  <span className="detail-meta-label">{t('workshop.detail.workshopId')}</span>
                  <span className="detail-meta-value">{item.workshopId}</span>
                </div>
              )}
              <div className="detail-meta-item">
                <span className="detail-meta-label">{t('workshop.detail.status')}</span>
                <span className="detail-meta-value" style={{ color: isDownloaded ? 'var(--md-sys-color-success)' : isKnown ? 'var(--md-sys-color-tertiary)' : 'var(--md-sys-color-outline)' }}>
                  {isDownloaded ? t('workshop.badges.downloaded') : isKnown ? t('workshop.badges.known') : t('workshop.badges.notDownloaded')}
                </span>
              </div>
            </div>
          </div>

          {/* Right: title, author, tags, description */}
          <div className="detail-right">
            <h3 style={{ margin: 0, fontSize: '20px', color: '#fff' }}>{item.title}</h3>

            <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap', alignItems: 'center' }}>
              {item.authorName && (
                <a href="#" onClick={(e) => { e.preventDefault(); onOpenLink(item.authorUrl); }} style={{ fontSize: '13px', color: 'var(--md-sys-color-primary)', textDecoration: 'underline', display: 'inline-flex', alignItems: 'center', gap: '4px' }}>
                  {item.authorName}
                </a>
              )}
            </div>

            {item.tags && item.tags.length > 0 && (
              <div style={{ display: 'flex', gap: '6px', flexWrap: 'wrap' }}>
                {item.tags.map((tag, i) => (
                  <span key={i} className="tag-chip" style={{ fontSize: '11px', padding: '4px 10px' }}>{tag}</span>
                ))}
              </div>
            )}

            {item.shortDescription && (
              <>
                <div style={{ fontWeight: '600', fontSize: '14px', color: '#fff', marginTop: '8px' }}>{t('workshop.detail.descriptionLabel')}</div>
                <div className="description-block">{item.shortDescription}</div>
              </>
            )}
          </div>
        </div>

        {/* Bottom actions */}
        <div className="modal-actions" style={{
          display: 'flex', justifyContent: 'space-between', alignItems: 'center',
          padding: '16px 0 0 0', borderTop: '1px solid var(--md-sys-color-outline-variant)', marginTop: '24px',
        }}>
          <div style={{ display: 'flex', gap: '12px' }}>
            <button
              className="btn btn-primary"
              onClick={() => onDownload(item.workshopId)}
              disabled={isSubmitting || isDownloaded || downloading}
            >
              <Download size={14} />
              <span>
                {downloading
                  ? t('workshop.detail.downloading', { progress: Math.round((progress || 0) * 100) })
                  : isDownloaded
                    ? t('workshop.detail.downloaded')
                    : t('workshop.detail.download')}
              </span>
            </button>
            {!isDownloaded && (
              <button
                className="btn btn-secondary"
                onClick={async () => {
                  try {
                    await invoke('add_known_addon', { workshopId: item.workshopId });
                  } catch (e) {
                    console.error('Failed to add to known list:', e);
                  }
                }}
                disabled={isSubmitting || isKnown}
              >
                <PlusCircle size={14} />
                <span>{isKnown ? t('workshop.badges.known') : t('workshop.detail.addToKnownList')}</span>
              </button>
            )}
            <a href="#" onClick={(e) => { e.preventDefault(); onOpenLink(`steam://url/CommunityFilePage/${item.workshopId}`); }} className="btn btn-text" style={{ display: 'inline-flex', gap: '6px' }}>
              <ExternalLink size={14} />
              <span>{t('workshop.detail.openInSteam')}</span>
            </a>
            <a href="#" onClick={(e) => { e.preventDefault(); onOpenLink(`https://steamcommunity.com/sharedfiles/filedetails/?id=${item.workshopId}`); }} className="btn btn-text" style={{ display: 'inline-flex', gap: '6px' }}>
              <span>{t('workshop.detail.viewOnSteam')}</span>
            </a>
          </div>
        </div>
      </div>
    </div>
  );
};
