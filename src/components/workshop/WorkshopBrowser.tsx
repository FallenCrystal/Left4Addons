/**
 * WorkshopBrowser — main container for the built-in Steam Workshop browser.
 *
 * Splits into two views:
 *   • Homepage  — section carousels (trending, most-subscribed, …)
 *   • Browse    — filterable grid with tag sidebar
 */

import React, { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Search, ChevronLeft, ChevronRight, Loader2,
  Home, Compass, User, Tag, AlertTriangle,
} from 'lucide-react';

import {
  WorkshopItem,
  HomepageSection,
  TagCategory,
  WorkshopBrowserProps,
} from './types';
import { ItemCard } from './ItemCard';
import { TagBrowserModal } from './TagBrowserModal';
import { SectionCarousel } from './SectionCarousel';
import { WorkshopDetailModal } from './WorkshopDetailModal';
import { TaskCenterButton } from '../layout/TaskCenterButton';
import { AlertModal } from '../modals/AlertModal';
import { CustomSelect } from '../common/CustomSelect';
import {
  fetchWorkshopCollection,
  fetchWorkshopHome,
  fetchWorkshopItem,
  fetchWorkshopItems,
  mapSteamDetailToWorkshopItem,
  setWorkshopWarningReporter,
} from '../../services/workshopClient';

// ── Browse sort options ───────────────────────────────────────────────────────

const SORT_OPTIONS = [
  { value: 'trend', labelKey: 'workshop.browse.sortTrend' },
  { value: 'textsearch', labelKey: 'workshop.browse.sortTextSearch' },
  { value: 'totalprofiles', labelKey: 'workshop.browse.sortTotalProfiles' },
  { value: 'mostrecent', labelKey: 'workshop.browse.sortMostRecent' },
  { value: 'toprated', labelKey: 'workshop.browse.sortTopRated' },
  { value: 'lastupdated', labelKey: 'workshop.browse.sortLastUpdated' },
] as const;

// ── Search Bar Component ───────────────────────────────────────────────────────

interface WorkshopSearchBarProps {
  initialValue: string;
  placeholder: string;
  searchLabel: string;
  onSubmit: (val: string) => void;
  onClear: () => void;
  onFocus: () => void;
}

const WorkshopSearchBarComponent: React.FC<WorkshopSearchBarProps> = ({
  initialValue,
  placeholder,
  searchLabel,
  onSubmit,
  onClear,
  onFocus,
}) => {
  const [localQuery, setLocalQuery] = useState(initialValue);

  useEffect(() => {
    setLocalQuery(initialValue);
  }, [initialValue]);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    onSubmit(localQuery);
  };

  return (
    <form onSubmit={handleSubmit} style={{ display: 'flex', gap: '8px', flex: 1, maxWidth: '480px' }}>
      <div style={{ position: 'relative', flex: 1 }}>
        <Search size={16} style={{ position: 'absolute', left: '14px', top: '50%', transform: 'translateY(-50%)', color: 'var(--md-sys-color-outline)' }} />
        <input
          type="text"
          placeholder={placeholder}
          value={localQuery}
          onChange={(e) => {
            const val = e.target.value;
            setLocalQuery(val);
            if (val.trim() === '') {
              onClear();
            }
          }}
          onFocus={onFocus}
          style={{ width: '100%', padding: '8px 14px 8px 40px', borderRadius: '100px', border: '1px solid var(--md-sys-color-outline-variant)', backgroundColor: 'var(--md-sys-color-surface-container-high)', color: 'var(--md-sys-color-on-surface)', outline: 'none', fontSize: '13px' }}
        />
      </div>
      <button type="submit" className="btn btn-primary" style={{ borderRadius: '100px', padding: '0 20px', fontSize: '13px' }}>
        {searchLabel}
      </button>
    </form>
  );
};

const WorkshopSearchBar = React.memo(WorkshopSearchBarComponent);

// ── Workshop ID parser ────────────────────────────────────────────────────────
function parseWorkshopId(input: string): string | null {
  const trimmed = input.trim();
  
  // If it's a URL/URI structure containing an ID, extract it regardless of length
  const idMatch = trimmed.match(/[?&]id=(\d+)/i);
  if (idMatch) {
    return idMatch[1];
  }
  const pathMatch = trimmed.match(/CommunityFilePage\/(\d+)/i);
  if (pathMatch) {
    return pathMatch[1];
  }

  // If it's just a number, require it to be at least 8 digits to be treated as a workshop ID
  if (/^\d{8,15}$/.test(trimmed)) {
    return trimmed;
  }
  
  return null;
}

// ── Main Component ────────────────────────────────────────────────────────────

export const WorkshopBrowser: React.FC<WorkshopBrowserProps> = ({
  addons,
  knownUninstalledAddons,
  downloadProgress,
  onDownload,
  onDownloadMany,
  onOpenLink,
  onImportCollection,
  onRecordSeenItems,
  onDatabaseUpdate,
  isSubmitting,
  groups,
  backgroundTasks,
  syncingSteam,
  onOpenTaskCenter,
  onWarning,
  workshopSourceSettings,
}) => {
  const { t } = useTranslation();
  const scrollIdleTimerRef = useRef<number | null>(null);
  const knownCollectionIds = useMemo(() => new Set(
    (groups || [])
      .map((group) => group.workshopCollectionId?.trim())
      .filter((id): id is string => !!id),
  ), [groups]);

  // View mode
  const [viewMode, setViewMode] = useState<'home' | 'browse' | 'search'>('home');
  const [query, setQuery] = useState('');           // input field value
  const [committedQuery, setCommittedQuery] = useState(''); // actually fetched
  const [sort, setSort] = useState('trend');
  const [section, setSection] = useState('readytouseitems');
  const [page, setPage] = useState(1);
  const [creatorId, setCreatorId] = useState<string | null>(null);
  const [creatorName, setCreatorName] = useState<string | null>(null);
  const [activeTag, setActiveTag] = useState<string | null>(null);
  const [activeTagName, setActiveTagName] = useState<string | null>(null);

  // Data
  const [loading, setLoading] = useState(false);
  const [items, setItems] = useState<WorkshopItem[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [homepageSections, setHomepageSections] = useState<HomepageSection[]>([]);
  const [homepageLoading, setHomepageLoading] = useState(false);
  const [homepageError, setHomepageError] = useState<string | null>(null);
  const [tagCategories, setTagCategories] = useState<TagCategory[]>([]);

  const searchRequestRef = useRef(0);
  const homepageRequestRef = useRef(0);

  // Detail modal / view
  const [selectedItem, setSelectedItem] = useState<any | null>(null);
  const [selectedCollection, setSelectedCollection] = useState<{
    title: string;
    description: string;
    imagePath: string;
    creatorName: string;
    creatorId: string;
    items: WorkshopItem[];
    workshopId?: string;
  } | null>(null);
  
  // History stack for detail navigation
  const [detailHistory, setDetailHistory] = useState<Array<{ item: any; collection: any }>>([]);

  const navigateToDetail = useCallback((newItem: any, newCollection: any) => {
    setSelectedItem((currItem) => {
      setSelectedCollection((currCol) => {
        if (currItem || currCol) {
          setDetailHistory((prev) => [...prev, { item: currItem, collection: currCol }]);
        } else {
          setDetailHistory([]);
        }
        return newCollection;
      });
      return newItem;
    });
  }, []);

  const handleBack = useCallback(() => {
    if (detailHistory.length > 0) {
      const prev = detailHistory[detailHistory.length - 1];
      setDetailHistory((prevStack) => prevStack.slice(0, prevStack.length - 1));
      setSelectedItem(prev.item);
      setSelectedCollection(prev.collection);
    } else {
      setSelectedItem(null);
      setSelectedCollection(null);
    }
  }, [detailHistory]);

  const [tagModalOpen, setTagModalOpen] = useState(false);
  const [loadingDetailId, setLoadingDetailId] = useState<string | null>(null);
  const [alertInfo, setAlertInfo] = useState<{ open: boolean; title: string; message: string }>({
    open: false,
    title: '',
    message: '',
  });
  const [isScrollInteracting, setIsScrollInteracting] = useState(false);

  const markScrollInteraction = useCallback(() => {
    setIsScrollInteracting(true);
    if (scrollIdleTimerRef.current !== null) {
      window.clearTimeout(scrollIdleTimerRef.current);
    }
    scrollIdleTimerRef.current = window.setTimeout(() => {
      setIsScrollInteracting(false);
      scrollIdleTimerRef.current = null;
    }, 140);
  }, []);

  useEffect(() => {
    setWorkshopWarningReporter(onWarning || null);
    return () => {
      setWorkshopWarningReporter(null);
    };
  }, [onWarning]);

  useEffect(() => {
    return () => {
      if (scrollIdleTimerRef.current !== null) {
        window.clearTimeout(scrollIdleTimerRef.current);
      }
    };
  }, []);

  // ── Fetch homepage ─────────────────────────────────────────────────────────

  const fetchHomepage = useCallback(async () => {
    const requestId = ++homepageRequestRef.current;
    setHomepageLoading(true);
    setHomepageError(null);
    try {
      const data = await fetchWorkshopHome();
      if (requestId !== homepageRequestRef.current) return;
      setHomepageSections(data.sections);
      onRecordSeenItems?.(data.sections.flatMap((sec: { items: WorkshopItem[] }) => sec.items), 'workshop-home');
      setTagCategories(data.tagCategories);
    } catch (err) {
      if (requestId !== homepageRequestRef.current) return;
      console.error('Failed to fetch homepage:', err);
      setHomepageSections([]);
      setTagCategories([]);
      setHomepageError(String(err));
    } finally {
      if (requestId === homepageRequestRef.current) {
        setHomepageLoading(false);
      }
    }
  }, [onRecordSeenItems]);

  useEffect(() => {
    if (viewMode === 'home' && homepageSections.length === 0 && !homepageLoading) {
      fetchHomepage();
    }
  }, [viewMode, fetchHomepage, homepageSections.length, homepageLoading]);

  // ── Fetch browse items ─────────────────────────────────────────────────────

  const fetchItems = useCallback(async () => {
    const requestId = ++searchRequestRef.current;
    setLoading(true);
    setError(null);
    try {
      const data = await fetchWorkshopItems({
        query: committedQuery,
        sort,
        section,
        page,
        creatorId,
        activeTag,
        activeTagName,
      });
      if (requestId !== searchRequestRef.current) return;
      setItems(data.items);
      onRecordSeenItems?.(data.items, creatorId ? 'workshop-creator' : committedQuery ? 'workshop-search' : 'workshop-browse');
    } catch (err) {
      if (requestId !== searchRequestRef.current) return;
      console.error(err);
      setError(`${t('common.error')}: ${err}`);
    } finally {
      if (requestId === searchRequestRef.current) {
        setLoading(false);
      }
    }
  }, [committedQuery, sort, section, page, creatorId, activeTag, activeTagName, onRecordSeenItems, t]);

  useEffect(() => {
    if (viewMode === 'browse' || viewMode === 'search') fetchItems();
  }, [viewMode, fetchItems]);

  // ── Navigation helpers ─────────────────────────────────────────────────────

  const enterBrowseMode = useCallback(() => {
    setViewMode('browse');
    setCreatorId(null); setCreatorName(null);
    setActiveTag(null); setActiveTagName(null);
    setPage(1); setSort('trend'); setSection('readytouseitems');
  }, []);

  const handleDirectNavigation = useCallback(async (workshopId: string) => {
    setLoadingDetailId(workshopId);
    try {
      const data = await fetchWorkshopItem(workshopId);
      
      let isCollection = false;
      
      // Check if it's a collection based on SDK/cached details first
      if (data.item) {
        const anyItem = data.item as any;
        if (anyItem.childItemIds && anyItem.childItemIds.length > 0) {
          isCollection = true;
        } else if (anyItem.childCount && anyItem.childCount > 0) {
          isCollection = true;
        }
      }

      // Check scraped details as backup/supplement
      try {
        const pageDetails = await fetchWorkshopPageDetails(workshopId, 'workshop-direct-nav');
        if (pageDetails.collectionItems && pageDetails.collectionItems.length > 0) {
          isCollection = true;
        } else if (pageDetails.childItemIds && pageDetails.childItemIds.length > 0) {
          isCollection = true;
        } else if (pageDetails.fileType === 'Collection' || pageDetails.fileType?.toLowerCase().includes('collection')) {
          isCollection = true;
        }
      } catch (e) {
        console.warn('Failed to fetch page details during direct nav check:', e);
      }

      if (isCollection) {
        const colData = await fetchWorkshopCollection(workshopId);
        const raw = colData.collection;
        const collectionItems = colData.items;
        if (raw && raw.publishedfileid) {
          const collectionItem = mapSteamDetailToWorkshopItem(raw, colData.source);
          navigateToDetail(null, {
            title: collectionItem.title,
            description: raw.description || '',
            imagePath: collectionItem.imagePath,
            creatorName: collectionItem.authorName,
            creatorId: collectionItem.authorId,
            items: collectionItems,
            workshopId: raw.publishedfileid,
          });
          onRecordSeenItems?.([collectionItem, ...collectionItems], 'workshop-collection');
        }
      } else {
        if (data.item && data.item.workshopId) {
          navigateToDetail(data.item, null);
          onRecordSeenItems?.([data.item], 'workshop-item-detail');
        } else {
          throw new Error('Item details not found');
        }
      }
    } catch (err) {
      setAlertInfo({
        open: true,
        title: t('common.error') || '错误',
        message: t('workshop.detail.fetchFailed', { err: String(err) }),
      });
    } finally {
      setLoadingDetailId(null);
    }
  }, [navigateToDetail, onRecordSeenItems, t]);

  const handleSearchSubmit = useCallback((submittedVal: string) => {
    const trimmed = submittedVal.trim();
    setQuery(trimmed);
    if (!trimmed) {
      setCommittedQuery('');
      if (viewMode === 'search') {
        setSort('trend');
        setSection('readytouseitems');
        setViewMode('browse');
        setPage(1);
      }
      setSelectedItem(null);
      setSelectedCollection(null);
      setDetailHistory([]);
      return;
    }

    const parsedId = parseWorkshopId(trimmed);
    if (parsedId) {
      handleDirectNavigation(parsedId);
      return;
    }

    setCreatorId(null);
    setCreatorName(null);
    setActiveTag(null);
    setActiveTagName(null);
    setSection('readytouseitems');
    setSort('textsearch');
    setPage(1);
    setCommittedQuery(trimmed);
    setViewMode('search');

    // Close details view to show search results
    setSelectedItem(null);
    setSelectedCollection(null);
    setDetailHistory([]);
  }, [viewMode, handleDirectNavigation, setSelectedItem, setSelectedCollection, setDetailHistory]);

  const handleSearchClear = useCallback(() => {
    setQuery('');
    if (viewMode === 'search') {
      setCommittedQuery('');
      setViewMode('browse');
      setPage(1);
    }
    // Close details view
    setSelectedItem(null);
    setSelectedCollection(null);
    setDetailHistory([]);
  }, [viewMode, setSelectedItem, setSelectedCollection, setDetailHistory]);

  const handleSearchFocus = useCallback(() => {
    if (viewMode === 'home') {
      enterBrowseMode();
    }
  }, [viewMode, enterBrowseMode]);

  const handleClearCreator = () => { setCreatorId(null); setCreatorName(null); setPage(1); };
  const handleClearTag = () => { setActiveTag(null); setActiveTagName(null); setPage(1); };

  const handleTagClick = (tagId: string, tagName: string) => {
    setActiveTag(tagId);
    setActiveTagName(tagName);
    setCreatorId(null);
    setCreatorName(null);
    setPage(1);
    setViewMode('browse');
  };

  const handleViewAllSection = useCallback((sec: HomepageSection) => {
    setSort(sec.browseParams.sort);
    setSection(sec.browseParams.section);
    setPage(1);
    setCreatorId(null); setCreatorName(null);
    setActiveTag(null); setActiveTagName(null);
    setViewMode('browse');
  }, []);

  // ── Detail helpers ─────────────────────────────────────────────────────────

  const viewItemDetails = useCallback(async (workshopId: string) => {
    setLoadingDetailId(workshopId);
    try {
      const data = await fetchWorkshopItem(workshopId);
      if (data.item && data.item.workshopId) {
        const item = data.item;
        navigateToDetail(item, null);
        onRecordSeenItems?.([item], 'workshop-item-detail');
      }
    } catch (err) {
      setAlertInfo({
        open: true,
        title: t('common.error') || '错误',
        message: t('workshop.detail.fetchFailed', { err: String(err) }),
      });
    } finally {
      setLoadingDetailId(null);
    }
  }, [navigateToDetail, onRecordSeenItems, t]);

  const viewCollectionDetails = useCallback(async (collectionId: string) => {
    setLoadingDetailId(collectionId);
    try {
      const data = await fetchWorkshopCollection(collectionId);
      const raw = data.collection;
      const collectionItems = data.items;
      if (raw && raw.publishedfileid) {
        const collectionItem = mapSteamDetailToWorkshopItem(raw, data.source);
        navigateToDetail(null, {
          title: collectionItem.title,
          description: raw.description || '',
          imagePath: collectionItem.imagePath,
          creatorName: collectionItem.authorName,
          creatorId: collectionItem.authorId,
          items: collectionItems,
          workshopId: raw.publishedfileid,
        });
        onRecordSeenItems?.([collectionItem, ...collectionItems], 'workshop-collection');
      }
    } catch (err) {
      setAlertInfo({
        open: true,
        title: t('common.error') || '错误',
        message: t('workshop.detail.fetchFailed', { err: String(err) }),
      });
    } finally {
      setLoadingDetailId(null);
    }
  }, [navigateToDetail, onRecordSeenItems, t]);

  const handleItemClick = useCallback((item: WorkshopItem) => {
    if (section === 'collections') {
      viewCollectionDetails(item.workshopId);
    } else {
      viewItemDetails(item.workshopId);
    }
  }, [section, viewCollectionDetails, viewItemDetails]);

  const handleCarouselItemClick = useCallback((item: WorkshopItem) => {
    viewItemDetails(item.workshopId);
  }, [viewItemDetails]);

  // ── Render: Homepage ───────────────────────────────────────────────────────

  const renderHomepage = () => {
    if (homepageLoading) {
      return (
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', padding: '64px 0', flex: 1, color: 'var(--md-sys-color-outline)' }}>
          <Loader2 size={36} className="animate-spin" />
          <p style={{ marginTop: '16px' }}>{t('workshop.home.loading')}</p>
        </div>
      );
    }
    if (homepageSections.length === 0) {
      return (
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', padding: '64px 0', flex: 1, color: 'var(--md-sys-color-outline)' }}>
          <p>{t('workshop.home.loadFailed')}</p>
          {homepageError && (
            <p style={{ marginTop: '8px', fontSize: '12px', maxWidth: '520px', textAlign: 'center' }}>{homepageError}</p>
          )}
          <button className="btn btn-outline" onClick={fetchHomepage} style={{ marginTop: '12px', borderRadius: '100px' }}>
            {t('workshop.browse.retry')}
          </button>
        </div>
      );
    }
    return (
      <div style={{ flex: 1, minHeight: 0, paddingRight: '8px' }}>
        {homepageSections.map((sec) => (
          <SectionCarousel
            key={sec.id}
            section={sec}
            sectionType={sec.id === 'collections' ? 'collections' : 'readytouseitems'}
            addons={addons}
            knownUninstalledAddons={knownUninstalledAddons}
            knownCollectionIds={knownCollectionIds}
            onItemClick={handleCarouselItemClick}
            onViewAll={handleViewAllSection}
            loadingDetailId={loadingDetailId}
          />
        ))}
      </div>
    );
  };

  // ── Render: Browse view ────────────────────────────────────────────────────

  const renderBrowseView = () => (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0, minWidth: 0 }}>
      {/* SDK Only Search Warning */}
      {workshopSourceSettings?.allowSteamworksSdk && !workshopSourceSettings?.allowSteamCommunityHtml && (
        <div style={{ padding: '12px 14px', borderRadius: '12px', background: 'rgba(255, 180, 171, 0.1)', border: '1px solid rgba(255, 180, 171, 0.3)', color: 'var(--md-sys-color-error)', fontSize: '13px', lineHeight: '1.5', marginBottom: '16px', display: 'flex', gap: '10px' }}>
          <AlertTriangle size={18} style={{ flexShrink: 0 }} />
          <div>
            <strong>{t('workshop.browse.sdkSearchWarningTitle', '搜索与浏览功能受限')}</strong><br/>
            {t('workshop.browse.sdkSearchWarningDesc', '当前网页抓取功能已被禁用，所有数据仅通过 SDK 获取。搜索与分类浏览将受到影响，具体限制请查阅相关文档。')}
          </div>
        </div>
      )}

      {/* Active filter chips */}
        {(creatorName || activeTagName) && (
          <div style={{ display: 'flex', gap: '8px', marginBottom: '16px', flexWrap: 'wrap' }}>
            {creatorName && (
              <div style={{ display: 'flex', alignItems: 'center', gap: '8px', padding: '6px 16px', backgroundColor: 'var(--md-sys-color-secondary-container)', color: 'var(--md-sys-color-on-secondary-container)', borderRadius: '100px', fontSize: '13px' }}>
                <User size={14} />
                <span>{t('workshop.item.author', { author: creatorName })}</span>
                <button onClick={handleClearCreator} style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'inherit', fontWeight: 'bold', marginLeft: '4px' }}>×</button>
              </div>
            )}
            {activeTagName && (
              <div style={{ display: 'flex', alignItems: 'center', gap: '8px', padding: '6px 16px', backgroundColor: 'var(--md-sys-color-tertiary-container)', color: 'var(--md-sys-color-on-tertiary-container)', borderRadius: '100px', fontSize: '13px' }}>
                <Tag size={14} />
                <span>{t('workshop.tags.label', { name: activeTagName })}</span>
                <button onClick={handleClearTag} style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'inherit', fontWeight: 'bold', marginLeft: '4px' }}>×</button>
              </div>
            )}
          </div>
        )}

        {/* Loading / Error / Items */}
        {loading ? (
          <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', padding: '64px 0', flex: 1, color: 'var(--md-sys-color-outline)' }}>
            <Loader2 size={36} className="animate-spin" />
            <p style={{ marginTop: '16px' }}>{t('workshop.browse.loading')}</p>
          </div>
        ) : error ? (
          <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', padding: '64px 0', flex: 1, color: 'var(--md-sys-color-error)' }}>
            <p>{error}</p>
            <button className="btn btn-outline" onClick={fetchItems}>{t('workshop.browse.retry')}</button>
          </div>
        ) : items.length === 0 ? (
          <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', padding: '64px 0', flex: 1, color: 'var(--md-sys-color-outline)' }}>
            <p>{t('workshop.browse.empty')}</p>
          </div>
        ) : (
          <>
            <div className="addons-grid" style={{ marginBottom: '24px', gap: '24px', padding: '4px' }}>
              {items.map((item) => (
                <ItemCard
                  key={item.workshopId}
                  item={item}
                  section={section}
                  addons={addons}
                  knownUninstalledAddons={knownUninstalledAddons}
                  knownCollectionIds={knownCollectionIds}
                  onClick={handleItemClick}
                  isLoading={loadingDetailId === item.workshopId}
                />
              ))}
            </div>
            {/* Pagination */}
            <div style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', gap: '16px', marginTop: 'auto', paddingBottom: '16px' }}>
              <button className="btn btn-outline" disabled={page === 1 || loading} onClick={() => setPage(p => p - 1)} style={{ borderRadius: '50%', padding: '12px', minWidth: '40px', width: '40px', height: '40px', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                <ChevronLeft size={16} />
              </button>
              <span style={{ fontSize: '14px', fontWeight: 500 }}>{t('workshop.browse.page', { page })}</span>
              <button className="btn btn-outline" disabled={items.length < 30 || loading} onClick={() => setPage(p => p + 1)} style={{ borderRadius: '50%', padding: '12px', minWidth: '40px', width: '40px', height: '40px', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                <ChevronRight size={16} />
              </button>
            </div>
          </>
        )}
    </div>
  );

  // ── Main render ────────────────────────────────────────────────────────────

  return (
    <div className={`workshop-browser${isScrollInteracting ? ' is-scroll-interacting' : ''}`} style={{ display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0, minWidth: 0, overflow: 'hidden' }}>
      {/* Top navigation bar */}
      <div style={{ display: 'flex', alignItems: 'center', gap: '16px', padding: '16px 24px', borderBottom: '1px solid var(--md-sys-color-outline-variant)', flexShrink: 0 }}>
        <div style={{ display: 'flex', borderRadius: '100px', backgroundColor: 'var(--md-sys-color-surface-container-high)', padding: '4px' }}>
          <button
            className={`btn ${viewMode === 'home' && !selectedItem && !selectedCollection ? 'btn-primary' : ''}`}
            onClick={() => { setViewMode('home'); setPage(1); setSelectedItem(null); setSelectedCollection(null); setDetailHistory([]); }}
            style={{ borderRadius: '100px', padding: '6px 16px', border: 'none', boxShadow: 'none', fontSize: '13px', display: 'flex', alignItems: 'center', gap: '6px', background: viewMode === 'home' && !selectedItem && !selectedCollection ? undefined : 'transparent', color: viewMode === 'home' && !selectedItem && !selectedCollection ? undefined : 'var(--md-sys-color-on-surface)' }}
          >
            <Home size={14} /> {t('workshop.nav.home')}
          </button>
          <button
            className={`btn ${viewMode === 'browse' && !selectedItem && !selectedCollection ? 'btn-primary' : ''}`}
            onClick={() => { enterBrowseMode(); setSelectedItem(null); setSelectedCollection(null); setDetailHistory([]); }}
            style={{ borderRadius: '100px', padding: '6px 16px', border: 'none', boxShadow: 'none', fontSize: '13px', display: 'flex', alignItems: 'center', gap: '6px', background: viewMode === 'browse' && !selectedItem && !selectedCollection ? undefined : 'transparent', color: viewMode === 'browse' && !selectedItem && !selectedCollection ? undefined : 'var(--md-sys-color-on-surface)' }}
          >
            <Compass size={14} /> {t('workshop.nav.browse')}
          </button>
        </div>

        <WorkshopSearchBar
          initialValue={query}
          placeholder={t('workshop.nav.searchPlaceholder')}
          searchLabel={t('common.search')}
          onSubmit={handleSearchSubmit}
          onClear={handleSearchClear}
          onFocus={handleSearchFocus}
        />

        {(viewMode === 'browse' || viewMode === 'search') && !creatorId && !selectedItem && !selectedCollection && (
          <>
            <CustomSelect
              options={SORT_OPTIONS.map((opt) => ({
                value: opt.value,
                label: t(opt.labelKey),
              }))}
              value={sort}
              onChange={(val) => { setSort(val); setPage(1); }}
              minWidth="140px"
              style={{ height: '36px' }}
            />
            <div style={{ display: 'flex', borderRadius: '100px', backgroundColor: 'var(--md-sys-color-surface-container-high)', padding: '3px' }}>
              <button
                className={`btn ${section === 'readytouseitems' ? 'btn-primary' : ''}`}
                onClick={() => { setSection('readytouseitems'); setPage(1); }}
                style={{ borderRadius: '100px', padding: '5px 14px', border: 'none', boxShadow: 'none', fontSize: '12px', background: section === 'readytouseitems' ? undefined : 'transparent', color: section === 'readytouseitems' ? undefined : 'var(--md-sys-color-on-surface)' }}
              >
                {t('workshop.browse.sectionAddons')}
              </button>
              <button
                className={`btn ${section === 'collections' ? 'btn-primary' : ''}`}
                onClick={() => { setSection('collections'); setPage(1); }}
                style={{ borderRadius: '100px', padding: '5px 14px', border: 'none', boxShadow: 'none', fontSize: '12px', background: section === 'collections' ? undefined : 'transparent', color: section === 'collections' ? undefined : 'var(--md-sys-color-on-surface)' }}
              >
                {t('workshop.browse.sectionCollections')}
              </button>
            </div>
          </>
        )}

        {/* Tag browser button */}
        {tagCategories.length > 0 && !selectedItem && !selectedCollection && (
          <button
            className={`btn ${activeTag ? 'btn-primary' : 'btn-secondary'}`}
            onClick={() => setTagModalOpen(true)}
            style={{ borderRadius: '100px', padding: '6px 16px', fontSize: '13px', display: 'flex', alignItems: 'center', gap: '6px', marginLeft: 'auto' }}
          >
            <Tag size={14} />
            <span>{activeTagName || t('workshop.tags.browse')}</span>
          </button>
        )}

        <div style={{ marginLeft: (tagCategories.length > 0 && !selectedItem && !selectedCollection) ? '0' : 'auto' }}>
          <TaskCenterButton
            syncingSteam={syncingSteam}
            backgroundTasks={backgroundTasks}
            onClick={onOpenTaskCenter}
          />
        </div>
      </div>

      {/* Content area */}
      <div
        className="workshop-browser-scroll-root"
        style={{ flex: 1, padding: '24px', display: 'flex', flexDirection: 'column' }}
        onWheelCapture={markScrollInteraction}
        onScroll={markScrollInteraction}
      >
        {selectedItem || selectedCollection ? (
          <WorkshopDetailModal
            open={!!(selectedItem || selectedCollection)}
            item={selectedItem}
            collection={selectedCollection}
            onClose={handleBack}
            onDownload={onDownload}
            onDownloadMany={onDownloadMany}
            onOpenLink={onOpenLink}
            onImportCollection={onImportCollection}
            onItemNavigate={viewItemDetails}
            onCollectionNavigate={viewCollectionDetails}
            onDirectNavigate={handleDirectNavigation}
            addons={addons}
            knownUninstalledAddons={knownUninstalledAddons}
            downloadProgress={downloadProgress}
            isSubmitting={isSubmitting}
            groups={groups}
            isLoading={!!loadingDetailId}
            onDatabaseUpdate={onDatabaseUpdate}
          />
        ) : viewMode === 'home' ? (
          renderHomepage()
        ) : (
          renderBrowseView()
        )}
      </div>

      {/* Tag browser modal */}
      <TagBrowserModal
        open={tagModalOpen}
        categories={tagCategories}
        activeTag={activeTag}
        onClose={() => setTagModalOpen(false)}
        onTagClick={handleTagClick}
      />

      {/* Alert modal */}
      <AlertModal
        open={alertInfo.open}
        title={alertInfo.title}
        message={alertInfo.message}
        onClose={() => setAlertInfo({ open: false, title: '', message: '' })}
      />
    </div>
  );
};
