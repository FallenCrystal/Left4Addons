import { useTranslation } from 'react-i18next';
import {
  ArrowUpCircle,
  CheckSquare,
  CloudOff,
  EyeOff,
  LayoutGrid,
  Layers,
  List,
  ListFilter,
  Map as MapIcon,
  RefreshCw,
  Search,
  X,
} from 'lucide-react';
import {
  Button,
  FilterChip,
  IconButton,
  SegmentedButton,
  Select,
} from '../../m3/primitives';
import type {
  ExternalAddonFilter,
  LibraryScope,
  LibraryScopeCounts,
  LibrarySort,
} from './helpers';

export interface LibraryToolbarProps {
  query: string;
  onQueryChange: (next: string) => void;
  mapMode: boolean;
  onToggleMapMode: () => void;
  sortBy: LibrarySort;
  onSortChange: (next: LibrarySort) => void;
  layout: 'grid' | 'list';
  onToggleLayout: () => void;
  showBatchBar: boolean;
  onToggleBatch: () => void;
  syncing: boolean;
  onSync: () => void;
  scope: LibraryScope;
  onScopeChange: (next: LibraryScope) => void;
  counts: LibraryScopeCounts;
  hasCategories: boolean;
  effectiveCategory: string;
  categoryOptions: Array<{ value: string; label: string; count: number }>;
  onCategoryChange: (next: string) => void;
  hasCollapsibleBundles: boolean;
  collapseBundles: boolean;
  onToggleCollapseBundles?: (next: boolean) => void;
  externalFilter: ExternalAddonFilter | null;
  onClearExternalFilter: () => void;
  filteredOutCount?: number;
  showingFilteredCards?: boolean;
  onToggleShowFilteredCards?: () => void;
  evalError?: string | null;
  onRetryEval?: () => void;
}

export function LibraryToolbar({
  query,
  onQueryChange,
  mapMode,
  onToggleMapMode,
  sortBy,
  onSortChange,
  layout,
  onToggleLayout,
  showBatchBar,
  onToggleBatch,
  syncing,
  onSync,
  scope,
  onScopeChange,
  counts,
  hasCategories,
  effectiveCategory,
  categoryOptions,
  onCategoryChange,
  hasCollapsibleBundles,
  collapseBundles,
  onToggleCollapseBundles,
  externalFilter,
  onClearExternalFilter,
  filteredOutCount = 0,
  showingFilteredCards = false,
  onToggleShowFilteredCards,
  evalError = null,
  onRetryEval,
}: LibraryToolbarProps) {
  const { t } = useTranslation();

  return (
    <>
      <header className="l4a-view-header">
        <div className="md-search-bar">
          <Search
            size={18}
            style={{ color: 'var(--md-sys-color-on-surface-variant)', flexShrink: 0 }}
          />
          <input
            className="md-search-input"
            value={query}
            onChange={(e) => onQueryChange(e.target.value)}
            placeholder={
              mapMode ? t('library.mapSearchPlaceholder') : t('library.searchPlaceholder')
            }
            aria-label={t('common.search')}
          />
          {query && (
            <IconButton size="sm" label={t('common.clear')} onClick={() => onQueryChange('')}>
              <X size={16} />
            </IconButton>
          )}
          <FilterChip
            label={t('library.mapMode')}
            selected={mapMode}
            icon={<MapIcon size={14} />}
            onClick={onToggleMapMode}
          />
        </div>

        <Select<LibrarySort>
          ariaLabel={t('common.sort')}
          value={sortBy}
          onChange={onSortChange}
          options={[
            { value: 'title', label: t('sort.title') },
            { value: 'size', label: t('sort.size') },
            { value: 'workshopId', label: t('sort.workshopId') },
            { value: 'status', label: t('sort.status') },
          ]}
        />

        <IconButton
          label={layout === 'grid' ? t('library.listView') : t('library.gridView')}
          onClick={onToggleLayout}
        >
          {layout === 'grid' ? <List size={20} /> : <LayoutGrid size={20} />}
        </IconButton>

        <IconButton label={t('library.batch')} active={showBatchBar} onClick={onToggleBatch}>
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
          onChange={onScopeChange}
          options={[
            { value: 'all', label: t('scope.all'), count: counts.all },
            { value: 'loading', label: t('scope.loading'), count: counts.loading },
            { value: 'workshop', label: t('scope.workshop'), count: counts.workshop },
            { value: 'disabled', label: t('scope.disabled'), count: counts.disabled },
            { value: 'uninstalled', label: t('scope.uninstalled'), count: counts.uninstalled },
          ]}
        />

        {hasCategories && (
          <Select<string>
            variant="filter"
            align="start"
            active={effectiveCategory !== 'All'}
            ariaLabel={t('library.category')}
            icon={<ListFilter size={14} />}
            value={effectiveCategory}
            onChange={onCategoryChange}
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
            onClick={() => onScopeChange(scope === 'updates' ? 'all' : 'updates')}
          />
        )}

        {counts.removed > 0 && (
          <FilterChip
            label={t('scope.removed')}
            count={counts.removed}
            selected={scope === 'removed'}
            tone={scope === 'removed' ? 'default' : 'error'}
            icon={<CloudOff size={14} />}
            onClick={() => onScopeChange(scope === 'removed' ? 'all' : 'removed')}
          />
        )}

        {filteredOutCount > 0 && (
          <FilterChip
            label={
              showingFilteredCards
                ? t('filter.filteredCountShowing', { count: filteredOutCount })
                : t('filter.filteredCount', { count: filteredOutCount })
            }
            selected={showingFilteredCards}
            tone={showingFilteredCards ? 'tertiary' : 'surface'}
            icon={<EyeOff size={14} />}
            onClick={onToggleShowFilteredCards}
            onRemove={showingFilteredCards ? onToggleShowFilteredCards : undefined}
          />
        )}

        {evalError && onRetryEval && (
          <FilterChip
            label={t('filter.evalFailedRetry')}
            tone="error"
            icon={<RefreshCw size={13} />}
            onClick={onRetryEval}
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
    </>
  );
}
