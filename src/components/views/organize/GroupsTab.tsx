import { useTranslation } from 'react-i18next';
import { ExternalLink, FolderKanban, FolderOpen, Pencil, Trash2 } from 'lucide-react';
import type { AddonNamePreference, AddonSummary, UserGroup } from '../../../api/ipc';
import { useContentFilterContext } from '../../../state/ContentFilterContext';
import {
  formatFilteredAddonTitle,
  resolveLibraryAddonCardFilter,
} from '../../../utils/contentFilter';
import { formatBytes } from '../../../utils/format';
import { EmptyState, IconButton, Switch } from '../../m3/primitives';
import type { ExternalAddonFilter } from '../LibraryView';
import { summarizeGroupMembers } from './helpers';
import { GroupStatusPill } from './OrganizeShared';

export interface GroupsTabProps {
  userGroups: UserGroup[];
  filteredGroups: UserGroup[];
  addonsById: Map<string, AddonSummary>;
  namePreference: AddonNamePreference;
  busy: boolean;
  onInspectGroup: (groupId: string) => void;
  onBulkToggle: (addonIds: string[], enabled: boolean) => void;
  onOpenInFileManager: (path: string) => void;
  onOpenFilterInLibrary: (filter: ExternalAddonFilter) => void;
  onEditGroup: (group: UserGroup) => void;
  onDeleteGroup: (groupId: string) => void;
}

export function GroupsTab({
  userGroups,
  filteredGroups,
  addonsById,
  namePreference,
  busy,
  onInspectGroup,
  onBulkToggle,
  onOpenInFileManager,
  onOpenFilterInLibrary,
  onEditGroup,
  onDeleteGroup,
}: GroupsTabProps) {
  const { t } = useTranslation();
  const filterCtx = useContentFilterContext();

  if (filteredGroups.length === 0) {
    return (
      <EmptyState
        icon={<FolderKanban size={26} />}
        title={userGroups.length === 0 ? t('common.empty') : t('common.noResults')}
      />
    );
  }

  return (
    <div className="l4a-card-grid-wide">
      {filteredGroups.map((group) => {
        const visibleAddonIds = group.addonIds.filter((id) => {
          const addon = addonsById.get(id);
          return !addon || !resolveLibraryAddonCardFilter(addon, filterCtx).hiddenCard;
        });
        const summary = summarizeGroupMembers(visibleAddonIds, addonsById);
        const toggleableIds = summary.loading.map((a) => a.id);
        const accent = group.color || 'var(--md-sys-color-primary)';

        return (
          <article
            key={group.id}
            className="md-card l4a-group-card"
            data-clickable="true"
            data-muted={summary.loading.length > 0 && summary.noneEnabled ? 'true' : 'false'}
          >
            <div className="l4a-group-accent" style={{ backgroundColor: accent }} />
            <div className="l4a-addon-card-body">
              <button
                type="button"
                className="l4a-card-trigger"
                onClick={() => onInspectGroup(group.id)}
              >
                <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                  <div className="l4a-row-wrap" style={{ minWidth: 0, flex: 1 }}>
                    <span
                      style={{
                        width: 10,
                        height: 10,
                        borderRadius: 999,
                        backgroundColor: accent,
                        flexShrink: 0,
                      }}
                    />
                    <span className="md-title-medium l4a-truncate">{group.name}</span>
                  </div>
                  <GroupStatusPill summary={summary} />
                </div>

                <div className="l4a-addon-meta" style={{ marginTop: 6 }}>
                  <span>{t('common.items', { count: visibleAddonIds.length })}</span>
                  {summary.totalSize > 0 && (
                    <>
                      <span>·</span>
                      <span>{formatBytes(summary.totalSize)}</span>
                    </>
                  )}
                </div>

                {group.tags.length > 0 && (
                  <div className="md-chip-row" style={{ marginTop: 8 }}>
                    {group.tags.map((tag) => (
                      <span key={tag} className="md-pill" data-tone="secondary">
                        {tag}
                      </span>
                    ))}
                  </div>
                )}

                {summary.members.length > 0 && (
                  <div className="l4a-member-preview">
                    {summary.members.slice(0, 3).map((m) => (
                      <span key={m.id} className="l4a-member-chip l4a-truncate">
                        {formatFilteredAddonTitle(m, namePreference, filterCtx, t)}
                      </span>
                    ))}
                    {summary.members.length > 3 && (
                      <span className="l4a-member-chip">
                        {t('organize.moreMembers', { count: summary.members.length - 3 })}
                      </span>
                    )}
                  </div>
                )}
              </button>

              <div className="l4a-addon-card-footer">
                <div className="l4a-row-wrap" style={{ gap: 6 }}>
                  {toggleableIds.length > 0 && (
                    <Switch
                      compact
                      checked={summary.allEnabled}
                      disabled={busy}
                      label={t('organize.toggleAll')}
                      onChange={(next) => onBulkToggle(toggleableIds, next)}
                    />
                  )}
                  {summary.firstPath && (
                    <IconButton
                      size="sm"
                      label={t('common.openInExplorer')}
                      onClick={() => onOpenInFileManager(summary.firstPath!)}
                    >
                      <FolderOpen size={15} />
                    </IconButton>
                  )}
                  <IconButton
                    size="sm"
                    label={t('organize.filterInLibrary')}
                    onClick={() =>
                      onOpenFilterInLibrary({
                        label: group.name,
                        addonIds: visibleAddonIds,
                      })
                    }
                  >
                    <ExternalLink size={15} />
                  </IconButton>
                </div>
                <div className="l4a-row-wrap" style={{ gap: 4 }}>
                  <IconButton
                    size="sm"
                    label={t('common.edit')}
                    onClick={() => onEditGroup(group)}
                  >
                    <Pencil size={15} />
                  </IconButton>
                  <IconButton
                    size="sm"
                    label={t('common.delete')}
                    disabled={busy}
                    onClick={() => onDeleteGroup(group.id)}
                  >
                    <Trash2 size={15} />
                  </IconButton>
                </div>
              </div>
            </div>
          </article>
        );
      })}
    </div>
  );
}
