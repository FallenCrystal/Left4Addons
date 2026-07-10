import React from 'react';
import { Package, Link2, CheckCircle, FolderPlus } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { findKnownWorkshopEntry, isKnownWorkshopItem } from '../utils/workshopKnown';

function resolveRequiredItemTitle(
  title: string | undefined,
  workshopId: string,
  addons: Record<string, any>,
  knownUninstalledAddons: Record<string, any>,
): string {
  const normalizedTitle = String(title || '').trim();
  if (normalizedTitle) {
    return normalizedTitle;
  }

  const knownEntry = findKnownWorkshopEntry(addons, workshopId)
    || findKnownWorkshopEntry(knownUninstalledAddons, workshopId);
  if (knownEntry) {
    const knownTitle = String(
      knownEntry.workshopDetails?.title
      || knownEntry.steamDetails?.title
      || knownEntry.addonInfo?.addontitle
      || knownEntry.vpkName
      || '',
    ).trim();
    if (knownTitle) {
      return knownTitle;
    }
  }

  return workshopId;
}

interface RequiredItemsProps {
  requiredItems: { title: string; workshopId: string }[];
  addons: Record<string, any>;
  knownUninstalledAddons: Record<string, any>;
  onItemNavigate: (workshopId: string) => void;
}

export const RequiredItems: React.FC<RequiredItemsProps> = ({
  requiredItems,
  addons,
  knownUninstalledAddons,
  onItemNavigate,
}) => {
  const { t } = useTranslation();

  if (!requiredItems || requiredItems.length === 0) return null;

  return (
    <div style={{ marginTop: '12px' }}>
      <div
        style={{
          fontWeight: '600',
          fontSize: '14px',
          color: '#fff',
          marginBottom: '6px',
          display: 'flex',
          alignItems: 'center',
          gap: '6px',
        }}
      >
        <Package size={14} /> {t('workshop.detail.requiredItems')} ({requiredItems.length})
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: '4px', maxHeight: '120px', overflowY: 'auto' }}>
        {requiredItems.map((req) => {
          const reqIsKnown = isKnownWorkshopItem(knownUninstalledAddons, req.workshopId);
          const reqIsDownloaded = isKnownWorkshopItem(addons, req.workshopId);
          const reqTitle = resolveRequiredItemTitle(req.title, req.workshopId, addons, knownUninstalledAddons);
          return (
            <div key={req.workshopId} style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
              <a
                href="#"
                onClick={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  onItemNavigate(req.workshopId);
                }}
                style={{
                  fontSize: '12px',
                  color: 'var(--md-sys-color-primary)',
                  textDecoration: 'underline',
                  display: 'inline-flex',
                  alignItems: 'center',
                  gap: '4px',
                }}
              >
                <Link2 size={12} /> {reqTitle}
              </a>
              {reqIsDownloaded && (
                <CheckCircle size={14} className="icon-success" style={{ color: 'var(--md-sys-color-success)' }} />
              )}
              {!reqIsDownloaded && reqIsKnown && (
                <span style={{ fontSize: '11px', color: 'var(--md-sys-color-tertiary)' }}>
                  {t('workshop.badges.known')}
                </span>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
};

interface ParentCollectionsProps {
  parentCollections: { title: string; workshopId: string }[];
  onCollectionNavigate: (workshopId: string) => void;
}

export const ParentCollections: React.FC<ParentCollectionsProps> = ({
  parentCollections,
  onCollectionNavigate,
}) => {
  const { t } = useTranslation();

  if (!parentCollections || parentCollections.length === 0) return null;

  return (
    <div style={{ marginTop: '12px' }}>
      <div
        style={{
          fontWeight: '600',
          fontSize: '14px',
          color: '#fff',
          marginBottom: '6px',
          display: 'flex',
          alignItems: 'center',
          gap: '6px',
        }}
      >
        <FolderPlus size={14} /> {t('workshop.detail.parentCollections')} ({parentCollections.length})
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: '4px' }}>
        {parentCollections.map((col) => (
          <a
            key={col.workshopId}
            href="#"
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              onCollectionNavigate(col.workshopId);
            }}
            style={{
              fontSize: '12px',
              color: 'var(--md-sys-color-primary)',
              textDecoration: 'underline',
              display: 'inline-flex',
              alignItems: 'center',
              gap: '4px',
            }}
          >
            <FolderPlus size={12} /> {col.title}
          </a>
        ))}
      </div>
    </div>
  );
};
