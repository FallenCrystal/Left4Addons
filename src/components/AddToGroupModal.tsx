import React, { useState, useEffect, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Group } from '../types/addon';
import { FolderPlus } from 'lucide-react';

interface AddToGroupModalProps {
  open: boolean;
  groups: Group[];
  onCancel: () => void;
  onConfirm: (groupId: string) => void;
  isSubmitting?: boolean;
}

export const AddToGroupModal: React.FC<AddToGroupModalProps> = ({
  open,
  groups,
  onCancel,
  onConfirm,
  isSubmitting = false,
}) => {
  const { t } = useTranslation();
  const [search, setSearch] = useState('');
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setSearch('');
      setSelectedGroupId(null);
    }
  }, [open]);

  const filteredGroups = useMemo(() => {
    if (!search.trim()) return groups;
    const q = search.toLowerCase();
    return groups.filter(g => g.name.toLowerCase().includes(q));
  }, [groups, search]);

  if (!open) return null;

  const handleSubmit = () => {
    if (!selectedGroupId || isSubmitting) return;
    onConfirm(selectedGroupId);
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div
        className="modal-content"
        style={{ width: '420px' }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="modal-title">{t('batchActionBar.addToGroupTitle')}</h2>
        <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '16px' }}>
          {t('batchActionBar.addToGroupDesc')}
        </p>

        <div className="form-group">
          <input
            type="text"
            className="form-input"
            placeholder={t('batchActionBar.searchGroups')}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            disabled={isSubmitting}
            autoFocus
            style={{ marginBottom: '8px' }}
          />
          <div style={{
            maxHeight: '240px',
            overflowY: 'auto',
            backgroundColor: 'var(--md-sys-surface-container)',
            border: '1px solid var(--md-sys-color-outline-variant)',
            borderRadius: '12px',
            padding: '4px'
          }}>
            {filteredGroups.length === 0 ? (
              <div style={{ padding: '16px', textAlign: 'center', color: 'var(--md-sys-color-outline)', fontSize: '13px' }}>
                {t('batchActionBar.noGroups')}
              </div>
            ) : (
              filteredGroups.map(group => {
                const isSelected = selectedGroupId === group.id;
                return (
                  <button
                    key={group.id}
                    type="button"
                    onClick={() => setSelectedGroupId(group.id)}
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: '10px',
                      padding: '10px 12px',
                      cursor: isSubmitting ? 'not-allowed' : 'pointer',
                      borderRadius: '8px',
                      backgroundColor: isSelected ? 'rgba(179, 197, 255, 0.12)' : 'transparent',
                      border: isSelected ? '1px solid var(--md-sys-color-primary)' : '1px solid transparent',
                      width: '100%',
                      textAlign: 'left',
                      color: 'inherit',
                      fontSize: '13px',
                    }}
                    disabled={isSubmitting}
                  >
                    <FolderPlus size={16} style={{ color: isSelected ? 'var(--md-sys-color-primary)' : 'var(--md-sys-color-outline)', flexShrink: 0 }} />
                    <span style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                      {group.name}
                    </span>
                    <span style={{ fontSize: '11px', color: 'var(--md-sys-color-outline)' }}>
                      {group.addons?.length || 0}
                    </span>
                  </button>
                );
              })
            )}
          </div>
        </div>

        <div className="modal-actions">
          <button
            type="button"
            className="btn btn-secondary"
            onClick={onCancel}
            disabled={isSubmitting}
          >
            {t('common.cancel')}
          </button>
          <button
            type="button"
            className="btn btn-primary"
            onClick={handleSubmit}
            disabled={isSubmitting || !selectedGroupId}
          >
            {isSubmitting ? t('common.creating') : t('common.confirm')}
          </button>
        </div>
      </div>
    </div>
  );
};
