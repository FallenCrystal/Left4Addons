import React from 'react';
import { AlertTriangle } from 'lucide-react';

interface ConfirmModalProps {
  open: boolean;
  title: string;
  message: string;
  onConfirm: () => void;
  onCancel: () => void;
}

export const ConfirmModal: React.FC<ConfirmModalProps> = ({
  open,
  title,
  message,
  onConfirm,
  onCancel,
}) => {
  if (!open) return null;

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-content" style={{ width: '450px' }} onClick={(e) => e.stopPropagation()}>
        <h2 className="modal-title" style={{ display: 'flex', alignItems: 'center', gap: '8px', color: '#ffb300' }}>
          <AlertTriangle size={24} />
          <span>{title}</span>
        </h2>
        
        <div style={{ marginTop: '16px', marginBottom: '20px', color: '#e2e2e9', fontSize: '14px', lineHeight: '1.6' }}>
          {message}
        </div>

        <div className="modal-actions">
          <button 
            type="button" 
            className="btn btn-secondary" 
            onClick={onCancel}
          >
            取消
          </button>
          
          <button 
            type="button" 
            className="btn btn-primary"
            onClick={() => {
              onConfirm();
              onCancel();
            }}
          >
            确定
          </button>
        </div>
      </div>
    </div>
  );
};
