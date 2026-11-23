import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { HardDrive, Store, Trash2 } from 'lucide-react';
import type { AddonNamePreference, AddonSummary } from '../../api/ipc';
import { formatAddonDisplayName } from '../../utils/format';
import { Button, Dialog, Switch } from '../m3/primitives';

export interface DeleteDialogProps {
  addons: AddonSummary[];
  namePreference?: AddonNamePreference;
  busy: boolean;
  onClose: () => void;
  onConfirm: (
    addonIds: string[],
    deleteDiskFile: boolean,
    purgeKnownRecord: boolean,
  ) => Promise<void>;
}

export function DeleteDialog({
  addons,
  namePreference = 'addon_title',
  busy,
  onClose,
  onConfirm,
}: DeleteDialogProps) {
  const { t } = useTranslation();
  const hasInstalled = addons.some((a) => a.installState === 'installed');
  const [deleteDisk, setDeleteDisk] = useState(hasInstalled);
  const [purgeRecord, setPurgeRecord] = useState(!hasInstalled);

  if (addons.length === 0) return null;

  const workshopAddons = addons.filter((a) => Boolean(a.workshopId));
  const localAddons = addons.filter((a) => !a.workshopId);

  return (
    <Dialog
      open={addons.length > 0}
      title={t('dialogs.deleteTitle')}
      icon={<Trash2 size={20} style={{ color: 'var(--md-sys-color-error)' }} />}
      onClose={onClose}
      actions={
        <>
          <Button variant="text" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          {workshopAddons.length > 0 && localAddons.length > 0 && (
            <Button
              variant="outlined"
              tone="error"
              disabled={busy || (!deleteDisk && !purgeRecord)}
              onClick={() =>
                void onConfirm(
                  workshopAddons.map((a) => a.id),
                  deleteDisk,
                  purgeRecord,
                )
              }
            >
              {t('dialogs.deleteWorkshopOnly')} ({workshopAddons.length})
            </Button>
          )}
          <Button
            variant="filled"
            tone="error"
            disabled={busy || (!deleteDisk && !purgeRecord)}
            onClick={() =>
              void onConfirm(
                addons.map((a) => a.id),
                deleteDisk,
                purgeRecord,
              )
            }
          >
            {t('dialogs.deleteAll')} ({addons.length})
          </Button>
        </>
      }
    >
      <div className="md-chip-row">
        {workshopAddons.length > 0 && (
          <span className="md-pill" data-tone="secondary">
            <Store size={12} />
            {t('dialogs.workshopGroup', { count: workshopAddons.length })}
          </span>
        )}
        {localAddons.length > 0 && (
          <span className="md-pill" data-tone="error">
            <HardDrive size={12} />
            {t('dialogs.localGroup', { count: localAddons.length })}
          </span>
        )}
      </div>

      <div className="l4a-picker-list" style={{ maxHeight: 160 }}>
        {addons.map((a) => (
          <div key={a.id} className="l4a-picker-row" style={{ cursor: 'default' }}>
            <span className="md-body-medium l4a-truncate" style={{ flex: 1 }}>
              {formatAddonDisplayName(a, namePreference)}
            </span>
            {a.workshopId ? (
              <span className="md-body-small md-mono" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                #{a.workshopId}
              </span>
            ) : (
              <span className="md-pill" data-tone="error">
                {t('common.local')}
              </span>
            )}
          </div>
        ))}
      </div>

      {hasInstalled && (
        <div className="l4a-setting-row">
          <span className="md-body-medium">{t('dialogs.deleteDiskFile')}</span>
          <Switch
            checked={deleteDisk}
            label={t('dialogs.deleteDiskFile')}
            onChange={(next) => {
              setDeleteDisk(next);
              if (!next && hasInstalled) setPurgeRecord(false);
            }}
          />
        </div>
      )}

      <div className="l4a-setting-row">
        <span className="md-body-medium">{t('dialogs.purgeKnownRecord')}</span>
        <Switch
          checked={purgeRecord}
          disabled={hasInstalled && !deleteDisk}
          label={t('dialogs.purgeKnownRecord')}
          onChange={setPurgeRecord}
        />
      </div>
    </Dialog>
  );
}
