import React, { useState, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { Search, RefreshCw } from 'lucide-react';

interface SettingsModalProps {
  open: boolean;
  initialLoadingDir: string;
  onCancel: () => void;
  onConfirm: (loadingDir: string) => void;
  onShowToast?: (message: string, type?: 'success' | 'error') => void;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({
  open,
  initialLoadingDir,
  onCancel,
  onConfirm,
  onShowToast,
}) => {
  const { t } = useTranslation();
  const [loadingDir, setLoadingDir] = useState('');
  const [isDetecting, setIsDetecting] = useState(false);
  const [detectMessage, setDetectMessage] = useState<{ text: string; isError: boolean } | null>(null);

  useEffect(() => {
    if (open) {
      setLoadingDir(initialLoadingDir);
      setDetectMessage(null);
    }
  }, [open, initialLoadingDir]);

  if (!open) return null;

  const handleAutoDetect = async () => {
    setIsDetecting(true);
    setDetectMessage(null);
    try {
      const detectedPath = await invoke<string | null>('auto_detect_addons_path');
      if (detectedPath) {
        setLoadingDir(detectedPath);
        const msg = t('settings.autoDetectSuccess');
        setDetectMessage({ text: msg, isError: false });
        onShowToast?.(msg, 'success');
      } else {
        const msg = t('settings.autoDetectFailed');
        setDetectMessage({ text: msg, isError: true });
        onShowToast?.(msg, 'error');
      }
    } catch (err) {
      const msg = t('settings.autoDetectFailed');
      setDetectMessage({ text: msg, isError: true });
      onShowToast?.(msg, 'error');
    } finally {
      setIsDetecting(false);
    }
  };

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
          <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
            <input 
              type="text" 
              className="form-input" 
              value={loadingDir}
              onChange={(e) => setLoadingDir(e.target.value)}
              placeholder={t('settings.addonsPathPlaceholder')}
              style={{ flex: 1 }}
              required
              disabled={isDetecting}
            />
            <button
              type="button"
              className="btn btn-secondary"
              onClick={handleAutoDetect}
              disabled={isDetecting}
              style={{
                whiteSpace: 'nowrap',
                display: 'inline-flex',
                alignItems: 'center',
                gap: '6px',
                height: '38px',
                padding: '0 16px',
              }}
            >
              {isDetecting ? (
                <>
                  <RefreshCw className="animate-spin" size={16} />
                  <span>{t('settings.autoDetecting')}</span>
                </>
              ) : (
                <>
                  <Search size={16} />
                  <span>{t('settings.autoDetect')}</span>
                </>
              )}
            </button>
          </div>
          {detectMessage && (
            <span style={{ fontSize: '12px', color: detectMessage.isError ? 'var(--md-sys-color-error)' : 'var(--md-sys-color-primary)', display: 'block', marginTop: '4px', fontWeight: '500' }}>
              {detectMessage.text}
            </span>
          )}
          <span style={{ fontSize: '11px', color: 'var(--md-sys-color-outline)', display: 'block', marginTop: '4px' }}>
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
