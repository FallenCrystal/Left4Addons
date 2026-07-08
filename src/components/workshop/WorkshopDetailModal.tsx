import React, { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { X, Download, ExternalLink, PlusCircle, FolderPlus, Loader2, FileText, CheckCircle } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { WorkshopItem, WorkshopBrowserProps, WorkshopPageDetails } from './types';
import { CacheImage } from '../CacheImage';
import { DatabasePayload, Group } from '../../types/addon';
import { Gallery } from '../Gallery';
import { RequiredItems, ParentCollections } from '../WorkshopCommon';
import { fetchWorkshopPageDetails, getWorkshopPageSnapshot, persistWorkshopPageDetails } from '../../services/workshopClient';
import { resolveWorkshopItemAuthor } from './authorDirectory';
import { isKnownWorkshopItem } from '../../utils/workshopKnown';

interface WorkshopDetailModalProps {
  open: boolean;
  item: WorkshopItem | null;
  collection: {
    title: string;
    description: string;
    imagePath: string;
    creatorName: string;
    creatorId: string;
    items: WorkshopItem[];
    workshopId?: string;
  } | null;
  onClose: () => void;
  onDownload: WorkshopBrowserProps['onDownload'];
  onOpenLink: WorkshopBrowserProps['onOpenLink'];
  onImportCollection: WorkshopBrowserProps['onImportCollection'];
  /** Navigate to an addon detail within the app */
  onItemNavigate: (workshopId: string) => void;
  /** Navigate to a collection detail within the app */
  onCollectionNavigate: (workshopId: string) => void;
  addons: Record<string, any>;
  knownUninstalledAddons: Record<string, any>;
  downloadProgress: Record<string, number>;
  isSubmitting: boolean;
  groups?: Group[];
  isLoading?: boolean;
  onDatabaseUpdate?: (data: DatabasePayload) => void;
}

export const WorkshopDetailModal: React.FC<WorkshopDetailModalProps> = ({
  open,
  item,
  collection,
  onClose,
  onDownload,
  onOpenLink,
  onImportCollection,
  onItemNavigate,
  onCollectionNavigate,
  addons,
  knownUninstalledAddons,
  downloadProgress,
  isSubmitting,
  groups,
  isLoading,
  onDatabaseUpdate,
}) => {
  const { t } = useTranslation();
  const [pageDetails, setPageDetails] = useState<WorkshopPageDetails | null>(null);
  const [pageDetailsLoading, setPageDetailsLoading] = useState(false);


  // Fetch extra details by scraping the workshop page HTML
  const fetchPageDetails = useCallback(async (workshopId: string) => {
    setPageDetailsLoading(true);
    const snapshot = await getWorkshopPageSnapshot(workshopId);
    if (snapshot) {
      setPageDetails(snapshot);
    } else {
      setPageDetails(null);
    }
    try {
      const details = await fetchWorkshopPageDetails(workshopId, 'workshop-detail');
      setPageDetails(details);
      const data: DatabasePayload = await persistWorkshopPageDetails(workshopId, details, 'workshop-detail') as DatabasePayload;
      onDatabaseUpdate?.(data);
    } catch (err) {
      console.error('Failed to fetch workshop page details:', err);
    } finally {
      setPageDetailsLoading(false);
    }
  }, [onDatabaseUpdate]);

  const firstItemWorkshopId = collection?.items?.[0]?.workshopId;

  useEffect(() => {
    if (open) {
      const id = item?.workshopId || collection?.workshopId || firstItemWorkshopId;
      if (id) {
        fetchPageDetails(id);
      }
    } else {
      setPageDetails(null);
    }
  }, [open, item?.workshopId, collection?.workshopId, firstItemWorkshopId, fetchPageDetails]);

  if (!open || (!item && !collection)) return null;

  // ── Collection detail ────────────────────────────────────────────────────────
  if (collection) {
    const allWorkshopIds = collection.items.map((i) => i.workshopId);
    const collectionId = collection.workshopId || allWorkshopIds[0];
    const isKnownCollection = !!collectionId && !!groups?.some(
      (group) => group.workshopCollectionId?.trim() === collectionId,
    );
    // Use the large background image from the scraped page if available
    const heroImage = pageDetails?.backgroundImageUrl || collection.imagePath;
    const collectionCreatorName = pageDetails?.creatorName || collection.creatorName;

    return (
      <div className="modal-overlay" onClick={onClose}>
        <div className="modal-content modal-content-wide" onClick={(e) => e.stopPropagation()} style={{ position: 'relative' }}>
          {isLoading && (
            <div style={{ position: 'absolute', top: 0, left: 0, right: 0, bottom: 0, background: 'rgba(0,0,0,0.5)', zIndex: 10, display: 'flex', alignItems: 'center', justifyContent: 'center', borderRadius: '12px' }}>
              <Loader2 size={36} className="animate-spin text-primary" style={{ color: 'var(--md-sys-color-primary)' }} />
            </div>
          )}
          {/* Header */}
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '16px' }}>
            <h2 className="modal-title" style={{ margin: 0 }}>{t('workshop.detail.collectionTitle')}</h2>
            <button className="btn btn-secondary btn-icon-only" onClick={onClose}>
              <X size={16} />
            </button>
          </div>

          <div className="detail-modal-layout">
            {/* Left: preview + metadata */}
            <div className="detail-left">
              <div className="detail-image-box">
                {heroImage ? (
                  <CacheImage
                    srcPath={heroImage}
                    cacheRemote={isKnownCollection}
                    alt={collection.title}
                    className="detail-image"
                  />
                ) : (
                  <FolderPlus size={64} className="text-secondary" />
                )}
              </div>

              <div className="detail-meta-list">
                <div className="detail-meta-item">
                  <span className="detail-meta-label">{t('workshop.detail.collectionAuthor')}</span>
                  <span className="detail-meta-value">{collectionCreatorName}</span>
                </div>
                <div className="detail-meta-item">
                  <span className="detail-meta-label">{t('workshop.detail.collectionItems')}</span>
                  <span className="detail-meta-value">{t('workshop.detail.collectionItemsCount', { count: collection.items.length })}</span>
                </div>
                {collectionId && (
                  <div className="detail-meta-item">
                    <span className="detail-meta-label">{t('workshop.detail.workshopId')}</span>
                    <span className="detail-meta-value">{collectionId}</span>
                  </div>
                )}
                {/* Tags from scraped page */}
                {pageDetails && pageDetails.tags.length > 0 && (
                  <div className="detail-meta-item" style={{ flexDirection: 'column', alignItems: 'flex-start', gap: '4px' }}>
                    <span className="detail-meta-label">{t('workshop.detail.type')}</span>
                    <div style={{ display: 'flex', gap: '6px', flexWrap: 'wrap' }}>
                      {pageDetails.tags.map((tag, i) => (
                        <span key={i} className="tag-chip" style={{ fontSize: '11px', padding: '2px 8px' }}>
                          {tag.name}
                        </span>
                      ))}
                    </div>
                  </div>
                )}
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
                  const isKnown = isKnownWorkshopItem(knownUninstalledAddons, ci.workshopId);
                  const isDownloaded = isKnownWorkshopItem(addons, ci.workshopId);
                  const shouldCacheRemote = isDownloaded || isKnown;
                  return (
                    <div
                      key={ci.workshopId}
                      onClick={() => onItemNavigate(ci.workshopId)}
                      style={{
                        display: 'flex', alignItems: 'center', gap: '10px',
                        padding: '6px 10px', borderRadius: '8px', cursor: 'pointer',
                        background: 'var(--md-sys-color-surface-container-low)',
                      }}
                    >
                      {ci.imagePath ? (
                        <CacheImage srcPath={ci.imagePath} alt={ci.title}
                          cacheRemote={shouldCacheRemote}
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
              <button className="btn btn-secondary" onClick={() => collection.items.forEach((subItem) => onDownload(subItem.workshopId, subItem.title, subItem.imagePath))} disabled={isSubmitting}>
                <Download size={14} />
                <span>{t('workshop.detail.downloadAll', { count: allWorkshopIds.length })}</span>
              </button>
            </div>
            <div style={{ display: 'flex', gap: '12px' }}>
              <a href="#" onClick={(e) => { e.preventDefault(); onOpenLink(`steam://url/CommunityFilePage/${collectionId}`); }} className="btn btn-text" style={{ display: 'inline-flex', gap: '6px' }}>
                <ExternalLink size={14} />
                <span>{t('workshop.detail.openInSteam')}</span>
              </a>
              <a href="#" onClick={(e) => { e.preventDefault(); onOpenLink(`https://steamcommunity.com/sharedfiles/filedetails/?id=${collectionId}`); }} className="btn btn-text" style={{ display: 'inline-flex', gap: '6px' }}>
                <span>{t('workshop.detail.viewOnSteam')}</span>
              </a>
            </div>
          </div>
        </div>
      </div>
    );
  }

  // ── Item detail ──────────────────────────────────────────────────────────────
  if (!item) return null;

  const isKnown = isKnownWorkshopItem(knownUninstalledAddons, item.workshopId);
  const isDownloaded = isKnownWorkshopItem(addons, item.workshopId);
  const downloading = downloadProgress[item.workshopId] !== undefined;
  const progress = downloadProgress[item.workshopId];
  const shouldCacheRemote = isDownloaded || isKnown;
  const resolvedItem = resolveWorkshopItemAuthor(item);
  const displayAuthorName = pageDetails?.creatorName || resolvedItem.authorName;
  const displayAuthorUrl = pageDetails?.creatorProfileUrl || resolvedItem.authorUrl;

  // Find group this addon belongs to
  const itemGroup = groups?.find(g => (
    g.addons.includes(item.workshopId) ||
    g.addons.some((addonId) => {
      const addon = addons[addonId] || knownUninstalledAddons[addonId];
      return addon?.workshopId === item.workshopId;
    })
  ));

  // Build gallery: cover image first, then scraped screenshots
  const scrapedGallery = pageDetails?.imageGallery || [];
  const coverUrl = item.imagePath || '';
  // Deduplicate: if cover is already in scraped gallery, don't add it twice
  const gallery = coverUrl
    ? [coverUrl, ...scrapedGallery.filter((u) => u !== coverUrl)]
    : scrapedGallery;

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal-content modal-content-wide" onClick={(e) => e.stopPropagation()} style={{ position: 'relative' }}>
        {isLoading && (
          <div style={{ position: 'absolute', top: 0, left: 0, right: 0, bottom: 0, background: 'rgba(0,0,0,0.5)', zIndex: 10, display: 'flex', alignItems: 'center', justifyContent: 'center', borderRadius: '12px' }}>
            <Loader2 size={36} className="animate-spin text-primary" style={{ color: 'var(--md-sys-color-primary)' }} />
          </div>
        )}
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
            <Gallery
              gallery={gallery}
              title={item.title}
              fallbackImage={item.imagePath}
              cacheRemote={shouldCacheRemote}
            />

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
              {/* Tags from scraped page */}
              {pageDetails && pageDetails.tags.length > 0 && (
                <div className="detail-meta-item" style={{ flexDirection: 'column', alignItems: 'flex-start', gap: '4px' }}>
                  <span className="detail-meta-label">{t('workshop.detail.type')}</span>
                  <div style={{ display: 'flex', gap: '6px', flexWrap: 'wrap' }}>
                    {pageDetails.tags.map((tag, i) => (
                      <span key={i} className="tag-chip" style={{ fontSize: '11px', padding: '2px 8px' }}>
                        {tag.name}
                      </span>
                    ))}
                  </div>
                </div>
              )}
            </div>
          </div>

          {/* Right: title, author, tags, description, required items, parent collections */}
          <div className="detail-right">
              <h3 style={{ margin: 0, fontSize: '20px', color: '#fff' }}>{item.title}</h3>

            <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap', alignItems: 'center' }}>
              {displayAuthorName && displayAuthorUrl && (
                <a href="#" onClick={(e) => { e.preventDefault(); onOpenLink(displayAuthorUrl); }} style={{ fontSize: '13px', color: 'var(--md-sys-color-primary)', textDecoration: 'underline', display: 'inline-flex', alignItems: 'center', gap: '4px' }}>
                  {displayAuthorName}
                </a>
              )}
              {displayAuthorName && !displayAuthorUrl && (
                <span style={{ fontSize: '13px', color: 'var(--md-sys-color-primary)' }}>{displayAuthorName}</span>
              )}
            </div>

            {/* Group info */}
            {itemGroup && (
              <div className="group-tag" style={{ fontSize: '13px' }}>
                <FolderPlus size={14} />
                <span>{t('detailModal.belongsToGroup', { name: '' })}<strong>{itemGroup.name}</strong></span>
              </div>
            )}

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

            {/* Required items — navigate within the app */}
            <RequiredItems
              requiredItems={pageDetails?.requiredItems || []}
              addons={addons}
              knownUninstalledAddons={knownUninstalledAddons}
              onItemNavigate={onItemNavigate}
            />

            {/* Parent collections — navigate within the app */}
            <ParentCollections
              parentCollections={pageDetails?.parentCollections || []}
              onCollectionNavigate={onCollectionNavigate}
            />

            {/* Loading indicator for page details */}
            {pageDetailsLoading && (
              <div style={{ fontSize: '12px', color: 'var(--md-sys-color-outline)', marginTop: '8px' }}>
                {t('workshop.detail.loadingExtra')}
              </div>
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
              onClick={() => onDownload(item.workshopId, item.title, (item as any).previewUrl || item.imagePath)}
              disabled={isSubmitting || isDownloaded || downloading}
            >
              <Download size={14} />
              <span>
                {downloading
                  ? t('workshop.detail.downloading', { progress: Math.round(progress || 0) })
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
