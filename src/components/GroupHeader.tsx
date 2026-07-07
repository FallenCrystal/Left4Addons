import { Edit3, Trash2, Unlock, Lock, FolderOpen, Download } from 'lucide-react';
import { Addon, Group } from '../types/addon';
import { useTranslation } from 'react-i18next';

interface GroupHeaderProps {
  currentGroup: Group;
  groupAddons: Addon[];
  onRenameGroup: () => void;
  onDeleteGroup: () => void;
  onGroupActionBatch: (actionType: 'enable' | 'disable' | 'move-load') => void;
  onDownloadUninstalled?: (workshopIds: string[]) => void;
  downloadProgress?: Record<string, number>;
}

export function GroupHeader({
  currentGroup,
  groupAddons,
  onRenameGroup,
  onDeleteGroup,
  onGroupActionBatch,
  onDownloadUninstalled,
  downloadProgress = {},
}: GroupHeaderProps) {
  const { t } = useTranslation();

  const installedAddons = groupAddons.filter(ad => ad.dirType !== 'none');
  const uninstalledAddons = groupAddons.filter(ad => ad.dirType === 'none');
  const uninstalledWithWorkshopId = uninstalledAddons.filter(ad => ad.workshopId);
  const isAnyDownloading = uninstalledWithWorkshopId.some(ad => downloadProgress[ad.workshopId!] !== undefined);

  return (
    <div style={{
      backgroundColor: 'var(--md-sys-surface-container-high)',
      border: '1px solid var(--md-sys-color-outline-variant)',
      padding: '24px',
      borderRadius: '24px',
      marginBottom: '24px',
      display: 'flex',
      flexDirection: 'column',
      gap: '16px'
    }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <div>
          <h2 style={{ margin: 0, fontSize: '24px', color: '#fff' }}>{currentGroup.name}</h2>
          <p style={{ margin: '4px 0 0 0', fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
            {t('groupHeader.desc', { count: currentGroup.addons?.length || 0 })}
            {uninstalledAddons.length > 0 && ` · ${t('groupHeader.uninstalledCount', { count: uninstalledAddons.length })}`}
          </p>
        </div>
      </div>

      <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
        {installedAddons.length > 0 && (
          <>
            <button className="btn btn-primary" onClick={() => onGroupActionBatch('enable')}>
              <Unlock size={14} />
              <span>{t('groupHeader.enableAll')}</span>
            </button>
            <button className="btn btn-secondary" onClick={() => onGroupActionBatch('disable')}>
              <Lock size={14} />
              <span>{t('groupHeader.disableAll')}</span>
            </button>
            <button className="btn btn-tertiary" onClick={() => onGroupActionBatch('move-load')}>
              <FolderOpen size={14} />
              <span>{t('groupHeader.moveToManualAll')}</span>
            </button>
          </>
        )}
        {uninstalledWithWorkshopId.length > 0 && onDownloadUninstalled && (
          <button
            className="btn btn-primary"
            onClick={() => onDownloadUninstalled(uninstalledWithWorkshopId.map(ad => ad.workshopId!))}
            disabled={isAnyDownloading}
          >
            <Download size={14} />
            <span>{t('groupHeader.downloadAllUninstalled', { count: uninstalledWithWorkshopId.length })}</span>
          </button>
        )}
        <div style={{ flex: 1 }} />
        <button className="btn btn-secondary" onClick={onRenameGroup}>
          <Edit3 size={14} />
          <span>{t('groupHeader.renameGroup')}</span>
        </button>
        <button
          className="btn btn-secondary"
          style={{ color: 'var(--md-sys-color-error)' }}
          onClick={onDeleteGroup}
        >
          <Trash2 size={14} />
          <span>{t('groupHeader.deleteGroup')}</span>
        </button>
      </div>
    </div>
  );
}
