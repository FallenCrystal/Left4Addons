import { useTranslation } from 'react-i18next';
import {
  AlertTriangle,
  Download,
  ExternalLink,
  EyeOff,
  Pencil,
  Play,
  Trash2,
} from 'lucide-react';
import type {
  AddonNamePreference,
  AddonSummary,
  BundleRecord,
  LoadoutProfile,
  TrackedCollectionRecord,
  UserGroup,
} from '../../../api/ipc';
import { useContentFilterContext } from '../../../state/ContentFilterContext';
import type { LiveDownloadState } from '../../../state/useAppController';
import {
  addonFilterKey,
  formatDiagnosticVerdictBadge,
  formatFilteredAddonTitle,
  isAddonTitleRedacted,
  resolveLibraryAddonCardFilter,
} from '../../../utils/contentFilter';
import { formatBytes } from '../../../utils/format';
import { FilterCardOverlay, FilterFailureBadge } from '../../m3/ContentFilterIndicators';
import { Button, SideSheet, Switch } from '../../m3/primitives';
import type { ExternalAddonFilter } from '../LibraryView';
import { BundleInspectorSheet } from './BundleInspectorSheet';
import { computeProfileDiff, summarizeGroupMembers } from './helpers';
import { GroupStatusPill, OrganizeMemberRow } from './OrganizeShared';

export interface OrganizeInspectorSheetProps {
  inspectedBundle: BundleRecord | null;
  inspectedCollection: TrackedCollectionRecord | null;
  inspectedGroup: UserGroup | null;
  inspectedProfile: LoadoutProfile | null;
  addonsById: Map<string, AddonSummary>;
  addonsByWorkshopId: Map<string, AddonSummary>;
  loadingAddons: AddonSummary[];
  namePreference: AddonNamePreference;
  liveDownloads?: Record<string, LiveDownloadState>;
  busy: boolean;
  onClose: () => void;
  onToggleBundleLock: (bundleId: string, locked: boolean) => void;
  onDeleteBundle: (bundleId: string) => void;
  onUntrackCollection: (collectionId: string) => void;
  onDownloadWorkshopItem: (workshopId: string, title?: string) => void;
  onEditGroup: (group: UserGroup) => void;
  onSaveUserGroup: (group: UserGroup) => Promise<boolean>;
  onDeleteGroup: (groupId: string) => void;
  onEditProfile: (profile: LoadoutProfile) => void;
  onDeleteProfile: (profileId: string) => void;
  onApplyProfile: (profileId: string) => void;
  onBulkToggle: (addonIds: string[], enabled: boolean) => void;
  onToggleSingle: (addon: AddonSummary, enabled: boolean) => void;
  onSelectAddon: (addon: AddonSummary) => void;
  onOpenInFileManager: (path: string) => void;
  onOpenFilterInLibrary: (filter: ExternalAddonFilter) => void;
  onCopy: (text: string) => void;
}

export function OrganizeInspectorSheet({
  inspectedBundle,
  inspectedCollection,
  inspectedGroup,
  inspectedProfile,
  addonsById,
  addonsByWorkshopId,
  loadingAddons,
  namePreference,
  liveDownloads,
  busy,
  onClose,
  onToggleBundleLock,
  onDeleteBundle,
  onUntrackCollection,
  onDownloadWorkshopItem,
  onEditGroup,
  onSaveUserGroup,
  onDeleteGroup,
  onEditProfile,
  onDeleteProfile,
  onApplyProfile,
  onBulkToggle,
  onToggleSingle,
  onSelectAddon,
  onOpenInFileManager,
  onOpenFilterInLibrary,
  onCopy,
}: OrganizeInspectorSheetProps) {
  const { t } = useTranslation();
  const filterCtx = useContentFilterContext();
  const { verdicts, revealCard } = filterCtx;

  const visibleCollectionWids = inspectedCollection
    ? inspectedCollection.memberWorkshopIds.filter((wid) => {
        const local = addonsByWorkshopId.get(wid);
        return !local || !resolveLibraryAddonCardFilter(local, filterCtx).hiddenCard;
      })
    : [];
  const collectionHiddenCount = inspectedCollection
    ? inspectedCollection.memberWorkshopIds.length - visibleCollectionWids.length
    : 0;
  const collectionMissing = visibleCollectionWids.filter(
    (wid) => addonsByWorkshopId.get(wid)?.installState !== 'installed',
  );
  const collectionMissingIdle = collectionMissing.filter((wid) => !liveDownloads?.[wid]);

  const visibleGroupAddonIds = inspectedGroup
    ? inspectedGroup.addonIds.filter((id) => {
        const addon = addonsById.get(id);
        return !addon || !resolveLibraryAddonCardFilter(addon, filterCtx).hiddenCard;
      })
    : [];
  const groupHiddenCount = inspectedGroup
    ? inspectedGroup.addonIds.length - visibleGroupAddonIds.length
    : 0;
  const groupSummary = inspectedGroup
    ? summarizeGroupMembers(visibleGroupAddonIds, addonsById)
    : null;
  const groupToggleableIds = groupSummary ? groupSummary.loading.map((a) => a.id) : [];

  const profileDiff = inspectedProfile
    ? computeProfileDiff(inspectedProfile, addonsById, loadingAddons)
    : null;
  const profileHiddenCount = profileDiff
    ? profileDiff.toEnable.filter((a) => resolveLibraryAddonCardFilter(a, filterCtx).hiddenCard)
        .length +
      profileDiff.toDisable.filter((a) => resolveLibraryAddonCardFilter(a, filterCtx).hiddenCard)
        .length +
      profileDiff.unchangedEnabled.filter(
        (a) => resolveLibraryAddonCardFilter(a, filterCtx).hiddenCard,
      ).length +
      profileDiff.missingIds.filter((id) => {
        const found = addonsById.get(id);
        return Boolean(found && resolveLibraryAddonCardFilter(found, filterCtx).hiddenCard);
      }).length
    : 0;

  return (
    <>
      <BundleInspectorSheet
        inspectedBundle={inspectedBundle}
        addonsById={addonsById}
        namePreference={namePreference}
        busy={busy}
        onClose={onClose}
        onToggleBundleLock={onToggleBundleLock}
        onDeleteBundle={onDeleteBundle}
        onBulkToggle={onBulkToggle}
        onToggleSingle={onToggleSingle}
        onSelectAddon={onSelectAddon}
        onOpenInFileManager={onOpenInFileManager}
        onOpenFilterInLibrary={onOpenFilterInLibrary}
        onCopy={onCopy}
      />

      {/* Inline Collection Inspector */}
      <SideSheet
        open={Boolean(inspectedCollection)}
        title={inspectedCollection?.title ?? ''}
        onClose={onClose}
        footer={
          inspectedCollection && (
            <>
              {collectionMissing.length > 0 && (
                <Button
                  variant="filled"
                  size="sm"
                  disabled={busy || collectionMissingIdle.length === 0}
                  icon={<Download size={14} />}
                  onClick={() => {
                    for (const wid of collectionMissingIdle) {
                      const found = addonsByWorkshopId.get(wid);
                      const safeTitle =
                        found && !isAddonTitleRedacted(found, filterCtx)
                          ? found.title
                          : undefined;
                      onDownloadWorkshopItem(wid, safeTitle);
                    }
                  }}
                >
                  {t('organize.downloadMissing', { count: collectionMissing.length })}
                </Button>
              )}
              <Button
                variant="outlined"
                size="sm"
                icon={<ExternalLink size={14} />}
                onClick={() => {
                  onOpenFilterInLibrary({
                    label: inspectedCollection.title,
                    workshopIds: visibleCollectionWids,
                  });
                  onClose();
                }}
              >
                {t('organize.filterInLibrary')}
              </Button>
              <Button
                variant="text"
                tone="error"
                size="sm"
                disabled={busy}
                icon={<Trash2 size={14} />}
                onClick={() => {
                  onUntrackCollection(inspectedCollection.collectionId);
                  onClose();
                }}
              >
                {t('organize.untrack')}
              </Button>
            </>
          )
        }
      >
        {inspectedCollection && (
          <div className="md-sheet-body">
            <div className="md-chip-row">
              <span className="md-pill md-mono" data-tone="surface">
                #{inspectedCollection.collectionId}
              </span>
              {inspectedCollection.autoSync && (
                <span className="md-pill" data-tone="primary">
                  {t('organize.autoSync')}
                </span>
              )}
              {inspectedCollection.removedUpstreamIds.length > 0 && (
                <span className="md-pill" data-tone="error">
                  {t('organize.upstreamRemoved', {
                    count: inspectedCollection.removedUpstreamIds.length,
                  })}
                </span>
              )}
              {collectionHiddenCount > 0 && (
                <span className="md-pill" data-tone="surface">
                  <EyeOff size={12} />
                  {t('filter.filteredCount', { count: collectionHiddenCount })}
                </span>
              )}
            </div>

            {inspectedCollection.description && (
              <div className="md-body-small selectable l4a-description-box">
                {inspectedCollection.description}
              </div>
            )}

            <div className="l4a-list">
              {visibleCollectionWids.length === 0 ? (
                <div
                  style={{
                    padding: 20,
                    textAlign: 'center',
                    color: 'var(--md-sys-color-on-surface-variant)',
                  }}
                >
                  {t('common.empty')}
                </div>
              ) : (
                visibleCollectionWids.map((wid) => {
                  const local = addonsByWorkshopId.get(wid);
                  const removed = inspectedCollection.removedUpstreamIds.includes(wid);
                  if (local && local.installState === 'installed') {
                    return (
                      <OrganizeMemberRow
                        key={local.id}
                        addon={local}
                        namePreference={namePreference}
                        busy={busy}
                        badge={
                          removed
                            ? { text: t('organize.memberRemoved'), tone: 'error' }
                            : { text: t('organize.memberInstalled'), tone: 'primary' }
                        }
                        onSelectAddon={onSelectAddon}
                        onOpenInFileManager={onOpenInFileManager}
                        onToggleSingle={onToggleSingle}
                      />
                    );
                  }

                  const localKey = local ? addonFilterKey(local.id) : undefined;
                  const localVerdict = localKey ? verdicts[localKey] : undefined;
                  const localFilter = local
                    ? resolveLibraryAddonCardFilter(local, filterCtx)
                    : null;
                  const localLabel = local
                    ? formatFilteredAddonTitle(local, namePreference, filterCtx, t)
                    : `#${wid}`;
                  const localDesc =
                    local?.addonInfo.addondescription ?? local?.addonInfo.description;

                  return (
                    <div
                      key={wid}
                      className="l4a-list-row l4a-list-row-dense"
                      style={{ cursor: local ? 'pointer' : 'default' }}
                      data-blur-card={localFilter?.blurWholeCard ? 'true' : 'false'}
                    >
                      {localFilter && localKey && (
                        <FilterCardOverlay state={localFilter} onReveal={() => revealCard(localKey)} />
                      )}
                      <div
                        className="l4a-blur-row-inner"
                        inert={localFilter?.blurWholeCard || undefined}
                        aria-hidden={localFilter?.blurWholeCard || undefined}
                      >
                        {local ? (
                          <button
                            type="button"
                            className="l4a-card-trigger l4a-list-main"
                            onClick={() => onSelectAddon(local)}
                          >
                            <div className="l4a-row-wrap" style={{ gap: 6 }}>
                              <span className="md-title-small l4a-truncate">{localLabel}</span>
                              {localFilter && <FilterFailureBadge state={localFilter} />}
                              {localFilter?.exclusionActive &&
                                localFilter.exclusionDisplayLevel &&
                                localFilter.exclusionDisplayLevel !== 'normal' && (
                                  <span className="md-pill" data-tone="secondary">
                                    {t(`filter.levels.${localFilter.exclusionDisplayLevel}`)}
                                  </span>
                                )}
                              {localFilter?.exclusionInvalidated && (
                                <span className="md-pill" data-tone="error">
                                  <AlertTriangle size={11} />
                                  {t('filter.exclusionInvalidPill')}
                                </span>
                              )}
                              {localFilter?.showDiagnosticBadge && localVerdict && (
                                <span className="md-pill" data-tone="tertiary">
                                  {formatDiagnosticVerdictBadge(localVerdict, localFilter, {
                                    imageTag: t('filter.modImageShort'),
                                    textTag: t('filter.modTextShort'),
                                  })}
                                </span>
                              )}
                            </div>
                            <span
                              className="md-body-small md-mono"
                              style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                            >
                              #{wid}
                            </span>
                            {localFilter?.isBlurCardRevealed && (
                              <div className="l4a-revealed-desc">
                                {localFilter.redactDescription
                                  ? t('filter.redactedDescription')
                                  : localDesc?.trim() || t('filter.noDescription')}
                              </div>
                            )}
                          </button>
                        ) : (
                          <div className="l4a-list-main">
                            <span className="md-title-small l4a-truncate">{localLabel}</span>
                            <span
                              className="md-body-small md-mono"
                              style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                            >
                              #{wid}
                            </span>
                          </div>
                        )}
                        <div className="l4a-list-trailing">
                          <span className="md-pill" data-tone={removed ? 'error' : 'surface'}>
                            {removed ? t('organize.memberRemoved') : t('organize.memberMissing')}
                          </span>
                          {!removed && (
                            <Button
                              variant="tonal"
                              size="sm"
                              disabled={busy || Boolean(liveDownloads?.[wid])}
                              icon={<Download size={13} />}
                              onClick={() =>
                                onDownloadWorkshopItem(
                                  wid,
                                  local && !isAddonTitleRedacted(local, filterCtx)
                                    ? local.title
                                    : undefined,
                                )
                              }
                            >
                              {t('common.download')}
                            </Button>
                          )}
                        </div>
                      </div>
                    </div>
                  );
                })
              )}
            </div>
          </div>
        )}
      </SideSheet>

      {/* Inline Group Inspector */}
      <SideSheet
        open={Boolean(inspectedGroup)}
        title={inspectedGroup?.name ?? ''}
        onClose={onClose}
        footer={
          inspectedGroup && (
            <>
              <Button
                variant="tonal"
                size="sm"
                icon={<Pencil size={14} />}
                onClick={() => onEditGroup(inspectedGroup)}
              >
                {t('organize.addMembers')}
              </Button>
              <Button
                variant="outlined"
                size="sm"
                icon={<ExternalLink size={14} />}
                onClick={() => {
                  onOpenFilterInLibrary({
                    label: inspectedGroup.name,
                    addonIds: visibleGroupAddonIds,
                  });
                  onClose();
                }}
              >
                {t('organize.filterInLibrary')}
              </Button>
              <Button
                variant="text"
                tone="error"
                size="sm"
                disabled={busy}
                icon={<Trash2 size={14} />}
                onClick={() => {
                  onDeleteGroup(inspectedGroup.id);
                  onClose();
                }}
              >
                {t('common.delete')}
              </Button>
            </>
          )
        }
      >
        {inspectedGroup && groupSummary && (
          <div className="md-sheet-body">
            <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
              <div className="md-chip-row">
                <GroupStatusPill summary={groupSummary} />
                <span className="md-pill" data-tone="surface">
                  {formatBytes(groupSummary.totalSize)}
                </span>
                {inspectedGroup.tags.map((tag) => (
                  <span key={tag} className="md-pill" data-tone="secondary">
                    {tag}
                  </span>
                ))}
                {groupHiddenCount > 0 && (
                  <span className="md-pill" data-tone="surface">
                    <EyeOff size={12} />
                    {t('filter.filteredCount', { count: groupHiddenCount })}
                  </span>
                )}
              </div>
              {groupToggleableIds.length > 0 && (
                <Switch
                  compact
                  checked={groupSummary.allEnabled}
                  disabled={busy}
                  label={t('organize.toggleAll')}
                  onChange={(next) => onBulkToggle(groupToggleableIds, next)}
                />
              )}
            </div>

            <div className="l4a-list">
              {groupSummary.members.length === 0 ? (
                <div
                  style={{
                    padding: 20,
                    textAlign: 'center',
                    color: 'var(--md-sys-color-on-surface-variant)',
                  }}
                >
                  {t('common.empty')}
                </div>
              ) : (
                groupSummary.members.map((addon) => (
                  <OrganizeMemberRow
                    key={addon.id}
                    addon={addon}
                    namePreference={namePreference}
                    busy={busy}
                    onSelectAddon={onSelectAddon}
                    onOpenInFileManager={onOpenInFileManager}
                    onToggleSingle={onToggleSingle}
                    onRemove={() => {
                      void onSaveUserGroup({
                        ...inspectedGroup,
                        addonIds: inspectedGroup.addonIds.filter((id) => id !== addon.id),
                      });
                    }}
                  />
                ))
              )}
            </div>
          </div>
        )}
      </SideSheet>

      {/* Inline Profile Inspector with Live Diff Preview */}
      <SideSheet
        open={Boolean(inspectedProfile)}
        title={inspectedProfile?.name ?? ''}
        onClose={onClose}
        footer={
          inspectedProfile && (
            <>
              <Button
                variant="outlined"
                size="sm"
                icon={<Pencil size={14} />}
                onClick={() => onEditProfile(inspectedProfile)}
              >
                {t('common.edit')}
              </Button>
              <Button
                variant="text"
                tone="error"
                size="sm"
                disabled={busy}
                icon={<Trash2 size={14} />}
                onClick={() => {
                  onDeleteProfile(inspectedProfile.id);
                  onClose();
                }}
              >
                {t('common.delete')}
              </Button>
              <Button
                variant="filled"
                size="sm"
                disabled={busy}
                icon={<Play size={14} />}
                onClick={() => onApplyProfile(inspectedProfile.id)}
              >
                {t('common.apply')}
              </Button>
            </>
          )
        }
      >
        {inspectedProfile && profileDiff && (
          <div className="md-sheet-body">
            {inspectedProfile.description && (
              <div className="md-body-small l4a-description-box">
                {inspectedProfile.description}
              </div>
            )}

            <div className="md-chip-row">
              <span className="md-pill" data-tone="primary">
                {t('organize.profileDiffEnable', { count: profileDiff.toEnable.length })}
              </span>
              <span className="md-pill" data-tone="tertiary">
                {t('organize.profileDiffDisable', { count: profileDiff.toDisable.length })}
              </span>
              <span className="md-pill" data-tone="surface">
                {t('organize.profileDiffKeep', { count: profileDiff.unchangedEnabled.length })}
              </span>
              {profileDiff.missingIds.length > 0 && (
                <span className="md-pill" data-tone="error">
                  {t('organize.profileMissing', { count: profileDiff.missingIds.length })}
                </span>
              )}
              {profileHiddenCount > 0 && (
                <span className="md-pill" data-tone="surface">
                  <EyeOff size={12} />
                  {t('filter.filteredCount', { count: profileHiddenCount })}
                </span>
              )}
            </div>

            {profileDiff.toEnable.length > 0 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                <span className="md-label-medium" style={{ color: 'var(--md-sys-color-primary)' }}>
                  {t('organize.profileDiffEnable', { count: profileDiff.toEnable.length })}
                </span>
                <div className="l4a-list">
                  {profileDiff.toEnable.map((addon) => (
                    <OrganizeMemberRow
                      key={addon.id}
                      addon={addon}
                      namePreference={namePreference}
                      busy={busy}
                      includeHiddenAsRedacted
                      badge={{ text: `+ ${t('common.enable')}`, tone: 'primary' }}
                      onSelectAddon={onSelectAddon}
                      onOpenInFileManager={onOpenInFileManager}
                      onToggleSingle={onToggleSingle}
                    />
                  ))}
                </div>
              </div>
            )}

            {profileDiff.toDisable.length > 0 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                <span
                  className="md-label-medium"
                  style={{ color: 'var(--md-sys-color-tertiary)' }}
                >
                  {t('organize.profileDiffDisable', { count: profileDiff.toDisable.length })}
                </span>
                <div className="l4a-list">
                  {profileDiff.toDisable.map((addon) => (
                    <OrganizeMemberRow
                      key={addon.id}
                      addon={addon}
                      namePreference={namePreference}
                      busy={busy}
                      includeHiddenAsRedacted
                      badge={{ text: `- ${t('common.disable')}`, tone: 'tertiary' }}
                      onSelectAddon={onSelectAddon}
                      onOpenInFileManager={onOpenInFileManager}
                      onToggleSingle={onToggleSingle}
                    />
                  ))}
                </div>
              </div>
            )}

            {profileDiff.unchangedEnabled.length > 0 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                <span
                  className="md-label-medium"
                  style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                >
                  {t('organize.profileDiffKeep', { count: profileDiff.unchangedEnabled.length })}
                </span>
                <div className="l4a-list">
                  {profileDiff.unchangedEnabled.map((addon) => (
                    <OrganizeMemberRow
                      key={addon.id}
                      addon={addon}
                      namePreference={namePreference}
                      busy={busy}
                      includeHiddenAsRedacted
                      onSelectAddon={onSelectAddon}
                      onOpenInFileManager={onOpenInFileManager}
                      onToggleSingle={onToggleSingle}
                    />
                  ))}
                </div>
              </div>
            )}
          </div>
        )}
      </SideSheet>
    </>
  );
}
