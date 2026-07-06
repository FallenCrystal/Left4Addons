import React, { useState, useEffect } from 'react';
import { useTranslation } from 'react-i18next';

interface SettingsModalProps {
  open: boolean;
  initialLoadingDir: string;
  onCancel: () => void;
  onConfirm: (loadingDir: string) => void;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({
  open,
  initialLoadingDir,
  onCancel,
  onConfirm,
}) => {
  const { t } = useTranslation();
  const [loadingDir, setLoadingDir] = useState('');

  useEffect(() => {
    if (open) {
      setLoadingDir(initialLoadingDir);
    }
  }, [open, initialLoadingDir]);

  if (!open) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!loadingDir.trim()) return;
    onConfirm(loadingDir.trim());
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <form 
        className="modal-content" 
        onSubmit={handleSubmit} 
        style={{ width: '560px' }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="modal-title">{t('settingsModal.title')}</h2>
        <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '16px' }}>
          {t('settings.desc')}
        </p>

        <div className="form-group">
          <label className="form-label">{t('settings.addonsPathLabel')}</label>
          <input 
            type="text" 
            className="form-input" 
            value={loadingDir}
            onChange={(e) => setLoadingDir(e.target.value)}
            placeholder={t('settings.addonsPathPlaceholder')}
            required
          />
          <span style={{ fontSize: '11px', color: 'var(--md-sys-color-outline)' }}>
            {t('settings.addonsPathHelp')}
          </span>
        </div>

        <div className="modal-actions">
          <button 
            type="button" 
            className="btn btn-secondary" 
            onClick={onCancel}
          >
            {t('common.cancel')}
          </button>
          <button type="submit" className="btn btn-primary" disabled={!loadingDir.trim()}>
            {t('settings.saveAndRescan')}
          </button>
        </div>
      </form>
    </div>
  );
};
