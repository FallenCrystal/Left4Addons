/** Tag browser modal — replaces the sidebar tag panel */

import React from 'react';
import { X, Tag } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { TagCategory } from './types';

interface TagBrowserModalProps {
  open: boolean;
  categories: TagCategory[];
  activeTag: string | null;
  onClose: () => void;
  onTagClick: (tagId: string, tagName: string) => void;
}

export const TagBrowserModal: React.FC<TagBrowserModalProps> = ({
  open,
  categories,
  activeTag,
  onClose,
  onTagClick,
}) => {
  const { t } = useTranslation();
  if (!open || categories.length === 0) return null;

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div
        className="modal-content"
        style={{ maxWidth: '640px', maxHeight: '80vh', overflowY: 'auto' }}
        onClick={(e) => e.stopPropagation()}
      >
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '20px' }}>
          <h2 className="modal-title" style={{ margin: 0, display: 'flex', alignItems: 'center', gap: '8px' }}>
            <Tag size={18} /> {t('workshop.tags.browse')}
          </h2>
          <button className="btn btn-secondary btn-icon-only" onClick={onClose}>
            <X size={16} />
          </button>
        </div>

        {categories.map((cat) => (
          <div key={cat.name} style={{ marginBottom: '20px' }}>
            <div style={{
              fontSize: '13px', fontWeight: 600, color: 'var(--md-sys-color-outline)',
              marginBottom: '8px', textTransform: 'uppercase', letterSpacing: '0.5px',
            }}>
              {cat.name}
            </div>
            <div style={{ display: 'flex', flexWrap: 'wrap', gap: '6px' }}>
              {cat.tags.map((tag) => (
                <button
                  key={tag.id}
                  onClick={() => { onTagClick(tag.id, tag.name); onClose(); }}
                  style={{
                    padding: '6px 16px',
                    borderRadius: '100px',
                    border: '1px solid var(--md-sys-color-outline-variant)',
                    background: activeTag === tag.id
                      ? 'var(--md-sys-color-primary-container)'
                      : 'var(--md-sys-color-surface-container)',
                    color: activeTag === tag.id
                      ? 'var(--md-sys-color-on-primary-container)'
                      : 'var(--md-sys-color-on-surface-variant)',
                    fontSize: '13px',
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
    </div>
  );
};
