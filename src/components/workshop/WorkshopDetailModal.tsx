import React, { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ChevronLeft, Download, ExternalLink, PlusCircle, FolderPlus, Loader2, FileText, CheckCircle } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { WorkshopItem, WorkshopBrowserProps, WorkshopPageDetails } from './types';
import { CacheImage } from '../common/CacheImage';
import { DatabasePayload, Group } from '../../types/addon';
import { Gallery } from '../addons/Gallery';
import { RequiredItems, ParentCollections } from './WorkshopCommon';
import { renderTextWithLinks } from './linkHelper';
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
  onDownloadMany?: WorkshopBrowserProps['onDownloadMany'];
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
  onDirectNavigate?: (workshopId: string) => void;
}

export const WorkshopDetailModal: React.FC<WorkshopDetailModalProps> = ({
  open,
  item,
  collection,
  onClose,
  onDownload,
  onDownloadMany,
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
  onDirectNavigate,
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
        setPageDetails(null); // Reset page details immediately to avoid stale details on navigation!
        fetchPageDetails(id);
      }
    } else {
      setPageDetails(null);
    }
  }, [open, item?.workshopId, collection?.workshopId, firstItemWorkshopId, fetchPageDetails]);

  if (!open || (!item && !collection)) return null;

  // ── Collection detail ────────────────────────────────────────────────────────
  if (collection) {
    const displayCollectionItems = pageDetails?.collectionItems?.length
      ? pageDetails.collectionItems
      : collection.items;
    const allWorkshopIds = displayCollectionItems.map((i) => i.workshopId);
    const collectionId = collection.workshopId || allWorkshopIds[0];
    const isKnownCollection = !!collectionId && !!groups?.some(
      (group) => group.workshopCollectionId?.trim() === collectionId,
    );
    // Use the large background image from the scraped page if available
    const heroImage = pageDetails?.backgroundImageUrl || collection.imagePath;
    const collectionCreatorName = pageDetails?.creatorName || collection.creatorName;
    const collectionDescription = pageDetails?.description || collection.description;

    return (
      <div className="workshop-detail-page" style={{ position: 'relative' }}>
        {isLoading && (
          <div style={{ position: 'absolute', top: 0, left: 0, right: 0, bottom: 0, background: 'rgba(0,0,0,0.5)', zIndex: 10, display: 'flex', alignItems: 'center', justifyContent: 'center', borderRadius: '24px' }}>
            <Loader2 size={36} className="animate-spin text-primary" style={{ color: 'var(--md-sys-color-primary)' }} />
          </div>
        )}
        {/* Header */}
        <div className="workshop-detail-page-header">
          <button className="btn btn-outline workshop-detail-back-btn" onClick={onClose}>
            <ChevronLeft size={16} />
            <span>{t('common.back', '返回')}</span>
          </button>
          <h2 style={{ margin: 0, fontSize: '18px', fontWeight: 600, color: 'var(--md-sys-color-on-surface)', textOverflow: 'ellipsis', overflow: 'hidden', whiteSpace: 'nowrap' }}>
            {collection.title}
          </h2>
        </div>

        <div className="workshop-detail-columns">
          {/* Left Main Column */}
          <div className="workshop-detail-main">
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

            {collectionDescription && (
              <div className="workshop-detail-card">
                <div style={{ fontWeight: '600', fontSize: '16px', color: 'var(--md-sys-color-on-surface)', marginBottom: '8px' }}>{t('workshop.detail.descriptionLabel')}</div>
                <div className="description-block" style={{ border: 'none', padding: 0, backgroundColor: 'transparent', maxHeight: 'none', overflowY: 'visible' }}>{renderTextWithLinks(collectionDescription, onDirectNavigate, onOpenLink)}</div>
              </div>
            )}

            <div className="workshop-detail-card">
              <div style={{ fontWeight: '600', fontSize: '16px', color: 'var(--md-sys-color-on-surface)', marginBottom: '8px' }}>
                {t('workshop.detail.itemsInCollection')}
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', maxHeight: '400px', overflowY: 'auto' }}>
                {displayCollectionItems.map((ci) => {
                  const isKnown = isKnownWorkshopItem(knownUninstalledAddons, ci.workshopId);
                  const isDownloaded = isKnownWorkshopItem(addons, ci.workshopId);
                  const shouldCacheRemote = isDownloaded || isKnown;
                  return (
                    <div
                      key={ci.workshopId}
                      onClick={() => onItemNavigate(ci.workshopId)}
                      style={{
                        display: 'flex', alignItems: 'center', gap: '16px',
                        padding: '12px 18px', borderRadius: '16px', cursor: 'pointer',
                        background: 'var(--md-sys-color-surface-container-high)',
                        transition: 'background-color 0.15s ease',
                      }}
                      onMouseEnter={(e) => e.currentTarget.style.backgroundColor = 'var(--md-sys-color-surface-container-highest)'}
                      onMouseLeave={(e) => e.currentTarget.style.backgroundColor = 'var(--md-sys-color-surface-container-high)'}
                    >
                      {ci.imagePath ? (
                        <CacheImage srcPath={ci.imagePath} alt={ci.title}
                          cacheRemote={shouldCacheRemote}
                          className="addon-thumb" style={{ width: 56, height: 56, borderRadius: 10, objectFit: 'cover' }} />
                      ) : (
                        <div style={{ width: 56, height: 56, borderRadius: 10, background: 'var(--md-sys-color-surface-container-highest)', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                          <FileText size={24} className="text-secondary" />
                        </div>
                      )}
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div style={{ fontSize: '15px', fontWeight: 600, color: 'var(--md-sys-color-on-surface)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', marginBottom: '4px' }}>{ci.title}</div>
                        <div style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>{ci.authorName}</div>
                      </div>
                      {isDownloaded && <CheckCircle size={18} className="icon-success" />}
                      {!isDownloaded && isKnown && <span style={{ fontSize: '12px', color: 'var(--md-sys-color-tertiary)' }}>{t('workshop.badges.known')}</span>}
                    </div>
                  );
                })}
              </div>
            </div>
          </div>

          {/* Right Sidebar Column */}
          <div className="workshop-detail-sidebar">
            <div className="workshop-detail-title-section">
              <h1 className="workshop-detail-title" style={{ fontSize: '22px' }}>{collection.title}</h1>
            </div>

            {/* Actions for Collection */}
            <div className="workshop-detail-card" style={{ gap: '16px' }}>
              <div style={{ display: 'flex', flexDirection: 'column', gap: '10px' }}>
                <button className="btn btn-secondary" onClick={() => onImportCollection(collection.title, allWorkshopIds)} disabled={isSubmitting} style={{ width: '100%', justifyContent: 'center' }}>
                  <FolderPlus size={14} />
                  <span>{t('workshop.detail.importAsGroup')}</span>
                </button>
                <button className="btn btn-primary" onClick={() => {
                  const downloadItems = displayCollectionItems.map((subItem) => ({
                    workshopId: subItem.workshopId,
                    title: subItem.title,
                    imagePath: subItem.imagePath,
                  }));
                  if (onDownloadMany) {
                    onDownloadMany(downloadItems);
                  } else {
                    downloadItems.forEach((subItem) => onDownload(subItem.workshopId, subItem.title, subItem.imagePath));
                  }
                }} disabled={isSubmitting} style={{ width: '100%', justifyContent: 'center' }}>
                  <Download size={14} />
                  <span>{t('workshop.detail.downloadAll', { count: allWorkshopIds.length })}</span>
                </button>
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', borderTop: '1px solid var(--md-sys-color-outline-variant)', paddingTop: '12px' }}>
                <a href="#" onClick={(e) => { e.preventDefault(); onOpenLink(`steam://url/CommunityFilePage/${collectionId}`); }} className="btn btn-text" style={{ display: 'inline-flex', gap: '6px', justifyContent: 'flex-start', padding: '6px 0' }}>
                  <ExternalLink size={14} />
                  <span>{t('workshop.detail.openInSteam')}</span>
                </a>
                <a href="#" onClick={(e) => { e.preventDefault(); onOpenLink(`https://steamcommunity.com/sharedfiles/filedetails/?id=${collectionId}`); }} className="btn btn-text" style={{ display: 'inline-flex', gap: '6px', justifyContent: 'flex-start', padding: '6px 0' }}>
                  <ExternalLink size={14} />
                  <span>{t('workshop.detail.viewOnSteam')}</span>
                </a>
              </div>
            </div>

            {/* Metadata Card */}
            <div className="workshop-detail-card">
              <div style={{ fontWeight: '600', fontSize: '14px', color: 'var(--md-sys-color-on-surface-variant)' }}>{t('workshop.detail.metadata', '属性')}</div>
              <div className="detail-meta-list" style={{ border: 'none', padding: 0, backgroundColor: 'transparent', gap: '12px' }}>
                <div className="detail-meta-item">
                  <span className="detail-meta-label">{t('workshop.detail.collectionAuthor')}</span>
                  <span className="detail-meta-value">{collectionCreatorName}</span>
                </div>
                <div className="detail-meta-item">
                  <span className="detail-meta-label">{t('workshop.detail.collectionItems')}</span>
                  <span className="detail-meta-value">{t('workshop.detail.collectionItemsCount', { count: displayCollectionItems.length })}</span>
                </div>
                {collectionId && (
                  <div className="detail-meta-item">
                    <span className="detail-meta-label">{t('workshop.detail.workshopId')}</span>
                    <span className="detail-meta-value">{collectionId}</span>
                  </div>
                )}
                {pageDetails && pageDetails.tags.length > 0 && (
                  <div className="detail-meta-item" style={{ flexDirection: 'column', alignItems: 'flex-start', gap: '6px', borderTop: '1px solid var(--md-sys-color-outline-variant)', paddingTop: '10px', marginTop: '4px' }}>
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
  const displayDescription = pageDetails?.description || item.shortDescription;

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
  const coverUrl = pageDetails?.previewUrl || item.imagePath || '';
  // Deduplicate: if cover is already in scraped gallery, don't add it twice
  const gallery = coverUrl
    ? [coverUrl, ...scrapedGallery.filter((u) => u !== coverUrl)]
    : scrapedGallery;

  return (
    <div className="workshop-detail-page" style={{ position: 'relative' }}>
      {isLoading && (
        <div style={{ position: 'absolute', top: 0, left: 0, right: 0, bottom: 0, background: 'rgba(0,0,0,0.5)', zIndex: 10, display: 'flex', alignItems: 'center', justifyContent: 'center', borderRadius: '24px' }}>
          <Loader2 size={36} className="animate-spin text-primary" style={{ color: 'var(--md-sys-color-primary)' }} />
        </div>
      )}
      {/* Header */}
      <div className="workshop-detail-page-header">
        <button className="btn btn-outline workshop-detail-back-btn" onClick={onClose}>
          <ChevronLeft size={16} />
          <span>{t('common.back', '返回')}</span>
        </button>
        <h2 style={{ margin: 0, fontSize: '18px', fontWeight: 600, color: 'var(--md-sys-color-on-surface)', textOverflow: 'ellipsis', overflow: 'hidden', whiteSpace: 'nowrap' }}>
          {item.title}
        </h2>
      </div>

      <div className="workshop-detail-columns">
        {/* Left Main Column */}
        <div className="workshop-detail-main">
          <Gallery
            gallery={gallery}
            title={item.title}
            fallbackImage={coverUrl}
            cacheRemote={shouldCacheRemote}
          />

          {displayDescription && (
            <div className="workshop-detail-card">
              <div style={{ fontWeight: '600', fontSize: '16px', color: 'var(--md-sys-color-on-surface)', marginBottom: '8px' }}>{t('workshop.detail.descriptionLabel')}</div>
              <div className="description-block" style={{ border: 'none', padding: 0, backgroundColor: 'transparent', maxHeight: 'none', overflowY: 'visible' }}>{renderTextWithLinks(displayDescription, onDirectNavigate, onOpenLink)}</div>
            </div>
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
        </div>

        {/* Right Sidebar Column */}
        <div className="workshop-detail-sidebar">
          <div className="workshop-detail-title-section">
            <h1 className="workshop-detail-title" style={{ fontSize: '22px' }}>{item.title}</h1>
            <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap', alignItems: 'center', marginTop: '4px' }}>
              {displayAuthorName && displayAuthorUrl && (
                <a href="#" onClick={(e) => { e.preventDefault(); onOpenLink(displayAuthorUrl); }} style={{ fontSize: '13px', color: 'var(--md-sys-color-primary)', textDecoration: 'underline', display: 'inline-flex', alignItems: 'center', gap: '4px' }}>
                  {displayAuthorName}
                </a>
              )}
              {displayAuthorName && !displayAuthorUrl && (
                <span style={{ fontSize: '13px', color: 'var(--md-sys-color-primary)' }}>{displayAuthorName}</span>
              )}
            </div>
          </div>

          {/* Actions for Item */}
          <div className="workshop-detail-card" style={{ gap: '16px' }}>
            <div style={{ display: 'flex', flexDirection: 'column', gap: '10px' }}>
              <button
                className="btn btn-primary"
                onClick={() => onDownload(item.workshopId, item.title, coverUrl)}
                disabled={isSubmitting || isDownloaded || downloading}
                style={{ width: '100%', justifyContent: 'center' }}
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
                  style={{ width: '100%', justifyContent: 'center' }}
                >
                  <PlusCircle size={14} />
                  <span>{isKnown ? t('workshop.badges.known') : t('workshop.detail.addToKnownList')}</span>
                </button>
              )}
            </div>
            
            <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', borderTop: '1px solid var(--md-sys-color-outline-variant)', paddingTop: '12px' }}>
              <a href="#" onClick={(e) => { e.preventDefault(); onOpenLink(`steam://url/CommunityFilePage/${item.workshopId}`); }} className="btn btn-text" style={{ display: 'inline-flex', gap: '6px', justifyContent: 'flex-start', padding: '6px 0' }}>
                <ExternalLink size={14} />
                <span>{t('workshop.detail.openInSteam')}</span>
              </a>
              <a href="#" onClick={(e) => { e.preventDefault(); onOpenLink(`https://steamcommunity.com/sharedfiles/filedetails/?id=${item.workshopId}`); }} className="btn btn-text" style={{ display: 'inline-flex', gap: '6px', justifyContent: 'flex-start', padding: '6px 0' }}>
                <ExternalLink size={14} />
                <span>{t('workshop.detail.viewOnSteam')}</span>
              </a>
            </div>
          </div>

          {/* Group info */}
          {itemGroup && (
            <div className="group-tag" style={{ fontSize: '13px', display: 'flex', alignItems: 'center', gap: '8px', padding: '12px 16px', borderRadius: '16px', background: 'rgba(var(--md-sys-color-primary-rgb), 0.08)', color: 'var(--md-sys-color-primary)' }}>
              <FolderPlus size={14} />
              <span>{t('detailModal.belongsToGroup', { name: '' })}<strong>{itemGroup.name}</strong></span>
            </div>
          )}

          {/* Metadata Card */}
          <div className="workshop-detail-card">
            <div style={{ fontWeight: '600', fontSize: '14px', color: 'var(--md-sys-color-on-surface-variant)' }}>{t('workshop.detail.metadata', '属性')}</div>
            <div className="detail-meta-list" style={{ border: 'none', padding: 0, backgroundColor: 'transparent', gap: '12px' }}>
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
              {pageDetails && pageDetails.tags.length > 0 && (
                <div className="detail-meta-item" style={{ flexDirection: 'column', alignItems: 'flex-start', gap: '6px', borderTop: '1px solid var(--md-sys-color-outline-variant)', paddingTop: '10px', marginTop: '4px' }}>
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

          {/* Loading indicator for page details */}
          {pageDetailsLoading && (
            <div style={{ fontSize: '12px', color: 'var(--md-sys-color-outline)', marginTop: '8px', textAlign: 'center' }}>
              {t('workshop.detail.loadingExtra')}
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
