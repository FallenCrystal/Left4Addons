import { useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Bookmark,
  Copy,
  Download,
  ExternalLink,
  FolderKanban,
  FolderOpen,
  Layers,
  Lock,
  Map as MapIcon,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Search,
  SlidersHorizontal,
  Trash2,
  Unlock,
  X,
} from 'lucide-react';
import type {
  AddonNamePreference,
  AddonSummary,
  BundleRecord,
  LoadoutProfile,
  TrackedCollectionRecord,
  UserGroup,
} from '../../api/ipc';
import {
  formatAddonDisplayName,
  formatBundleMemberLabel,
  formatBytes,
  formatTimestamp,
  matchesAddonSearch,
} from '../../utils/format';
import { AddonCover } from '../m3/AddonCover';
import {
  Button,
  EmptyState,
  IconButton,
  LinearProgress,
  SideSheet,
  Switch,
  Tabs,
} from '../m3/primitives';
import type { ExternalAddonFilter } from './LibraryView';

export type OrganizeTab = 'bundles' | 'collections' | 'groups' | 'profiles';

type InspectedTarget =
  | { kind: 'bundle'; id: string }
  | { kind: 'collection'; id: string }
  | { kind: 'group'; id: string }
  | { kind: 'profile'; id: string }
  | null;

export interface OrganizeViewProps {
  addons: AddonSummary[];
  namePreference?: AddonNamePreference;
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
  onDownloadWorkshopItem: (workshopId: string, title?: string) => void;
  onCreateGroup: () => void;
  onEditGroup: (group: UserGroup) => void;
  onSaveUserGroup: (group: UserGroup) => Promise<boolean>;
  onDeleteGroup: (groupId: string) => void;
  onCreateProfile: () => void;
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

function summarizeGroupMembers(memberIds: string[], addonsById: Map<string, AddonSummary>) {
  const members: AddonSummary[] = [];
  for (const id of memberIds) {
    const a = addonsById.get(id);
    if (a) members.push(a);
  }
  const installed = members.filter((a) => a.installState === 'installed');
  const loading = installed.filter((a) => a.primaryDir === 'loading');
  const enabledLoading = loading.filter((a) => a.enabled);
  const totalSize = installed.reduce((sum, a) => sum + a.fileSize, 0);
  const firstCover =
    members.find((a) => a.maps.length > 0 && a.coverImage)?.coverImage ??
    members.find((a) => a.coverImage)?.coverImage;
  const firstLocalCover =
    members.find((a) => a.maps.length > 0 && a.localCoverImage)?.localCoverImage ??
    members.find((a) => a.localCoverImage)?.localCoverImage;
  const firstPath = installed.find((a) => a.currentPath)?.currentPath;

  const allEnabled = loading.length > 0 && enabledLoading.length === loading.length;
  const noneEnabled = enabledLoading.length === 0;
  return {
    members,
    installed,
    loading,
    enabledLoading,
    totalSize,
    firstCover,
    firstLocalCover,
    firstPath,
    allEnabled,
    noneEnabled,
  };
}

export function OrganizeView({
  addons,
  namePreference = 'workshop_title',
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
  onSaveUserGroup,
  onDeleteGroup,
  onCreateProfile,
  onEditProfile,
  onDeleteProfile,
  onApplyProfile,
  onBulkToggle,
  onToggleSingle,
  onSelectAddon,
  onOpenInFileManager,
  onOpenFilterInLibrary,
  onCopy,
}: OrganizeViewProps) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<OrganizeTab>('bundles');
  const [query, setQuery] = useState('');
  const [inspected, setInspected] = useState<InspectedTarget>(null);
  const [bundleSheetTab, setBundleSheetTab] = useState<'members' | 'maps'>('members');
  const tabsId = useId();
  const bundleTabsId = useId();

  const addonsById = useMemo(() => {
    const map = new Map<string, AddonSummary>();
    for (const a of addons) map.set(a.id, a);
    return map;
  }, [addons]);

  const addonsByWorkshopId = useMemo(() => {
    const map = new Map<string, AddonSummary>();
    for (const a of addons) {
      if (a.workshopId) map.set(a.workshopId, a);
    }
    return map;
  }, [addons]);

  const loadingAddons = useMemo(
    () => addons.filter((a) => a.installState === 'installed' && a.primaryDir === 'loading'),
    [addons],
  );

  const needle = query.trim().toLowerCase();

  const filteredBundles = useMemo(() => {
    if (!needle) return bundles;
    return bundles.filter((b) => {
      if (b.name.toLowerCase().includes(needle)) return true;
      return b.memberAddonIds.some((id) => {
        const addon = addonsById.get(id);
        return addon ? matchesAddonSearch(addon, needle) : false;
      });
    });
  }, [bundles, needle, addonsById]);

  const filteredCollections = useMemo(() => {
    if (!needle) return collections;
    return collections.filter((c) => {
      if (c.title.toLowerCase().includes(needle) || c.collectionId.includes(needle)) return true;
      return c.memberWorkshopIds.some((wid) => {
        const addon = addonsByWorkshopId.get(wid);
        return addon ? matchesAddonSearch(addon, needle) : false;
      });
    });
  }, [collections, needle, addonsByWorkshopId]);

  const filteredGroups = useMemo(() => {
    if (!needle) return userGroups;
    return userGroups.filter((g) => {
      if (g.name.toLowerCase().includes(needle)) return true;
      if (g.tags.some((tag) => tag.toLowerCase().includes(needle))) return true;
      return g.addonIds.some((id) => {
        const addon = addonsById.get(id);
        return addon ? matchesAddonSearch(addon, needle) : false;
      });
    });
  }, [userGroups, needle, addonsById]);

  const filteredProfiles = useMemo(() => {
    if (!needle) return profiles;
    return profiles.filter((p) => {
      if (p.name.toLowerCase().includes(needle) || p.description.toLowerCase().includes(needle)) {
        return true;
      }
      return p.enabledAddonIds.some((id) => {
        const addon = addonsById.get(id);
        return addon ? matchesAddonSearch(addon, needle) : false;
      });
    });
  }, [profiles, needle, addonsById]);

  const inspectedBundle =
    inspected?.kind === 'bundle' ? (bundles.find((b) => b.id === inspected.id) ?? null) : null;
  const inspectedCollection =
    inspected?.kind === 'collection'
      ? (collections.find((c) => c.collectionId === inspected.id) ?? null)
      : null;
  const inspectedGroup =
    inspected?.kind === 'group' ? (userGroups.find((g) => g.id === inspected.id) ?? null) : null;
  const inspectedProfile =
    inspected?.kind === 'profile' ? (profiles.find((p) => p.id === inspected.id) ?? null) : null;

  const computeProfileDiff = (profile: LoadoutProfile) => {
    const desired = new Set(profile.enabledAddonIds);
    const toEnable: AddonSummary[] = [];
    const toDisable: AddonSummary[] = [];
    const unchangedEnabled: AddonSummary[] = [];
    const missingIds: string[] = [];

    for (const id of desired) {
      const found = addonsById.get(id);
      if (!found || found.installState !== 'installed') {
        missingIds.push(id);
      }
    }
    for (const addon of loadingAddons) {
      const want = desired.has(addon.id);
      if (want && !addon.enabled) toEnable.push(addon);
      else if (!want && addon.enabled) toDisable.push(addon);
      else if (want && addon.enabled) unchangedEnabled.push(addon);
    }
    return { toEnable, toDisable, unchangedEnabled, missingIds };
  };

  const renderStatusPill = (summary: ReturnType<typeof summarizeGroupMembers>) => {
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
  };

  const renderMemberRow = (
    addon: AddonSummary,
    options?: {
      displayLabel?: string;
      indexBadge?: number;
      showFileName?: boolean;
      onRemove?: () => void;
      badge?: { text: string; tone: 'primary' | 'secondary' | 'tertiary' | 'error' | 'surface' };
    },
  ) => {
    const installed = addon.installState === 'installed';
    const displayLabel = options?.displayLabel ?? formatAddonDisplayName(addon, namePreference);
    return (
      <div
        key={addon.id}
        className="l4a-list-row l4a-list-row-dense"
        data-muted={installed && !addon.enabled ? 'true' : 'false'}
      >
        <div className="l4a-picker-thumb">
          <AddonCover
            compact
            src={addon.coverImage}
            fallbackSrc={addon.localCoverImage}
            title={displayLabel}
            category={addon.categories[0]}
          />
        </div>
        <button
          type="button"
          className="l4a-card-trigger l4a-list-main"
          onClick={() => onSelectAddon(addon)}
        >
          <div className="l4a-row-wrap" style={{ gap: 6 }}>
            {options?.indexBadge !== undefined && (
              <span className="md-pill md-mono" data-tone="secondary">
                #{options.indexBadge}
              </span>
            )}
            <span className="md-title-small l4a-truncate">{displayLabel}</span>
            {options?.badge && (
              <span className="md-pill" data-tone={options.badge.tone}>
                {options.badge.text}
              </span>
            )}
          </div>
          <div className="l4a-addon-meta">
            {options?.showFileName && addon.currentFileName ? (
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
              label={displayLabel}
              onChange={(next) => onToggleSingle(addon, next)}
            />
          )}
          {options?.onRemove && (
            <IconButton
              size="sm"
              label={t('organize.removeMember')}
              disabled={busy}
              onClick={options.onRemove}
            >
              <X size={15} />
            </IconButton>
          )}
        </div>
      </div>
    );
  };

  return (
    <div className="l4a-view">
      <header className="l4a-view-header">
        <h1 className="md-headline-small">{t('nav.organize')}</h1>
        <div className="md-search-bar" style={{ maxWidth: 360, height: 40, minWidth: 180 }}>
          <Search
            size={16}
            style={{ color: 'var(--md-sys-color-on-surface-variant)', flexShrink: 0 }}
          />
          <input
            className="md-search-input"
            style={{ fontSize: 13 }}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t('organize.searchPlaceholder')}
            aria-label={t('common.search')}
          />
          {query && (
            <IconButton size="sm" label={t('common.clear')} onClick={() => setQuery('')}>
              <X size={14} />
            </IconButton>
          )}
        </div>
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
          (filteredBundles.length === 0 ? (
            <EmptyState
              icon={<Layers size={26} />}
              title={bundles.length === 0 ? t('common.empty') : t('common.noResults')}
            />
          ) : (
            <div className="l4a-card-grid-wide">
              {filteredBundles.map((bundle) => {
                const summary = summarizeGroupMembers(bundle.memberAddonIds, addonsById);
                const toggleableIds = summary.loading.map((a) => a.id);
                return (
                  <article
                    key={bundle.id}
                    className="md-card"
                    data-clickable="true"
                    data-muted={summary.loading.length > 0 && summary.noneEnabled ? 'true' : 'false'}
                  >
                    <div className="l4a-addon-cover-wrap" style={{ aspectRatio: '21 / 9' }}>
                      <button
                        type="button"
                        tabIndex={-1}
                        aria-hidden="true"
                        className="l4a-card-trigger"
                        style={{ height: '100%' }}
                        onClick={() => setInspected({ kind: 'bundle', id: bundle.id })}
                      >
                        <AddonCover
                          src={bundle.coverImage ?? summary.firstCover}
                          fallbackSrc={summary.firstLocalCover}
                          title={bundle.name}
                          category="Campaign"
                        />
                      </button>
                      <div className="l4a-cover-overlay-top">
                        <span className="md-pill" data-tone="secondary">
                          {t(`organize.bundleKind.${bundle.kind}`)}
                        </span>
                        <div className="l4a-row-wrap" style={{ gap: 6 }}>
                          {renderStatusPill(summary)}
                          <IconButton
                            size="sm"
                            variant={bundle.locked ? 'filled' : 'tonal'}
                            label={bundle.locked ? t('organize.unlock') : t('organize.lock')}
                            disabled={busy}
                            onClick={(e) => {
                              e.stopPropagation();
                              onToggleBundleLock(bundle.id, !bundle.locked);
                            }}
                          >
                            {bundle.locked ? <Lock size={14} /> : <Unlock size={14} />}
                          </IconButton>
                        </div>
                      </div>
                    </div>

                    <div className="l4a-addon-card-body">
                      <button
                        type="button"
                        className="l4a-card-trigger"
                        onClick={() => setInspected({ kind: 'bundle', id: bundle.id })}
                      >
                        <div className="md-title-medium l4a-truncate">{bundle.name}</div>
                        <div className="l4a-addon-meta" style={{ marginTop: 4 }}>
                          <span>{t('common.items', { count: bundle.memberAddonIds.length })}</span>
                          {summary.totalSize > 0 && (
                            <>
                              <span>·</span>
                              <span>{formatBytes(summary.totalSize)}</span>
                            </>
                          )}
                          {bundle.maps.length > 0 && (
                            <>
                              <span>·</span>
                              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 3 }}>
                                <MapIcon size={11} />
                                {bundle.maps.length}
                              </span>
                            </>
                          )}
                        </div>
                        {summary.members.length > 0 && (
                          <div className="l4a-member-preview">
                            {summary.members.slice(0, 3).map((m) => (
                              <span key={m.id} className="l4a-member-chip l4a-truncate">
                                {formatBundleMemberLabel(
                                  m,
                                  summary.members,
                                  namePreference,
                                  bundle.name,
                                )}
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
          (filteredCollections.length === 0 ? (
            <EmptyState
              icon={<Bookmark size={26} />}
              title={collections.length === 0 ? t('common.empty') : t('common.noResults')}
            />
          ) : (
            <div className="l4a-card-grid-wide">
              {filteredCollections.map((col) => {
                const total = col.memberWorkshopIds.length;
                const missing = col.memberWorkshopIds.filter(
                  (id) => addonsByWorkshopId.get(id)?.installState !== 'installed',
                );
                const installed = total - missing.length;
                const percent = total > 0 ? Math.round((installed / total) * 100) : 0;
                const previewTitles = col.memberWorkshopIds.slice(0, 3).map((wid) => {
                  const found = addonsByWorkshopId.get(wid);
                  return found ? formatAddonDisplayName(found, namePreference) : `#${wid}`;
                });

                return (
                  <article
                    key={col.collectionId}
                    className="md-card"
                    data-clickable="true"
                    style={{ padding: 16, gap: 10 }}
                  >
                    <button
                      type="button"
                      className="l4a-card-trigger"
                      onClick={() => setInspected({ kind: 'collection', id: col.collectionId })}
                    >
                      <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                        <span className="md-title-medium l4a-truncate" style={{ flex: 1 }}>
                          {col.title}
                        </span>
                        <span className="md-pill md-mono" data-tone="surface">
                          #{col.collectionId}
                        </span>
                      </div>

                      <div className="md-chip-row" style={{ marginTop: 8 }}>
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
                            {t('organize.upstreamRemoved', {
                              count: col.removedUpstreamIds.length,
                            })}
                          </span>
                        )}
                      </div>

                      <div style={{ marginTop: 10 }}>
                        <LinearProgress value={percent} label={col.title} />
                      </div>

                      {previewTitles.length > 0 && (
                        <div className="l4a-member-preview">
                          {previewTitles.map((label, idx) => (
                            <span key={`${col.collectionId}-${idx}`} className="l4a-member-chip l4a-truncate">
                              {label}
                            </span>
                          ))}
                          {total > 3 && (
                            <span className="l4a-member-chip">
                              {t('organize.moreMembers', { count: total - 3 })}
                            </span>
                          )}
                        </div>
                      )}
                    </button>

                    <div className="l4a-addon-card-footer">
                      <span
                        className="md-body-small"
                        style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                      >
                        {col.lastSyncedAt ? formatTimestamp(col.lastSyncedAt) : '—'}
                      </span>
                      <div className="l4a-row-wrap" style={{ gap: 4 }}>
                        {missing.length > 0 && (
                          <Button
                            variant="tonal"
                            size="sm"
                            icon={<Download size={13} />}
                            onClick={() => {
                              for (const id of missing) {
                                onDownloadWorkshopItem(id, addonsByWorkshopId.get(id)?.title);
                              }
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
          (filteredGroups.length === 0 ? (
            <EmptyState
              icon={<FolderKanban size={26} />}
              title={userGroups.length === 0 ? t('common.empty') : t('common.noResults')}
            />
          ) : (
            <div className="l4a-card-grid-wide">
              {filteredGroups.map((group) => {
                const summary = summarizeGroupMembers(group.addonIds, addonsById);
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
                        onClick={() => setInspected({ kind: 'group', id: group.id })}
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
                          {renderStatusPill(summary)}
                        </div>

                        <div className="l4a-addon-meta" style={{ marginTop: 6 }}>
                          <span>{t('common.items', { count: group.addonIds.length })}</span>
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
                                {formatAddonDisplayName(m, namePreference)}
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
                    </div>
                  </article>
                );
              })}
            </div>
          ))}

        {tab === 'profiles' &&
          (filteredProfiles.length === 0 ? (
            <EmptyState
              icon={<SlidersHorizontal size={26} />}
              title={profiles.length === 0 ? t('common.empty') : t('common.noResults')}
            />
          ) : (
            <div className="l4a-card-grid-wide">
              {filteredProfiles.map((profile) => {
                const diff = computeProfileDiff(profile);
                const noChange = diff.toEnable.length === 0 && diff.toDisable.length === 0;
                const previewMembers = profile.enabledAddonIds
                  .map((id) => addonsById.get(id))
                  .filter((a): a is AddonSummary => Boolean(a));

                return (
                  <article
                    key={profile.id}
                    className="md-card"
                    data-clickable="true"
                    style={{ padding: 16, gap: 10 }}
                  >
                    <button
                      type="button"
                      className="l4a-card-trigger"
                      onClick={() => setInspected({ kind: 'profile', id: profile.id })}
                    >
                      <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                        <span className="md-title-medium l4a-truncate" style={{ flex: 1 }}>
                          {profile.name}
                        </span>
                        <span className="md-pill" data-tone="primary">
                          {t('common.items', { count: profile.enabledAddonIds.length })}
                        </span>
                      </div>

                      {profile.description && (
                        <div
                          className="md-body-small"
                          style={{
                            color: 'var(--md-sys-color-on-surface-variant)',
                            marginTop: 4,
                          }}
                        >
                          {profile.description}
                        </div>
                      )}

                      <div className="md-chip-row" style={{ marginTop: 8 }}>
                        {noChange ? (
                          <span className="md-pill" data-tone="surface">
                            {t('organize.profileDiffNoChange')}
                          </span>
                        ) : (
                          <>
                            {diff.toEnable.length > 0 && (
                              <span className="md-pill" data-tone="primary">
                                {t('organize.profileDiffEnable', { count: diff.toEnable.length })}
                              </span>
                            )}
                            {diff.toDisable.length > 0 && (
                              <span className="md-pill" data-tone="tertiary">
                                {t('organize.profileDiffDisable', { count: diff.toDisable.length })}
                              </span>
                            )}
                          </>
                        )}
                        {diff.missingIds.length > 0 && (
                          <span className="md-pill" data-tone="error">
                            {t('organize.profileMissing', { count: diff.missingIds.length })}
                          </span>
                        )}
                      </div>

                      {previewMembers.length > 0 && (
                        <div className="l4a-member-preview">
                          {previewMembers.slice(0, 3).map((m) => (
                            <span key={m.id} className="l4a-member-chip l4a-truncate">
                              {formatAddonDisplayName(m, namePreference)}
                            </span>
                          ))}
                          {previewMembers.length > 3 && (
                            <span className="l4a-member-chip">
                              {t('organize.moreMembers', { count: previewMembers.length - 3 })}
                            </span>
                          )}
                        </div>
                      )}
                    </button>

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
                        variant={noChange ? 'tonal' : 'filled'}
                        size="sm"
                        disabled={busy}
                        icon={<Play size={14} />}
                        onClick={() => onApplyProfile(profile.id)}
                      >
                        {t('common.apply')}
                      </Button>
                    </div>
                  </article>
                );
              })}
            </div>
          ))}
      </div>

      {/* Inline Bundle Inspector */}
      <SideSheet
        open={Boolean(inspectedBundle)}
        title={inspectedBundle?.name ?? ''}
        onClose={() => setInspected(null)}
        headerTrailing={
          inspectedBundle && (
            <IconButton
              size="sm"
              variant={inspectedBundle.locked ? 'filled' : 'tonal'}
              label={inspectedBundle.locked ? t('organize.unlock') : t('organize.lock')}
              disabled={busy}
              onClick={() => onToggleBundleLock(inspectedBundle.id, !inspectedBundle.locked)}
            >
              {inspectedBundle.locked ? <Lock size={15} /> : <Unlock size={15} />}
            </IconButton>
          )
        }
        footer={
          inspectedBundle && (
            <>
              <Button
                variant="outlined"
                size="sm"
                icon={<ExternalLink size={14} />}
                onClick={() => {
                  onOpenFilterInLibrary({
                    label: inspectedBundle.name,
                    addonIds: inspectedBundle.memberAddonIds,
                  });
                  setInspected(null);
                }}
              >
                {t('organize.filterInLibrary')}
              </Button>
              {inspectedBundle.kind === 'manual' && (
                <Button
                  variant="text"
                  tone="error"
                  size="sm"
                  disabled={busy}
                  icon={<Trash2 size={14} />}
                  onClick={() => {
                    onDeleteBundle(inspectedBundle.id);
                    setInspected(null);
                  }}
                >
                  {t('common.delete')}
                </Button>
              )}
            </>
          )
        }
      >
        {inspectedBundle &&
          (() => {
            const summary = summarizeGroupMembers(inspectedBundle.memberAddonIds, addonsById);
            const toggleableIds = summary.loading.map((a) => a.id);
            const hasBundleMaps = inspectedBundle.maps.length > 0;
            const activeBundleTab: 'members' | 'maps' = hasBundleMaps ? bundleSheetTab : 'members';
            return (
              <>
                {hasBundleMaps && (
                  <Tabs
                    compact
                    idPrefix={bundleTabsId}
                    value={activeBundleTab}
                    onChange={setBundleSheetTab}
                    tabs={[
                      {
                        value: 'members',
                        label: t('organize.members'),
                        count: summary.members.length,
                      },
                      {
                        value: 'maps',
                        label: t('detail.maps'),
                        count: inspectedBundle.maps.length,
                      },
                    ]}
                  />
                )}
                <div
                  className="md-sheet-body"
                  role={hasBundleMaps ? 'tabpanel' : undefined}
                  id={hasBundleMaps ? `${bundleTabsId}-panel-${activeBundleTab}` : undefined}
                  aria-labelledby={
                    hasBundleMaps ? `${bundleTabsId}-tab-${activeBundleTab}` : undefined
                  }
                >
                  {activeBundleTab === 'members' ? (
                    <>
                      <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                        <div className="md-chip-row">
                          <span className="md-pill" data-tone="secondary">
                            {t(`organize.bundleKind.${inspectedBundle.kind}`)}
                          </span>
                          {renderStatusPill(summary)}
                          <span className="md-pill" data-tone="surface">
                            {formatBytes(summary.totalSize)}
                          </span>
                        </div>
                        {toggleableIds.length > 0 && (
                          <Switch
                            compact
                            checked={summary.allEnabled}
                            disabled={busy}
                            label={t('organize.toggleAll')}
                            onChange={(next) => onBulkToggle(toggleableIds, next)}
                          />
                        )}
                      </div>

                      <div className="l4a-list">
                        {summary.members.length === 0 ? (
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
                          summary.members.map((addon, idx) =>
                            renderMemberRow(addon, {
                              displayLabel: formatBundleMemberLabel(
                                addon,
                                summary.members,
                                namePreference,
                                inspectedBundle.name,
                              ),
                              indexBadge: idx + 1,
                              showFileName: true,
                            }),
                          )
                        )}
                      </div>
                    </>
                  ) : (
                    <div className="l4a-list">
                      {inspectedBundle.maps.map((m) => (
                        <div
                          key={m.code}
                          className="l4a-list-row"
                          style={{ minHeight: 56, cursor: 'default' }}
                        >
                          {m.imagePath && (
                            <div className="l4a-list-thumb">
                              <AddonCover
                                compact
                                src={m.imagePath}
                                title={m.name}
                                category="Map"
                              />
                            </div>
                          )}
                          <div className="l4a-list-main">
                            <span className="md-title-small">{m.name}</span>
                            <span
                              className="md-body-small md-mono"
                              style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                            >
                              {m.code}
                            </span>
                          </div>
                          <Button
                            variant="tonal"
                            size="sm"
                            icon={<Copy size={13} />}
                            title={t('detail.copyMapCode')}
                            aria-label={`${t('detail.copyMapCode')} ${m.code}`}
                            onClick={() => onCopy(m.code)}
                          >
                            {t('common.copy')}
                          </Button>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              </>
            );
          })()}
      </SideSheet>

      {/* Inline Collection Inspector */}
      <SideSheet
        open={Boolean(inspectedCollection)}
        title={inspectedCollection?.title ?? ''}
        onClose={() => setInspected(null)}
        footer={
          inspectedCollection &&
          (() => {
            const missing = inspectedCollection.memberWorkshopIds.filter(
              (wid) => addonsByWorkshopId.get(wid)?.installState !== 'installed',
            );
            return (
              <>
                {missing.length > 0 && (
                  <Button
                    variant="filled"
                    size="sm"
                    icon={<Download size={14} />}
                    onClick={() => {
                      for (const wid of missing) {
                        onDownloadWorkshopItem(wid, addonsByWorkshopId.get(wid)?.title);
                      }
                    }}
                  >
                    {t('organize.downloadMissing', { count: missing.length })}
                  </Button>
                )}
                <Button
                  variant="outlined"
                  size="sm"
                  icon={<ExternalLink size={14} />}
                  onClick={() => {
                    onOpenFilterInLibrary({
                      label: inspectedCollection.title,
                      workshopIds: inspectedCollection.memberWorkshopIds,
                    });
                    setInspected(null);
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
                    setInspected(null);
                  }}
                >
                  {t('organize.untrack')}
                </Button>
              </>
            );
          })()
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
            </div>

            {inspectedCollection.description && (
              <div className="md-body-small selectable l4a-description-box">
                {inspectedCollection.description}
              </div>
            )}

            <div className="l4a-list">
              {inspectedCollection.memberWorkshopIds.map((wid) => {
                const local = addonsByWorkshopId.get(wid);
                const removed = inspectedCollection.removedUpstreamIds.includes(wid);
                if (local && local.installState === 'installed') {
                  return renderMemberRow(local, {
                    badge: removed
                      ? { text: t('organize.memberRemoved'), tone: 'error' }
                      : { text: t('organize.memberInstalled'), tone: 'primary' },
                  });
                }
                return (
                  <div key={wid} className="l4a-list-row l4a-list-row-dense" style={{ cursor: 'default' }}>
                    <div className="l4a-list-main">
                      <span className="md-title-small l4a-truncate">
                        {local ? formatAddonDisplayName(local, namePreference) : `#${wid}`}
                      </span>
                      <span className="md-body-small md-mono" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                        #{wid}
                      </span>
                    </div>
                    <div className="l4a-list-trailing">
                      <span className="md-pill" data-tone={removed ? 'error' : 'surface'}>
                        {removed ? t('organize.memberRemoved') : t('organize.memberMissing')}
                      </span>
                      {!removed && (
                        <Button
                          variant="tonal"
                          size="sm"
                          icon={<Download size={13} />}
                          onClick={() => onDownloadWorkshopItem(wid, local?.title)}
                        >
                          {t('common.download')}
                        </Button>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        )}
      </SideSheet>

      {/* Inline Group Inspector */}
      <SideSheet
        open={Boolean(inspectedGroup)}
        title={inspectedGroup?.name ?? ''}
        onClose={() => setInspected(null)}
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
                    addonIds: inspectedGroup.addonIds,
                  });
                  setInspected(null);
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
                  setInspected(null);
                }}
              >
                {t('common.delete')}
              </Button>
            </>
          )
        }
      >
        {inspectedGroup &&
          (() => {
            const summary = summarizeGroupMembers(inspectedGroup.addonIds, addonsById);
            const toggleableIds = summary.loading.map((a) => a.id);
            return (
              <div className="md-sheet-body">
                <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                  <div className="md-chip-row">
                    {renderStatusPill(summary)}
                    <span className="md-pill" data-tone="surface">
                      {formatBytes(summary.totalSize)}
                    </span>
                    {inspectedGroup.tags.map((tag) => (
                      <span key={tag} className="md-pill" data-tone="secondary">
                        {tag}
                      </span>
                    ))}
                  </div>
                  {toggleableIds.length > 0 && (
                    <Switch
                      compact
                      checked={summary.allEnabled}
                      disabled={busy}
                      label={t('organize.toggleAll')}
                      onChange={(next) => onBulkToggle(toggleableIds, next)}
                    />
                  )}
                </div>

                <div className="l4a-list">
                  {summary.members.length === 0 ? (
                    <div style={{ padding: 20, textAlign: 'center', color: 'var(--md-sys-color-on-surface-variant)' }}>
                      {t('common.empty')}
                    </div>
                  ) : (
                    summary.members.map((addon) =>
                      renderMemberRow(addon, {
                        onRemove: () => {
                          void onSaveUserGroup({
                            ...inspectedGroup,
                            addonIds: inspectedGroup.addonIds.filter((id) => id !== addon.id),
                          });
                        },
                      }),
                    )
                  )}
                </div>
              </div>
            );
          })()}
      </SideSheet>

      {/* Inline Profile Inspector with Live Diff Preview */}
      <SideSheet
        open={Boolean(inspectedProfile)}
        title={inspectedProfile?.name ?? ''}
        onClose={() => setInspected(null)}
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
                  setInspected(null);
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
        {inspectedProfile &&
          (() => {
            const diff = computeProfileDiff(inspectedProfile);
            return (
              <div className="md-sheet-body">
                {inspectedProfile.description && (
                  <div className="md-body-small l4a-description-box">
                    {inspectedProfile.description}
                  </div>
                )}

                <div className="md-chip-row">
                  <span className="md-pill" data-tone="primary">
                    {t('organize.profileDiffEnable', { count: diff.toEnable.length })}
                  </span>
                  <span className="md-pill" data-tone="tertiary">
                    {t('organize.profileDiffDisable', { count: diff.toDisable.length })}
                  </span>
                  <span className="md-pill" data-tone="surface">
                    {t('organize.profileDiffKeep', { count: diff.unchangedEnabled.length })}
                  </span>
                  {diff.missingIds.length > 0 && (
                    <span className="md-pill" data-tone="error">
                      {t('organize.profileMissing', { count: diff.missingIds.length })}
                    </span>
                  )}
                </div>

                {diff.toEnable.length > 0 && (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                    <span className="md-label-medium" style={{ color: 'var(--md-sys-color-primary)' }}>
                      {t('organize.profileDiffEnable', { count: diff.toEnable.length })}
                    </span>
                    <div className="l4a-list">
                      {diff.toEnable.map((addon) =>
                        renderMemberRow(addon, {
                          badge: { text: `+ ${t('common.enable')}`, tone: 'primary' },
                        }),
                      )}
                    </div>
                  </div>
                )}

                {diff.toDisable.length > 0 && (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                    <span className="md-label-medium" style={{ color: 'var(--md-sys-color-tertiary)' }}>
                      {t('organize.profileDiffDisable', { count: diff.toDisable.length })}
                    </span>
                    <div className="l4a-list">
                      {diff.toDisable.map((addon) =>
                        renderMemberRow(addon, {
                          badge: { text: `- ${t('common.disable')}`, tone: 'tertiary' },
                        }),
                      )}
                    </div>
                  </div>
                )}

                {diff.unchangedEnabled.length > 0 && (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                    <span className="md-label-medium" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                      {t('organize.profileDiffKeep', { count: diff.unchangedEnabled.length })}
                    </span>
                    <div className="l4a-list">
                      {diff.unchangedEnabled.map((addon) => renderMemberRow(addon))}
                    </div>
                  </div>
                )}
              </div>
            );
          })()}
      </SideSheet>
    </div>
  );
}
