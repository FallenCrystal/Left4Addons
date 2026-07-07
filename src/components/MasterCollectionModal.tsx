import React, { useState, useEffect } from 'react';
import { useTranslation } from 'react-i18next';

interface MasterCollectionModalProps {
  open: boolean;
  onCancel: () => void;
  onConfirm: (name: string) => void;
  isSubmitting?: boolean;
}

export const MasterCollectionModal: React.FC<MasterCollectionModalProps> = ({
  open,
  onCancel,
  onConfirm,
  isSubmitting = false,
}) => {
  const { t } = useTranslation();
  const [name, setName] = useState('');

  useEffect(() => {
    if (open) {
      setName('');
    }
  }, [open]);

  if (!open) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim() || isSubmitting) return;
    onConfirm(name.trim());
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <form
        className="modal-content"
        onSubmit={handleSubmit}
        style={{ width: '420px' }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="modal-title">{t('masterCollections.createTitle')}</h2>
        <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '16px' }}>
          {t('masterCollections.createDesc')}
        </p>

        <div className="form-group">
          <label className="form-label">{t('masterCollections.collectionNameLabel')}</label>
          <input
            type="text"
            className="form-input"
            placeholder={t('masterCollections.collectionNamePlaceholder')}
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            disabled={isSubmitting}
            autoFocus
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
          <button type="submit" className="btn btn-primary" disabled={isSubmitting || !name.trim()}>
            {isSubmitting ? t('common.creating') : t('masterCollections.createCollectionBtn')}
          </button>
        </div>
      </form>
    </div>
  );
};
