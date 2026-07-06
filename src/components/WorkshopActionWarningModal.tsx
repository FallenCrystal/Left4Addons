import React, { useState } from 'react';
import { AlertTriangle, ExternalLink, CheckCircle2 } from 'lucide-react';
import { Addon } from '../types/addon';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import { TransHTML } from '../i18n';

interface WorkshopActionWarningModalProps {
  open: boolean;
  actionName: string;
  addons: Addon[];
  onCancel: () => void;
  onConfirm: (skipWarning: boolean) => void;
}

export const WorkshopActionWarningModal: React.FC<WorkshopActionWarningModalProps> = ({
  open,
  actionName,
  addons,
  onCancel,
  onConfirm,
}) => {
  const { t } = useTranslation();
  const [skipWarning, setSkipWarning] = useState(false);

  if (!open) return null;

  const handleOpenWorkshop = async () => {
    const workshopId = addons.find(ad => ad.workshopId)?.workshopId;
    if (workshopId) {
      await invoke('open_url', { url: `steam://url/CommunityFilePage/${workshopId}` });
    }
    onConfirm(skipWarning);
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-content" style={{ width: '500px' }} onClick={(e) => e.stopPropagation()}>
        <h2 className="modal-title" style={{ display: 'flex', alignItems: 'center', gap: '8px', color: '#8be19a' }}>
          <AlertTriangle size={24} />
          <span>{t('workshopWarningModal.title')}</span>
        </h2>
        
        <div className="warning-box" style={{ marginTop: '16px', marginBottom: '20px', backgroundColor: 'rgba(139, 225, 154, 0.1)', borderColor: 'rgba(139, 225, 154, 0.3)', color: '#8be19a' }}>
          <div>
            <div className="warning-title" style={{ color: '#8be19a', display: 'flex', alignItems: 'center', gap: '6px' }}>
              <CheckCircle2 size={18} />
              <span>{t('workshopWarningModal.successTitle')}</span>
            </div>
            <div style={{ color: '#e2e2e9', marginTop: '8px' }}>
              <TransHTML i18nKey="workshopWarningModal.warningDesc" values={{ actionName }} />
            </div>
          </div>
        </div>

        <div style={{ marginBottom: '20px', display: 'flex', alignItems: 'center', gap: '8px' }}>
          <label className="switch" title={t('workshopWarningModal.dontShowAgain')} style={{ transform: 'scale(0.8)', transformOrigin: 'left center' }}>
            <input 
              type="checkbox" 
              checked={skipWarning} 
              onChange={(e) => setSkipWarning(e.target.checked)}
            />
            <span className="slider"></span>
          </label>
          <span style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>{t('workshopWarningModal.dontShowAgain')}</span>
        </div>

        <div className="modal-actions">
          <button 
            type="button" 
            className="btn btn-secondary" 
            onClick={() => onConfirm(skipWarning)}
          >
            {t('workshopWarningModal.iUnderstand')}
          </button>
          
          <button 
            type="button" 
            className="btn btn-primary"
            onClick={handleOpenWorkshop}
            disabled={!addons.some(ad => ad.workshopId)}
          >
            <ExternalLink size={14} />
            <span>{t('workshopWarningModal.goToWorkshop')}</span>
          </button>
        </div>
      </div>
    </div>
  );
};
