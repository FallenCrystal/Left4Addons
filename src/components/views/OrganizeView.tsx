import { useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Bookmark,
  Download,
  ExternalLink,
  FolderKanban,
  Layers,
  Lock,
  Map as MapIcon,
  Pencil,
  Play,
  Plus,
  Power,
  PowerOff,
  RefreshCw,
  SlidersHorizontal,
  Trash2,
  Unlock,
} from 'lucide-react';
import type {
  AddonSummary,
  BundleRecord,
  LoadoutProfile,
  TrackedCollectionRecord,
  UserGroup,
} from '../../api/ipc';
import { formatTimestamp } from '../../utils/format';
import { AddonCover } from '../m3/AddonCover';
import {
  Button,
  EmptyState,
  IconButton,
  LinearProgress,
  Tabs,
} from '../m3/primitives';
import type { ExternalAddonFilter } from './LibraryView';

export type OrganizeTab = 'bundles' | 'collections' | 'groups' | 'profiles';

export interface OrganizeViewProps {
  addons: AddonSummary[];
  bundles: BundleRecord[];
  collections: TrackedCollectionRecord[];
  userGroups: UserGroup[];
  profiles: LoadoutProfile[];
  busy: boolean;
  onRebuildBundles: () => void;
  onCreateBundle: () => void;
  onDeleteBundle: (bundleId: string) => void;
  onToggleBundleLock: (bundleId: string, locked: boolean) => void;
  onTrackCollection: () => void;
  onUntrackCollection: (collectionId: string) => void;
  onDownloadWorkshopItem: (workshopId: string) => void;
  onCreateGroup: () => void;
  onEditGroup: (group: UserGroup) => void;
  onDeleteGroup: (groupId: string) => void;
  onCreateProfile: () => void;
  onEditProfile: (profile: LoadoutProfile) => void;
  onDeleteProfile: (profileId: string) => void;
  onApplyProfile: (profileId: string) => void;
  onBulkToggle: (addonIds: string[], enabled: boolean) => void;
  onOpenFilterInLibrary: (filter: ExternalAddonFilter) => void;
}

export function OrganizeView({
  addons,
  bundles,
  collections,
  userGroups,
  profiles,
  busy,
  onRebuildBundles,
  onCreateBundle,
  onDeleteBundle,
  onToggleBundleLock,
  onTrackCollection,
  onUntrackCollection,
  onDownloadWorkshopItem,
  onCreateGroup,
  onEditGroup,
  onDeleteGroup,
  onCreateProfile,
  onEditProfile,
  onDeleteProfile,
  onApplyProfile,
  onBulkToggle,
  onOpenFilterInLibrary,
}: OrganizeViewProps) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<OrganizeTab>('bundles');
  const tabsId = useId();

  const installedWorkshopIds = useMemo(
    () =>
      new Set(
        addons
          .filter((a) => a.installState === 'installed' && a.workshopId)
          .map((a) => a.workshopId!),
      ),
    [addons],
  );

  const loadingAddonIds = useMemo(
    () =>
      new Set(
        addons
          .filter((a) => a.installState === 'installed' && a.primaryDir === 'loading')
          .map((a) => a.id),
      ),
    [addons],
  );

  return (
    <div className="l4a-view">
      <header className="l4a-view-header">
        <h1 className="md-headline-small">{t('nav.organize')}</h1>
        <div className="l4a-spacer" />
        {tab === 'bundles' && (
          <>
            <Button
              variant="outlined"
              disabled={busy}
              icon={<RefreshCw size={15} />}
              onClick={onRebuildBundles}
            >
              {t('organize.rebuild')}
            </Button>
            <Button variant="filled" icon={<Plus size={16} />} onClick={onCreateBundle}>
              {t('organize.newBundle')}
            </Button>
          </>
        )}
        {tab === 'collections' && (
          <Button variant="filled" icon={<Plus size={16} />} onClick={onTrackCollection}>
            {t('organize.newCollection')}
          </Button>
        )}
        {tab === 'groups' && (
          <Button variant="filled" icon={<Plus size={16} />} onClick={onCreateGroup}>
            {t('organize.newGroup')}
          </Button>
        )}
        {tab === 'profiles' && (
          <Button variant="filled" icon={<Plus size={16} />} onClick={onCreateProfile}>
            {t('organize.newProfile')}
          </Button>
        )}
      </header>

      <Tabs
        idPrefix={tabsId}
        value={tab}
        onChange={setTab}
        tabs={[
          { value: 'bundles', label: t('organize.bundles'), count: bundles.length },
          { value: 'collections', label: t('organize.collections'), count: collections.length },
          { value: 'groups', label: t('organize.groups'), count: userGroups.length },
          { value: 'profiles', label: t('organize.profiles'), count: profiles.length },
        ]}
      />

      <div
        className="l4a-view-scroll"
        style={{ paddingTop: 16 }}
        role="tabpanel"
        id={`${tabsId}-panel-${tab}`}
        aria-labelledby={`${tabsId}-tab-${tab}`}
      >
        {tab === 'bundles' &&
          (bundles.length === 0 ? (
            <EmptyState icon={<Layers size={26} />} title={t('common.empty')} />
          ) : (
            <div className="l4a-card-grid-wide">
              {bundles.map((bundle) => {
                const toggleable = bundle.memberAddonIds.filter((id) => loadingAddonIds.has(id));
                return (
                  <article key={bundle.id} className="md-card">
                    <div className="l4a-addon-cover-wrap" style={{ aspectRatio: '21 / 9' }}>
                      <AddonCover src={bundle.coverImage} title={bundle.name} category="Campaign" />
                      <div className="l4a-cover-overlay-top">
                        <span className="md-pill" data-tone="secondary">
                          {t(`organize.bundleKind.${bundle.kind}`)}
                        </span>
                        <IconButton
                          size="sm"
                          variant={bundle.locked ? 'filled' : 'tonal'}
                          label={bundle.locked ? t('organize.unlock') : t('organize.lock')}
                          disabled={busy}
                          onClick={() => onToggleBundleLock(bundle.id, !bundle.locked)}
                        >
                          {bundle.locked ? <Lock size={14} /> : <Unlock size={14} />}
                        </IconButton>
                      </div>
                    </div>
                    <div className="l4a-addon-card-body">
                      <div className="md-title-medium l4a-truncate">{bundle.name}</div>
                      <div className="md-chip-row">
                        <span className="md-pill" data-tone="surface">
                          {t('common.items', { count: bundle.memberAddonIds.length })}
                        </span>
                        {bundle.maps.length > 0 && (
                          <span className="md-pill" data-tone="surface">
                            <MapIcon size={11} />
                            {t('common.maps', { count: bundle.maps.length })}
                          </span>
                        )}
                      </div>
                      <div className="l4a-addon-card-footer">
                        <div className="l4a-row-wrap" style={{ gap: 4 }}>
                          <IconButton
                            size="sm"
                            label={t('common.enable')}
                            disabled={busy || toggleable.length === 0}
                            onClick={() => onBulkToggle(toggleable, true)}
                          >
                            <Power size={15} />
                          </IconButton>
                          <IconButton
                            size="sm"
                            label={t('common.disable')}
                            disabled={busy || toggleable.length === 0}
                            onClick={() => onBulkToggle(toggleable, false)}
                          >
                            <PowerOff size={15} />
                          </IconButton>
                          {bundle.kind === 'manual' && (
                            <IconButton
                              size="sm"
                              label={t('common.delete')}
                              disabled={busy}
                              onClick={() => onDeleteBundle(bundle.id)}
                            >
                              <Trash2 size={15} />
                            </IconButton>
                          )}
                        </div>
                        <Button
                          variant="text"
                          size="sm"
                          icon={<ExternalLink size={14} />}
                          onClick={() =>
                            onOpenFilterInLibrary({
                              label: bundle.name,
                              addonIds: bundle.memberAddonIds,
                            })
                          }
                        >
                          {t('organize.filterInLibrary')}
                        </Button>
                      </div>
                    </div>
                  </article>
                );
              })}
            </div>
          ))}

        {tab === 'collections' &&
          (collections.length === 0 ? (
            <EmptyState icon={<Bookmark size={26} />} title={t('common.empty')} />
          ) : (
            <div className="l4a-card-grid-wide">
              {collections.map((col) => {
                const total = col.memberWorkshopIds.length;
                const missing = col.memberWorkshopIds.filter(
                  (id) => !installedWorkshopIds.has(id),
                );
                const installed = total - missing.length;
                const percent = total > 0 ? Math.round((installed / total) * 100) : 0;

                return (
                  <article key={col.collectionId} className="md-card" style={{ padding: 16, gap: 12 }}>
                    <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                      <span className="md-title-medium l4a-truncate" style={{ flex: 1 }}>
                        {col.title}
                      </span>
                      <span className="md-pill md-mono" data-tone="surface">
                        #{col.collectionId}
                      </span>
                    </div>

                    <div className="md-chip-row">
                      {col.autoSync && (
                        <span className="md-pill" data-tone="primary">
                          {t('organize.autoSync')}
                        </span>
                      )}
                      <span className="md-pill" data-tone="secondary">
                        {t('organize.installedRatio', { installed, total })}
                      </span>
                      {col.removedUpstreamIds.length > 0 && (
                        <span className="md-pill" data-tone="error">
                          {t('organize.upstreamRemoved', { count: col.removedUpstreamIds.length })}
                        </span>
                      )}
                    </div>

                    <LinearProgress value={percent} label={col.title} />

                    <div className="l4a-addon-card-footer">
                      <span className="md-body-small" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                        {col.lastSyncedAt ? formatTimestamp(col.lastSyncedAt) : '—'}
                      </span>
                      <div className="l4a-row-wrap" style={{ gap: 4 }}>
                        {missing.length > 0 && (
                          <Button
                            variant="tonal"
                            size="sm"
                            icon={<Download size={13} />}
                            onClick={() => {
                              for (const id of missing) onDownloadWorkshopItem(id);
                            }}
                          >
                            {t('organize.downloadMissing', { count: missing.length })}
                          </Button>
                        )}
                        <IconButton
                          size="sm"
                          label={t('organize.filterInLibrary')}
                          onClick={() =>
                            onOpenFilterInLibrary({
                              label: col.title,
                              workshopIds: col.memberWorkshopIds,
                            })
                          }
                        >
                          <ExternalLink size={15} />
                        </IconButton>
                        <IconButton
                          size="sm"
                          label={t('organize.untrack')}
                          disabled={busy}
                          onClick={() => onUntrackCollection(col.collectionId)}
                        >
                          <Trash2 size={15} />
                        </IconButton>
                      </div>
                    </div>
                  </article>
                );
              })}
            </div>
          ))}

        {tab === 'groups' &&
          (userGroups.length === 0 ? (
            <EmptyState icon={<FolderKanban size={26} />} title={t('common.empty')} />
          ) : (
            <div className="l4a-card-grid-wide">
              {userGroups.map((group) => {
                const toggleable = group.addonIds.filter((id) => loadingAddonIds.has(id));
                return (
                  <article key={group.id} className="md-card" style={{ padding: 16, gap: 12 }}>
                    <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                      <div className="l4a-row-wrap" style={{ minWidth: 0, flex: 1 }}>
                        <span
                          style={{
                            width: 12,
                            height: 12,
                            borderRadius: 999,
                            backgroundColor: group.color || 'var(--md-sys-color-primary)',
                            flexShrink: 0,
                          }}
                        />
                        <span className="md-title-medium l4a-truncate">{group.name}</span>
                      </div>
                      <span className="md-pill" data-tone="secondary">
                        {t('common.items', { count: group.addonIds.length })}
                      </span>
                    </div>

                    {group.tags.length > 0 && (
                      <div className="md-chip-row">
                        {group.tags.map((tag) => (
                          <span key={tag} className="md-pill" data-tone="surface">
                            {tag}
                          </span>
                        ))}
                      </div>
                    )}

                    <div className="l4a-addon-card-footer">
                      <div className="l4a-row-wrap" style={{ gap: 4 }}>
                        <IconButton
                          size="sm"
                          label={t('common.enable')}
                          disabled={busy || toggleable.length === 0}
                          onClick={() => onBulkToggle(toggleable, true)}
                        >
                          <Power size={15} />
                        </IconButton>
                        <IconButton
                          size="sm"
                          label={t('common.disable')}
                          disabled={busy || toggleable.length === 0}
                          onClick={() => onBulkToggle(toggleable, false)}
                        >
                          <PowerOff size={15} />
                        </IconButton>
                        <IconButton
                          size="sm"
                          label={t('organize.filterInLibrary')}
                          onClick={() =>
                            onOpenFilterInLibrary({
                              label: group.name,
                              addonIds: group.addonIds,
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
                  </article>
                );
              })}
            </div>
          ))}

        {tab === 'profiles' &&
          (profiles.length === 0 ? (
            <EmptyState icon={<SlidersHorizontal size={26} />} title={t('common.empty')} />
          ) : (
            <div className="l4a-card-grid-wide">
              {profiles.map((profile) => (
                <article key={profile.id} className="md-card" style={{ padding: 16, gap: 12 }}>
                  <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                    <span className="md-title-medium l4a-truncate" style={{ flex: 1 }}>
                      {profile.name}
                    </span>
                    <span className="md-pill" data-tone="primary">
                      {t('common.items', { count: profile.enabledAddonIds.length })}
                    </span>
                  </div>

                  {profile.description && (
                    <span className="md-body-small" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                      {profile.description}
                    </span>
                  )}

                  <div className="l4a-addon-card-footer">
                    <div className="l4a-row-wrap" style={{ gap: 4 }}>
                      <IconButton
                        size="sm"
                        label={t('common.edit')}
                        onClick={() => onEditProfile(profile)}
                      >
                        <Pencil size={15} />
                      </IconButton>
                      <IconButton
                        size="sm"
                        label={t('common.delete')}
                        disabled={busy}
                        onClick={() => onDeleteProfile(profile.id)}
                      >
                        <Trash2 size={15} />
                      </IconButton>
                    </div>
                    <Button
                      variant="filled"
                      size="sm"
                      disabled={busy}
                      icon={<Play size={14} />}
                      onClick={() => onApplyProfile(profile.id)}
                    >
                      {t('common.apply')}
                    </Button>
                  </div>
                </article>
              ))}
            </div>
          ))}
      </div>
    </div>
  );
}
