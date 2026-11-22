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
  RefreshCw,
  Search,
  Settings,
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
  type WorkshopItemRecord,
} from '../../api/ipc';
import type { LiveDownloadState } from '../../state/useAppController';
import {
  buildSteamProtocolUrl,
  buildWorkshopUrl,
  extractWorkshopIdInput,
  formatBytes,
  formatCompactNumber,
  formatTimestamp,
} from '../../utils/format';
import { AddonCover } from '../m3/AddonCover';
import {
  Button,
  Dialog,
  EmptyState,
  FilterChip,
  IconButton,
  LinearProgress,
  LoadingIndicator,
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
  const { t } = useTranslation();
  const [draftQuery, setDraftQuery] = useState('');
  const [searchParams, setSearchParams] = useState<{
    query: string;
    tag?: string;
    sort: string;
    page: number;
    retryNonce: number;
  }>({
    query: '',
    tag: undefined,
    sort: 'relevance',
    page: 1,
    retryNonce: 0,
  });
  const [directInput, setDirectInput] = useState('');
  const [directBusy, setDirectBusy] = useState(false);
  const [searchResult, setSearchResult] = useState<{
    reqKey: string;
    response: SearchResponse | null;
    error: { unsupported: boolean; message: string } | null;
  } | null>(null);
  const [activeItem, setActiveItem] = useState<WorkshopItemRecord | null>(null);
  const [missingSubs, setMissingSubs] = useState<string[] | null>(null);

  const searchSeqRef = useRef(0);
  const reqKey = JSON.stringify(searchParams);

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
  }, [searchParams, reqKey, onSearch]);

  const loading = searchResult?.reqKey !== reqKey;
  const response = searchResult?.response ?? null;
  const searchError = searchResult?.error ?? null;

  const totalPages = response ? Math.max(1, Math.ceil(response.total / response.pageSize)) : 1;

  const handleDirectAction = async (kind: 'inspect' | 'download' | 'track') => {
    const id = extractWorkshopIdInput(directInput);
    if (!id) {
      onInvalidId();
      return;
    }
    setDirectBusy(true);
    try {
      if (kind === 'inspect') {
        const item = await onFetchItem(id);
        if (item) {
          setActiveItem(item);
          setDirectInput('');
        }
      } else if (kind === 'download') {
        const local = localByWorkshopId.get(id);
        const result =
          local?.installState === 'installed'
            ? await onUpdate(id, local.title)
            : await onDownload(id, local?.title);
        if (result) setDirectInput('');
      } else {
        const tracked = await onTrackCollection(id, true);
        if (tracked) setDirectInput('');
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
    const unavailable = item.status === 'removed' || item.status === 'private_or_unavailable';

    if (isCollection) {
      return (
        <Button
          variant={isTracked ? 'text' : 'tonal'}
          size="sm"
          disabled={isTracked}
          icon={isTracked ? <BookmarkCheck size={14} /> : <BookmarkPlus size={14} />}
          onClick={(e) => {
            if (stopProp) e.stopPropagation();
            void onTrackCollection(item.workshopId, true);
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
              void onUpdate(item.workshopId, item.title);
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
          void onDownload(item.workshopId, item.title);
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
            disabled={directBusy || !directInput.trim()}
            onClick={() => void handleDirectAction('track')}
          >
            <BookmarkPlus size={15} />
          </IconButton>
        </div>
      </header>

      <div className="l4a-view-toolbar">
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
        <div className="l4a-spacer" />
        {loading && response && <LoadingIndicator size="sm" label={t('common.search')} />}
        {response && (
          <span className="md-pill" data-tone="surface">
            {response.source}
          </span>
        )}
      </div>

      <div className="l4a-view-scroll">
        {loading && !response ? (
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
        ) : !response || response.items.length === 0 ? (
          !loading && <EmptyState icon={<Store size={26} />} title={t('common.noResults')} />
        ) : (
          <>
            <div className="l4a-grid">
              {response.items.map((item) => {
                const local = localByWorkshopId.get(item.workshopId);
                const dl = liveDownloads[item.workshopId];
                const isTracked = item.fileType === 'collection' && trackedIds.has(item.workshopId);

                return (
                  <article key={item.workshopId} className="md-card" data-clickable="true">
                    <div className="l4a-addon-cover-wrap">
                      <button
                        type="button"
                        tabIndex={-1}
                        aria-hidden="true"
                        className="l4a-card-trigger"
                        style={{ height: '100%' }}
                        onClick={() => setActiveItem(item)}
                      >
                        <AddonCover
                          src={item.previewUrl}
                          title={item.title}
                          category={item.tags[0]}
                        />
                      </button>
                      <div className="l4a-cover-overlay-top">
                        <span className="md-pill" data-tone="surface">
                          {t(`workshop.type.${item.fileType}`)}
                        </span>
                        <div className="l4a-status-pills">
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
                        label={item.title}
                      />
                    )}

                    <div className="l4a-addon-card-body">
                      <button
                        type="button"
                        className="l4a-card-trigger"
                        onClick={() => setActiveItem(item)}
                      >
                        <div className="l4a-addon-title">{item.title}</div>
                      </button>
                      <div className="l4a-addon-meta">
                        <span className="l4a-truncate">
                          {item.creatorName || t('common.unknown')}
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
                            <span style={{ display: 'inline-flex', alignItems: 'center', gap: 3 }}>
                              <Users size={11} />
                              {formatCompactNumber(item.subscriptions)}
                            </span>
                          </>
                        ) : null}
                      </div>

                      <div className="l4a-addon-card-footer">
                        <span className="md-body-small md-mono" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                          #{item.workshopId}
                        </span>
                        {renderPrimaryAction(item, true)}
                      </div>
                    </div>
                  </article>
                );
              })}
            </div>

            {totalPages > 1 && (
              <div
                className="l4a-row-wrap"
                style={{ justifyContent: 'center', marginTop: 20, gap: 12 }}
              >
                <IconButton
                  variant="outlined"
                  label={t('common.previous')}
                  disabled={searchParams.page <= 1}
                  onClick={() =>
                    setSearchParams((prev) => ({ ...prev, page: Math.max(1, prev.page - 1) }))
                  }
                >
                  <ChevronLeft size={18} />
                </IconButton>
                <span className="md-label-large">
                  {t('workshop.page', { page: searchParams.page, total: totalPages })}
                </span>
                <IconButton
                  variant="outlined"
                  label={t('common.next')}
                  disabled={searchParams.page >= totalPages}
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
        title={activeItem?.title ?? ''}
        onClose={() => setActiveItem(null)}
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
          <div className="md-sheet-body">
            <div
              className="l4a-addon-cover-wrap"
              style={{ borderRadius: 'var(--md-sys-shape-corner-medium)' }}
            >
              <AddonCover
                src={activeItem.previewUrl}
                title={activeItem.title}
                category={activeItem.tags[0]}
              />
            </div>

            <div className="l4a-kv-grid">
              <div className="l4a-kv-item">
                <span className="l4a-kv-label">{t('detail.workshopId')}</span>
                <span className="l4a-kv-value md-mono selectable">{activeItem.workshopId}</span>
              </div>
              <div className="l4a-kv-item">
                <span className="l4a-kv-label">{t('detail.author')}</span>
                <span className="l4a-kv-value">
                  {activeItem.creatorName || t('common.unknown')}
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

            {activeItem.tags.length > 0 && (
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
                {activeItem.description}
              </div>
            )}

            {activeItem.dependencies.length > 0 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                <span className="md-label-medium">{t('workshop.dependencies')}</span>
                <div className="l4a-list">
                  {activeItem.dependencies.map((dep) => {
                    const installed =
                      localByWorkshopId.get(dep.workshopId)?.installState === 'installed';
                    return (
                      <div key={dep.workshopId} className="l4a-list-row" style={{ cursor: 'default' }}>
                        <div className="l4a-list-main">
                          <span className="md-title-small l4a-truncate">
                            {dep.title || `#${dep.workshopId}`}
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
                          onClick={() => void onDownload(dep.workshopId, dep.title)}
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
                    const installed = localByWorkshopId.get(cid)?.installState === 'installed';
                    return (
                      <Button
                        key={cid}
                        variant={installed ? 'text' : 'outlined'}
                        size="sm"
                        disabled={installed || Boolean(liveDownloads[cid])}
                        icon={<Download size={12} />}
                        onClick={() => void onDownload(cid, localByWorkshopId.get(cid)?.title)}
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
              icon={<Download size={15} />}
              onClick={() => {
                const ids = missingSubs ?? [];
                setMissingSubs(null);
                for (const id of ids) void onDownload(id);
              }}
            >
              {t('common.download')} ({missingSubs?.length ?? 0})
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
