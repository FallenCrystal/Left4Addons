import React from 'react';
import { AlertTriangle } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Addon } from '../types/addon';

interface DeleteConfirmModalProps {
  open: boolean;
  addons: Addon[];
  onConfirm: (deleteMode: 'all' | 'workshop-only', removeFromKnown: boolean) => void;
  onCancel: () => void;
  isSubmitting?: boolean;
}

export const DeleteConfirmModal: React.FC<DeleteConfirmModalProps> = ({
  open,
  addons,
  onConfirm,
  onCancel,
  isSubmitting = false,
}) => {
  const { t } = useTranslation();

  if (!open || addons.length === 0) return null;

  const workshopItems = addons.filter(ad => ad.workshopId);
  const nonWorkshopItems = addons.filter(ad => !ad.workshopId);

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-content" style={{ width: '450px' }} onClick={(e) => e.stopPropagation()}>
        <h2 className="modal-title" style={{ display: 'flex', alignItems: 'center', gap: '8px', color: 'var(--md-sys-color-error)' }}>
          <AlertTriangle size={24} />
          <span>{t('deleteConfirmModal.title')}</span>
        </h2>
        
        <div style={{ whiteSpace: 'pre-wrap', marginTop: '16px', marginBottom: '20px', color: '#e2e2e9', fontSize: '14px', lineHeight: '1.6' }}>
          <p style={{ marginBottom: '12px', fontWeight: '500' }}>
            {t('deleteConfirmModal.subtitle', { count: addons.length })}
          </p>

          <div style={{ display: 'flex', flexDirection: 'column', gap: '12px' }}>
            {workshopItems.length > 0 && (
              <div>
                <strong style={{ color: 'var(--md-sys-color-primary)' }}>{t('deleteConfirmModal.workshopTitle', { count: workshopItems.length })}</strong>
                <p style={{ margin: '4px 0 0 0', fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
                  {t('deleteConfirmModal.workshopDesc')}
                </p>
              </div>
            )}

            {nonWorkshopItems.length > 0 && (
              <div>
                <strong style={{ color: 'var(--md-sys-color-error)' }}>{t('deleteConfirmModal.localTitle', { count: nonWorkshopItems.length })}</strong>
                <p style={{ margin: '4px 0 0 0', fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
                  {t('deleteConfirmModal.localDesc')}
                </p>
              </div>
            )}
          </div>

          <p style={{ marginTop: '16px' }}>
            {t('deleteConfirmModal.confirmQuestion')}
          </p>
        </div>

        <div className="modal-actions" style={{ justifyContent: 'flex-end', gap: '8px' }}>
          <button 
            type="button" 
            className="btn btn-secondary" 
            onClick={onCancel}
            disabled={isSubmitting}
          >
            {t('common.cancel')}
          </button>
          
          {nonWorkshopItems.length > 0 && workshopItems.length > 0 && (
            <button 
              type="button" 
              className="btn btn-primary"
              disabled={isSubmitting}
              style={{ background: 'var(--md-sys-color-tertiary)', color: 'var(--md-sys-color-on-tertiary)' }}
              onClick={() => {
                onConfirm('workshop-only', true);
                onCancel();
              }}
            >
              {t('deleteConfirmModal.btnWorkshopOnly')}
            </button>
          )}
          
          <button 
            type="button" 
            className="btn btn-primary"
            disabled={isSubmitting}
            style={{ background: 'var(--md-sys-color-error)', color: 'var(--md-sys-color-on-error)' }}
            onClick={() => {
              onConfirm('all', true);
              onCancel();
            }}
          >
            {t('deleteConfirmModal.btnDeleteAll')}
          </button>
        </div>
      </div>
    </div>
  );
};
