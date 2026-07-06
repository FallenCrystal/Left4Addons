/**
 * WorkshopBrowser — main container for the built-in Steam Workshop browser.
 *
 * Splits into two views:
 *   • Homepage  — section carousels (trending, most-subscribed, …)
 *   • Browse    — filterable grid with tag sidebar
 */

import React, { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import {
  Search, ChevronLeft, ChevronRight, Loader2,
  Home, Compass, User, Tag,
} from 'lucide-react';

import {
  WorkshopItem,
  HomepageSection,
  TagCategory,
  WorkshopBrowserProps,
} from './workshop/types';
import {
  parseSSRItems,
  parseHomepageSections,
  parseTagCategories,
} from './workshop/ssrParser';
import { ItemCard } from './workshop/ItemCard';
import { TagBrowserModal } from './workshop/TagBrowserModal';
import { SectionCarousel } from './workshop/SectionCarousel';
import { WorkshopDetailModal } from './workshop/WorkshopDetailModal';

// ── Browse sort options ───────────────────────────────────────────────────────

const SORT_OPTIONS = [
  { value: 'trend', labelKey: 'workshop.browse.sortTrend' },
  { value: 'textsearch', labelKey: 'workshop.browse.sortTextSearch' },
  { value: 'totalprofiles', labelKey: 'workshop.browse.sortTotalProfiles' },
  { value: 'mostrecent', labelKey: 'workshop.browse.sortMostRecent' },
  { value: 'toprated', labelKey: 'workshop.browse.sortTopRated' },
  { value: 'lastupdated', labelKey: 'workshop.browse.sortLastUpdated' },
] as const;

// ── Main Component ────────────────────────────────────────────────────────────

export const WorkshopBrowser: React.FC<WorkshopBrowserProps> = ({
  addons,
  knownUninstalledAddons,
  downloadProgress,
  onDownload,
  onOpenLink,
  onImportCollection,
  isSubmitting,
  groups,
}) => {
  const { t } = useTranslation();

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
  const [tagCategories, setTagCategories] = useState<TagCategory[]>([]);

  // Detail modal
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
  const [tagModalOpen, setTagModalOpen] = useState(false);
  const [loadingDetailId, setLoadingDetailId] = useState<string | null>(null);

  // ── Fetch homepage ─────────────────────────────────────────────────────────

  const fetchHomepage = useCallback(async () => {
    setHomepageLoading(true);
    try {
      const html: string = await invoke('fetch_workshop_html', {
        url: 'https://steamcommunity.com/app/550/workshop/',
      });
      setHomepageSections(parseHomepageSections(html));
      setTagCategories(parseTagCategories(html));
    } catch (err) {
      console.error('Failed to fetch homepage:', err);
    } finally {
      setHomepageLoading(false);
    }
  }, []);

  useEffect(() => {
    if (viewMode === 'home') fetchHomepage();
  }, [viewMode, fetchHomepage]);

  // ── Fetch browse items ─────────────────────────────────────────────────────

  const fetchItems = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      let url = 'https://steamcommunity.com/workshop/browse/?appid=550';
      if (creatorId) {
        url += `&browsesort=myfiles&creatorid=${creatorId}&p=${page}`;
      } else {
        url += `&searchtext=${encodeURIComponent(committedQuery)}&browsesort=${sort}&section=${section}&p=${page}`;
        if (activeTag) {
          url += `&requiredtags[]=${encodeURIComponent(activeTagName || activeTag)}`;
        }
      }
      const html: string = await invoke('fetch_workshop_html', { url });
      setItems(parseSSRItems(html, 'workshop_query'));
    } catch (err) {
      console.error(err);
      setError(`${t('common.error')}: ${err}`);
    } finally {
      setLoading(false);
    }
  }, [committedQuery, sort, section, page, creatorId, activeTag, activeTagName, t]);

  useEffect(() => {
    if (viewMode === 'browse' || viewMode === 'search') fetchItems();
  }, [viewMode, fetchItems]);

  // ── Navigation helpers ─────────────────────────────────────────────────────

  const handleSearchSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!query.trim()) {
      setCommittedQuery('');
      if (viewMode === 'search') {
        setViewMode('browse');
        setPage(1);
      }
      return;
    }
    setCreatorId(null);
    setCreatorName(null);
    setActiveTag(null);
    setActiveTagName(null);
    setPage(1);
    setCommittedQuery(query.trim());
    setViewMode('search');
  };


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

  const handleViewAllSection = (sec: HomepageSection) => {
    setSort(sec.browseParams.sort);
    setSection(sec.browseParams.section);
    setPage(1);
    setCreatorId(null); setCreatorName(null);
    setActiveTag(null); setActiveTagName(null);
    setViewMode('browse');
  };

  const enterBrowseMode = () => {
    setViewMode('browse');
    setCreatorId(null); setCreatorName(null);
    setActiveTag(null); setActiveTagName(null);
    setPage(1); setSort('trend'); setSection('readytouseitems');
  };

  // ── Detail helpers ─────────────────────────────────────────────────────────

  /** Map raw Steam API detail → WorkshopItem for the modal */
  const mapSteamDetailToItem = (d: any): WorkshopItem => ({
    workshopId: d.publishedfileid || '',
    title: (d.title || '').trim(),
    imagePath: d.preview_url || '',
    authorName: d.creator_name || '',
    authorId: d.creator || '',
    authorUrl: d.creator ? `https://steamcommunity.com/profiles/${d.creator}` : '',
    stars: d.star_rating ?? 0,
    shortDescription: d.short_description || d.description || '',
    fileSize: d.file_size ? `${(parseInt(d.file_size) / 1024 / 1024).toFixed(1)} MB` : undefined,
    tags: d.tags ? d.tags.map((t: any) => t.display_name || t.tag || '') : [],
    subscriptions: d.subscriptions ? parseInt(d.subscriptions) : undefined,
    timeCreated: d.time_created ? parseInt(d.time_created) : undefined,
    timeUpdated: d.time_updated ? parseInt(d.time_updated) : undefined,
    childCount: d.num_children !== undefined ? parseInt(d.num_children) : undefined,
  });

  const viewItemDetails = async (workshopId: string) => {
    setLoadingDetailId(workshopId);
    try {
      const data: any = await invoke('fetch_collection', { collectionId: workshopId });
      const raw = data.collection;
      if (raw && raw.publishedfileid) {
        setSelectedItem(mapSteamDetailToItem(raw));
        setSelectedCollection(null);
      }
    } catch (err) {
      alert(t('workshop.detail.fetchFailed', { err: String(err) }));
    } finally {
      setLoadingDetailId(null);
    }
  };

  const viewCollectionDetails = async (collectionId: string) => {
    setLoadingDetailId(collectionId);
    try {
      const data: any = await invoke('fetch_collection', { collectionId });
      const raw = data.collection;
      const rawItems: any[] = data.items || [];
      if (raw && raw.publishedfileid) {
        setSelectedCollection({
          title: (raw.title || '').trim(),
          description: raw.description || '',
          imagePath: raw.preview_url || '',
          creatorName: raw.creator_name || '',
          creatorId: raw.creator || '',
          items: rawItems.map(mapSteamDetailToItem),
          workshopId: raw.publishedfileid,
        });
        setSelectedItem(null);
      }
    } catch (err) {
      alert(t('workshop.detail.fetchFailed', { err: String(err) }));
    } finally {
      setLoadingDetailId(null);
    }
  };

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
          <button className="btn btn-outline" onClick={enterBrowseMode} style={{ marginTop: '12px', borderRadius: '100px' }}>
            {t('workshop.home.switchToBrowse')}
          </button>
        </div>
      );
    }
    return (
      <div style={{ flex: 1, overflowY: 'auto', paddingRight: '8px' }}>
        {homepageSections.map((sec) => (
          <SectionCarousel
            key={sec.id}
            section={sec}
            sectionType={sec.id === 'collections' ? 'collections' : 'readytouseitems'}
            addons={addons}
            knownUninstalledAddons={knownUninstalledAddons}
            onItemClick={(item) => viewItemDetails(item.workshopId)}
            onViewAll={handleViewAllSection}
            loadingDetailId={loadingDetailId}
          />
        ))}
      </div>
    );
  };

  // ── Render: Browse view ────────────────────────────────────────────────────

  const renderBrowseView = () => (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', overflowY: 'auto' }}>
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
                  onClick={() => section === 'collections' ? viewCollectionDetails(item.workshopId) : viewItemDetails(item.workshopId)}
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
    <div className="workshop-browser" style={{ display: 'flex', flexDirection: 'column', height: '100%', overflow: 'hidden' }}>
      {/* Top navigation bar */}
      <div style={{ display: 'flex', alignItems: 'center', gap: '16px', padding: '16px 24px', borderBottom: '1px solid var(--md-sys-color-outline-variant)', flexShrink: 0 }}>
        <div style={{ display: 'flex', borderRadius: '100px', backgroundColor: 'var(--md-sys-color-surface-container-high)', padding: '4px' }}>
          <button
            className={`btn ${viewMode === 'home' ? 'btn-primary' : ''}`}
            onClick={() => { setViewMode('home'); setPage(1); }}
            style={{ borderRadius: '100px', padding: '6px 16px', border: 'none', boxShadow: 'none', fontSize: '13px', display: 'flex', alignItems: 'center', gap: '6px', background: viewMode === 'home' ? undefined : 'transparent', color: viewMode === 'home' ? undefined : 'var(--md-sys-color-on-surface)' }}
          >
            <Home size={14} /> {t('workshop.nav.home')}
          </button>
          <button
            className={`btn ${viewMode === 'browse' ? 'btn-primary' : ''}`}
            onClick={enterBrowseMode}
            style={{ borderRadius: '100px', padding: '6px 16px', border: 'none', boxShadow: 'none', fontSize: '13px', display: 'flex', alignItems: 'center', gap: '6px', background: viewMode === 'browse' ? undefined : 'transparent', color: viewMode === 'browse' ? undefined : 'var(--md-sys-color-on-surface)' }}
          >
            <Compass size={14} /> {t('workshop.nav.browse')}
          </button>
        </div>

        <form onSubmit={handleSearchSubmit} style={{ display: 'flex', gap: '8px', flex: 1, maxWidth: '480px' }}>
          <div style={{ position: 'relative', flex: 1 }}>
            <Search size={16} style={{ position: 'absolute', left: '14px', top: '50%', transform: 'translateY(-50%)', color: 'var(--md-sys-color-outline)' }} />
            <input
              type="text"
              placeholder={t('workshop.nav.searchPlaceholder')}
              value={query}
              onChange={(e) => {
                const val = e.target.value;
                setQuery(val);
                if (val.trim() === '' && viewMode === 'search') {
                  setCommittedQuery('');
                  setViewMode('browse');
                  setPage(1);
                }
              }}
              onFocus={() => {
                if (viewMode === 'home') {
                  enterBrowseMode();
                }
              }}
              style={{ width: '100%', padding: '8px 14px 8px 40px', borderRadius: '100px', border: '1px solid var(--md-sys-color-outline-variant)', backgroundColor: 'var(--md-sys-color-surface-container-high)', color: 'var(--md-sys-color-on-surface)', outline: 'none', fontSize: '13px' }}
            />
          </div>
          <button type="submit" className="btn btn-primary" style={{ borderRadius: '100px', padding: '0 20px', fontSize: '13px' }}>
            {t('common.search')}
          </button>
        </form>

        {(viewMode === 'browse' || viewMode === 'search') && !creatorId && (
          <>
            <select
              value={sort}
              onChange={(e) => { setSort(e.target.value); setPage(1); }}
              style={{ padding: '8px 14px', borderRadius: '100px', border: '1px solid var(--md-sys-color-outline-variant)', backgroundColor: 'var(--md-sys-color-surface-container-low)', color: 'var(--md-sys-color-on-surface)', outline: 'none', fontSize: '12px' }}
            >
              {SORT_OPTIONS.map((opt) => (
                <option key={opt.value} value={opt.value}>{t(opt.labelKey)}</option>
              ))}
            </select>
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
        {tagCategories.length > 0 && (
          <button
            className={`btn ${activeTag ? 'btn-primary' : 'btn-secondary'}`}
            onClick={() => setTagModalOpen(true)}
            style={{ borderRadius: '100px', padding: '6px 16px', fontSize: '13px', display: 'flex', alignItems: 'center', gap: '6px', marginLeft: 'auto' }}
          >
            <Tag size={14} />
            <span>{activeTagName || t('workshop.tags.browse')}</span>
          </button>
        )}
      </div>

      {/* Content area */}
      <div style={{ flex: 1, overflow: 'hidden', padding: '24px', display: 'flex', flexDirection: 'column' }}>
        {viewMode === 'home' ? renderHomepage() : renderBrowseView()}
      </div>

      {/* Detail modal */}
      <WorkshopDetailModal
        open={!!(selectedItem || selectedCollection)}
        item={selectedItem}
        collection={selectedCollection}
        onClose={() => { setSelectedItem(null); setSelectedCollection(null); }}
        onDownload={onDownload}
        onOpenLink={onOpenLink}
        onImportCollection={onImportCollection}
        onItemNavigate={viewItemDetails}
        onCollectionNavigate={viewCollectionDetails}
        addons={addons}
        knownUninstalledAddons={knownUninstalledAddons}
        downloadProgress={downloadProgress}
        isSubmitting={isSubmitting}
        groups={groups}
        isLoading={!!loadingDetailId}
      />

      {/* Tag browser modal */}
      <TagBrowserModal
        open={tagModalOpen}
        categories={tagCategories}
        activeTag={activeTag}
        onClose={() => setTagModalOpen(false)}
        onTagClick={handleTagClick}
      />
    </div>
  );
};
