import React from 'react';
import { AlertCircle } from 'lucide-react';
import { useTranslation } from 'react-i18next';

interface AlertModalProps {
  open: boolean;
  title: string;
  message: string;
  onClose: () => void;
}

export const AlertModal: React.FC<AlertModalProps> = ({
  open,
  title,
  message,
  onClose,
}) => {
  const { t } = useTranslation();

  if (!open) return null;

  return (
    <div className="modal-overlay" onClick={onClose} style={{ zIndex: 1100 }}>
      <div 
        className="modal-content" 
        style={{ width: '400px', borderRadius: '24px', padding: '24px' }} 
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="modal-title" style={{ display: 'flex', alignItems: 'center', gap: '8px', color: '#ffb300' }}>
          <AlertCircle size={24} />
          <span>{title}</span>
        </h2>
        
        <div style={{ whiteSpace: 'pre-wrap', marginTop: '16px', marginBottom: '24px', color: '#e2e2e9', fontSize: '14px', lineHeight: '1.6' }}>
          {message}
        </div>

        <div className="modal-actions" style={{ marginTop: 0 }}>
          <button 
            type="button" 
            className="btn btn-primary"
            onClick={onClose}
            style={{ borderRadius: '12px' }}
          >
            {t('common.confirm')}
          </button>
        </div>
      </div>
    </div>
  );
};
