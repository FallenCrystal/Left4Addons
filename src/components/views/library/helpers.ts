import type {
  AddonDirType,
  AddonNamePreference,
  AddonSummary,
  BundleRecord,
} from '../../../api/ipc';
import {
  getAddonSearchRedaction,
  isAddonTitleRedacted,
  type FilterLookupSource,
} from '../../../utils/contentFilter';
import {
  formatAddonDisplayName,
  matchesAddonSearch,
  resolveAddonMetadataTitle,
} from '../../../utils/format';

export type LibraryScope =
  | 'all'
  | 'loading'
  | 'workshop'
  | 'disabled'
  | 'uninstalled'
  | 'updates'
  | 'removed';

export type LibrarySort = 'title' | 'size' | 'workshopId' | 'status';

export interface ExternalAddonFilter {
  label: string;
  addonIds?: string[];
  workshopIds?: string[];
}

export type LibraryDisplayItem =
  | { kind: 'addon'; addon: AddonSummary }
  | {
      kind: 'bundle';
      bundle: BundleRecord;
      allMembers: AddonSummary[];
      visibleMembers: AddonSummary[];
      matchedMembers: AddonSummary[];
    };

export interface LibraryScopeCounts {
  all: number;
  loading: number;
  workshop: number;
  disabled: number;
  uninstalled: number;
  updates: number;
  removed: number;
}

export interface BundleSummaryInfo {
  installed: AddonSummary[];
  loading: AddonSummary[];
  workshopInstalled: AddonSummary[];
  enabledLoading: AddonSummary[];
  enabledInstalled: AddonSummary[];
  totalSize: number;
  firstCover: string | undefined;
  firstLocalCover: string | undefined;
  firstAuthor: string | undefined;
  firstPath: string | undefined;
  firstWorkshopId: string | undefined;
  hasDummyBypass: boolean;
  updateAvailable: boolean;
  removedUpstream: boolean;
  allLoadingEnabled: boolean;
  noneLoadingEnabled: boolean;
  primaryDir: AddonDirType | undefined;
}

export function isRemoteUnavailable(addon: AddonSummary): boolean {
  return (
    addon.remoteStatus === 'removed' ||
    addon.remoteStatus === 'private_or_unavailable'
  );
}

export function resolveLibraryMetadataTitle(
  addon: AddonSummary,
  namePreference: AddonNamePreference,
  filterCtx?: FilterLookupSource,
): string | undefined {
  if (filterCtx && isAddonTitleRedacted(addon, filterCtx)) {
    return undefined;
  }
  return resolveAddonMetadataTitle(
    addon,
    namePreference === 'addon_title' ? 'addon_title' : 'workshop_title',
  );
}

export function getInstalledBundleMembers(
  bundle: BundleRecord,
  addonsById: ReadonlyMap<string, AddonSummary>,
): AddonSummary[] {
  return bundle.memberAddonIds
    .map((id) => addonsById.get(id))
    .filter((addon): addon is AddonSummary => addon?.installState === 'installed');
}

export function canCollapseCampaignBundle(installedMembers: readonly AddonSummary[]): boolean {
  return (
    installedMembers.length >= 2 &&
    installedMembers.some(
      (member) =>
        member.maps.length > 0 ||
        member.categories.includes('Campaign') ||
        member.categories.includes('Map'),
    )
  );
}

export function summarizeBundle(allMembers: AddonSummary[]): BundleSummaryInfo {
  const installed = allMembers.filter((m) => m.installState === 'installed');
  const loading = installed.filter((m) => m.primaryDir === 'loading');
  const workshopInstalled = installed.filter((m) => m.primaryDir === 'workshop');
  const enabledLoading = loading.filter((m) => m.enabled);
  const enabledInstalled = installed.filter((m) => m.enabled);
  const totalSize = installed.reduce((acc, m) => acc + m.fileSize, 0);
  const firstCover =
    allMembers.find((m) => m.maps.length > 0 && m.coverImage)?.coverImage ??
    allMembers.find((m) => m.coverImage)?.coverImage;
  const firstLocalCover =
    allMembers.find((m) => m.maps.length > 0 && m.localCoverImage)?.localCoverImage ??
    allMembers.find((m) => m.localCoverImage)?.localCoverImage;
  const firstAuthor = allMembers.find((m) => m.author)?.author;
  const firstPath = installed.find((m) => m.currentPath)?.currentPath;
  const firstWorkshopId = allMembers.find((m) => m.workshopId)?.workshopId;
  const hasDummyBypass = allMembers.some((m) => m.hasDummyBypass);
  const updateAvailable = allMembers.some((m) => m.updateAvailable);
  const removedUpstream = allMembers.some(isRemoteUnavailable);
  const allLoadingEnabled = loading.length > 0 && enabledLoading.length === loading.length;
  const noneLoadingEnabled = loading.length > 0 && enabledLoading.length === 0;
  const primaryDir: AddonDirType | undefined =
    loading.length > 0
      ? 'loading'
      : workshopInstalled.length > 0
        ? 'workshop'
        : undefined;

  return {
    installed,
    loading,
    workshopInstalled,
    enabledLoading,
    enabledInstalled,
    totalSize,
    firstCover,
    firstLocalCover,
    firstAuthor,
    firstPath,
    firstWorkshopId,
    hasDummyBypass,
    updateAvailable,
    removedUpstream,
    allLoadingEnabled,
    noneLoadingEnabled,
    primaryDir,
  };
}

export function computeLibraryCounts(addons: AddonSummary[]): LibraryScopeCounts {
  let loadingCount = 0;
  let workshopCount = 0;
  let disabledCount = 0;
  let uninstalledCount = 0;
  let updatesCount = 0;
  let removedCount = 0;

  for (const a of addons) {
    if (a.installState === 'installed') {
      if (a.primaryDir === 'loading') loadingCount += 1;
      if (a.primaryDir === 'workshop') workshopCount += 1;
      if (!a.enabled) disabledCount += 1;
    } else {
      uninstalledCount += 1;
    }
    if (a.updateAvailable) updatesCount += 1;
    if (isRemoteUnavailable(a)) {
      removedCount += 1;
    }
  }

  return {
    all: addons.length,
    loading: loadingCount,
    workshop: workshopCount,
    disabled: disabledCount,
    uninstalled: uninstalledCount,
    updates: updatesCount,
    removed: removedCount,
  };
}

export function computeAvailableCategories(addons: AddonSummary[]): Array<[string, number]> {
  const map = new Map<string, number>();
  for (const a of addons) {
    if (a.categories.length === 0) {
      map.set('Other', (map.get('Other') ?? 0) + 1);
    } else {
      for (const c of a.categories) {
        map.set(c, (map.get(c) ?? 0) + 1);
      }
    }
  }
  return Array.from(map.entries()).sort((a, b) => b[1] - a[1]);
}

export interface BuildLibraryDisplayItemsParams {
  addons: AddonSummary[];
  addonsById: Map<string, AddonSummary>;
  bundles: BundleRecord[];
  collapseBundles: boolean;
  externalFilter: ExternalAddonFilter | null;
  scope: LibraryScope;
  effectiveCategory: string;
  trimmedQuery: string;
  mapMode: boolean;
  mapMatchedIds: Set<string> | null;
  sortBy: LibrarySort;
  namePreference: AddonNamePreference;
  hiddenAddonIds?: ReadonlySet<string>;
  enforceHide?: boolean;
  filterCtx?: FilterLookupSource;
}

export function buildLibraryDisplayItems({
  addons,
  addonsById,
  bundles,
  collapseBundles,
  externalFilter,
  scope,
  effectiveCategory,
  trimmedQuery,
  mapMode,
  mapMatchedIds,
  sortBy,
  namePreference,
  hiddenAddonIds,
  enforceHide = true,
  filterCtx,
}: BuildLibraryDisplayItemsParams): {
  displayItems: LibraryDisplayItem[];
  visibleAddons: AddonSummary[];
  filteredOutCount: number;
} {
  const extAddonSet = externalFilter?.addonIds ? new Set(externalFilter.addonIds) : null;
  const extWorkshopSet = externalFilter?.workshopIds
    ? new Set(externalFilter.workshopIds)
    : null;

  const matchesScope = (a: AddonSummary): boolean => {
    if (extAddonSet && !extAddonSet.has(a.id)) return false;
    if (extWorkshopSet && (!a.workshopId || !extWorkshopSet.has(a.workshopId))) return false;

    if (scope === 'loading' && (a.installState !== 'installed' || a.primaryDir !== 'loading')) {
      return false;
    }
    if (scope === 'workshop' && (a.installState !== 'installed' || a.primaryDir !== 'workshop')) {
      return false;
    }
    if (scope === 'disabled' && (a.installState !== 'installed' || a.enabled)) {
      return false;
    }
    if (scope === 'uninstalled' && a.installState === 'installed') {
      return false;
    }
    if (scope === 'updates' && !a.updateAvailable) {
      return false;
    }
    if (scope === 'removed' && !isRemoteUnavailable(a)) {
      return false;
    }
    return true;
  };

  const matchesCategory = (a: AddonSummary): boolean => {
    if (effectiveCategory === 'All') return true;
    if (effectiveCategory === 'Other') {
      return a.categories.length === 0 || a.categories.includes('Other');
    }
    return a.categories.includes(effectiveCategory);
  };

  const matchesQuery = (a: AddonSummary): boolean => {
    if (!trimmedQuery) return true;
    const redaction = filterCtx ? getAddonSearchRedaction(a, filterCtx) : undefined;
    if (mapMode) {
      const needle = trimmedQuery.toLowerCase();
      if (redaction?.redactTitle) {
        return a.maps.some((m) => m.code.toLowerCase().includes(needle));
      }
      if (mapMatchedIds !== null) {
        return mapMatchedIds.has(a.id);
      }
      return a.maps.some(
        (m) => m.code.toLowerCase().includes(needle) || m.name.toLowerCase().includes(needle),
      );
    }
    return matchesAddonSearch(a, trimmedQuery, redaction);
  };

  let filteredOutCount = 0;
  for (const addon of addons) {
    if (
      hiddenAddonIds?.has(addon.id) &&
      matchesScope(addon) &&
      matchesCategory(addon) &&
      matchesQuery(addon)
    ) {
      filteredOutCount += 1;
    }
  }

  const isOmittedByFilter = (addon: AddonSummary): boolean =>
    Boolean(enforceHide && hiddenAddonIds?.has(addon.id));

  const items: LibraryDisplayItem[] = [];
  const claimedAddonIds = new Set<string>();
  const isGeneralBrowse =
    scope === 'all' || scope === 'loading' || scope === 'workshop' || scope === 'disabled';

  if (collapseBundles && !externalFilter) {
    for (const bundle of bundles) {
      const unclaimedMembers = getInstalledBundleMembers(bundle, addonsById).filter(
        (addon) => !claimedAddonIds.has(addon.id) && !isOmittedByFilter(addon),
      );
      if (!canCollapseCampaignBundle(unclaimedMembers)) continue;

      const scopeMembers = unclaimedMembers.filter(matchesScope);
      if (scopeMembers.length < 2) continue;

      const categoryMembers =
        effectiveCategory === 'All'
          ? scopeMembers
          : effectiveCategory === 'Other'
            ? scopeMembers.every(matchesCategory)
              ? scopeMembers
              : []
            : scopeMembers.filter(matchesCategory);
      if (categoryMembers.length === 0) continue;

      const matchedMembers = categoryMembers.filter(matchesQuery);
      const allCategoryMembersRedacted = Boolean(
        filterCtx &&
          categoryMembers.length > 0 &&
          categoryMembers.every((m) => getAddonSearchRedaction(m, filterCtx).redactTitle),
      );
      const bundleNameMatched =
        Boolean(trimmedQuery) &&
        !mapMode &&
        !allCategoryMembersRedacted &&
        bundle.name.toLowerCase().includes(trimmedQuery.toLowerCase());

      const shouldCollapse =
        matchedMembers.length >= 2 ||
        bundleNameMatched ||
        (isGeneralBrowse && matchedMembers.length >= 1);

      if (shouldCollapse) {
        const visibleMembers = matchedMembers.length > 0 ? matchedMembers : categoryMembers;
        for (const member of unclaimedMembers) {
          claimedAddonIds.add(member.id);
        }
        items.push({
          kind: 'bundle',
          bundle,
          allMembers: unclaimedMembers,
          visibleMembers,
          matchedMembers,
        });
      }
    }
  }

  for (const addon of addons) {
    if (claimedAddonIds.has(addon.id) || isOmittedByFilter(addon)) continue;
    if (matchesScope(addon) && matchesCategory(addon) && matchesQuery(addon)) {
      items.push({ kind: 'addon', addon });
    }
  }

  const itemTitle = (entry: LibraryDisplayItem): string => {
    if (entry.kind === 'bundle') {
      if (
        filterCtx &&
        entry.visibleMembers.length > 0 &&
        entry.visibleMembers.every((m) => isAddonTitleRedacted(m, filterCtx))
      ) {
        return entry.visibleMembers.find((m) => m.workshopId)?.workshopId ?? entry.bundle.id;
      }
      return entry.bundle.name;
    }
    if (filterCtx && isAddonTitleRedacted(entry.addon, filterCtx)) {
      return entry.addon.workshopId ?? entry.addon.id;
    }
    return formatAddonDisplayName(entry.addon, namePreference);
  };

  const itemSize = (entry: LibraryDisplayItem): number =>
    entry.kind === 'bundle'
      ? entry.allMembers.reduce(
          (sum, m) => (m.installState === 'installed' ? sum + m.fileSize : sum),
          0,
        )
      : entry.addon.fileSize;

  const itemWorkshopId = (entry: LibraryDisplayItem): string =>
    entry.kind === 'bundle'
      ? (entry.allMembers.find((m) => m.workshopId)?.workshopId ?? '')
      : (entry.addon.workshopId ?? '');

  const itemStatusScore = (entry: LibraryDisplayItem): number => {
    if (entry.kind === 'addon') {
      return entry.addon.installState !== 'installed' ? 2 : entry.addon.enabled ? 0 : 1;
    }
    const installed = entry.allMembers.filter((m) => m.installState === 'installed');
    if (installed.length === 0) return 2;
    return installed.some((m) => m.enabled) ? 0 : 1;
  };

  items.sort((a, b) => {
    if (sortBy === 'size') return itemSize(b) - itemSize(a);
    if (sortBy === 'workshopId') {
      return itemWorkshopId(b).localeCompare(itemWorkshopId(a), undefined, { numeric: true });
    }
    if (sortBy === 'status') {
      const diff = itemStatusScore(a) - itemStatusScore(b);
      if (diff !== 0) return diff;
    }
    return itemTitle(a).localeCompare(itemTitle(b));
  });

  const flattenedVisible: AddonSummary[] = [];
  const seenVisible = new Set<string>();
  for (const item of items) {
    if (item.kind === 'addon') {
      if (!seenVisible.has(item.addon.id)) {
        seenVisible.add(item.addon.id);
        flattenedVisible.push(item.addon);
      }
    } else {
      for (const m of item.visibleMembers) {
        if (!seenVisible.has(m.id)) {
          seenVisible.add(m.id);
          flattenedVisible.push(m);
        }
      }
    }
  }

  return { displayItems: items, visibleAddons: flattenedVisible, filteredOutCount };
}
