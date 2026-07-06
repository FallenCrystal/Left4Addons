/** Workshop item card component */

import React from 'react';
import { Star, Loader2, Package } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { WorkshopItem } from './types';

interface ItemCardProps {
  item: WorkshopItem;
  section: string;
  addons: Record<string, any>;
  knownUninstalledAddons: Record<string, any>;
  onClick: () => void;
  isLoading?: boolean;
}

export const ItemCard: React.FC<ItemCardProps> = ({
  item,
  section,
  addons,
  knownUninstalledAddons,
  onClick,
  isLoading,
}) => {
  const { t } = useTranslation();
  const isDownloaded = addons[item.workshopId + '.vpk'] !== undefined;
  const isKnown = knownUninstalledAddons[item.workshopId + '.vpk'] !== undefined;

  return (
    <div className="addon-card" style={{ cursor: isLoading ? 'wait' : 'pointer' }} onClick={isLoading ? undefined : onClick}>
      {isLoading && (
        <div style={{ position: 'absolute', top: 0, left: 0, right: 0, bottom: 0, backgroundColor: 'rgba(0,0,0,0.5)', zIndex: 10, display: 'flex', alignItems: 'center', justifyContent: 'center', borderRadius: '28px', backdropFilter: 'blur(2px)' }}>
          <Loader2 size={32} color="var(--md-sys-color-primary)" className="animate-spin" />
        </div>
      )}
      <div className="addon-card-clickable-area">
        <div className="addon-card-image-wrapper">
          <img className="addon-card-image" src={item.imagePath} alt={item.title} />
        </div>
        <div className="addon-card-badges">
          {isDownloaded && (
            <span className="badge badge-enabled">{t('workshop.badges.downloaded')}</span>
          )}
          {!isDownloaded && isKnown && (
            <span
              className="badge badge-disabled"
              style={{
                backgroundColor: 'var(--md-sys-color-secondary)',
                color: 'var(--md-sys-color-on-secondary)',
              }}
            >
              {t('workshop.badges.known')}
            </span>
          )}
          <span
            className="badge badge-dir"
            style={{
              background: 'var(--md-sys-color-primary)',
              color: 'var(--md-sys-color-on-primary)',
            }}
          >
            {section === 'collections' ? t('workshop.badges.collection') : t('workshop.badges.addon')}
          </span>
        </div>
        <div className="addon-card-info" style={{ gap: '6px' }}>
          <h3 className="addon-card-title" title={item.title}>
            {item.title}
          </h3>
          <div className="addon-card-author">
            {t('workshop.item.author', { author: item.authorName })}
          </div>
          {item.shortDescription && (
            <p
              className="addon-card-desc"
              style={{
                fontSize: '12px',
                height: '50px',
                display: '-webkit-box',
                WebkitLineClamp: 3,
                WebkitBoxOrient: 'vertical',
                overflow: 'hidden',
              }}
              title={item.shortDescription}
            >
              {item.shortDescription}
            </p>
          )}
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              alignItems: 'center',
              fontSize: '11px',
              color: 'var(--md-sys-color-outline)',
              marginTop: 'auto',
              paddingTop: '8px',
            }}
          >
            {item.fileSize && <span>{t('workshop.item.size', { size: item.fileSize })}</span>}
            {item.childCount !== undefined && item.childCount > 0 && (
              <span style={{ display: 'flex', alignItems: 'center', gap: '3px' }}>
                <Package size={11} /> {t('workshop.item.childCount', { count: item.childCount })}
              </span>
            )}
            {item.stars > 0 && (
              <div style={{ display: 'flex', gap: '2px', alignItems: 'center' }}>
                <Star size={12} fill="#ffb300" color="#ffb300" />
                <span style={{ fontWeight: 'bold' }}>{item.stars}</span>
              </div>
            )}
            {item.subscriptions !== undefined && item.subscriptions > 0 && (
              <span>{t('workshop.item.subscriptions', { count: item.subscriptions.toLocaleString() })}</span>
            )}
          </div>
          {item.tags && item.tags.length > 0 && (
            <div className="addon-card-tags" style={{ paddingTop: '4px' }}>
              {item.tags.slice(0, 3).map((t) => (
                <span key={t} className="tag-chip">
                  {t}
                </span>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
