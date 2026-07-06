/** Homepage section carousel component */

import React from 'react';
import { ChevronRight, Clock, Flame, Package, Users, RefreshCw, Star } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { HomepageSection, WorkshopItem } from './types';
import { ItemCard } from './ItemCard';

const ICON_MAP: Record<string, React.ReactNode> = {
  Clock: <Clock size={16} />,
  Flame: <Flame size={16} />,
  Package: <Package size={16} />,
  Users: <Users size={16} />,
  RefreshCw: <RefreshCw size={16} />,
  Star: <Star size={16} />,
};

interface SectionCarouselProps {
  section: HomepageSection;
  sectionType: string;
  addons: Record<string, any>;
  knownUninstalledAddons: Record<string, any>;
  onItemClick: (item: WorkshopItem) => void;
  onViewAll: (section: HomepageSection) => void;
}

export const SectionCarousel: React.FC<SectionCarouselProps> = ({
  section,
  sectionType,
  addons,
  knownUninstalledAddons,
  onItemClick,
  onViewAll,
}) => {
  const { t } = useTranslation();

  if (section.items.length === 0) return null;

  return (
    <div style={{ marginBottom: '32px' }}>
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
          marginBottom: '12px',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
          <div style={{ color: 'var(--md-sys-color-primary)', display: 'flex' }}>
            {ICON_MAP[section.icon as string] || <Star size={16} />}
          </div>
          <div>
            <h3
              style={{
                margin: 0,
                fontSize: '16px',
                fontWeight: 600,
                color: 'var(--md-sys-color-on-surface)',
              }}
            >
              {t(section.title)}
            </h3>
            <p
              style={{
                margin: '2px 0 0',
                fontSize: '12px',
                color: 'var(--md-sys-color-outline)',
              }}
            >
              {t(section.subtitle)}
            </p>
          </div>
        </div>
        <button
          className="btn btn-outline"
          onClick={() => onViewAll(section)}
          style={{
            borderRadius: '100px',
            padding: '6px 16px',
            fontSize: '12px',
            display: 'flex',
            alignItems: 'center',
            gap: '4px',
          }}
        >
          {t('workshop.home.viewAll')} <ChevronRight size={14} />
        </button>
      </div>
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fill, minmax(200px, 1fr))',
          gap: '16px',
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
          />
        ))}
      </div>
    </div>
  );
};
