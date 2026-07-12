import { Edit3, Trash2 } from 'lucide-react';
import { MasterCollection, Group } from '../types/addon';
import { useTranslation } from 'react-i18next';

interface MasterCollectionHeaderProps {
  currentMasterCollection: MasterCollection;
  groupsInCollection: Group[];
  onRenameCollection: () => void;
  onDeleteCollection: () => void;
}

export function MasterCollectionHeader({
  currentMasterCollection,
  groupsInCollection,
  onRenameCollection,
  onDeleteCollection,
}: MasterCollectionHeaderProps) {
  const { t } = useTranslation();

  const displayName = currentMasterCollection.nameKey
    ? t(currentMasterCollection.nameKey, currentMasterCollection.name)
    : currentMasterCollection.name;

  const totalAddons = groupsInCollection.reduce((sum, g) => sum + (g.addons?.length || 0), 0);

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
          <h2 style={{ margin: 0, fontSize: '24px', color: '#fff' }}>{displayName}</h2>
          <p style={{ margin: '4px 0 0 0', fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
            {t('masterCollections.desc', { count: groupsInCollection.length })}
            {totalAddons > 0 && ` · ${t('masterCollections.totalAddons', { count: totalAddons })}`}
          </p>
        </div>
        {!currentMasterCollection.isSystem && (
          <div style={{ display: 'flex', gap: '8px' }}>
            <button className="btn btn-secondary" onClick={onRenameCollection}>
              <Edit3 size={14} />
              <span>{t('masterCollections.renameCollection')}</span>
            </button>
            <button
              className="btn btn-secondary"
              style={{ color: 'var(--md-sys-color-error)' }}
              onClick={onDeleteCollection}
            >
              <Trash2 size={14} />
              <span>{t('masterCollections.deleteCollection')}</span>
            </button>
          </div>
        )}
      </div>

      {groupsInCollection.length > 0 && (
        <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
          {groupsInCollection.map(g => (
            <span
              key={g.id}
              style={{
                padding: '4px 12px',
                borderRadius: '8px',
                fontSize: '12px',
                fontWeight: 500,
                backgroundColor: 'var(--md-sys-color-secondary-container)',
                color: 'var(--md-sys-color-on-secondary-container)'
              }}
            >
              {g.name} ({g.addons?.length || 0})
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
