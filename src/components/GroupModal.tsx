import React, { useState, useEffect } from 'react';
import { Addon } from '../types/addon';
import { useTranslation } from 'react-i18next';

interface GroupModalProps {
  open: boolean;
  addons: Record<string, Addon>;
  onCancel: () => void;
  onConfirm: (name: string, selectedAddons: string[]) => void;
  isSubmitting?: boolean;
}

export const GroupModal: React.FC<GroupModalProps> = ({
  open,
  addons,
  onCancel,
  onConfirm,
  isSubmitting = false,
}) => {
  const { t } = useTranslation();
  const [name, setName] = useState('');
  const [selectedAddons, setSelectedAddons] = useState<string[]>([]);

  useEffect(() => {
    if (open) {
      setName('');
      setSelectedAddons([]);
    }
  }, [open]);

  if (!open) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!name || selectedAddons.length === 0 || isSubmitting) return;
    onConfirm(name, selectedAddons);
  };

  const handleCheckboxChange = (vpkName: string, checked: boolean) => {
    if (checked) {
      setSelectedAddons(prev => [...prev, vpkName]);
    } else {
      setSelectedAddons(prev => prev.filter(item => item !== vpkName));
    }
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <form 
        className="modal-content" 
        onSubmit={handleSubmit} 
        style={{ width: '560px' }} 
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="modal-title">{t('groupModal.title')}</h2>
        <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '16px' }}>
          {t('groupModal.desc')}
        </p>

        <div className="form-group">
          <label className="form-label">{t('groupModal.groupNameLabel')}</label>
          <input 
            type="text" 
            className="form-input" 
            placeholder={t('groupModal.groupNamePlaceholder')}
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            disabled={isSubmitting}
          />
        </div>

        <div className="form-group">
          <label className="form-label">{t('groupModal.selectAddonsLabel')}</label>
          <div style={{ 
            maxHeight: '200px', 
            overflowY: 'auto', 
            backgroundColor: 'var(--md-sys-surface-container)',
            border: '1px solid var(--md-sys-color-outline-variant)',
            borderRadius: '12px',
            padding: '8px'
          }}>
            {Object.values(addons).map(addon => {
              const title = addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName;
              const isChecked = selectedAddons.includes(addon.vpkName);
              return (
                <label 
                  key={addon.vpkName} 
                  style={{ 
                    display: 'flex', 
                    alignItems: 'center', 
                    gap: '8px', 
                    padding: '8px',
                    cursor: isSubmitting ? 'not-allowed' : 'pointer',
                    borderRadius: '8px',
                    backgroundColor: isChecked ? 'rgba(179, 197, 255, 0.08)' : 'transparent'
                  }}
                >
                  <input 
                     type="checkbox" 
                     checked={isChecked}
                     onChange={(e) => handleCheckboxChange(addon.vpkName, e.target.checked)}
                     disabled={isSubmitting}
                  />
                  <span style={{ fontSize: '13px', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                    {title}
                  </span>
                </label>
              );
            })}
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
          <button type="submit" className="btn btn-primary" disabled={isSubmitting || !name || selectedAddons.length === 0}>
            {isSubmitting ? t('common.creating') : t('groupModal.createGroupBtn')}
          </button>
        </div>
      </form>
    </div>
  );
};
