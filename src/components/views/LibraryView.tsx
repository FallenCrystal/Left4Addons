import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AlertTriangle,
  ArrowRightLeft,
  ArrowUpCircle,
  CheckSquare,
  CloudOff,
  Download,
  Folder,
  FolderPlus,
  LayoutGrid,
  Layers,
  List,
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
import type { AddonDirType, AddonSummary } from '../../api/ipc';
import type { LiveDownloadState } from '../../state/useAppController';
import { formatBytes, matchesAddonSearch } from '../../utils/format';
import { AddonCover } from '../m3/AddonCover';
import {
  Button,
  Checkbox,
  EmptyState,
  FilterChip,
  IconButton,
  LinearProgress,
  SegmentedButton,
  Switch,
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
  onDownloadItem: (workshopId: string) => void;
  onUpdateItem: (workshopId: string) => void;
}

export function LibraryView({
  addons,
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

  const filteredAddons = useMemo(() => {
    const extAddonSet = externalFilter?.addonIds ? new Set(externalFilter.addonIds) : null;
    const extWorkshopSet = externalFilter?.workshopIds
      ? new Set(externalFilter.workshopIds)
      : null;

    const list = addons.filter((a) => {
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

      if (category !== 'All') {
        if (category === 'Other') {
          if (a.categories.length > 0 && !a.categories.includes('Other')) return false;
        } else if (!a.categories.includes(category)) {
          return false;
        }
      }

      if (trimmedQuery) {
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
      }
      return true;
    });

    return [...list].sort((a, b) => {
      if (sortBy === 'size') return b.fileSize - a.fileSize;
      if (sortBy === 'workshopId') {
        return (b.workshopId ?? '').localeCompare(a.workshopId ?? '', undefined, {
          numeric: true,
        });
      }
      if (sortBy === 'status') {
        const score = (x: AddonSummary) =>
          x.installState !== 'installed' ? 2 : x.enabled ? 0 : 1;
        const diff = score(a) - score(b);
        if (diff !== 0) return diff;
      }
      return a.title.localeCompare(b.title);
    });
  }, [addons, externalFilter, scope, category, trimmedQuery, mapMode, mapMatchedIds, sortBy]);

  const existingIds = useMemo(() => new Set(addons.map((a) => a.id)), [addons]);
  const validSelectedIds = useMemo(() => {
    const next = new Set<string>();
    for (const id of selectedIds) {
      if (existingIds.has(id)) next.add(id);
    }
    return next;
  }, [selectedIds, existingIds]);

  const visibleSelectedAddons = useMemo(
    () => filteredAddons.filter((a) => validSelectedIds.has(a.id)),
    [filteredAddons, validSelectedIds],
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

  const clearSelection = () => {
    setSelectedIds(new Set());
    setBatchMode(false);
  };

  const keepOnlyVisibleSelection = () => {
    setSelectedIds(new Set(visibleSelectedIds));
  };

  const selectAllVisible = () => {
    setSelectedIds(new Set(filteredAddons.map((a) => a.id)));
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

        <select
          className="l4a-select"
          aria-label={t('common.sort')}
          value={sortBy}
          onChange={(e) => setSortBy(e.target.value as LibrarySort)}
        >
          <option value="title">{t('sort.title')}</option>
          <option value="size">{t('sort.size')}</option>
          <option value="workshopId">{t('sort.workshopId')}</option>
          <option value="status">{t('sort.status')}</option>
        </select>

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

      {availableCategories.length > 0 && (
        <div className="l4a-view-toolbar" style={{ paddingTop: 0 }}>
          <div className="md-chip-row">
            <FilterChip
              label={t('common.all')}
              selected={category === 'All'}
              onClick={() => setCategory('All')}
            />
            {availableCategories.map(([cat, count]) => (
              <FilterChip
                key={cat}
                label={t(`categories.${cat}`, { defaultValue: cat })}
                count={count}
                selected={category === cat}
                onClick={() => setCategory((curr) => (curr === cat ? 'All' : cat))}
              />
            ))}
          </div>
        </div>
      )}

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
                  if (item.workshopId) onDownloadItem(item.workshopId);
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
                  if (item.workshopId) onUpdateItem(item.workshopId);
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
        {filteredAddons.length === 0 ? (
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
            {filteredAddons.map((addon) => {
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
                        title={addon.title}
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
                          label={addon.title}
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
                      label={addon.title}
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
                      <div className="l4a-addon-title">{addon.title}</div>
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
                        <Switch
                          compact
                          checked={addon.enabled}
                          disabled={busy}
                          label={addon.title}
                          onChange={(next) => onToggleSingle(addon, next)}
                        />
                      ) : addon.workshopId && !remoteUnavailable ? (
                        <Button
                          variant="tonal"
                          size="sm"
                          disabled={Boolean(dl)}
                          icon={<Download size={13} />}
                          onClick={() => onDownloadItem(addon.workshopId!)}
                        >
                          {dl ? (dl.cancelling ? t('common.cancelling') : `${dl.percent}%`) : t('common.download')}
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
            {filteredAddons.map((addon) => {
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
                    label={addon.title}
                    onChange={() => toggleSelect(addon.id)}
                  />
                  <div className="l4a-list-thumb">
                    <AddonCover
                      compact
                      src={addon.coverImage}
                      title={addon.title}
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
                      <span className="md-title-small l4a-truncate">{addon.title}</span>
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
                      <Switch
                        compact
                        checked={addon.enabled}
                        disabled={busy}
                        label={addon.title}
                        onChange={(next) => onToggleSingle(addon, next)}
                      />
                    ) : addon.workshopId && !remoteUnavailable ? (
                      <Button
                        variant="tonal"
                        size="sm"
                        disabled={Boolean(dl)}
                        icon={<Download size={13} />}
                        onClick={() => onDownloadItem(addon.workshopId!)}
                      >
                        {dl ? (dl.cancelling ? t('common.cancelling') : `${dl.percent}%`) : t('common.download')}
                      </Button>
                    ) : null}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
