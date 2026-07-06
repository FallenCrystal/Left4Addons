import React from 'react';
import { AlertTriangle } from 'lucide-react';
import { useTranslation } from 'react-i18next';

interface ConfirmModalProps {
  open: boolean;
  title: string;
  message: string;
  onConfirm: () => void;
  onCancel: () => void;
  isSubmitting?: boolean;
}

export const ConfirmModal: React.FC<ConfirmModalProps> = ({
  open,
  title,
  message,
  onConfirm,
  onCancel,
  isSubmitting = false,
}) => {
  const { t } = useTranslation();

  if (!open) return null;

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-content" style={{ width: '450px' }} onClick={(e) => e.stopPropagation()}>
        <h2 className="modal-title" style={{ display: 'flex', alignItems: 'center', gap: '8px', color: '#ffb300' }}>
          <AlertTriangle size={24} />
          <span>{title}</span>
        </h2>
        
        <div style={{ whiteSpace: 'pre-wrap', marginTop: '16px', marginBottom: '20px', color: '#e2e2e9', fontSize: '14px', lineHeight: '1.6' }}>
          {message}
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
            disabled={isSubmitting}
            onClick={() => {
              onConfirm();
              onCancel();
            }}
          >
            {t('common.confirm')}
          </button>
        </div>
      </div>
    </div>
  );
};
