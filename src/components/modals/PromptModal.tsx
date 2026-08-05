import React, { useState, useEffect } from 'react';
import { useTranslation } from 'react-i18next';

interface PromptModalProps {
  open: boolean;
  title: string;
  message?: string;
  placeholder?: string;
  defaultValue?: string;
  onCancel: () => void;
  onConfirm: (value: string) => void;
  isSubmitting?: boolean;
}

export const PromptModal: React.FC<PromptModalProps> = ({
  open,
  title,
  message,
  placeholder,
  defaultValue = '',
  onCancel,
  onConfirm,
  isSubmitting = false,
}) => {
  const { t } = useTranslation();
  const [value, setValue] = useState('');

  useEffect(() => {
    if (open) {
      setValue(defaultValue);
    }
  }, [open, defaultValue]);

  if (!open) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (isSubmitting) return;
    onConfirm(value);
  };

  return (
    <div className="modal-overlay" onClick={onCancel} style={{ zIndex: 1100 }}>
      <form
        className="modal-content"
        onSubmit={handleSubmit}
        style={{ width: '420px', borderRadius: '24px', padding: '24px' }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="modal-title">{title}</h2>
        {message && (
          <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '16px', marginTop: '8px' }}>
            {message}
          </p>
        )}

        <div className="form-group" style={{ marginTop: '16px', marginBottom: '24px' }}>
          <input
            type="text"
            className="form-input"
            placeholder={placeholder}
            value={value}
            onChange={(e) => setValue(e.target.value)}
            required
            disabled={isSubmitting}
            autoFocus
            style={{ width: '100%', borderRadius: '12px' }}
          />
        </div>

        <div className="modal-actions" style={{ marginTop: 0 }}>
          <button
            type="button"
            className="btn btn-secondary"
            onClick={onCancel}
            disabled={isSubmitting}
            style={{ borderRadius: '12px' }}
          >
            {t('common.cancel')}
          </button>
          <button 
            type="submit" 
            className="btn btn-primary" 
            disabled={isSubmitting}
            style={{ borderRadius: '12px' }}
          >
            {t('common.confirm')}
          </button>
        </div>
      </form>
    </div>
  );
};
