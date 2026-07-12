/** Tag category sidebar for filtering workshop items */

import React from 'react';
import { Tag } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { TagCategory } from './types';

interface TagSidebarProps {
  categories: TagCategory[];
  activeTag: string | null;
  onTagClick: (tagId: string, tagName: string) => void;
}

export const TagSidebar: React.FC<TagSidebarProps> = ({
  categories,
  activeTag,
  onTagClick,
}) => {
  const { t } = useTranslation();

  if (categories.length === 0) return null;

  return (
    <div
      style={{
        width: '220px',
        flexShrink: 0,
        overflowY: 'auto',
        borderLeft: '1px solid var(--md-sys-color-outline-variant)',
        paddingLeft: '16px',
      }}
    >
      <h4
        style={{
          margin: '0 0 12px',
          fontSize: '14px',
          fontWeight: 600,
          color: 'var(--md-sys-color-on-surface)',
          display: 'flex',
          alignItems: 'center',
          gap: '6px',
        }}
      >
        <Tag size={14} /> {t('workshop.tags.browse')}
      </h4>
      {categories.map((cat) => (
        <div key={cat.name} style={{ marginBottom: '16px' }}>
          <div
            style={{
              fontSize: '12px',
              fontWeight: 600,
              color: 'var(--md-sys-color-outline)',
              marginBottom: '6px',
              textTransform: 'uppercase',
              letterSpacing: '0.5px',
            }}
          >
            {cat.name}
          </div>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: '4px' }}>
            {cat.tags.map((tag) => (
              <button
                key={tag.id}
                onClick={() => onTagClick(tag.id, tag.name)}
                style={{
                  padding: '3px 10px',
                  borderRadius: '100px',
                  border: '1px solid var(--md-sys-color-outline-variant)',
                  background:
                    activeTag === tag.id
                      ? 'var(--md-sys-color-primary-container)'
                      : 'transparent',
                  color:
                    activeTag === tag.id
                      ? 'var(--md-sys-color-on-primary-container)'
                      : 'var(--md-sys-color-on-surface-variant)',
                  fontSize: '11px',
                  cursor: 'pointer',
                  transition: 'all 0.15s',
                  whiteSpace: 'nowrap',
                }}
              >
                {tag.display_name}
              </button>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
};
