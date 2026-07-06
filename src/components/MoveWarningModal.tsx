import React from 'react';
import { AlertTriangle, ExternalLink } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { TransHTML } from '../i18n';

interface MoveWarningModalProps {
  open: boolean;
  vpkName: string;
  currentDirType: string;
  workshopId: string;
  onCancel: () => void;
  onConfirm: (vpkName: string, currentDirType: string, unsubscribe: boolean) => void;
}

export const MoveWarningModal: React.FC<MoveWarningModalProps> = ({
  open,
  vpkName,
  currentDirType,
  workshopId,
  onCancel,
  onConfirm,
}) => {
  const { t } = useTranslation();

  if (!open) return null;

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-content" style={{ width: '500px' }} onClick={(e) => e.stopPropagation()}>
        <h2 className="modal-title" style={{ display: 'flex', alignItems: 'center', gap: '8px', color: '#ffb300' }}>
          <AlertTriangle size={24} />
          <span>{t('moveWarningModal.title')}</span>
        </h2>
        
        <div className="warning-box" style={{ marginTop: '16px', marginBottom: '20px' }}>
          <div>
            <div className="warning-title" style={{ display: 'flex', alignItems: 'center', gap: '6px' }}><AlertTriangle size={18} /><span>{t('moveWarningModal.veryImportant')}</span></div>
            <div style={{ color: '#e2e2e9' }}>
              <TransHTML i18nKey="moveWarningModal.warningDesc" />
            </div>
          </div>
        </div>

        <div className="modal-actions">
          <button 
            type="button" 
            className="btn btn-secondary" 
            onClick={onCancel}
          >
            {t('common.cancel')}
          </button>
          
          {workshopId && (
            <button
              type="button"
              className="btn btn-tertiary"
              onClick={() => onConfirm(vpkName, currentDirType, true)}
              style={{ display: 'inline-flex', alignItems: 'center', gap: '4px' }}
            >
              <ExternalLink size={14} />
              <span>{t('moveWarningModal.unsubscribeAndMove')}</span>
            </button>
          )}

          <button 
            type="button" 
            className="btn btn-primary"
            onClick={() => onConfirm(vpkName, currentDirType, false)}
          >
            {t('moveWarningModal.moveDirectly')}
          </button>
        </div>
      </div>
    </div>
  );
};
