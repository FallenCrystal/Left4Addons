import { useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AlertTriangle,
  ArrowRightLeft,
  ArrowUpCircle,
  CheckSquare,
  CloudOff,
  Copy,
  Download,
  Folder,
  FolderOpen,
  FolderPlus,
  LayoutGrid,
  Layers,
  List,
  ListFilter,
  Map as MapIcon,
  Package,
  Pencil,
  Power,
  PowerOff,
  RefreshCw,
  Search,
  ShieldCheck,
  Store,
  Trash2,
  X,
} from 'lucide-react';
import type {
  AddonDirType,
  AddonNamePreference,
  AddonSummary,
  BundleRecord,
} from '../../api/ipc';
import type { LiveDownloadState } from '../../state/useAppController';
import {
  formatAddonDisplayName,
  formatBundleMemberLabel,
  formatBytes,
  matchesAddonSearch,
  resolveAddonMetadataTitle,
} from '../../utils/format';
import { AddonCover } from '../m3/AddonCover';
import {
  Button,
  Checkbox,
  EmptyState,
  FilterChip,
  IconButton,
  LinearProgress,
  SegmentedButton,
  Select,
  SideSheet,
  Switch,
  Tabs,
} from '../m3/primitives';

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

export interface LibraryViewProps {
  addons: AddonSummary[];
  bundles: BundleRecord[];
  namePreference?: AddonNamePreference;
  collapseBundles?: boolean;
  onToggleCollapseBundles?: (next: boolean) => void;
  snapshotVersion: number;
  liveDownloads: Record<string, LiveDownloadState>;
  externalFilter: ExternalAddonFilter | null;
  onClearExternalFilter: () => void;
  busy: boolean;
  syncing: boolean;
  onSync: () => void;
  onScan: () => void;
  onSearchMapCode: (query: string) => Promise<AddonSummary[]>;
  onSelectAddon: (addon: AddonSummary) => void;
  onToggleSingle: (addon: AddonSummary, enabled: boolean) => void;
  onBatchToggle: (addonIds: string[], enabled: boolean) => void;
  onBatchMove: (addonIds: string[], targetDir: AddonDirType) => void;
  onBatchRename: (addonIds: string[]) => void;
  onBatchAddToGroup: (addonIds: string[]) => void;
  onBatchCreateBundle: (addonIds: string[]) => void;
  onBatchDelete: (addons: AddonSummary[]) => void;
  onDownloadItem: (workshopId: string, title?: string) => void;
  onUpdateItem: (workshopId: string, title?: string) => void;
  onOpenInFileManager: (path: string) => void;
  onCopy: (text: string) => void;
}

type LibraryDisplayItem =
  | { kind: 'addon'; addon: AddonSummary }
  | {
      kind: 'bundle';
      bundle: BundleRecord;
      allMembers: AddonSummary[];
      visibleMembers: AddonSummary[];
    };

function summarizeBundle(allMembers: AddonSummary[]) {
  const installed = allMembers.filter((m) => m.installState === 'installed');
  const loading = installed.filter((m) => m.primaryDir === 'loading');
  const workshopInstalled = installed.filter((m) => m.primaryDir === 'workshop');
  const enabledLoading = loading.filter((m) => m.enabled);
  const enabledInstalled = installed.filter((m) => m.enabled);
  const totalSize = installed.reduce((acc, m) => acc + m.fileSize, 0);
  const firstCover = allMembers.find((m) => m.coverImage)?.coverImage;
  const firstAuthor = allMembers.find((m) => m.author)?.author;
  const firstPath = installed.find((m) => m.currentPath)?.currentPath;
  const firstWorkshopId = allMembers.find((m) => m.workshopId)?.workshopId;
  const hasDummyBypass = allMembers.some((m) => m.hasDummyBypass);
  const updateAvailable = allMembers.some((m) => m.updateAvailable);
  const removedUpstream = allMembers.some(
    (m) => m.remoteStatus === 'removed' || m.remoteStatus === 'private_or_unavailable',
  );
  const allLoadingEnabled = loading.length > 0 && enabledLoading.length === loading.length;
  const noneLoadingEnabled = loading.length > 0 && enabledLoading.length === 0;

  return {
    installed,
    loading,
    workshopInstalled,
    enabledLoading,
    enabledInstalled,
    totalSize,
    firstCover,
    firstAuthor,
    firstPath,
    firstWorkshopId,
    hasDummyBypass,
    updateAvailable,
    removedUpstream,
    allLoadingEnabled,
    noneLoadingEnabled,
  };
}

export function LibraryView({
  addons,
  bundles,
  namePreference = 'addon_title',
  collapseBundles = true,
  onToggleCollapseBundles,
  snapshotVersion,
  liveDownloads,
  externalFilter,
  onClearExternalFilter,
  busy,
  syncing,
  onSync,
  onScan,
  onSearchMapCode,
  onSelectAddon,
  onToggleSingle,
  onBatchToggle,
  onBatchMove,
  onBatchRename,
  onBatchAddToGroup,
  onBatchCreateBundle,
  onBatchDelete,
  onDownloadItem,
  onUpdateItem,
  onOpenInFileManager,
  onCopy,
}: LibraryViewProps) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const [mapMode, setMapMode] = useState(false);
  const [mapResult, setMapResult] = useState<{
    key: string;
    ids: Set<string>;
  } | null>(null);
  const [scope, setScope] = useState<LibraryScope>('all');
  const [category, setCategory] = useState<string>('All');
  const [sortBy, setSortBy] = useState<LibrarySort>('title');
  const [layout, setLayout] = useState<'grid' | 'list'>('grid');
  const [batchMode, setBatchMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [inspectedBundleId, setInspectedBundleId] = useState<string | null>(null);
  const [bundleTab, setBundleTab] = useState<'members' | 'maps'>('members');
  const bundleTabsId = useId();

  const trimmedQuery = query.trim();
  const mapQueryKey = `${snapshotVersion}:${trimmedQuery}`;

  useEffect(() => {
    if (!mapMode || !trimmedQuery) {
      return;
    }
    let active = true;
    const timer = window.setTimeout(() => {
      void onSearchMapCode(trimmedQuery)
        .then((matches) => {
          if (!active) return;
          setMapResult({ key: mapQueryKey, ids: new Set(matches.map((m) => m.id)) });
        })
        .catch(() => {
          // Fall back cleanly to local map matching on IPC failure.
        });
    }, 150);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [mapMode, trimmedQuery, mapQueryKey, onSearchMapCode]);

  const mapMatchedIds =
    mapMode && trimmedQuery && mapResult?.key === mapQueryKey ? mapResult.ids : null;

  const addonsById = useMemo(() => {
    const map = new Map<string, AddonSummary>();
    for (const a of addons) map.set(a.id, a);
    return map;
  }, [addons]);

  const counts = useMemo(() => {
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
      if (a.remoteStatus === 'removed' || a.remoteStatus === 'private_or_unavailable') {
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
  }, [addons]);

  const availableCategories = useMemo(() => {
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
  }, [addons]);

  const effectiveCategory = useMemo(() => {
    if (category === 'All') return 'All';
    return availableCategories.some(([cat]) => cat === category) ? category : 'All';
  }, [category, availableCategories]);

  const categoryOptions = useMemo(
    () => [
      { value: 'All', label: t('library.allCategories'), count: addons.length },
      ...availableCategories.map(([cat, count]) => ({
        value: cat,
        label: t(`categories.${cat}`, { defaultValue: cat }),
        count,
      })),
    ],
    [addons.length, availableCategories, t],
  );

  const hasCollapsibleBundles = useMemo(
    () =>
      bundles.some(
        (b) =>
          b.memberAddonIds.reduce((n, id) => {
            const addon = addonsById.get(id);
            return addon?.installState === 'installed' ? n + 1 : n;
          }, 0) >= 2,
      ),
    [bundles, addonsById],
  );

  const { displayItems, visibleAddons } = useMemo(() => {
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
      if (
        scope === 'removed' &&
        a.remoteStatus !== 'removed' &&
        a.remoteStatus !== 'private_or_unavailable'
      ) {
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
      if (mapMode) {
        if (mapMatchedIds !== null) {
          return mapMatchedIds.has(a.id);
        }
        const needle = trimmedQuery.toLowerCase();
        return a.maps.some(
          (m) =>
            m.code.toLowerCase().includes(needle) || m.name.toLowerCase().includes(needle),
        );
      }
      return matchesAddonSearch(a, trimmedQuery);
    };

    const items: LibraryDisplayItem[] = [];
    const claimedAddonIds = new Set<string>();
    const isGeneralBrowse =
      scope === 'all' || scope === 'loading' || scope === 'workshop' || scope === 'disabled';

    if (collapseBundles && !externalFilter) {
      for (const bundle of bundles) {
        const unclaimedMembers = bundle.memberAddonIds
          .map((id) => addonsById.get(id))
          .filter(
            (a): a is AddonSummary =>
              Boolean(a) && a.installState === 'installed' && !claimedAddonIds.has(a.id),
          );
        if (unclaimedMembers.length < 2) continue;

        const scopeMembers = unclaimedMembers.filter(matchesScope);
        if (scopeMembers.length < 2) continue;

        const bundleMatchesCategory =
          effectiveCategory === 'All' ||
          (effectiveCategory === 'Other'
            ? scopeMembers.every(matchesCategory)
            : scopeMembers.some(matchesCategory));
        if (!bundleMatchesCategory) continue;

        const queryMembers = scopeMembers.filter(matchesQuery);
        const bundleNameMatched =
          Boolean(trimmedQuery) &&
          !mapMode &&
          bundle.name.toLowerCase().includes(trimmedQuery.toLowerCase());

        const shouldCollapse =
          queryMembers.length >= 2 ||
          (bundleNameMatched && scopeMembers.length >= 2) ||
          (isGeneralBrowse && scopeMembers.length >= 2 && queryMembers.length >= 1);

        if (shouldCollapse) {
          const visibleMembers =
            bundleNameMatched || (isGeneralBrowse && scopeMembers.length >= 2)
              ? scopeMembers
              : queryMembers;
          for (const member of unclaimedMembers) {
            claimedAddonIds.add(member.id);
          }
          items.push({
            kind: 'bundle',
            bundle,
            allMembers: unclaimedMembers,
            visibleMembers,
          });
        }
      }
    }

    for (const addon of addons) {
      if (claimedAddonIds.has(addon.id)) continue;
      if (matchesScope(addon) && matchesCategory(addon) && matchesQuery(addon)) {
        items.push({ kind: 'addon', addon });
      }
    }

    const itemTitle = (entry: LibraryDisplayItem): string =>
      entry.kind === 'bundle'
        ? entry.bundle.name
        : formatAddonDisplayName(entry.addon, namePreference);

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

    return { displayItems: items, visibleAddons: flattenedVisible };
  }, [
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
  ]);

  const existingIds = useMemo(() => new Set(addons.map((a) => a.id)), [addons]);
  const validSelectedIds = useMemo(() => {
    const next = new Set<string>();
    for (const id of selectedIds) {
      if (existingIds.has(id)) next.add(id);
    }
    return next;
  }, [selectedIds, existingIds]);

  const visibleSelectedAddons = useMemo(
    () => visibleAddons.filter((a) => validSelectedIds.has(a.id)),
    [visibleAddons, validSelectedIds],
  );
  const visibleSelectedIds = useMemo(
    () => visibleSelectedAddons.map((a) => a.id),
    [visibleSelectedAddons],
  );
  const hiddenSelectedCount = validSelectedIds.size - visibleSelectedAddons.length;
  const showBatchBar = batchMode || validSelectedIds.size > 0;

  const toggleSelect = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set<string>();
      for (const existing of prev) {
        if (existingIds.has(existing)) next.add(existing);
      }
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const toggleSelectMany = (ids: string[]) => {
    if (ids.length === 0) return;
    setSelectedIds((prev) => {
      const next = new Set<string>();
      for (const existing of prev) {
        if (existingIds.has(existing)) next.add(existing);
      }
      const allSelected = ids.every((id) => next.has(id));
      for (const id of ids) {
        if (allSelected) next.delete(id);
        else if (existingIds.has(id)) next.add(id);
      }
      return next;
    });
  };

  const clearSelection = () => {
    setSelectedIds(new Set());
    setBatchMode(false);
  };

  const keepOnlyVisibleSelection = () => {
    setSelectedIds(new Set(visibleSelectedIds));
  };

  const selectAllVisible = () => {
    setSelectedIds(new Set(visibleAddons.map((a) => a.id)));
  };

  const loadingToggleTargets = visibleSelectedAddons
    .filter((a) => a.installState === 'installed' && a.primaryDir === 'loading')
    .map((a) => a.id);
  const workshopToLoadingTargets = visibleSelectedAddons
    .filter((a) => a.installState === 'installed' && a.primaryDir === 'workshop')
    .map((a) => a.id);
  const loadingToWorkshopTargets = loadingToggleTargets;
  const renameTargets = loadingToggleTargets;
  const installableTargets = visibleSelectedAddons.filter(
    (a) =>
      a.workshopId &&
      a.installState !== 'installed' &&
      a.remoteStatus !== 'removed' &&
      a.remoteStatus !== 'private_or_unavailable',
  );
  const updatableTargets = visibleSelectedAddons.filter(
    (a) =>
      a.workshopId &&
      a.installState === 'installed' &&
      a.updateAvailable &&
      a.remoteStatus !== 'removed' &&
      a.remoteStatus !== 'private_or_unavailable',
  );

  const inspectedBundle = useMemo(
    () => (inspectedBundleId ? (bundles.find((b) => b.id === inspectedBundleId) ?? null) : null),
    [bundles, inspectedBundleId],
  );

  return (
    <div className="l4a-view">
      <header className="l4a-view-header">
        <div className="md-search-bar">
          <Search
            size={18}
            style={{ color: 'var(--md-sys-color-on-surface-variant)', flexShrink: 0 }}
          />
          <input
            className="md-search-input"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={
              mapMode ? t('library.mapSearchPlaceholder') : t('library.searchPlaceholder')
            }
            aria-label={t('common.search')}
          />
          {query && (
            <IconButton size="sm" label={t('common.clear')} onClick={() => setQuery('')}>
              <X size={16} />
            </IconButton>
          )}
          <FilterChip
            label={t('library.mapMode')}
            selected={mapMode}
            icon={<MapIcon size={14} />}
            onClick={() => setMapMode((v) => !v)}
          />
        </div>

        <Select<LibrarySort>
          ariaLabel={t('common.sort')}
          value={sortBy}
          onChange={setSortBy}
          options={[
            { value: 'title', label: t('sort.title') },
            { value: 'size', label: t('sort.size') },
            { value: 'workshopId', label: t('sort.workshopId') },
            { value: 'status', label: t('sort.status') },
          ]}
        />

        <IconButton
          label={layout === 'grid' ? t('library.listView') : t('library.gridView')}
          onClick={() => setLayout((l) => (l === 'grid' ? 'list' : 'grid'))}
        >
          {layout === 'grid' ? <List size={20} /> : <LayoutGrid size={20} />}
        </IconButton>

        <IconButton
          label={t('library.batch')}
          active={showBatchBar}
          onClick={() => {
            if (showBatchBar) clearSelection();
            else setBatchMode(true);
          }}
        >
          <CheckSquare size={19} />
        </IconButton>

        <Button
          variant="tonal"
          disabled={syncing}
          icon={<RefreshCw size={16} className={syncing ? 'md-spin' : undefined} />}
          onClick={onSync}
        >
          {t('nav.sync')}
        </Button>
      </header>

      <div className="l4a-view-toolbar">
        <SegmentedButton<LibraryScope>
          ariaLabel={t('common.scope')}
          value={scope === 'updates' || scope === 'removed' ? 'all' : scope}
          onChange={setScope}
          options={[
            { value: 'all', label: t('scope.all'), count: counts.all },
            { value: 'loading', label: t('scope.loading'), count: counts.loading },
            { value: 'workshop', label: t('scope.workshop'), count: counts.workshop },
            { value: 'disabled', label: t('scope.disabled'), count: counts.disabled },
            { value: 'uninstalled', label: t('scope.uninstalled'), count: counts.uninstalled },
          ]}
        />

        {availableCategories.length > 0 && (
          <Select<string>
            variant="filter"
            align="start"
            active={effectiveCategory !== 'All'}
            ariaLabel={t('library.category')}
            icon={<ListFilter size={14} />}
            value={effectiveCategory}
            onChange={setCategory}
            options={categoryOptions}
          />
        )}

        {hasCollapsibleBundles && !externalFilter && onToggleCollapseBundles && (
          <FilterChip
            label={t('library.collapseBundles')}
            selected={collapseBundles}
            icon={<Layers size={14} />}
            onClick={() => onToggleCollapseBundles(!collapseBundles)}
          />
        )}

        {counts.updates > 0 && (
          <FilterChip
            label={t('scope.updates')}
            count={counts.updates}
            selected={scope === 'updates'}
            tone={scope === 'updates' ? 'default' : 'tertiary'}
            icon={<ArrowUpCircle size={14} />}
            onClick={() => setScope((s) => (s === 'updates' ? 'all' : 'updates'))}
          />
        )}

        {counts.removed > 0 && (
          <FilterChip
            label={t('scope.removed')}
            count={counts.removed}
            selected={scope === 'removed'}
            tone={scope === 'removed' ? 'default' : 'error'}
            icon={<CloudOff size={14} />}
            onClick={() => setScope((s) => (s === 'removed' ? 'all' : 'removed'))}
          />
        )}

        {externalFilter && (
          <FilterChip
            label={externalFilter.label}
            selected
            onRemove={onClearExternalFilter}
            onClick={onClearExternalFilter}
          />
        )}
      </div>

      {showBatchBar && (
        <div className="l4a-batch-bar">
          <span className="md-label-large">
            {t('common.selected', { count: visibleSelectedIds.length })}
          </span>
          {hiddenSelectedCount > 0 && (
            <FilterChip
              label={t('library.hiddenSelected', { count: hiddenSelectedCount })}
              tone="surface"
              onRemove={keepOnlyVisibleSelection}
              onClick={keepOnlyVisibleSelection}
            />
          )}
          <Button variant="text" size="sm" onClick={selectAllVisible}>
            {t('library.selectAll')}
          </Button>
          <div className="l4a-spacer" />
          <Button
            variant="text"
            size="sm"
            disabled={busy || loadingToggleTargets.length === 0}
            icon={<Power size={14} />}
            onClick={() => onBatchToggle(loadingToggleTargets, true)}
          >
            {t('common.enable')}
          </Button>
          <Button
            variant="text"
            size="sm"
            disabled={busy || loadingToggleTargets.length === 0}
            icon={<PowerOff size={14} />}
            onClick={() => onBatchToggle(loadingToggleTargets, false)}
          >
            {t('common.disable')}
          </Button>
          {workshopToLoadingTargets.length > 0 && (
            <Button
              variant="text"
              size="sm"
              disabled={busy}
              icon={<ArrowRightLeft size={14} />}
              onClick={() => onBatchMove(workshopToLoadingTargets, 'loading')}
            >
              {t('library.moveToLoading')}
            </Button>
          )}
          {loadingToWorkshopTargets.length > 0 && (
            <Button
              variant="text"
              size="sm"
              disabled={busy}
              icon={<ArrowRightLeft size={14} />}
              onClick={() => onBatchMove(loadingToWorkshopTargets, 'workshop')}
            >
              {t('library.moveToWorkshop')}
            </Button>
          )}
          <Button
            variant="text"
            size="sm"
            disabled={busy || renameTargets.length === 0}
            icon={<Pencil size={14} />}
            onClick={() => onBatchRename(renameTargets)}
          >
            {t('library.autoRename')}
          </Button>
          <Button
            variant="text"
            size="sm"
            disabled={busy || visibleSelectedIds.length === 0}
            icon={<FolderPlus size={14} />}
            onClick={() => onBatchAddToGroup(visibleSelectedIds)}
          >
            {t('library.addToGroup')}
          </Button>
          <Button
            variant="text"
            size="sm"
            disabled={busy || visibleSelectedIds.length === 0}
            icon={<Layers size={14} />}
            onClick={() => onBatchCreateBundle(visibleSelectedIds)}
          >
            {t('library.createBundle')}
          </Button>
          {installableTargets.length > 0 && (
            <Button
              variant="filled"
              size="sm"
              disabled={busy}
              icon={<Download size={14} />}
              onClick={() => {
                for (const item of installableTargets) {
                  if (item.workshopId) {
                    onDownloadItem(item.workshopId, resolveAddonMetadataTitle(item));
                  }
                }
              }}
            >
              {t('common.download')} ({installableTargets.length})
            </Button>
          )}
          {updatableTargets.length > 0 && (
            <Button
              variant="filled"
              size="sm"
              disabled={busy}
              icon={<ArrowUpCircle size={14} />}
              onClick={() => {
                for (const item of updatableTargets) {
                  if (item.workshopId) {
                    onUpdateItem(item.workshopId, resolveAddonMetadataTitle(item));
                  }
                }
              }}
            >
              {t('common.update')} ({updatableTargets.length})
            </Button>
          )}
          <Button
            variant="text"
            tone="error"
            size="sm"
            disabled={busy || visibleSelectedAddons.length === 0}
            icon={<Trash2 size={14} />}
            onClick={() => onBatchDelete(visibleSelectedAddons)}
          >
            {t('common.delete')}
          </Button>
          <IconButton size="sm" label={t('common.close')} onClick={clearSelection}>
            <X size={16} />
          </IconButton>
        </div>
      )}

      <div className="l4a-view-scroll">
        {displayItems.length === 0 ? (
          <EmptyState
            icon={<Package size={26} />}
            title={addons.length === 0 ? t('common.empty') : t('common.noResults')}
            action={
              addons.length === 0 ? (
                <Button variant="filled" onClick={onScan}>
                  {t('nav.scan')}
                </Button>
              ) : undefined
            }
          />
        ) : layout === 'grid' ? (
          <div className="l4a-grid">
            {displayItems.map((entry) => {
              if (entry.kind === 'bundle') {
                const { bundle, allMembers, visibleMembers } = entry;
                const summary = summarizeBundle(allMembers);
                const visibleIds = visibleMembers.map((m) => m.id);
                const selected =
                  visibleIds.length > 0 && visibleIds.every((id) => validSelectedIds.has(id));
                const loadingIds = summary.loading.map((m) => m.id);
                const muted =
                  summary.installed.length > 0 && summary.enabledInstalled.length === 0;
                const primaryDir: AddonDirType | undefined =
                  summary.loading.length > 0
                    ? 'loading'
                    : summary.workshopInstalled.length > 0
                      ? 'workshop'
                      : undefined;
                const switchChecked =
                  loadingIds.length > 0
                    ? summary.allLoadingEnabled
                    : summary.enabledInstalled.length > 0;

                return (
                  <article
                    key={`bundle:${bundle.id}`}
                    className="md-card l4a-bundle-card"
                    data-clickable="true"
                    data-selected={selected ? 'true' : 'false'}
                    data-muted={muted ? 'true' : 'false'}
                  >
                    <div className="l4a-addon-cover-wrap">
                      <button
                        type="button"
                        tabIndex={-1}
                        aria-hidden="true"
                        className="l4a-card-trigger"
                        style={{ height: '100%' }}
                        onClick={() => {
                          if (batchMode) toggleSelectMany(visibleIds);
                          else {
                            setBundleTab('members');
                            setInspectedBundleId(bundle.id);
                          }
                        }}
                      >
                        <AddonCover
                          src={bundle.coverImage ?? summary.firstCover}
                          title={bundle.name}
                          category="Campaign"
                        />
                      </button>
                      <div className="l4a-cover-overlay-top">
                        <div
                          className="l4a-card-check"
                          data-visible={showBatchBar || selected ? 'true' : 'false'}
                        >
                          <Checkbox
                            checked={selected}
                            label={bundle.name}
                            onChange={() => toggleSelectMany(visibleIds)}
                          />
                        </div>
                        <div className="l4a-status-pills">
                          <span className="md-pill" data-tone="secondary">
                            <Layers size={12} />
                            {t('library.bundleParts', { count: allMembers.length })}
                          </span>
                          {summary.hasDummyBypass && (
                            <span className="md-pill" data-tone="primary">
                              <ShieldCheck size={12} />
                            </span>
                          )}
                          {summary.updateAvailable && (
                            <span className="md-pill" data-tone="tertiary">
                              <ArrowUpCircle size={12} />
                            </span>
                          )}
                          {summary.removedUpstream && (
                            <span className="md-pill" data-tone="error">
                              <CloudOff size={12} />
                            </span>
                          )}
                        </div>
                      </div>
                    </div>

                    <div className="l4a-addon-card-body">
                      <button
                        type="button"
                        className="l4a-card-trigger"
                        onClick={() => {
                          if (batchMode) toggleSelectMany(visibleIds);
                          else {
                            setBundleTab('members');
                            setInspectedBundleId(bundle.id);
                          }
                        }}
                      >
                        <div className="l4a-addon-title">{bundle.name}</div>
                      </button>
                      <div className="l4a-addon-meta">
                        <span className="l4a-truncate">
                          {summary.firstAuthor || t('common.unknown')}
                        </span>
                        <span>·</span>
                        <span style={{ flexShrink: 0 }}>{formatBytes(summary.totalSize)}</span>
                        {bundle.maps.length > 0 && (
                          <>
                            <span>·</span>
                            <span
                              style={{
                                flexShrink: 0,
                                display: 'inline-flex',
                                alignItems: 'center',
                                gap: 3,
                              }}
                            >
                              <MapIcon size={12} />
                              {bundle.maps.length}
                            </span>
                          </>
                        )}
                      </div>

                      <div className="l4a-addon-card-footer">
                        <div className="l4a-row-wrap" style={{ gap: 6 }}>
                          {primaryDir && (
                            <span className="md-pill" data-tone="surface">
                              {primaryDir === 'loading' ? (
                                <Folder size={11} />
                              ) : (
                                <Store size={11} />
                              )}
                              {primaryDir === 'loading'
                                ? t('library.loadingDir')
                                : t('library.workshopDir')}
                            </span>
                          )}
                          {loadingIds.length > 0 && !summary.allLoadingEnabled && (
                            <span
                              className="md-pill"
                              data-tone={summary.noneLoadingEnabled ? 'surface' : 'tertiary'}
                            >
                              {summary.noneLoadingEnabled
                                ? t('common.disabled')
                                : t('organize.statusPartial', {
                                    enabled: summary.enabledLoading.length,
                                    total: summary.loading.length,
                                  })}
                            </span>
                          )}
                        </div>

                        {summary.installed.length > 0 && (
                          <div className="l4a-row-wrap" style={{ gap: 6, flexWrap: 'nowrap' }}>
                            {summary.firstPath && (
                              <IconButton
                                size="sm"
                                label={t('common.openInExplorer')}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  onOpenInFileManager(summary.firstPath!);
                                }}
                              >
                                <FolderOpen size={15} />
                              </IconButton>
                            )}
                            {loadingIds.length > 0 && (
                              <Switch
                                compact
                                checked={switchChecked}
                                disabled={busy}
                                label={bundle.name}
                                onChange={(next) => onBatchToggle(loadingIds, next)}
                              />
                            )}
                          </div>
                        )}
                      </div>
                    </div>
                  </article>
                );
              }

              const { addon } = entry;
              const displayName = formatAddonDisplayName(addon, namePreference);
              const selected = validSelectedIds.has(addon.id);
              const installed = addon.installState === 'installed';
              const remoteUnavailable =
                addon.remoteStatus === 'removed' ||
                addon.remoteStatus === 'private_or_unavailable';
              const dl = addon.workshopId ? liveDownloads[addon.workshopId] : undefined;
              return (
                <article
                  key={addon.id}
                  className="md-card"
                  data-clickable="true"
                  data-selected={selected ? 'true' : 'false'}
                  data-muted={installed && !addon.enabled ? 'true' : 'false'}
                >
                  <div className="l4a-addon-cover-wrap">
                    <button
                      type="button"
                      tabIndex={-1}
                      aria-hidden="true"
                      className="l4a-card-trigger"
                      style={{ height: '100%' }}
                      onClick={() => {
                        if (batchMode) toggleSelect(addon.id);
                        else onSelectAddon(addon);
                      }}
                    >
                      <AddonCover
                        src={addon.coverImage}
                        title={displayName}
                        category={addon.categories[0]}
                      />
                    </button>
                    <div className="l4a-cover-overlay-top">
                      <div
                        className="l4a-card-check"
                        data-visible={showBatchBar || selected ? 'true' : 'false'}
                      >
                        <Checkbox
                          checked={selected}
                          label={displayName}
                          onChange={() => toggleSelect(addon.id)}
                        />
                      </div>
                      <div className="l4a-status-pills">
                        {addon.hasDummyBypass && (
                          <span className="md-pill" data-tone="primary">
                            <ShieldCheck size={12} />
                          </span>
                        )}
                        {addon.updateAvailable && (
                          <span className="md-pill" data-tone="tertiary">
                            <ArrowUpCircle size={12} />
                          </span>
                        )}
                        {addon.remoteStatus === 'removed' && (
                          <span className="md-pill" data-tone="error">
                            <CloudOff size={12} />
                          </span>
                        )}
                        {addon.remoteStatus === 'private_or_unavailable' && (
                          <span className="md-pill" data-tone="error">
                            <CloudOff size={12} />
                            {t('library.privateUpstream')}
                          </span>
                        )}
                        {addon.installState === 'uninstalled_known' && (
                          <span className="md-pill" data-tone="surface">
                            {t('library.uninstalledKnown')}
                          </span>
                        )}
                        {addon.installState === 'missing_file' && (
                          <span className="md-pill" data-tone="error">
                            <AlertTriangle size={12} />
                            {t('library.missingFile')}
                          </span>
                        )}
                      </div>
                    </div>
                  </div>

                  {dl && (
                    <LinearProgress
                      value={dl.percent}
                      indeterminate={dl.cancelling || dl.percent === 0}
                      label={displayName}
                    />
                  )}

                  <div className="l4a-addon-card-body">
                    <button
                      type="button"
                      className="l4a-card-trigger"
                      onClick={() => {
                        if (batchMode) toggleSelect(addon.id);
                        else onSelectAddon(addon);
                      }}
                    >
                      <div className="l4a-addon-title">{displayName}</div>
                    </button>
                    <div className="l4a-addon-meta">
                      <span className="l4a-truncate">{addon.author || t('common.unknown')}</span>
                      <span>·</span>
                      <span style={{ flexShrink: 0 }}>{formatBytes(addon.fileSize)}</span>
                      {addon.maps.length > 0 && (
                        <>
                          <span>·</span>
                          <span
                            style={{
                              flexShrink: 0,
                              display: 'inline-flex',
                              alignItems: 'center',
                              gap: 3,
                            }}
                          >
                            <MapIcon size={12} />
                            {addon.maps.length}
                          </span>
                        </>
                      )}
                    </div>

                    <div className="l4a-addon-card-footer">
                      <div className="l4a-row-wrap" style={{ gap: 6 }}>
                        {addon.primaryDir && (
                          <span className="md-pill" data-tone="surface">
                            {addon.primaryDir === 'loading' ? (
                              <Folder size={11} />
                            ) : (
                              <Store size={11} />
                            )}
                            {addon.primaryDir === 'loading'
                              ? t('library.loadingDir')
                              : t('library.workshopDir')}
                          </span>
                        )}
                        {installed && !addon.enabled && (
                          <span className="md-pill" data-tone="surface">
                            {t('common.disabled')}
                          </span>
                        )}
                      </div>

                      {installed ? (
                        <div className="l4a-row-wrap" style={{ gap: 6, flexWrap: 'nowrap' }}>
                          {addon.currentPath && (
                            <IconButton
                              size="sm"
                              label={t('common.openInExplorer')}
                              onClick={(e) => {
                                e.stopPropagation();
                                onOpenInFileManager(addon.currentPath!);
                              }}
                            >
                              <FolderOpen size={15} />
                            </IconButton>
                          )}
                          <Switch
                            compact
                            checked={addon.enabled}
                            disabled={busy}
                            label={displayName}
                            onChange={(next) => onToggleSingle(addon, next)}
                          />
                        </div>
                      ) : addon.workshopId && !remoteUnavailable ? (
                        <Button
                          variant="tonal"
                          size="sm"
                          disabled={Boolean(dl)}
                          icon={<Download size={13} />}
                          onClick={() =>
                            onDownloadItem(addon.workshopId!, resolveAddonMetadataTitle(addon))
                          }
                        >
                          {dl
                            ? dl.cancelling
                              ? t('common.cancelling')
                              : `${dl.percent}%`
                            : t('common.download')}
                        </Button>
                      ) : null}
                    </div>
                  </div>
                </article>
              );
            })}
          </div>
        ) : (
          <div className="l4a-list">
            {displayItems.map((entry) => {
              if (entry.kind === 'bundle') {
                const { bundle, allMembers, visibleMembers } = entry;
                const summary = summarizeBundle(allMembers);
                const visibleIds = visibleMembers.map((m) => m.id);
                const selected =
                  visibleIds.length > 0 && visibleIds.every((id) => validSelectedIds.has(id));
                const loadingIds = summary.loading.map((m) => m.id);
                const muted =
                  summary.installed.length > 0 && summary.enabledInstalled.length === 0;
                const primaryDir: AddonDirType | undefined =
                  summary.loading.length > 0
                    ? 'loading'
                    : summary.workshopInstalled.length > 0
                      ? 'workshop'
                      : undefined;

                return (
                  <div
                    key={`bundle:${bundle.id}`}
                    className="l4a-list-row"
                    data-selected={selected ? 'true' : 'false'}
                    data-muted={muted ? 'true' : 'false'}
                  >
                    <Checkbox
                      checked={selected}
                      label={bundle.name}
                      onChange={() => toggleSelectMany(visibleIds)}
                    />
                    <div className="l4a-list-thumb">
                      <AddonCover
                        compact
                        src={bundle.coverImage ?? summary.firstCover}
                        title={bundle.name}
                        category="Campaign"
                      />
                    </div>
                    <button
                      type="button"
                      className="l4a-card-trigger l4a-list-main"
                      onClick={() => {
                        if (batchMode) toggleSelectMany(visibleIds);
                        else {
                          setBundleTab('members');
                          setInspectedBundleId(bundle.id);
                        }
                      }}
                    >
                      <div className="l4a-row-wrap" style={{ gap: 6 }}>
                        <span className="md-title-small l4a-truncate">{bundle.name}</span>
                        <span className="md-pill" data-tone="secondary">
                          <Layers size={11} />
                          {t('library.bundleParts', { count: allMembers.length })}
                        </span>
                        {summary.hasDummyBypass && (
                          <span className="md-pill" data-tone="primary">
                            <ShieldCheck size={11} />
                          </span>
                        )}
                        {summary.updateAvailable && (
                          <span className="md-pill" data-tone="tertiary">
                            <ArrowUpCircle size={11} />
                          </span>
                        )}
                        {summary.removedUpstream && (
                          <span className="md-pill" data-tone="error">
                            <CloudOff size={11} />
                          </span>
                        )}
                        {loadingIds.length > 0 && !summary.allLoadingEnabled && (
                          <span
                            className="md-pill"
                            data-tone={summary.noneLoadingEnabled ? 'surface' : 'tertiary'}
                          >
                            {summary.noneLoadingEnabled
                              ? t('common.disabled')
                              : t('organize.statusPartial', {
                                  enabled: summary.enabledLoading.length,
                                  total: summary.loading.length,
                                })}
                          </span>
                        )}
                      </div>
                      <div className="l4a-addon-meta">
                        <span className="l4a-truncate">
                          {summary.firstAuthor || t('common.unknown')}
                        </span>
                        <span>·</span>
                        <span>{formatBytes(summary.totalSize)}</span>
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
                    </button>
                    <div className="l4a-list-trailing">
                      {primaryDir && (
                        <span className="md-pill" data-tone="surface">
                          {primaryDir === 'loading'
                            ? t('library.loadingDir')
                            : t('library.workshopDir')}
                        </span>
                      )}
                      {summary.firstPath && (
                        <IconButton
                          size="sm"
                          label={t('common.openInExplorer')}
                          onClick={(e) => {
                            e.stopPropagation();
                            onOpenInFileManager(summary.firstPath!);
                          }}
                        >
                          <FolderOpen size={15} />
                        </IconButton>
                      )}
                      {loadingIds.length > 0 && (
                        <Switch
                          compact
                          checked={summary.allLoadingEnabled}
                          disabled={busy}
                          label={bundle.name}
                          onChange={(next) => onBatchToggle(loadingIds, next)}
                        />
                      )}
                    </div>
                  </div>
                );
              }

              const { addon } = entry;
              const displayName = formatAddonDisplayName(addon, namePreference);
              const selected = validSelectedIds.has(addon.id);
              const installed = addon.installState === 'installed';
              const remoteUnavailable =
                addon.remoteStatus === 'removed' ||
                addon.remoteStatus === 'private_or_unavailable';
              const dl = addon.workshopId ? liveDownloads[addon.workshopId] : undefined;
              return (
                <div
                  key={addon.id}
                  className="l4a-list-row"
                  data-selected={selected ? 'true' : 'false'}
                  data-muted={installed && !addon.enabled ? 'true' : 'false'}
                >
                  <Checkbox
                    checked={selected}
                    label={displayName}
                    onChange={() => toggleSelect(addon.id)}
                  />
                  <div className="l4a-list-thumb">
                    <AddonCover
                      compact
                      src={addon.coverImage}
                      title={displayName}
                      category={addon.categories[0]}
                    />
                  </div>
                  <button
                    type="button"
                    className="l4a-card-trigger l4a-list-main"
                    onClick={() => {
                      if (batchMode) toggleSelect(addon.id);
                      else onSelectAddon(addon);
                    }}
                  >
                    <div className="l4a-row-wrap" style={{ gap: 6 }}>
                      <span className="md-title-small l4a-truncate">{displayName}</span>
                      {addon.hasDummyBypass && (
                        <span className="md-pill" data-tone="primary">
                          <ShieldCheck size={11} />
                        </span>
                      )}
                      {addon.updateAvailable && (
                        <span className="md-pill" data-tone="tertiary">
                          <ArrowUpCircle size={11} />
                        </span>
                      )}
                      {remoteUnavailable && (
                        <span className="md-pill" data-tone="error">
                          <CloudOff size={11} />
                        </span>
                      )}
                      {addon.installState === 'missing_file' && (
                        <span className="md-pill" data-tone="error">
                          {t('library.missingFile')}
                        </span>
                      )}
                      {installed && !addon.enabled && (
                        <span className="md-pill" data-tone="surface">
                          {t('common.disabled')}
                        </span>
                      )}
                    </div>
                    <div className="l4a-addon-meta">
                      <span className="l4a-truncate">{addon.author || t('common.unknown')}</span>
                      <span>·</span>
                      <span>{formatBytes(addon.fileSize)}</span>
                      {addon.workshopId && (
                        <>
                          <span>·</span>
                          <span className="md-mono">#{addon.workshopId}</span>
                        </>
                      )}
                    </div>
                  </button>
                  <div className="l4a-list-trailing">
                    {addon.primaryDir && (
                      <span className="md-pill" data-tone="surface">
                        {addon.primaryDir === 'loading'
                          ? t('library.loadingDir')
                          : t('library.workshopDir')}
                      </span>
                    )}
                    {installed ? (
                      <>
                        {addon.currentPath && (
                          <IconButton
                            size="sm"
                            label={t('common.openInExplorer')}
                            onClick={(e) => {
                              e.stopPropagation();
                              onOpenInFileManager(addon.currentPath!);
                            }}
                          >
                            <FolderOpen size={15} />
                          </IconButton>
                        )}
                        <Switch
                          compact
                          checked={addon.enabled}
                          disabled={busy}
                          label={displayName}
                          onChange={(next) => onToggleSingle(addon, next)}
                        />
                      </>
                    ) : addon.workshopId && !remoteUnavailable ? (
                      <Button
                        variant="tonal"
                        size="sm"
                        disabled={Boolean(dl)}
                        icon={<Download size={13} />}
                        onClick={() =>
                          onDownloadItem(addon.workshopId!, resolveAddonMetadataTitle(addon))
                        }
                      >
                        {dl
                          ? dl.cancelling
                            ? t('common.cancelling')
                            : `${dl.percent}%`
                          : t('common.download')}
                      </Button>
                    ) : null}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Inline Campaign Bundle Inspector */}
      <SideSheet
        open={Boolean(inspectedBundle)}
        title={inspectedBundle?.name ?? ''}
        onClose={() => setInspectedBundleId(null)}
        headerTrailing={
          inspectedBundle &&
          (() => {
            const members = inspectedBundle.memberAddonIds
              .map((id) => addonsById.get(id))
              .filter((a): a is AddonSummary => Boolean(a));
            const summary = summarizeBundle(members);
            const loadingIds = summary.loading.map((m) => m.id);
            if (loadingIds.length === 0) return undefined;
            return (
              <Switch
                checked={summary.allLoadingEnabled}
                disabled={busy}
                label={
                  summary.allLoadingEnabled ? t('common.enabled') : t('common.disabled')
                }
                onChange={(next) => onBatchToggle(loadingIds, next)}
              />
            );
          })()
        }
        footer={
          inspectedBundle &&
          (() => {
            const members = inspectedBundle.memberAddonIds
              .map((id) => addonsById.get(id))
              .filter((a): a is AddonSummary => Boolean(a));
            const summary = summarizeBundle(members);
            const loadingIds = summary.loading.map((m) => m.id);
            const workshopIds = summary.workshopInstalled.map((m) => m.id);
            return (
              <>
                {summary.firstPath && (
                  <Button
                    variant="tonal"
                    size="sm"
                    icon={<FolderOpen size={15} />}
                    onClick={() => onOpenInFileManager(summary.firstPath!)}
                  >
                    {t('common.openInExplorer')}
                  </Button>
                )}
                {loadingIds.length > 0 && (
                  <Button
                    variant="text"
                    size="sm"
                    disabled={busy}
                    icon={<Pencil size={15} />}
                    onClick={() => onBatchRename(loadingIds)}
                  >
                    {t('library.autoRename')}
                  </Button>
                )}
                {workshopIds.length > 0 && (
                  <Button
                    variant="tonal"
                    size="sm"
                    disabled={busy}
                    icon={<ArrowRightLeft size={15} />}
                    onClick={() => onBatchMove(workshopIds, 'loading')}
                  >
                    {t('library.moveToLoading')}
                  </Button>
                )}
                {members.length > 0 && (
                  <Button
                    variant="text"
                    tone="error"
                    size="sm"
                    disabled={busy}
                    icon={<Trash2 size={15} />}
                    onClick={() => onBatchDelete(members)}
                  >
                    {t('common.delete')}
                  </Button>
                )}
              </>
            );
          })()
        }
      >
        {inspectedBundle &&
          (() => {
            const members = inspectedBundle.memberAddonIds
              .map((id) => addonsById.get(id))
              .filter((a): a is AddonSummary => Boolean(a));
            const summary = summarizeBundle(members);
            const hasBundleMaps = inspectedBundle.maps.length > 0;
            const activeTab: 'members' | 'maps' = hasBundleMaps ? bundleTab : 'members';

            return (
              <>
                {hasBundleMaps && (
                  <Tabs
                    compact
                    idPrefix={bundleTabsId}
                    value={activeTab}
                    onChange={setBundleTab}
                    tabs={[
                      {
                        value: 'members',
                        label: t('organize.members'),
                        count: members.length,
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
                  id={hasBundleMaps ? `${bundleTabsId}-panel-${activeTab}` : undefined}
                  aria-labelledby={hasBundleMaps ? `${bundleTabsId}-tab-${activeTab}` : undefined}
                >
                  {activeTab === 'members' ? (
                    <>
                      <div className="md-chip-row">
                        <span className="md-pill" data-tone="secondary">
                          <Layers size={12} />
                          {t('library.bundleParts', { count: members.length })}
                        </span>
                        <span className="md-pill" data-tone="surface">
                          {formatBytes(summary.totalSize)}
                        </span>
                        {summary.loading.length > 0 && (
                          <span
                            className="md-pill"
                            data-tone={
                              summary.allLoadingEnabled
                                ? 'primary'
                                : summary.noneLoadingEnabled
                                  ? 'surface'
                                  : 'tertiary'
                            }
                          >
                            {summary.allLoadingEnabled
                              ? t('organize.statusEnabled')
                              : summary.noneLoadingEnabled
                                ? t('organize.statusDisabled')
                                : t('organize.statusPartial', {
                                    enabled: summary.enabledLoading.length,
                                    total: summary.loading.length,
                                  })}
                          </span>
                        )}
                      </div>

                      <div className="l4a-list">
                        {members.map((member, idx) => {
                          const installed = member.installState === 'installed';
                          const remoteUnavailable =
                            member.remoteStatus === 'removed' ||
                            member.remoteStatus === 'private_or_unavailable';
                          const dl = member.workshopId
                            ? liveDownloads[member.workshopId]
                            : undefined;
                          const memberLabel = formatBundleMemberLabel(
                            member,
                            members,
                            namePreference,
                            inspectedBundle.name,
                          );
                          return (
                            <div
                              key={member.id}
                              className="l4a-list-row l4a-list-row-dense"
                              data-muted={installed && !member.enabled ? 'true' : 'false'}
                            >
                              <div className="l4a-picker-thumb">
                                <AddonCover
                                  compact
                                  src={member.coverImage}
                                  title={memberLabel}
                                  category={member.categories[0]}
                                />
                              </div>
                              <button
                                type="button"
                                className="l4a-card-trigger l4a-list-main"
                                onClick={() => {
                                  setInspectedBundleId(null);
                                  onSelectAddon(member);
                                }}
                              >
                                <div className="l4a-row-wrap" style={{ gap: 6 }}>
                                  <span className="md-pill md-mono" data-tone="secondary">
                                    #{idx + 1}
                                  </span>
                                  <span className="md-title-small l4a-truncate">
                                    {memberLabel}
                                  </span>
                                  {member.updateAvailable && (
                                    <span className="md-pill" data-tone="tertiary">
                                      <ArrowUpCircle size={11} />
                                    </span>
                                  )}
                                  {member.installState === 'missing_file' && (
                                    <span className="md-pill" data-tone="error">
                                      {t('library.missingFile')}
                                    </span>
                                  )}
                                  {member.installState === 'uninstalled_known' && (
                                    <span className="md-pill" data-tone="surface">
                                      {t('scope.uninstalled')}
                                    </span>
                                  )}
                                </div>
                                <div className="l4a-addon-meta">
                                  {member.currentFileName ? (
                                    <span className="md-mono l4a-truncate">
                                      {member.currentFileName}
                                    </span>
                                  ) : (
                                    <span className="l4a-truncate">
                                      {member.author || t('common.unknown')}
                                    </span>
                                  )}
                                  <span>·</span>
                                  <span>{formatBytes(member.fileSize)}</span>
                                  {member.maps.length > 0 && (
                                    <>
                                      <span>·</span>
                                      <span
                                        style={{
                                          display: 'inline-flex',
                                          alignItems: 'center',
                                          gap: 3,
                                        }}
                                      >
                                        <MapIcon size={11} />
                                        {member.maps.length}
                                      </span>
                                    </>
                                  )}
                                </div>
                              </button>
                              <div className="l4a-list-trailing">
                                {installed ? (
                                  <>
                                    {member.currentPath && (
                                      <IconButton
                                        size="sm"
                                        label={t('common.openInExplorer')}
                                        onClick={() => onOpenInFileManager(member.currentPath!)}
                                      >
                                        <FolderOpen size={15} />
                                      </IconButton>
                                    )}
                                    <Switch
                                      compact
                                      checked={member.enabled}
                                      disabled={busy}
                                      label={memberLabel}
                                      onChange={(next) => onToggleSingle(member, next)}
                                    />
                                  </>
                                ) : member.workshopId && !remoteUnavailable ? (
                                  <Button
                                    variant="tonal"
                                    size="sm"
                                    disabled={Boolean(dl)}
                                    icon={<Download size={13} />}
                                    onClick={() =>
                                      onDownloadItem(
                                        member.workshopId!,
                                        resolveAddonMetadataTitle(member),
                                      )
                                    }
                                  >
                                    {dl
                                      ? dl.cancelling
                                        ? t('common.cancelling')
                                        : `${dl.percent}%`
                                      : t('common.download')}
                                  </Button>
                                ) : null}
                              </div>
                            </div>
                          );
                        })}
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
    </div>
  );
}
