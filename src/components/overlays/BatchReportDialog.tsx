import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AlertCircle, CheckCircle2, MinusCircle, RefreshCw } from 'lucide-react';
import type { AddonNamePreference, AddonSummary } from '../../api/ipc';
import type { BatchDialogPayload } from '../../state/useAppController';
import { formatAddonDisplayName } from '../../utils/format';
import { Button, Dialog } from '../m3/primitives';

export interface BatchReportDialogProps {
  payload: BatchDialogPayload | null;
  addonsById: Map<string, AddonSummary>;
  namePreference?: AddonNamePreference;
  onClose: () => void;
  onRescan: () => Promise<boolean>;
}

export function BatchReportDialog({
  payload,
  addonsById,
  namePreference = 'addon_title',
  onClose,
  onRescan,
}: BatchReportDialogProps) {
  const { t } = useTranslation();
  const [rescanning, setRescanning] = useState(false);

  if (!payload) return null;

  const { batch, profileSummary } = payload;

  return (
    <Dialog
      open={Boolean(payload)}
      wide
      title={t('dialogs.batchReportTitle')}
      onClose={onClose}
      actions={
        <>
          {batch.refreshError && (
            <Button
              variant="tonal"
              disabled={rescanning}
              icon={<RefreshCw size={15} className={rescanning ? 'md-spin' : undefined} />}
              onClick={async () => {
                setRescanning(true);
                try {
                  const ok = await onRescan();
                  if (ok) onClose();
                } finally {
                  setRescanning(false);
                }
              }}
            >
              {t('nav.scan')}
            </Button>
          )}
          <Button variant="filled" onClick={onClose}>
            {t('common.close')}
          </Button>
        </>
      }
    >
      {profileSummary && (
        <div className="md-chip-row">
          <span className="md-pill" data-tone="primary">
            {t('organize.profileApplied', {
              enabled: profileSummary.enabledCount,
              disabled: profileSummary.disabledCount,
            })}
          </span>
          {profileSummary.skippedWorkshopOnly.length > 0 && (
            <span className="md-pill" data-tone="tertiary">
              {t('organize.profileSkipped', {
                count: profileSummary.skippedWorkshopOnly.length,
              })}
            </span>
          )}
          {profileSummary.missingAddons.length > 0 && (
            <span className="md-pill" data-tone="error">
              {t('organize.profileMissing', { count: profileSummary.missingAddons.length })}
            </span>
          )}
        </div>
      )}

      {batch.refreshError && (
        <div
          className="l4a-path-chip"
          style={{
            backgroundColor: 'var(--md-sys-color-error-container)',
            color: 'var(--md-sys-color-on-error-container)',
          }}
        >
          <AlertCircle size={16} />
          <span>
            {t('dialogs.refreshError')}: {batch.refreshError.message}
          </span>
        </div>
      )}

      <div className="l4a-diff-table">
        {batch.items.map((item) => {
          const addon = addonsById.get(item.addonId);
          const isRecovery =
            item.status === 'failed' && item.error.code === 'recovery_required';
          const tone =
            item.status === 'applied'
              ? 'primary'
              : item.status === 'failed'
                ? 'error'
                : 'surface';
          return (
            <div key={item.addonId} className="l4a-diff-row">
              <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                <div className="l4a-row-wrap" style={{ minWidth: 0, flex: 1 }}>
                  {item.status === 'applied' ? (
                    <CheckCircle2
                      size={16}
                      style={{ color: 'var(--md-sys-color-primary)', flexShrink: 0 }}
                    />
                  ) : item.status === 'failed' ? (
                    <AlertCircle
                      size={16}
                      style={{ color: 'var(--md-sys-color-error)', flexShrink: 0 }}
                    />
                  ) : (
                    <MinusCircle
                      size={16}
                      style={{ color: 'var(--md-sys-color-on-surface-variant)', flexShrink: 0 }}
                    />
                  )}
                  <span className="md-title-small l4a-truncate">
                    {addon ? formatAddonDisplayName(addon, namePreference) : item.addonId}
                  </span>
                </div>
                <div className="l4a-row-wrap">
                  {isRecovery && (
                    <span className="md-pill" data-tone="error">
                      {t('dialogs.recoveryRequired')}
                    </span>
                  )}
                  <span className="md-pill" data-tone={tone}>
                    {t(`dialogs.status.${item.status}`)}
                  </span>
                </div>
              </div>
              {item.status === 'failed' && (
                <span className="md-body-small selectable" style={{ color: 'var(--md-sys-color-error)' }}>
                  [{item.error.code}] {item.error.message}
                </span>
              )}
            </div>
          );
        })}
      </div>
    </Dialog>
  );
}
