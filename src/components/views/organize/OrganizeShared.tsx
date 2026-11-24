import { useTranslation } from 'react-i18next';
import { FolderOpen, Map as MapIcon, X } from 'lucide-react';
import type { AddonNamePreference, AddonSummary } from '../../../api/ipc';
import { formatAddonDisplayName, formatBytes } from '../../../utils/format';
import { AddonCover } from '../../m3/AddonCover';
import { IconButton, Switch } from '../../m3/primitives';
import type { GroupMemberSummary, MemberRowBadge } from './helpers';

export function GroupStatusPill({ summary }: { summary: GroupMemberSummary }) {
  const { t } = useTranslation();

  if (summary.loading.length === 0) {
    return (
      <span className="md-pill" data-tone="surface">
        {t('organize.statusEmpty')}
      </span>
    );
  }
  if (summary.allEnabled) {
    return (
      <span className="md-pill" data-tone="primary">
        {t('organize.statusEnabled')}
      </span>
    );
  }
  if (summary.noneEnabled) {
    return (
      <span className="md-pill" data-tone="surface">
        {t('organize.statusDisabled')}
      </span>
    );
  }
  return (
    <span className="md-pill" data-tone="tertiary">
      {t('organize.statusPartial', {
        enabled: summary.enabledLoading.length,
        total: summary.loading.length,
      })}
    </span>
  );
}

export interface OrganizeMemberRowProps {
  addon: AddonSummary;
  namePreference: AddonNamePreference;
  busy: boolean;
  displayLabel?: string;
  indexBadge?: number;
  showFileName?: boolean;
  badge?: MemberRowBadge;
  onSelectAddon: (addon: AddonSummary) => void;
  onOpenInFileManager: (path: string) => void;
  onToggleSingle: (addon: AddonSummary, enabled: boolean) => void;
  onRemove?: () => void;
}

export function OrganizeMemberRow({
  addon,
  namePreference,
  busy,
  displayLabel,
  indexBadge,
  showFileName,
  badge,
  onSelectAddon,
  onOpenInFileManager,
  onToggleSingle,
  onRemove,
}: OrganizeMemberRowProps) {
  const { t } = useTranslation();
  const installed = addon.installState === 'installed';
  const resolvedLabel = displayLabel ?? formatAddonDisplayName(addon, namePreference);

  return (
    <div
      className="l4a-list-row l4a-list-row-dense"
      data-muted={installed && !addon.enabled ? 'true' : 'false'}
    >
      <div className="l4a-picker-thumb">
        <AddonCover
          compact
          src={addon.coverImage}
          fallbackSrc={addon.localCoverImage}
          title={resolvedLabel}
          category={addon.categories[0]}
        />
      </div>
      <button
        type="button"
        className="l4a-card-trigger l4a-list-main"
        onClick={() => onSelectAddon(addon)}
      >
        <div className="l4a-row-wrap" style={{ gap: 6 }}>
          {indexBadge !== undefined && (
            <span className="md-pill md-mono" data-tone="secondary">
              #{indexBadge}
            </span>
          )}
          <span className="md-title-small l4a-truncate">{resolvedLabel}</span>
          {badge && (
            <span className="md-pill" data-tone={badge.tone}>
              {badge.text}
            </span>
          )}
        </div>
        <div className="l4a-addon-meta">
          {showFileName && addon.currentFileName ? (
            <span className="md-mono l4a-truncate">{addon.currentFileName}</span>
          ) : (
            <span className="l4a-truncate">{addon.author || t('common.unknown')}</span>
          )}
          <span>·</span>
          <span>{formatBytes(addon.fileSize)}</span>
          {addon.maps.length > 0 && (
            <>
              <span>·</span>
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 3 }}>
                <MapIcon size={11} />
                {addon.maps.length}
              </span>
            </>
          )}
          {addon.primaryDir && (
            <>
              <span>·</span>
              <span>
                {addon.primaryDir === 'loading'
                  ? t('library.loadingDir')
                  : t('library.workshopDir')}
              </span>
            </>
          )}
        </div>
      </button>
      <div className="l4a-list-trailing">
        {installed && addon.currentPath && (
          <IconButton
            size="sm"
            label={t('common.openInExplorer')}
            onClick={() => onOpenInFileManager(addon.currentPath!)}
          >
            <FolderOpen size={15} />
          </IconButton>
        )}
        {installed && (
          <Switch
            compact
            checked={addon.enabled}
            disabled={busy}
            label={resolvedLabel}
            onChange={(next) => onToggleSingle(addon, next)}
          />
        )}
        {onRemove && (
          <IconButton
            size="sm"
            label={t('organize.removeMember')}
            disabled={busy}
            onClick={onRemove}
          >
            <X size={15} />
          </IconButton>
        )}
      </div>
    </div>
  );
}
