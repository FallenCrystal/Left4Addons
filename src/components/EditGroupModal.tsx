import React, { useState, useEffect } from 'react';
import { useTranslation } from 'react-i18next';

interface EditGroupModalProps {
  open: boolean;
  groupId: string;
  currentName: string;
  onCancel: () => void;
  onConfirm: (groupId: string, newName: string) => void;
  isSubmitting?: boolean;
}

export const EditGroupModal: React.FC<EditGroupModalProps> = ({
  open,
  groupId,
  currentName,
  onCancel,
  onConfirm,
  isSubmitting = false,
}) => {
  const { t } = useTranslation();
  const [name, setName] = useState('');

  useEffect(() => {
    if (open) {
      setName(currentName);
    }
  }, [open, currentName]);

  if (!open) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim() || isSubmitting) return;
    onConfirm(groupId, name.trim());
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <form className="modal-content" onSubmit={handleSubmit} onClick={(e) => e.stopPropagation()}>
        <h2 className="modal-title">{t('editGroupModal.title')}</h2>
        
        <div className="form-group">
          <label className="form-label">{t('editGroupModal.newNameLabel')}</label>
          <input 
            type="text" 
            className="form-input" 
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            disabled={isSubmitting}
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
            {isSubmitting ? t('common.saving') : t('common.save')}
          </button>
        </div>
      </form>
    </div>
  );
};
