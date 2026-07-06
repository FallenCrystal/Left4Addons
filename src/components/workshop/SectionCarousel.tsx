/** Homepage section carousel component */

import React from 'react';
import { ChevronRight, Clock, Flame, Package, Users, RefreshCw, Star } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { HomepageSection, WorkshopItem } from './types';
import { ItemCard } from './ItemCard';

const ICON_MAP: Record<string, React.ReactNode> = {
  Clock: <Clock size={20} />,
  Flame: <Flame size={20} />,
  Package: <Package size={20} />,
  Users: <Users size={20} />,
  RefreshCw: <RefreshCw size={20} />,
  Star: <Star size={20} />,
};

interface SectionCarouselProps {
  section: HomepageSection;
  sectionType: string;
  addons: Record<string, any>;
  knownUninstalledAddons: Record<string, any>;
  onItemClick: (item: WorkshopItem) => void;
  onViewAll: (section: HomepageSection) => void;
  loadingDetailId?: string | null;
}

export const SectionCarousel: React.FC<SectionCarouselProps> = ({
  section,
  sectionType,
  addons,
  knownUninstalledAddons,
  onItemClick,
  onViewAll,
  loadingDetailId,
}) => {
  const { t } = useTranslation();

  if (section.items.length === 0) return null;

  return (
    <div style={{ marginBottom: '32px' }}>
      <div
        style={{
          display: 'flex',
          marginBottom: '16px',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
          <div style={{ color: 'var(--md-sys-color-primary)', display: 'flex' }}>
            {ICON_MAP[section.icon as string] || <Star size={20} />}
          </div>
          <div>
            <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
              <h3
                style={{
                  margin: 0,
                  fontSize: '18px',
                  fontWeight: 600,
                  color: 'var(--md-sys-color-on-surface)',
                }}
              >
                {t(section.title)}
              </h3>
              <button
                className="btn btn-outline"
                onClick={() => onViewAll(section)}
                style={{
                  borderRadius: '100px',
                  padding: '4px 16px',
                  fontSize: '14px',
                  display: 'flex',
                  alignItems: 'center',
                  gap: '4px',
                  minHeight: '32px',
                  height: '32px',
                }}
              >
                {t('workshop.home.viewAll')} <ChevronRight size={16} />
              </button>
            </div>
            <p
              style={{
                margin: '4px 0 0',
                fontSize: '13px',
                color: 'var(--md-sys-color-outline)',
              }}
            >
              {t(section.subtitle)}
            </p>
          </div>
        </div>
      </div>
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fill, minmax(200px, 1fr))',
          gap: '16px',
          paddingLeft: '32px',
        }}
      >
        {section.items.slice(0, 6).map((item) => (
          <ItemCard
            key={item.workshopId}
            item={item}
            section={sectionType}
            addons={addons}
            knownUninstalledAddons={knownUninstalledAddons}
            onClick={() => onItemClick(item)}
            isLoading={loadingDetailId === item.workshopId}
          />
        ))}
      </div>
    </div>
  );
};
