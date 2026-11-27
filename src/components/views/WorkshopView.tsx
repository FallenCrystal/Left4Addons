import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  ArrowUpCircle,
  BookmarkCheck,
  BookmarkPlus,
  ChevronLeft,
  ChevronRight,
  CloudDownload,
  Download,
  ExternalLink,
  Eye,
  EyeOff,
  RefreshCw,
  Search,
  Settings,
  ShieldAlert,
  Store,
  Users,
  X,
} from 'lucide-react';
import {
  IpcError,
  type AddonSummary,
  type SearchQuery,
  type SearchResponse,
  type TrackedCollectionRecord,
  type WorkshopFileType,
  type WorkshopItemRecord,
} from '../../api/ipc';
import { useContentFilterContext } from '../../state/ContentFilterContext';
import type { LiveDownloadState } from '../../state/useAppController';
import {
  addonFilterKey,
  collectWorkshopImageUrls,
  formatDiagnosticVerdictBadge,
  formatRedactedAddonLabel,
  hasFilterableWorkshopContent,
  isAddonTitleRedacted,
  isLinkedWorkshopTitleRedacted,
  matchesEvaluatedAddonTitle,
  resolveCardFilterState,
  resolveDetailFilterState,
  resolveWorkshopVerdict,
  workshopFilterKey,
} from '../../utils/contentFilter';
import {
  buildSteamProtocolUrl,
  buildWorkshopUrl,
  extractWorkshopIdInput,
  formatBytes,
  formatCompactNumber,
  formatTimestamp,
} from '../../utils/format';
import { AddonCover } from '../m3/AddonCover';
import { FilterCardOverlay, FilterFailureBadge } from '../m3/ContentFilterIndicators';
import {
  Button,
  Dialog,
  EmptyState,
  FilterChip,
  IconButton,
  LinearProgress,
  LoadingIndicator,
  SegmentedButton,
  Select,
  SideSheet,
} from '../m3/primitives';

const WORKSHOP_TAGS = [
  'Campaigns',
  'Maps',
  'Survivors',
  'Weapons',
  'Sounds',
  'Scripts',
  'Infected',
  'UI',
] as const;

export interface WorkshopViewProps {
  addons: AddonSummary[];
  collections: TrackedCollectionRecord[];
  liveDownloads: Record<string, LiveDownloadState>;
  busy?: boolean;
  onSearch: (query: SearchQuery) => Promise<SearchResponse>;
  onFetchItem: (workshopId: string) => Promise<WorkshopItemRecord | null>;
  onImportSubscribed: () => Promise<string[] | null>;
  onDownload: (workshopId: string, title?: string) => Promise<AddonSummary | null>;
  onUpdate: (workshopId: string, title?: string) => Promise<AddonSummary | null>;
  onCancelDownload: (workshopId: string) => void;
  onTrackCollection: (
    collectionId: string,
    autoSync: boolean,
  ) => Promise<TrackedCollectionRecord | null>;
  onOpenSourcesSettings: () => void;
  onOpenExternal: (url: string) => void;
  onInvalidId: () => void;
  onNotify: (msg: string) => void;
}

export function WorkshopView({
  addons,
  collections,
  liveDownloads,
  busy = false,
  onSearch,
  onFetchItem,
  onImportSubscribed,
  onDownload,
  onUpdate,
  onCancelDownload,
  onTrackCollection,
  onOpenSourcesSettings,
  onOpenExternal,
  onInvalidId,
  onNotify,
}: WorkshopViewProps) {
  const { t, i18n } = useTranslation();
  const searchLanguage = i18n.language?.trim() || undefined;
  const [trackedLanguage, setTrackedLanguage] = useState(searchLanguage);
  const [draftQuery, setDraftQuery] = useState('');
  const [searchParams, setSearchParams] = useState<{
    query: string;
    fileType: WorkshopFileType;
    tag?: string;
    sort: string;
    page: number;
    retryNonce: number;
  }>({
    query: '',
    fileType: 'item',
    tag: undefined,
    sort: 'relevance',
    page: 1,
    retryNonce: 0,
  });

  if (trackedLanguage !== searchLanguage) {
    setTrackedLanguage(searchLanguage);
    if (searchParams.page !== 1) {
      setSearchParams((prev) => (prev.page === 1 ? prev : { ...prev, page: 1 }));
    }
  }
  const [directInput, setDirectInput] = useState('');
  const [directBusy, setDirectBusy] = useState(false);
  const [trackingCollectionIds, setTrackingCollectionIds] = useState<Set<string>>(() => new Set());
  const [searchResult, setSearchResult] = useState<{
    reqKey: string;
    response: SearchResponse | null;
    error: { unsupported: boolean; message: string } | null;
  } | null>(null);
  const [activeItem, setActiveItem] = useState<WorkshopItemRecord | null>(null);
  const [missingSubs, setMissingSubs] = useState<string[] | null>(null);

  const searchSeqRef = useRef(0);
  const directInspectSeqRef = useRef(0);
  const reqKey = JSON.stringify({ ...searchParams, language: searchLanguage });

  const localByWorkshopId = useMemo(() => {
    const map = new Map<string, AddonSummary>();
    for (const a of addons) {
      if (a.workshopId) map.set(a.workshopId, a);
    }
    return map;
  }, [addons]);

  const trackedIds = useMemo(
    () => new Set(collections.map((c) => c.collectionId)),
    [collections],
  );

  useEffect(() => {
    searchSeqRef.current += 1;
    const seq = searchSeqRef.current;
    let active = true;

    void onSearch({
      query: searchParams.query.trim() || undefined,
      tag: searchParams.tag,
      sort: searchParams.sort === 'relevance' ? undefined : searchParams.sort,
      page: searchParams.page,
      pageSize: 24,
      language: searchLanguage,
      fileType: searchParams.fileType === 'item' ? undefined : searchParams.fileType,
    })
      .then((res) => {
        if (!active || seq !== searchSeqRef.current) return;
        setSearchResult({ reqKey, response: res, error: null });
      })
      .catch((err) => {
        if (!active || seq !== searchSeqRef.current) return;
        const unsupported = err instanceof IpcError && err.code === 'unsupported';
        setSearchResult({
          reqKey,
          response: null,
          error: {
            unsupported,
            message: err instanceof Error ? err.message : String(err),
          },
        });
      });

    return () => {
      active = false;
    };
  }, [searchParams, reqKey, searchLanguage, onSearch]);

  const loading = searchResult?.reqKey !== reqKey;
  const response = searchResult?.response ?? null;
  const searchError = searchResult?.error ?? null;

  const filterCtx = useContentFilterContext();
  const {
    filterConfig,
    verdicts,
    evaluatedRevisions,
    revealedCardIds,
    failedTargetKeys,
    revealCard,
    prioritizeFilterTargets,
    registerWorkshopItems,
  } = filterCtx;
  const [showingFilteredCards, setShowingFilteredCards] = useState(false);

  const filterResetKey = `${searchParams.query}|${searchParams.fileType}|${searchParams.tag ?? ''}|${searchParams.sort}|${searchParams.page}`;
  const [trackedFilterResetKey, setTrackedFilterResetKey] = useState(filterResetKey);
  if (trackedFilterResetKey !== filterResetKey) {
    setTrackedFilterResetKey(filterResetKey);
    if (showingFilteredCards) {
      setShowingFilteredCards(false);
    }
  }

  // Use the fetched detail in the list as well so a visible item has a single scored revision.
  const displayedItems = useMemo(
    () => (response?.items ?? []).map((item) =>
      activeItem?.workshopId === item.workshopId ? activeItem : item,
    ),
    [activeItem, response?.items],
  );

  const prioritizedVisibleWorkshopKeysSig = useMemo(() => {
    const keys: string[] = [];
    const seen = new Set<string>();
    const limit = 24;
    for (const item of displayedItems) {
      if (keys.length >= limit) break;
      const verdict = resolveWorkshopVerdict(item, filterCtx);
      if (verdict === null || (verdict !== undefined && verdict.complete !== false)) continue;
      const key = workshopFilterKey(item.workshopId);
      if (!seen.has(key)) {
        seen.add(key);
        keys.push(key);
      }
    }
    return keys.join('\0');
  }, [displayedItems, filterCtx]);

  const activeDetailPriorityKeysSig = useMemo(() => {
    if (!activeItem) return '';
    const keys: string[] = [];
    const seen = new Set<string>();
    const pushKey = (key: string) => {
      if (!seen.has(key)) {
        seen.add(key);
        keys.push(key);
      }
    };
    pushKey(workshopFilterKey(activeItem.workshopId));
    for (const dep of activeItem.dependencies) {
      const depTitle = dep.title?.trim();
      if (!depTitle) continue;
      const depLocal = localByWorkshopId.get(dep.workshopId);
      if (
        filterConfig.applyToLibrary &&
        depLocal &&
        matchesEvaluatedAddonTitle(depLocal, depTitle)
      ) {
        pushKey(addonFilterKey(depLocal.id));
      } else {
        pushKey(workshopFilterKey(dep.workshopId));
      }
    }
    return keys.join('\0');
  }, [activeItem, filterConfig.applyToLibrary, localByWorkshopId]);

  useEffect(() => {
    if (activeItem) {
      registerWorkshopItems([activeItem], { priority: 'urgent' });
    }
    const visibleBatch = activeItem
      ? displayedItems.filter((item) => item !== activeItem)
      : displayedItems;
    if (visibleBatch.length > 0) {
      registerWorkshopItems(visibleBatch, { priority: 'front' });
    }
    if (activeItem && activeItem.dependencies.length > 0) {
      const covered = new Set<string>(displayedItems.map((item) => item.workshopId));
      covered.add(activeItem.workshopId);
      const depStubs: WorkshopItemRecord[] = [];
      for (const dep of activeItem.dependencies) {
        const depTitle = dep.title?.trim();
        if (!depTitle || covered.has(dep.workshopId)) continue;
        const depLocal = localByWorkshopId.get(dep.workshopId);
        if (
          filterConfig.applyToLibrary &&
          depLocal &&
          matchesEvaluatedAddonTitle(depLocal, depTitle)
        ) {
          continue;
        }
        covered.add(dep.workshopId);
        depStubs.push({
          workshopId: dep.workshopId,
          fileType: 'item',
          status: 'active',
          title: depTitle,
          tags: [],
          dependencies: [],
          collectionChildren: [],
          source: activeItem.source,
          fetchedAt: activeItem.fetchedAt,
        });
      }
      if (depStubs.length > 0) {
        registerWorkshopItems(depStubs, { onlyIfMissing: true, priority: 'front' });
      }
    }
  }, [
    displayedItems,
    activeItem,
    filterConfig.applyToLibrary,
    localByWorkshopId,
    registerWorkshopItems,
  ]);

  useEffect(() => {
    prioritizeFilterTargets(
      prioritizedVisibleWorkshopKeysSig ? prioritizedVisibleWorkshopKeysSig.split('\0') : [],
      { owner: 'workshop-visible' },
    );
  }, [prioritizedVisibleWorkshopKeysSig, prioritizeFilterTargets]);

  useEffect(() => {
    prioritizeFilterTargets(
      activeDetailPriorityKeysSig ? activeDetailPriorityKeysSig.split('\0') : [],
      { urgent: true, owner: 'workshop-detail' },
    );
  }, [activeDetailPriorityKeysSig, prioritizeFilterTargets]);

  useEffect(() => {
    return () => {
      prioritizeFilterTargets([], { urgent: true, owner: 'workshop-detail' });
      prioritizeFilterTargets([], { owner: 'workshop-visible' });
    };
  }, [prioritizeFilterTargets]);

  const { visibleItems, filteredOutCount } = useMemo(() => {
    let hiddenCount = 0;
    const out: WorkshopItemRecord[] = [];
    for (const item of displayedItems) {
      const key = workshopFilterKey(item.workshopId);
      const state = resolveCardFilterState({
        config: filterConfig,
        scope: 'workshop',
        verdict: resolveWorkshopVerdict(item, { verdicts, evaluatedRevisions }),
        hasTargetFailed: failedTargetKeys.has(key),
        hasContent: hasFilterableWorkshopContent(item),
      });
      if (state.hiddenCard) {
        hiddenCount += 1;
        if (!showingFilteredCards) {
          continue;
        }
      }
      out.push(item);
    }
    return { visibleItems: out, filteredOutCount: hiddenCount };
  }, [
    displayedItems,
    filterConfig,
    verdicts,
    evaluatedRevisions,
    failedTargetKeys,
    showingFilteredCards,
  ]);

  const activeGalleryUrls = useMemo(
    () => (activeItem ? collectWorkshopImageUrls(activeItem) : []),
    [activeItem],
  );
  const activeKey = activeItem ? workshopFilterKey(activeItem.workshopId) : undefined;
  const activeVerdict = activeItem ? resolveWorkshopVerdict(activeItem, filterCtx) : undefined;
  const activeRevealed = activeKey !== undefined && revealedCardIds.has(activeKey);
  const activeDetailFilter = useMemo(
    () =>
      resolveDetailFilterState({
        config: filterConfig,
        scope: 'workshop',
        verdict: activeVerdict,
        revealed: activeRevealed,
        totalImages: activeGalleryUrls.length,
        hasTargetFailed: activeKey ? failedTargetKeys.has(activeKey) : false,
        hasContent: activeItem ? hasFilterableWorkshopContent(activeItem) : false,
      }),
    [
      filterConfig,
      activeKey,
      activeRevealed,
      activeVerdict,
      activeGalleryUrls.length,
      failedTargetKeys,
      activeItem,
    ],
  );
  const activeCardFilter = useMemo(
    () =>
      activeItem && activeKey
        ? resolveCardFilterState({
            config: filterConfig,
            scope: 'workshop',
            verdict: activeVerdict,
            revealed: activeRevealed,
            forceShowHiddenCards: showingFilteredCards,
            hasTargetFailed: failedTargetKeys.has(activeKey),
            hasContent: hasFilterableWorkshopContent(activeItem),
          })
        : null,
    [
      activeItem,
      activeKey,
      activeRevealed,
      activeVerdict,
      failedTargetKeys,
      filterConfig,
      showingFilteredCards,
    ],
  );
  const activeDetailTextShielded = Boolean(
    activeCardFilter &&
      !activeRevealed &&
      (activeCardFilter.blurWholeCard ||
        activeCardFilter.hiddenCard ||
        (!activeCardFilter.flagged && activeCardFilter.redactTitle)),
  );
  const activeDisplayTitle = activeItem
    ? activeDetailTextShielded
      ? formatRedactedAddonLabel(activeItem.workshopId, t, activeDetailFilter.failureDisplay)
      : activeItem.title
    : '';

  const safePageSize = response ? Math.max(1, response.pageSize) : 24;
  const totalPages = response ? Math.max(1, Math.ceil(response.total / safePageSize)) : 1;
  const displayPage = loading && response ? response.page : searchParams.page;

  const handleTrackCollection = async (collectionId: string) => {
    if (trackingCollectionIds.has(collectionId)) return;
    setTrackingCollectionIds((prev) => {
      const next = new Set(prev);
      next.add(collectionId);
      return next;
    });
    try {
      await onTrackCollection(collectionId, true);
    } finally {
      setTrackingCollectionIds((prev) => {
        if (!prev.has(collectionId)) return prev;
        const next = new Set(prev);
        next.delete(collectionId);
        return next;
      });
    }
  };

  const handleDirectAction = async (kind: 'inspect' | 'download' | 'track') => {
    const id = extractWorkshopIdInput(directInput);
    if (!id) {
      onInvalidId();
      return;
    }
    if (kind === 'download') {
      if (liveDownloads[id]) {
        return;
      }
      const local = localByWorkshopId.get(id);
      if (local?.installState === 'installed' && !local.updateAvailable) {
        onNotify(t('workshop.status.installed'));
        return;
      }
      const safeLocalTitle =
        local && !isLinkedWorkshopTitleRedacted(id, local, local.title, filterCtx)
          ? local.title
          : undefined;
      setDirectInput('');
      void (local?.installState === 'installed'
        ? onUpdate(id, safeLocalTitle)
        : onDownload(id, safeLocalTitle));
      return;
    }
    if (kind === 'track' && (busy || trackedIds.has(id) || trackingCollectionIds.has(id))) {
      return;
    }
    setDirectBusy(true);
    try {
      if (kind === 'inspect') {
        const seq = ++directInspectSeqRef.current;
        const item = await onFetchItem(id);
        if (item && seq === directInspectSeqRef.current) {
          setActiveItem(item);
          setDirectInput('');
        }
      } else {
        setTrackingCollectionIds((prev) => {
          const next = new Set(prev);
          next.add(id);
          return next;
        });
        try {
          const tracked = await onTrackCollection(id, true);
          if (tracked) setDirectInput('');
        } finally {
          setTrackingCollectionIds((prev) => {
            if (!prev.has(id)) return prev;
            const next = new Set(prev);
            next.delete(id);
            return next;
          });
        }
      }
    } finally {
      setDirectBusy(false);
    }
  };

  const handleImportSubscribed = async () => {
    const ids = await onImportSubscribed();
    if (!ids) return;
    const missing = ids.filter((id) => localByWorkshopId.get(id)?.installState !== 'installed');
    if (missing.length === 0) {
      onNotify(t('workshop.subscribedUpToDate'));
    } else {
      setMissingSubs(missing);
    }
  };

  const renderPrimaryAction = (item: WorkshopItemRecord, stopProp = false) => {
    const local = localByWorkshopId.get(item.workshopId);
    const dl = liveDownloads[item.workshopId];
    const isCollection = item.fileType === 'collection';
    const isTracked = trackedIds.has(item.workshopId);
    const isTracking = trackingCollectionIds.has(item.workshopId);
    const unavailable = item.status === 'removed' || item.status === 'private_or_unavailable';
    const itemKey = workshopFilterKey(item.workshopId);
    const itemCardFilter = resolveCardFilterState({
      config: filterConfig,
      scope: 'workshop',
      verdict: resolveWorkshopVerdict(item, filterCtx),
      revealed: revealedCardIds.has(itemKey),
      forceShowHiddenCards: showingFilteredCards,
      hasTargetFailed: failedTargetKeys.has(itemKey),
      hasContent: hasFilterableWorkshopContent(item),
    });
    const unverifiedCardShielded =
      (itemCardFilter.blurWholeCard || itemCardFilter.hiddenCard) &&
      !itemCardFilter.flagged &&
      !itemCardFilter.isBlurCardRevealed;
    const shouldOmitTaskTitle =
      (local ? isAddonTitleRedacted(local, filterCtx) : false) ||
      itemCardFilter.redactTitle ||
      ((itemCardFilter.blurWholeCard || itemCardFilter.hiddenCard) &&
        itemCardFilter.titleSensitive) ||
      unverifiedCardShielded;
    const safeTaskTitle = shouldOmitTaskTitle ? undefined : item.title;

    if (isCollection) {
      return (
        <Button
          variant={isTracked ? 'text' : 'tonal'}
          size="sm"
          disabled={isTracked || busy || isTracking}
          icon={isTracked ? <BookmarkCheck size={14} /> : <BookmarkPlus size={14} />}
          onClick={(e) => {
            if (stopProp) e.stopPropagation();
            void handleTrackCollection(item.workshopId);
          }}
        >
          {isTracked ? t('workshop.tracked') : t('workshop.directTrack')}
        </Button>
      );
    }

    if (dl) {
      return (
        <Button
          variant="outlined"
          size="sm"
          disabled={dl.cancelling}
          icon={<X size={14} />}
          onClick={(e) => {
            if (stopProp) e.stopPropagation();
            onCancelDownload(item.workshopId);
          }}
        >
          {dl.cancelling ? t('common.cancelling') : `${dl.percent}%`}
        </Button>
      );
    }

    if (unavailable) {
      return (
        <Button variant="text" size="sm" disabled>
          {t(`workshop.status.${item.status}`)}
        </Button>
      );
    }

    if (local?.installState === 'installed') {
      if (local.updateAvailable) {
        return (
          <Button
            variant="filled"
            size="sm"
            icon={<ArrowUpCircle size={14} />}
            onClick={(e) => {
              if (stopProp) e.stopPropagation();
              void onUpdate(item.workshopId, safeTaskTitle);
            }}
          >
            {t('common.update')}
          </Button>
        );
      }
      return (
        <Button variant="text" size="sm" disabled icon={<Download size={14} />}>
          {t('workshop.status.installed')}
        </Button>
      );
    }

    return (
      <Button
        variant="tonal"
        size="sm"
        icon={<Download size={14} />}
        onClick={(e) => {
          if (stopProp) e.stopPropagation();
          void onDownload(item.workshopId, safeTaskTitle);
        }}
      >
        {t('common.download')}
      </Button>
    );
  };

  return (
    <div className="l4a-view">
      <header className="l4a-view-header">
        <form
          className="md-search-bar"
          onSubmit={(e) => {
            e.preventDefault();
            setSearchParams((prev) => ({
              ...prev,
              query: draftQuery,
              page: 1,
              retryNonce:
                prev.query === draftQuery && prev.page === 1
                  ? prev.retryNonce + 1
                  : prev.retryNonce,
            }));
          }}
        >
          <Search
            size={18}
            style={{ color: 'var(--md-sys-color-on-surface-variant)', flexShrink: 0 }}
          />
          <input
            className="md-search-input"
            value={draftQuery}
            onChange={(e) => setDraftQuery(e.target.value)}
            placeholder={t('workshop.searchPlaceholder')}
            aria-label={t('workshop.searchPlaceholder')}
          />
          {draftQuery && (
            <IconButton
              size="sm"
              label={t('common.clear')}
              onClick={() => {
                setDraftQuery('');
                setSearchParams((prev) => ({ ...prev, query: '', page: 1 }));
              }}
            >
              <X size={16} />
            </IconButton>
          )}
          <IconButton type="submit" size="sm" variant="tonal" label={t('common.search')}>
            <Search size={15} />
          </IconButton>
        </form>

        <Select
          ariaLabel={t('common.sort')}
          value={searchParams.sort}
          onChange={(next) =>
            setSearchParams((prev) => ({ ...prev, sort: next, page: 1 }))
          }
          options={[
            { value: 'relevance', label: t('workshop.sort.relevance') },
            { value: 'updated', label: t('workshop.sort.updated') },
            { value: 'updated_asc', label: t('workshop.sort.updated_asc') },
            { value: 'subscriptions', label: t('workshop.sort.subscriptions') },
            { value: 'newest', label: t('workshop.sort.newest') },
            { value: 'title', label: t('workshop.sort.title') },
          ]}
        />

        <Button
          variant="outlined"
          icon={<CloudDownload size={15} />}
          onClick={() => void handleImportSubscribed()}
        >
          {t('workshop.importSubscribed')}
        </Button>

        <div
          className="md-search-bar"
          style={{ flex: '0 1 340px', minWidth: 230, height: 40, paddingRight: 6 }}
        >
          <input
            className="md-search-input md-mono"
            style={{ fontSize: 13 }}
            value={directInput}
            onChange={(e) => setDirectInput(e.target.value)}
            placeholder={t('workshop.directPlaceholder')}
            aria-label={t('workshop.directPlaceholder')}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                void handleDirectAction('inspect');
              }
            }}
          />
          <IconButton
            size="sm"
            label={t('workshop.directInspect')}
            disabled={directBusy || !directInput.trim()}
            onClick={() => void handleDirectAction('inspect')}
          >
            <Eye size={15} />
          </IconButton>
          <IconButton
            size="sm"
            variant="tonal"
            label={t('workshop.directDownload')}
            disabled={directBusy || !directInput.trim()}
            onClick={() => void handleDirectAction('download')}
          >
            <Download size={15} />
          </IconButton>
          <IconButton
            size="sm"
            label={t('workshop.directTrack')}
            disabled={busy || directBusy || !directInput.trim()}
            onClick={() => void handleDirectAction('track')}
          >
            <BookmarkPlus size={15} />
          </IconButton>
        </div>
      </header>

      <div className="l4a-view-toolbar">
        <SegmentedButton<WorkshopFileType>
          ariaLabel={t('workshop.contentType')}
          value={searchParams.fileType}
          onChange={(next) =>
            setSearchParams((prev) =>
              prev.fileType === next ? prev : { ...prev, fileType: next, page: 1 },
            )
          }
          options={[
            { value: 'item', label: t('workshop.type.item') },
            { value: 'collection', label: t('workshop.type.collection') },
          ]}
        />
        <div className="md-chip-row">
          <FilterChip
            label={t('common.all')}
            selected={!searchParams.tag}
            onClick={() => setSearchParams((prev) => ({ ...prev, tag: undefined, page: 1 }))}
          />
          {WORKSHOP_TAGS.map((tag) => (
            <FilterChip
              key={tag}
              label={t(`workshop.tags.${tag}`)}
              selected={searchParams.tag === tag}
              onClick={() =>
                setSearchParams((prev) => ({
                  ...prev,
                  tag: prev.tag === tag ? undefined : tag,
                  page: 1,
                }))
              }
            />
          ))}
        </div>
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
            onClick={() => setShowingFilteredCards((v) => !v)}
            onRemove={showingFilteredCards ? () => setShowingFilteredCards(false) : undefined}
          />
        )}
        <div className="l4a-spacer" />
        {loading && response && <LoadingIndicator size="sm" label={t('common.search')} />}
        {response && (
          <span className="md-pill" data-tone="surface">
            {response.source}
          </span>
        )}
      </div>

      <div className="l4a-view-scroll">
        {loading && (!response || response.items.length === 0) ? (
          <div className="l4a-loading-center">
            <LoadingIndicator size="lg" contained label={t('common.search')} />
          </div>
        ) : searchError ? (
          <EmptyState
            icon={<Store size={26} />}
            title={
              searchError.unsupported
                ? t('workshop.searchUnavailable')
                : t('workshop.searchFailed')
            }
            description={searchError.unsupported ? undefined : searchError.message}
            action={
              <div className="l4a-row-wrap">
                {!searchError.unsupported && (
                  <Button
                    variant="filled"
                    icon={<RefreshCw size={15} />}
                    onClick={() =>
                      setSearchParams((prev) => ({
                        ...prev,
                        retryNonce: prev.retryNonce + 1,
                      }))
                    }
                  >
                    {t('common.retry')}
                  </Button>
                )}
                <Button
                  variant="tonal"
                  icon={<Settings size={15} />}
                  onClick={onOpenSourcesSettings}
                >
                  {t('workshop.configureSources')}
                </Button>
                <Button
                  variant="outlined"
                  icon={<ExternalLink size={15} />}
                  onClick={() =>
                    onOpenExternal('https://steamcommunity.com/app/550/workshop/')
                  }
                >
                  {t('detail.openWeb')}
                </Button>
              </div>
            }
          />
        ) : (
          <>
            {!response || visibleItems.length === 0 ? (
              !loading && <EmptyState icon={<Store size={26} />} title={t('common.noResults')} />
            ) : (
              <div className="l4a-grid">
                {visibleItems.map((item) => {
                  const local = localByWorkshopId.get(item.workshopId);
                  const dl = liveDownloads[item.workshopId];
                  const isTracked = item.fileType === 'collection' && trackedIds.has(item.workshopId);
                  const cardKey = workshopFilterKey(item.workshopId);
                  const verdict = resolveWorkshopVerdict(item, filterCtx);
                  const cardFilter = resolveCardFilterState({
                    config: filterConfig,
                    scope: 'workshop',
                    verdict: verdict,
                    revealed: revealedCardIds.has(cardKey),
                    forceShowHiddenCards: showingFilteredCards,
                    hasTargetFailed: failedTargetKeys.has(cardKey),
                    hasContent: hasFilterableWorkshopContent(item),
                  });
                  const displayTitle = cardFilter.redactTitle
                    ? formatRedactedAddonLabel(item.workshopId, t, cardFilter.failureDisplay)
                    : item.title;

                  return (
                    <article
                      key={item.workshopId}
                      className="md-card"
                      data-clickable="true"
                      data-blur-card={cardFilter.blurWholeCard ? 'true' : 'false'}
                      data-pending-eval={cardFilter.pendingEvaluation ? 'true' : undefined}
                    >
                      <FilterCardOverlay state={cardFilter} onReveal={() => revealCard(cardKey)} />

                      <div
                        className="l4a-blur-card-inner"
                        inert={cardFilter.blurWholeCard || undefined}
                        aria-hidden={cardFilter.blurWholeCard || undefined}
                      >
                        <div className="l4a-addon-cover-wrap">
                          <div
                            className="l4a-card-trigger"
                            style={{ height: '100%' }}
                            onClick={() => setActiveItem(item)}
                          >
                            <AddonCover
                              src={item.previewUrl}
                              title={displayTitle}
                              category={item.tags[0]}
                              filterMode={cardFilter.coverMode}
                              evaluation={cardFilter}
                            />
                          </div>
                          <div className="l4a-cover-overlay-top">
                            <span className="md-pill" data-tone="surface">
                              {t(`workshop.type.${item.fileType}`)}
                            </span>
                            <div className="l4a-status-pills">
                              <FilterFailureBadge state={cardFilter} />
                              {cardFilter.showDiagnosticBadge && verdict && (
                                <span className="md-pill" data-tone="tertiary">
                                  {formatDiagnosticVerdictBadge(verdict, cardFilter, {
                                    imageTag: t('filter.modImageShort'),
                                    textTag: t('filter.modTextShort'),
                                  })}
                                </span>
                              )}
                              {isTracked && (
                                <span className="md-pill" data-tone="primary">
                                  {t('workshop.tracked')}
                                </span>
                              )}
                              {local?.installState === 'installed' && (
                                <span className="md-pill" data-tone="primary">
                                  {t('workshop.status.installed')}
                                </span>
                              )}
                              {local && local.installState !== 'installed' && (
                                <span className="md-pill" data-tone="secondary">
                                  {t('workshop.status.known')}
                                </span>
                              )}
                              {(item.status === 'removed' ||
                                item.status === 'private_or_unavailable') && (
                                <span className="md-pill" data-tone="error">
                                  {t(`workshop.status.${item.status}`)}
                                </span>
                              )}
                            </div>
                          </div>
                        </div>

                        {dl && (
                          <LinearProgress
                            value={dl.percent}
                            indeterminate={dl.cancelling || dl.percent === 0}
                            label={displayTitle}
                          />
                        )}

                        <div className="l4a-addon-card-body">
                          <button
                            type="button"
                            className="l4a-card-trigger"
                            onClick={() => setActiveItem(item)}
                          >
                            <div className="l4a-addon-title">{displayTitle}</div>
                          </button>
                          <div className="l4a-addon-meta">
                            <span className="l4a-truncate">
                              {cardFilter.redactMeta
                                ? t('filter.redactedAuthor')
                                : item.creatorName || t('common.unknown')}
                            </span>
                            {item.fileSize ? (
                              <>
                                <span>·</span>
                                <span>{formatBytes(item.fileSize)}</span>
                              </>
                            ) : null}
                            {item.subscriptions ? (
                              <>
                                <span>·</span>
                                <span
                                  style={{ display: 'inline-flex', alignItems: 'center', gap: 3 }}
                                >
                                  <Users size={11} />
                                  {formatCompactNumber(item.subscriptions)}
                                </span>
                              </>
                            ) : null}
                          </div>

                          {cardFilter.isBlurCardRevealed && (
                            <div className="l4a-revealed-desc">
                              {cardFilter.redactDescription
                                ? t('filter.redactedDescription')
                                : item.description?.trim() || t('filter.noDescription')}
                            </div>
                          )}

                          {!cardFilter.isBlurCardRevealed &&
                            cardFilter.redactDescription &&
                            !cardFilter.redactTitle && (
                              <div className="l4a-revealed-desc">
                                {t('filter.redactedDescription')}
                              </div>
                            )}

                          <div className="l4a-addon-card-footer">
                            <div className="l4a-row-wrap" style={{ gap: 6 }}>
                              <span
                                className="md-body-small md-mono"
                                style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                              >
                                #{item.workshopId}
                              </span>
                              {cardFilter.isBlurCardRevealed && (
                                <Button
                                  variant="text"
                                  size="sm"
                                  onClick={() => setActiveItem(item)}
                                >
                                  {t('common.inspect')}
                                </Button>
                              )}
                            </div>
                            {renderPrimaryAction(item, true)}
                          </div>
                        </div>
                      </div>
                    </article>
                  );
                })}
              </div>
            )}

            {response && (totalPages > 1 || searchParams.page > 1) && (
              <div
                className="l4a-row-wrap"
                style={{ justifyContent: 'center', marginTop: 20, gap: 12 }}
              >
                <IconButton
                  variant="outlined"
                  label={t('common.previous')}
                  disabled={loading || searchParams.page <= 1}
                  onClick={() =>
                    setSearchParams((prev) => ({ ...prev, page: Math.max(1, prev.page - 1) }))
                  }
                >
                  <ChevronLeft size={18} />
                </IconButton>
                <span className="md-label-large">
                  {t('workshop.page', { page: displayPage, total: totalPages })}
                </span>
                <IconButton
                  variant="outlined"
                  label={t('common.next')}
                  disabled={loading || searchParams.page >= totalPages}
                  onClick={() =>
                    setSearchParams((prev) => ({
                      ...prev,
                      page: Math.min(totalPages, prev.page + 1),
                    }))
                  }
                >
                  <ChevronRight size={18} />
                </IconButton>
              </div>
            )}
          </>
        )}
      </div>

      <SideSheet
        open={Boolean(activeItem)}
        title={activeDisplayTitle}
        onClose={() => {
          directInspectSeqRef.current += 1;
          setActiveItem(null);
        }}
        footer={
          activeItem && (
            <>
              <Button
                variant="text"
                size="sm"
                icon={<ExternalLink size={14} />}
                onClick={() => onOpenExternal(buildSteamProtocolUrl(activeItem.workshopId))}
              >
                {t('detail.openSteam')}
              </Button>
              <Button
                variant="text"
                size="sm"
                icon={<ExternalLink size={14} />}
                onClick={() => onOpenExternal(buildWorkshopUrl(activeItem.workshopId))}
              >
                {t('detail.openWeb')}
              </Button>
              {renderPrimaryAction(activeItem)}
            </>
          )
        }
      >
        {activeItem && (
          <div key={activeItem.workshopId} className="md-sheet-body">
            {activeDetailFilter.flagged && activeVerdict && (
              <div className="l4a-callout">
                <ShieldAlert
                  size={16}
                  style={{ color: 'var(--md-sys-color-tertiary)', flexShrink: 0, marginTop: 2 }}
                />
                <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                  <div className="l4a-row-wrap" style={{ gap: 6 }}>
                    <span className="md-label-large">{t('filter.flaggedBannerTitle')}</span>
                    <span className="md-pill" data-tone="tertiary">
                      {t(`filter.decisionSource.${activeVerdict.decisionSource}`)}
                    </span>
                    {activeDetailFilter.imageSensitive && (
                      <span className="md-pill" data-tone="secondary">
                        [{t('filter.modImageShort')}]
                      </span>
                    )}
                    {activeDetailFilter.textSensitive && (
                      <span className="md-pill" data-tone="secondary">
                        [{t('filter.modTextShort')}]
                      </span>
                    )}
                  </div>
                  <span
                    className="md-body-small"
                    style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                  >
                    {t('filter.scoreSummary', {
                      score: activeVerdict.evaluatedScore.toFixed(2),
                      nsfw: activeVerdict.probabilities.nsfwProb.toFixed(2),
                      gore: activeVerdict.probabilities.goreProb.toFixed(2),
                    })}
                  </span>
                </div>
              </div>
            )}

            {activeKey && activeDetailTextShielded && !activeDetailFilter.pendingEvaluation && (
              <div className="l4a-row-wrap">
                <Button
                  variant="tonal"
                  size="sm"
                  icon={<Eye size={14} />}
                  onClick={() => revealCard(activeKey)}
                >
                  {t(activeDetailFilter.failureDisplay === 'notice'
                    ? 'filter.evalFailedReveal'
                    : 'filter.blurCardOverlay')}
                </Button>
              </div>
            )}

            <div
              className="l4a-addon-cover-wrap"
              style={{ borderRadius: 'var(--md-sys-shape-corner-medium)' }}
            >
              <AddonCover
                src={activeGalleryUrls[0] ?? activeItem.previewUrl}
                title={activeDisplayTitle}
                category={activeDetailTextShielded ? undefined : activeItem.tags[0]}
                filterMode={activeDetailFilter.imageModeAt(0)}
                evaluation={activeDetailFilter}
                allowPeek={activeDetailFilter.imageModeAt(0) === 'blur'}
              />
              {activeDetailFilter.failureDisplay && (
                <div className="l4a-cover-overlay-top">
                  <FilterFailureBadge state={activeDetailFilter} />
                </div>
              )}
            </div>

            {activeGalleryUrls.length > 1 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                <span
                  className="md-label-medium"
                  style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                >
                  {t('filter.screenshots', { count: activeGalleryUrls.length - 1 })}
                </span>
                <div className="l4a-gallery-grid">
                  {activeGalleryUrls.slice(1).map((url, offset) => {
                    const imageIdx = offset + 1;
                    const imgMode = activeDetailFilter.imageModeAt(imageIdx);
                    return (
                      <div key={`${imageIdx}:${url}`} className="l4a-gallery-item">
                        <AddonCover
                          src={url}
                          title={`${activeDisplayTitle} #${imageIdx + 1}`}
                          category={activeDetailTextShielded ? undefined : activeItem.tags[0]}
                          filterMode={imgMode}
                          evaluation={activeDetailFilter}
                          allowPeek={imgMode === 'blur'}
                        />
                      </div>
                    );
                  })}
                </div>
              </div>
            )}

            <div className="l4a-kv-grid">
              <div className="l4a-kv-item">
                <span className="l4a-kv-label">{t('detail.workshopId')}</span>
                <span className="l4a-kv-value md-mono selectable">{activeItem.workshopId}</span>
              </div>
              <div className="l4a-kv-item">
                <span className="l4a-kv-label">{t('detail.author')}</span>
                <span className="l4a-kv-value">
                  {activeDetailTextShielded
                    ? t('filter.redactedAuthor')
                    : activeItem.creatorName || t('common.unknown')}
                </span>
              </div>
              <div className="l4a-kv-item">
                <span className="l4a-kv-label">{t('detail.size')}</span>
                <span className="l4a-kv-value">
                  {activeItem.fileSize ? formatBytes(activeItem.fileSize) : '—'}
                </span>
              </div>
              <div className="l4a-kv-item">
                <span className="l4a-kv-label">{t('workshop.subscriptions')}</span>
                <span className="l4a-kv-value">
                  {formatCompactNumber(activeItem.subscriptions)}
                </span>
              </div>
              <div className="l4a-kv-item">
                <span className="l4a-kv-label">{t('detail.remoteUpdated')}</span>
                <span className="l4a-kv-value">{formatTimestamp(activeItem.timeUpdated)}</span>
              </div>
              <div className="l4a-kv-item">
                <span className="l4a-kv-label">{t('workshop.source')}</span>
                <span className="l4a-kv-value md-mono">{activeItem.source}</span>
              </div>
            </div>

            {!activeDetailTextShielded && activeItem.tags.length > 0 && (
              <div className="md-chip-row">
                {activeItem.tags.map((tag) => (
                  <span key={tag} className="md-pill" data-tone="secondary">
                    {tag}
                  </span>
                ))}
              </div>
            )}

            {activeItem.description && (
              <div className="md-body-small selectable l4a-description-box">
                {activeDetailTextShielded
                  ? activeDetailFilter.pendingEvaluation
                    ? t('filter.evaluatingMedia')
                    : t('filter.redactedDescription')
                  : activeItem.description}
              </div>
            )}

            {activeItem.dependencies.length > 0 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                <span className="md-label-medium">{t('workshop.dependencies')}</span>
                <div className="l4a-list">
                  {activeItem.dependencies.map((dep) => {
                    const depLocal = localByWorkshopId.get(dep.workshopId);
                    const installed = depLocal?.installState === 'installed';
                    const depRedacted = isLinkedWorkshopTitleRedacted(
                      dep.workshopId,
                      depLocal,
                      dep.title,
                      filterCtx,
                    );
                    const depDisplayTitle = depRedacted
                      ? formatRedactedAddonLabel(dep.workshopId, t)
                      : dep.title || `#${dep.workshopId}`;
                    const safeDepTitle = depRedacted ? undefined : dep.title;
                    return (
                      <div key={dep.workshopId} className="l4a-list-row" style={{ cursor: 'default' }}>
                        <div className="l4a-list-main">
                          <span className="md-title-small l4a-truncate">
                            {depDisplayTitle}
                          </span>
                          <span className="md-body-small md-mono" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                            #{dep.workshopId}
                          </span>
                        </div>
                        <Button
                          variant={installed ? 'text' : 'tonal'}
                          size="sm"
                          disabled={installed || Boolean(liveDownloads[dep.workshopId])}
                          icon={<Download size={13} />}
                          onClick={() => void onDownload(dep.workshopId, safeDepTitle)}
                        >
                          {installed ? t('workshop.status.installed') : t('common.download')}
                        </Button>
                      </div>
                    );
                  })}
                </div>
              </div>
            )}

            {activeItem.collectionChildren.length > 0 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                <span className="md-label-medium">
                  {t('workshop.collectionItems')} ({activeItem.collectionChildren.length})
                </span>
                <div className="md-chip-row">
                  {activeItem.collectionChildren.map((cid) => {
                    const childLocal = localByWorkshopId.get(cid);
                    const installed = childLocal?.installState === 'installed';
                    const safeChildTitle =
                      childLocal && !isAddonTitleRedacted(childLocal, filterCtx)
                        ? childLocal.title
                        : undefined;
                    return (
                      <Button
                        key={cid}
                        variant={installed ? 'text' : 'outlined'}
                        size="sm"
                        disabled={installed || Boolean(liveDownloads[cid])}
                        icon={<Download size={12} />}
                        onClick={() => void onDownload(cid, safeChildTitle)}
                      >
                        #{cid}
                      </Button>
                    );
                  })}
                </div>
              </div>
            )}
          </div>
        )}
      </SideSheet>

      <Dialog
        open={Boolean(missingSubs)}
        title={t('workshop.subscribedFound', { count: missingSubs?.length ?? 0 })}
        onClose={() => setMissingSubs(null)}
        actions={
          <>
            <Button variant="text" onClick={() => setMissingSubs(null)}>
              {t('common.close')}
            </Button>
            <Button
              variant="filled"
              disabled={(missingSubs ?? []).every((id) => Boolean(liveDownloads[id]))}
              icon={<Download size={15} />}
              onClick={() => {
                const ids = (missingSubs ?? []).filter((id) => !liveDownloads[id]);
                setMissingSubs(null);
                for (const id of ids) void onDownload(id);
              }}
            >
              {t('common.download')} (
              {(missingSubs ?? []).filter((id) => !liveDownloads[id]).length})
            </Button>
          </>
        }
      >
        <div className="l4a-picker-list">
          {missingSubs?.map((id) => (
            <div key={id} className="l4a-picker-row" style={{ cursor: 'default' }}>
              <span className="md-body-medium md-mono" style={{ flex: 1 }}>
                #{id}
              </span>
              <Button
                variant="tonal"
                size="sm"
                disabled={Boolean(liveDownloads[id])}
                icon={<Download size={13} />}
                onClick={() => void onDownload(id)}
              >
                {t('common.download')}
              </Button>
            </div>
          ))}
        </div>
      </Dialog>
    </div>
  );
}
