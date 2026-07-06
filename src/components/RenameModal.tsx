import React, { useState, useEffect } from 'react';
import { Addon, Group } from '../types/addon';
import { getSuggestedVpkName } from '../utils/addonHelpers';
import { useTranslation } from 'react-i18next';

interface RenameModalProps {
  open: boolean;
  currentName: string;
  title: string;
  suggestedName: string;
  addon: Addon | undefined;
  itemGroup: Group | undefined;
  addons: Record<string, Addon>;
  onCancel: () => void;
  onConfirm: (currentName: string, newVpkName: string) => void;
  isSubmitting?: boolean;
}

export const RenameModal: React.FC<RenameModalProps> = ({
  open,
  currentName,
  title,
  suggestedName,
  addon,
  itemGroup,
  addons,
  onCancel,
  onConfirm,
  isSubmitting = false,
}) => {
  const { t } = useTranslation();
  const [vpkName, setVpkName] = useState('');

  useEffect(() => {
    if (open) {
      setVpkName(suggestedName);
    }
  }, [open, suggestedName]);

  if (!open) return null;

  const handleApplyTitle = () => {
    if (addon) {
      const name = getSuggestedVpkName(addon, itemGroup?.name, addons);
      setVpkName(name);
    }
  };


  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    onConfirm(currentName, vpkName);
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <form className="modal-content" onSubmit={handleSubmit} onClick={(e) => e.stopPropagation()}>
        <h2 className="modal-title">{t('renameModal.title')}</h2>
        <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '16px' }}>
          {t('renameModal.desc')}
        </p>
        
        {title && (
          <div style={{ 
            backgroundColor: 'var(--md-sys-surface-container)',
            border: '1px solid var(--md-sys-color-outline-variant)',
            padding: '12px 16px',
            borderRadius: '12px',
            marginBottom: '16px',
            fontSize: '12px'
          }}>
            <div style={{ fontWeight: '700', color: 'var(--md-sys-color-primary)' }}>{t('renameModal.workshopTitle')}</div>
            <div style={{ color: '#fff', marginTop: '4px' }}>{title}</div>
            <button 
              type="button" 
              className="btn btn-text" 
              style={{ padding: '4px 0', fontSize: '11px', marginTop: '4px' }}
              onClick={handleApplyTitle}
              disabled={isSubmitting}
            >
              {t('renameModal.applyWorkshopTitle')}
            </button>
          </div>
        )}

        <div className="form-group">
          <label className="form-label">{t('renameModal.currentNameLabel')}</label>
          <div style={{ fontStyle: 'italic', fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
            {currentName}
          </div>
        </div>

        <div className="form-group">
          <label className="form-label">{t('renameModal.newNameLabel')}</label>
          <input 
            type="text" 
            className="form-input" 
            value={vpkName}
            onChange={(e) => setVpkName(e.target.value)}
            required
            disabled={isSubmitting}
          />
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
          <button type="submit" className="btn btn-primary" disabled={isSubmitting || !vpkName.trim()}>
            {isSubmitting ? t('common.renaming') : t('common.rename')}
          </button>
        </div>
      </form>
    </div>
  );
};
