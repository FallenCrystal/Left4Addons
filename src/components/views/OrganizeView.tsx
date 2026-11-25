import { useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Plus, RefreshCw, Search, X } from 'lucide-react';
import type {
  AddonNamePreference,
  AddonSummary,
  BundleRecord,
  LoadoutProfile,
  TrackedCollectionRecord,
  UserGroup,
} from '../../api/ipc';
import { useContentFilterContext } from '../../state/ContentFilterContext';
import {
  getAddonSearchRedaction,
  resolveLibraryAddonCardFilter,
} from '../../utils/contentFilter';
import { matchesAddonSearch } from '../../utils/format';
import { Button, IconButton, Tabs } from '../m3/primitives';
import type { ExternalAddonFilter } from './LibraryView';
import { BundlesTab } from './organize/BundlesTab';
import { CollectionsTab } from './organize/CollectionsTab';
import { GroupsTab } from './organize/GroupsTab';
import type { InspectedTarget, OrganizeTab } from './organize/helpers';
import { OrganizeInspectorSheet } from './organize/OrganizeInspectorSheet';
import { ProfilesTab } from './organize/ProfilesTab';

export type { OrganizeTab };

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
  const tabsId = useId();

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

  const filterCtx = useContentFilterContext();
  const needle = query.trim().toLowerCase();

  const accessibleBundles = useMemo(
    () =>
      bundles.filter((b) => {
        const members = b.memberAddonIds
          .map((id) => addonsById.get(id))
          .filter((a): a is AddonSummary => Boolean(a));
        if (members.length === 0) return true;
        return members.some((m) => !resolveLibraryAddonCardFilter(m, filterCtx).hiddenCard);
      }),
    [bundles, addonsById, filterCtx],
  );

  const filteredBundles = useMemo(() => {
    if (!needle) return accessibleBundles;
    return accessibleBundles.filter((b) => {
      const visibleMembers = b.memberAddonIds
        .map((id) => addonsById.get(id))
        .filter(
          (a): a is AddonSummary =>
            Boolean(a) && !resolveLibraryAddonCardFilter(a!, filterCtx).hiddenCard,
        );
      const allRedacted =
        visibleMembers.length > 0 &&
        visibleMembers.every((m) => getAddonSearchRedaction(m, filterCtx).redactTitle);
      if (!allRedacted && b.name.toLowerCase().includes(needle)) return true;
      return visibleMembers.some((addon) =>
        matchesAddonSearch(addon, needle, getAddonSearchRedaction(addon, filterCtx)),
      );
    });
  }, [accessibleBundles, needle, addonsById, filterCtx]);

  const filteredCollections = useMemo(() => {
    if (!needle) return collections;
    return collections.filter((c) => {
      if (c.title.toLowerCase().includes(needle) || c.collectionId.includes(needle)) return true;
      return c.memberWorkshopIds.some((wid) => {
        const addon = addonsByWorkshopId.get(wid);
        if (!addon) return wid.toLowerCase().includes(needle);
        if (resolveLibraryAddonCardFilter(addon, filterCtx).hiddenCard) return false;
        return matchesAddonSearch(addon, needle, getAddonSearchRedaction(addon, filterCtx));
      });
    });
  }, [collections, needle, addonsByWorkshopId, filterCtx]);

  const filteredGroups = useMemo(() => {
    if (!needle) return userGroups;
    return userGroups.filter((g) => {
      if (g.name.toLowerCase().includes(needle)) return true;
      if (g.tags.some((tag) => tag.toLowerCase().includes(needle))) return true;
      return g.addonIds.some((id) => {
        const addon = addonsById.get(id);
        if (!addon || resolveLibraryAddonCardFilter(addon, filterCtx).hiddenCard) return false;
        return matchesAddonSearch(addon, needle, getAddonSearchRedaction(addon, filterCtx));
      });
    });
  }, [userGroups, needle, addonsById, filterCtx]);

  const filteredProfiles = useMemo(() => {
    if (!needle) return profiles;
    return profiles.filter((p) => {
      if (p.name.toLowerCase().includes(needle) || p.description.toLowerCase().includes(needle)) {
        return true;
      }
      return p.enabledAddonIds.some((id) => {
        const addon = addonsById.get(id);
        if (!addon || resolveLibraryAddonCardFilter(addon, filterCtx).hiddenCard) return false;
        return matchesAddonSearch(addon, needle, getAddonSearchRedaction(addon, filterCtx));
      });
    });
  }, [profiles, needle, addonsById, filterCtx]);

  const inspectedBundle =
    inspected?.kind === 'bundle'
      ? (accessibleBundles.find((b) => b.id === inspected.id) ?? null)
      : null;
  const inspectedCollection =
    inspected?.kind === 'collection'
      ? (collections.find((c) => c.collectionId === inspected.id) ?? null)
      : null;
  const inspectedGroup =
    inspected?.kind === 'group' ? (userGroups.find((g) => g.id === inspected.id) ?? null) : null;
  const inspectedProfile =
    inspected?.kind === 'profile' ? (profiles.find((p) => p.id === inspected.id) ?? null) : null;

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
          { value: 'bundles', label: t('organize.bundles'), count: accessibleBundles.length },
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
        {tab === 'bundles' && (
          <BundlesTab
            bundles={bundles}
            filteredBundles={filteredBundles}
            addonsById={addonsById}
            namePreference={namePreference}
            busy={busy}
            onInspectBundle={(id) => setInspected({ kind: 'bundle', id })}
            onToggleBundleLock={onToggleBundleLock}
            onBulkToggle={onBulkToggle}
            onOpenInFileManager={onOpenInFileManager}
            onDeleteBundle={onDeleteBundle}
            onOpenFilterInLibrary={onOpenFilterInLibrary}
          />
        )}

        {tab === 'collections' && (
          <CollectionsTab
            collections={collections}
            filteredCollections={filteredCollections}
            addonsByWorkshopId={addonsByWorkshopId}
            namePreference={namePreference}
            busy={busy}
            onInspectCollection={(id) => setInspected({ kind: 'collection', id })}
            onDownloadWorkshopItem={onDownloadWorkshopItem}
            onOpenFilterInLibrary={onOpenFilterInLibrary}
            onUntrackCollection={onUntrackCollection}
          />
        )}

        {tab === 'groups' && (
          <GroupsTab
            userGroups={userGroups}
            filteredGroups={filteredGroups}
            addonsById={addonsById}
            namePreference={namePreference}
            busy={busy}
            onInspectGroup={(id) => setInspected({ kind: 'group', id })}
            onBulkToggle={onBulkToggle}
            onOpenInFileManager={onOpenInFileManager}
            onOpenFilterInLibrary={onOpenFilterInLibrary}
            onEditGroup={onEditGroup}
            onDeleteGroup={onDeleteGroup}
          />
        )}

        {tab === 'profiles' && (
          <ProfilesTab
            profiles={profiles}
            filteredProfiles={filteredProfiles}
            addonsById={addonsById}
            loadingAddons={loadingAddons}
            namePreference={namePreference}
            busy={busy}
            onInspectProfile={(id) => setInspected({ kind: 'profile', id })}
            onEditProfile={onEditProfile}
            onDeleteProfile={onDeleteProfile}
            onApplyProfile={onApplyProfile}
          />
        )}
      </div>

      <OrganizeInspectorSheet
        inspectedBundle={inspectedBundle}
        inspectedCollection={inspectedCollection}
        inspectedGroup={inspectedGroup}
        inspectedProfile={inspectedProfile}
        addonsById={addonsById}
        addonsByWorkshopId={addonsByWorkshopId}
        loadingAddons={loadingAddons}
        namePreference={namePreference}
        busy={busy}
        onClose={() => setInspected(null)}
        onToggleBundleLock={onToggleBundleLock}
        onDeleteBundle={onDeleteBundle}
        onUntrackCollection={onUntrackCollection}
        onDownloadWorkshopItem={onDownloadWorkshopItem}
        onEditGroup={onEditGroup}
        onSaveUserGroup={onSaveUserGroup}
        onDeleteGroup={onDeleteGroup}
        onEditProfile={onEditProfile}
        onDeleteProfile={onDeleteProfile}
        onApplyProfile={onApplyProfile}
        onBulkToggle={onBulkToggle}
        onToggleSingle={onToggleSingle}
        onSelectAddon={onSelectAddon}
        onOpenInFileManager={onOpenInFileManager}
        onOpenFilterInLibrary={onOpenFilterInLibrary}
        onCopy={onCopy}
      />
    </div>
  );
}
