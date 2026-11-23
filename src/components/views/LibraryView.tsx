import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Package } from 'lucide-react';
import type {
  AddonDirType,
  AddonNamePreference,
  AddonSummary,
  BundleRecord,
} from '../../api/ipc';
import type { LiveDownloadState } from '../../state/useAppController';
import { Button, EmptyState } from '../m3/primitives';
import { BundleDetailSheet } from './library/BundleDetailSheet';
import {
  buildLibraryDisplayItems,
  computeAvailableCategories,
  computeLibraryCounts,
  type ExternalAddonFilter,
  type LibraryScope,
  type LibrarySort,
} from './library/helpers';
import { LibraryBatchBar } from './library/LibraryBatchBar';
import { LibraryGrid } from './library/LibraryGrid';
import { LibraryList } from './library/LibraryList';
import { LibraryToolbar } from './library/LibraryToolbar';

export type { ExternalAddonFilter, LibraryScope, LibrarySort };

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

export function LibraryView({
  addons,
  bundles,
  namePreference = 'workshop_title',
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

  const counts = useMemo(() => computeLibraryCounts(addons), [addons]);
  const availableCategories = useMemo(() => computeAvailableCategories(addons), [addons]);

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

  const { displayItems, visibleAddons } = useMemo(
    () =>
      buildLibraryDisplayItems({
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
      }),
    [
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
    ],
  );

  const existingIds = useMemo(() => new Set(addons.map((a) => a.id)), [addons]);
  const validSelectedIds = useMemo(() => {
    const next = new Set<string>();
    for (const id of selectedIds) {
      if (existingIds.has(id)) next.add(id);
    }
    return next;
  }, [selectedIds, existingIds]);

  const visibleSelectedAddons = useMemo(() => {
    const out: AddonSummary[] = [];
    const seen = new Set<string>();
    for (const item of displayItems) {
      if (item.kind === 'addon') {
        if (validSelectedIds.has(item.addon.id) && !seen.has(item.addon.id)) {
          seen.add(item.addon.id);
          out.push(item.addon);
        }
      } else {
        const eligible =
          item.matchedMembers.length > 0
            ? item.matchedMembers
            : item.visibleMembers.length > 0 &&
                item.visibleMembers.every((m) => validSelectedIds.has(m.id))
              ? item.visibleMembers
              : [];
        for (const m of eligible) {
          if (validSelectedIds.has(m.id) && !seen.has(m.id)) {
            seen.add(m.id);
            out.push(m);
          }
        }
      }
    }
    return out;
  }, [displayItems, validSelectedIds]);

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

  const handleInspectBundle = (bundleId: string) => {
    setBundleTab('members');
    setInspectedBundleId(bundleId);
  };

  const inspectedBundle = useMemo(
    () => (inspectedBundleId ? (bundles.find((b) => b.id === inspectedBundleId) ?? null) : null),
    [bundles, inspectedBundleId],
  );

  return (
    <div className="l4a-view">
      <LibraryToolbar
        query={query}
        onQueryChange={setQuery}
        mapMode={mapMode}
        onToggleMapMode={() => setMapMode((v) => !v)}
        sortBy={sortBy}
        onSortChange={setSortBy}
        layout={layout}
        onToggleLayout={() => setLayout((l) => (l === 'grid' ? 'list' : 'grid'))}
        showBatchBar={showBatchBar}
        onToggleBatch={() => {
          if (showBatchBar) clearSelection();
          else setBatchMode(true);
        }}
        syncing={syncing}
        onSync={onSync}
        scope={scope}
        onScopeChange={setScope}
        counts={counts}
        hasCategories={availableCategories.length > 0}
        effectiveCategory={effectiveCategory}
        categoryOptions={categoryOptions}
        onCategoryChange={setCategory}
        hasCollapsibleBundles={hasCollapsibleBundles}
        collapseBundles={collapseBundles}
        onToggleCollapseBundles={onToggleCollapseBundles}
        externalFilter={externalFilter}
        onClearExternalFilter={onClearExternalFilter}
      />

      {showBatchBar && (
        <LibraryBatchBar
          visibleSelectedAddons={visibleSelectedAddons}
          visibleSelectedIds={visibleSelectedIds}
          hiddenSelectedCount={hiddenSelectedCount}
          namePreference={namePreference}
          busy={busy}
          onKeepOnlyVisibleSelection={keepOnlyVisibleSelection}
          onSelectAllVisible={selectAllVisible}
          onClearSelection={clearSelection}
          onBatchToggle={onBatchToggle}
          onBatchMove={onBatchMove}
          onBatchRename={onBatchRename}
          onBatchAddToGroup={onBatchAddToGroup}
          onBatchCreateBundle={onBatchCreateBundle}
          onBatchDelete={onBatchDelete}
          onDownloadItem={onDownloadItem}
          onUpdateItem={onUpdateItem}
        />
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
          <LibraryGrid
            displayItems={displayItems}
            namePreference={namePreference}
            validSelectedIds={validSelectedIds}
            batchMode={batchMode}
            showBatchBar={showBatchBar}
            liveDownloads={liveDownloads}
            busy={busy}
            onToggleSelect={toggleSelect}
            onToggleSelectMany={toggleSelectMany}
            onSelectAddon={onSelectAddon}
            onInspectBundle={handleInspectBundle}
            onToggleSingle={onToggleSingle}
            onBatchToggle={onBatchToggle}
            onDownloadItem={onDownloadItem}
            onOpenInFileManager={onOpenInFileManager}
          />
        ) : (
          <LibraryList
            displayItems={displayItems}
            namePreference={namePreference}
            validSelectedIds={validSelectedIds}
            batchMode={batchMode}
            liveDownloads={liveDownloads}
            busy={busy}
            onToggleSelect={toggleSelect}
            onToggleSelectMany={toggleSelectMany}
            onSelectAddon={onSelectAddon}
            onInspectBundle={handleInspectBundle}
            onToggleSingle={onToggleSingle}
            onBatchToggle={onBatchToggle}
            onDownloadItem={onDownloadItem}
            onOpenInFileManager={onOpenInFileManager}
          />
        )}
      </div>

      <BundleDetailSheet
        bundle={inspectedBundle}
        addonsById={addonsById}
        namePreference={namePreference}
        liveDownloads={liveDownloads}
        busy={busy}
        bundleTab={bundleTab}
        onBundleTabChange={setBundleTab}
        onClose={() => setInspectedBundleId(null)}
        onSelectAddon={onSelectAddon}
        onToggleSingle={onToggleSingle}
        onBatchToggle={onBatchToggle}
        onBatchRename={onBatchRename}
        onBatchMove={onBatchMove}
        onBatchDelete={onBatchDelete}
        onDownloadItem={onDownloadItem}
        onOpenInFileManager={onOpenInFileManager}
        onCopy={onCopy}
      />
    </div>
  );
}
