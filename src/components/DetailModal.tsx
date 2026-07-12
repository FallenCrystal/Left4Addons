import React, { useState, useEffect, useCallback, useRef } from 'react';
import { X, ExternalLink, Move, FolderPlus, Loader2, Download } from 'lucide-react';
import { Addon, DatabasePayload, Group } from '../types/addon';
import { formatBytes, getAddonCategories, getAddonUrl, getAddonAuthor, getAddonInfoValue } from '../utils/addonHelpers';
import { useTranslation } from 'react-i18next';
import { WorkshopPageDetails } from './workshop/types';
import { Gallery } from './Gallery';
import { RequiredItems } from './WorkshopCommon';
import { fetchWorkshopPageDetails, getWorkshopPageSnapshot, persistWorkshopPageDetails } from '../services/workshopClient';

interface DetailModalProps {
  open: boolean;
  addon: Addon | null;
  groups: Group[];
  onCancel: () => void;
  onToggle: (id: string, isEnabled: boolean) => void;
  onMove: (addon: Addon) => void;
  onOpenLink: (url: string) => void;
  addons: Record<string, Addon>;
  knownUninstalledAddons: Record<string, any>;
  onItemNavigate: (workshopId: string) => void;
  onDatabaseUpdate?: (data: DatabasePayload) => void;
  onDownload?: (workshopId: string, title?: string, imagePath?: string) => void;
  downloadProgress?: Record<string, number>;
  isSubmitting?: boolean;
}

export const DetailModal: React.FC<DetailModalProps> = ({
  open,
  addon,
  groups,
  onCancel,
  onToggle,
  onMove,
  onOpenLink,
  addons,
  knownUninstalledAddons,
  onItemNavigate,
  onDatabaseUpdate,
  onDownload,
  downloadProgress = {},
  isSubmitting = false,
}) => {
  const { t } = useTranslation();

  const [pageDetails, setPageDetails] = useState<WorkshopPageDetails | null>(null);
  const [pageDetailsLoading, setPageDetailsLoading] = useState(false);
  const pageDetailsRequestRef = useRef(0);

  const fetchPageDetails = useCallback(async (workshopId: string) => {
    const requestId = ++pageDetailsRequestRef.current;
    setPageDetailsLoading(true);
    setPageDetails(null);

    const snapshot = await getWorkshopPageSnapshot(workshopId);
    if (pageDetailsRequestRef.current !== requestId) return;

    if (snapshot) {
      setPageDetails(snapshot);
      try {
        const data: DatabasePayload = await persistWorkshopPageDetails(workshopId, snapshot, 'addon-detail') as DatabasePayload;
        if (pageDetailsRequestRef.current === requestId) {
          onDatabaseUpdate?.(data);
        }
      } catch (err) {
        console.error('Failed to persist workshop snapshot details:', err);
      }
    }

    try {
      const details = await fetchWorkshopPageDetails(workshopId, 'addon-detail');
      if (pageDetailsRequestRef.current !== requestId) return;
      setPageDetails(details);
      const data: DatabasePayload = await persistWorkshopPageDetails(workshopId, details, 'addon-detail') as DatabasePayload;
      if (pageDetailsRequestRef.current === requestId) {
        onDatabaseUpdate?.(data);
      }
    } catch (err) {
      if (pageDetailsRequestRef.current === requestId) {
        console.error('Failed to fetch workshop page details:', err);
      }
    } finally {
      if (pageDetailsRequestRef.current === requestId) {
        setPageDetailsLoading(false);
      }
    }
  }, [onDatabaseUpdate]);

  useEffect(() => {
    if (open && addon?.workshopId) {
      void fetchPageDetails(addon.workshopId);
    } else {
      pageDetailsRequestRef.current += 1;
      setPageDetails(null);
      setPageDetailsLoading(false);
    }
  }, [open, addon?.workshopId, fetchPageDetails]);

  if (!open || !addon) return null;

  const title = pageDetails?.title
    || addon.workshopDetails?.title
    || addon.steamDetails?.title
    || getAddonInfoValue(addon, 'addontitle')
    || addon.vpkName;
  const author = pageDetails?.creatorName
    || addon.workshopDetails?.creatorName
    || addon.workshopDetails?.authorName
    || getAddonAuthor(addon);
  const desc = pageDetails?.description
    || addon.workshopDetails?.description
    || addon.steamDetails?.description
    || getAddonInfoValue(addon, 'addondescription')
    || getAddonInfoValue(addon, 'addontagline')
    || 'No description provided.';
  const categories = getAddonCategories(addon);
  const itemGroup = groups.find(g => g.addons.includes(addon.id));
  const addonUrl = getAddonUrl(addon);
  const cachedGallery = addon.workshopDetails?.imageGallery || addon.workshopDetails?.galleryUrls || [];
  const galleryDetails = pageDetails?.imageGallery || cachedGallery;
  const requiredItems = pageDetails?.requiredItems || addon.workshopDetails?.requiredItems || [];
  
  const addonauthorSteamID = pageDetails?.creatorSteamId
    || addon.workshopDetails?.creatorSteamId
    || getAddonInfoValue(addon, 'addonauthorsteamid');
  const addonversion = getAddonInfoValue(addon, 'addonversion');

  const isUninstalled = addon.dirType === 'none';
  const isDownloading = addon.workshopId ? (downloadProgress[addon.workshopId] !== undefined) : false;
  const downloadPercent = addon.workshopId ? (downloadProgress[addon.workshopId] ?? 0) : 0;
  
  const displaySize = addon.fileSize 
    ? formatBytes(addon.fileSize) 
    : addon.steamDetails?.file_size 
      ? formatBytes(parseInt(addon.steamDetails.file_size)) 
      : addon.workshopDetails?.fileSizeDisplay 
        ? addon.workshopDetails.fileSizeDisplay 
        : null;

  const handleOpenLinkClick = (e: React.MouseEvent, url: string) => {
    e.preventDefault();
    onOpenLink(url);
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-content modal-content-wide" onClick={(e) => e.stopPropagation()}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '16px' }}>
          <h2 className="modal-title" style={{ margin: 0 }}>{t('detailModal.title')}</h2>
          <button 
            className="btn btn-secondary btn-icon-only" 
            onClick={onCancel}
          >
            <X size={16} />
          </button>
        </div>

        <div className="detail-modal-layout">
          <div className="detail-left">
            <Gallery
              gallery={
                addon.imagePath
                  ? [addon.imagePath, ...galleryDetails.filter((u) => u !== addon.imagePath)]
                  : galleryDetails
              }
              title={title}
              fallbackImage={addon.imagePath}
              cacheRemote
              fallbackIcon={
                <div className="addon-placeholder-icon" style={{ height: '100%', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                  <svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="text-secondary"><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M10 9H8"/><path d="M16 13H8"/><path d="M16 17H8"/></svg>
                </div>
              }
            />

            <div className="detail-meta-list">
              <div className="detail-meta-item">
                <span className="detail-meta-label">{t('detailModal.fileName')}</span>
                <span className="detail-meta-value">{addon.vpkName}</span>
              </div>
              {displaySize && (
                <div className="detail-meta-item">
                  <span className="detail-meta-label">{t('detailModal.addonSize')}</span>
                  <span className="detail-meta-value">{displaySize}</span>
                </div>
              )}
              {addon.filesCount > 0 && (
                <div className="detail-meta-item">
                  <span className="detail-meta-label">{t('detailModal.filesInVpk')}</span>
                  <span className="detail-meta-value">{t('common.vpkCount', { count: addon.filesCount })}</span>
                </div>
              )}
              <div className="detail-meta-item">
                <span className="detail-meta-label">{t('detailModal.directory')}</span>
                <span className="detail-meta-value">
                  {isUninstalled 
                    ? t('addonCard.uninstalled') 
                    : addon.dirType === 'loading' 
                      ? t('detailModal.manualInstall') 
                      : t('detailModal.workshop')}
                </span>
              </div>
              <div className="detail-meta-item">
                <span className="detail-meta-label">{t('detailModal.currentStatus')}</span>
                {isUninstalled ? (
                  <span className="detail-meta-value" style={{ color: 'var(--md-sys-color-tertiary)' }}>
                    {t('addonCard.uninstalled')}
                  </span>
                ) : (
                  <span className="detail-meta-value" style={{ color: addon.isEnabled ? 'var(--md-sys-color-success)' : 'var(--md-sys-color-error)' }}>
                    {addon.isEnabled ? t('detailModal.enabled') : t('detailModal.disabled')}
                  </span>
                )}
              </div>
              {addon.workshopId && (
                <div className="detail-meta-item">
                  <span className="detail-meta-label">{t('detailModal.workshopId')}</span>
                  <span className="detail-meta-value">{addon.workshopId}</span>
                </div>
              )}
              {addonUrl && (
                <div className="detail-meta-item">
                  <span className="detail-meta-label">{t('detailModal.relatedLink')}</span>
                  <span className="detail-meta-value">
                    <a 
                      href="#" 
                      onClick={(e) => handleOpenLinkClick(e, addonUrl)}
                      style={{ color: 'var(--md-sys-color-primary)', textDecoration: 'underline', wordBreak: 'break-all', display: 'inline-flex', alignItems: 'center', gap: '4px' }}
                    >
                      <span>{t('detailModal.clickToVisit')}</span>
                      <ExternalLink size={10} />
                    </a>
                  </span>
                </div>
              )}
            </div>
          </div>

          <div className="detail-right">
            <h3 style={{ margin: 0, fontSize: '20px', color: '#fff' }}>{title}</h3>
            
            <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap', alignItems: 'center' }}>
              <span style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
                {t('detailModal.author', { author: '' })}<strong style={{ color: '#fff' }}>{author}</strong>
              </span>
              {addonauthorSteamID && (
                <a 
                  href="#"
                  onClick={(e) => handleOpenLinkClick(e, `https://steamcommunity.com/profiles/${addonauthorSteamID}`)}
                  className="btn btn-text"
                  style={{ padding: '2px 4px', fontSize: '11px', display: 'inline-flex', alignItems: 'center', gap: '4px' }}
                >
                  <span>{t('detailModal.viewSteamProfile')}</span>
                  <ExternalLink size={10} />
                </a>
              )}
            </div>

            {addonversion && (
              <div style={{ fontSize: '12px', color: 'var(--md-sys-color-outline)' }}>
                {t('detailModal.version', { version: addonversion })}
              </div>
            )}

            {itemGroup && (
              <div className="group-tag" style={{ fontSize: '13px' }}>
                <FolderPlus size={14} />
                <span>{t('detailModal.belongsToGroup', { name: '' })}<strong>{itemGroup.name}</strong></span>
              </div>
            )}

            <div style={{ display: 'flex', gap: '6px', flexWrap: 'wrap' }}>
              {categories.map(c => (
                <span key={c} className="tag-chip" style={{ fontSize: '11px', padding: '4px 10px' }}>{t(`categories.${c}`, c)}</span>
              ))}
            </div>

            <div style={{ fontWeight: '600', fontSize: '14px', color: '#fff', marginTop: '8px' }}>{t('detailModal.descriptionLabel')}</div>
            <div className="description-block">{desc}</div>

            {/* Required items */}
            <RequiredItems
              requiredItems={requiredItems}
              addons={addons}
              knownUninstalledAddons={knownUninstalledAddons}
              onItemNavigate={onItemNavigate}
            />

            {/* Loading indicator for page details */}
            {pageDetailsLoading && (
              <div style={{ display: 'flex', alignItems: 'center', gap: '8px', marginTop: '12px', fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
                <Loader2 size={14} className="animate-spin" />
                <span>{t('workshop.detail.loadingExtra')}</span>
              </div>
            )}
          </div>
        </div>

        <div className="modal-actions" style={{ 
          display: 'flex', 
          justifyContent: 'space-between', 
          alignItems: 'center', 
          padding: '16px 0 0 0', 
          borderTop: '1px solid var(--md-sys-color-outline-variant)',
          marginTop: '24px'
        }}>
          <div style={{ display: 'flex', gap: '12px' }}>
            {addon.workshopId && (
              <>
                <a 
                  href="#"
                  onClick={(e) => handleOpenLinkClick(e, `steam://url/CommunityFilePage/${addon.workshopId}`)}
                  className="btn btn-secondary"
                  style={{ display: 'inline-flex', gap: '6px' }}
                >
                  <ExternalLink size={14} />
                  <span>{t('detailModal.openInSteam')}</span>
                </a>
                <a 
                  href="#"
                  onClick={(e) => handleOpenLinkClick(e, `https://steamcommunity.com/sharedfiles/filedetails/?id=${addon.workshopId}`)}
                  className="btn btn-text"
                  style={{ display: 'inline-flex', gap: '6px' }}
                >
                  <span>{t('detailModal.webViewOfficial')}</span>
                </a>
              </>
            )}
            {!addon.workshopId && addonUrl && (
              <a 
                href="#"
                onClick={(e) => handleOpenLinkClick(e, addonUrl)}
                className="btn btn-secondary"
                style={{ display: 'inline-flex', gap: '6px' }}
              >
                <ExternalLink size={14} />
                <span>{t('detailModal.visitSource')}</span>
              </a>
            )}
          </div>
          
          <div style={{ display: 'flex', gap: '12px' }}>
            {isUninstalled ? (
              addon.workshopId && onDownload && (
                <button
                  className="btn btn-primary"
                  onClick={() => onDownload(addon.workshopId!, title, addon.imagePath)}
                  disabled={isSubmitting || isDownloading}
                  style={{ display: 'inline-flex', gap: '6px', alignItems: 'center' }}
                >
                  <Download size={14} />
                  <span>
                    {isDownloading
                      ? t('workshop.detail.downloading', { progress: Math.round(downloadPercent) })
                      : t('workshop.detail.download')}
                  </span>
                </button>
              )
            ) : (
              <button 
                className={`btn ${addon.isEnabled ? 'btn-secondary' : 'btn-primary'}`}
                onClick={() => onToggle(addon.id, addon.isEnabled)}
                disabled={isSubmitting}
              >
                {addon.isEnabled ? t('detailModal.disableAddon') : t('detailModal.enableAddon')}
              </button>
            )}

            {addon.dirType === 'workshop' && (
              <button 
                className="btn btn-primary"
                onClick={() => onMove(addon)}
                disabled={isSubmitting}
              >
                <Move size={14} />
                <span>{t('detailModal.moveToManual')}</span>
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
