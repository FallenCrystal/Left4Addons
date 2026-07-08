import { invoke } from '@tauri-apps/api/core';
import {
  HomepageSection,
  TagCategory,
  WorkshopCapabilities,
  WorkshopItem,
} from '../components/workshop/types';
import {
  parseHomepageSections,
  parseSSRItems,
  parseTagCategories,
  parseWorkshopPageDetails,
} from '../components/workshop/ssrParser';

interface WorkshopHomeResponse {
  source: string;
  sections: Array<{
    id: string;
    titleKey: string;
    subtitleKey: string;
    icon: string;
    items: any[];
    browseParams: {
      sort: string;
      section: string;
      days?: number;
    };
  }>;
}

interface WorkshopItemsResponse {
  source: string;
  items: any[];
}

interface WorkshopItemResponse {
  source: string;
  item: any;
}

interface WorkshopCollectionResponse {
  source: string;
  collection: any;
  items: any[];
}

export interface FetchWorkshopItemsInput {
  query?: string;
  sort?: string;
  section?: string;
  page?: number;
  creatorId?: string | null;
  activeTag?: string | null;
  activeTagName?: string | null;
}

let capabilitiesPromise: Promise<WorkshopCapabilities> | null = null;

export async function getWorkshopCapabilities(): Promise<WorkshopCapabilities> {
  capabilitiesPromise ??= invoke<WorkshopCapabilities>('get_workshop_capabilities').catch((err) => {
    capabilitiesPromise = null;
    throw err;
  });
  return capabilitiesPromise;
}

export async function fetchWorkshopHome() {
  const capabilities = await getWorkshopCapabilities().catch(() => null);
  let sdkSections: HomepageSection[] = [];
  let source = 'web-fallback';

  if (capabilities?.canQueryHome) {
    try {
      const data = await invoke<WorkshopHomeResponse>('query_workshop_home');
      sdkSections = data.sections.map((section) => ({
        id: section.id,
        title: section.titleKey,
        subtitle: section.subtitleKey,
        icon: section.icon,
        items: section.items.map((item) => mapSteamDetailToWorkshopItem(item, data.source)),
        browseParams: section.browseParams,
      }));
      source = data.source;
    } catch (err) {
      console.warn('Steam SDK home query failed, falling back to HTML:', err);
    }
  }

  const html = await invoke<string>('fetch_workshop_html', {
    url: 'https://steamcommunity.com/app/550/workshop/',
    source: 'workshop-home',
  });
  const htmlSections = parseHomepageSections(html);
  const tagCategories = parseTagCategories(html);

  if (sdkSections.length === 0) {
    return {
      source: 'web-fallback',
      sections: htmlSections,
      tagCategories,
    };
  }

  return {
    source: source === 'steam-sdk' ? 'hybrid' : source,
    sections: mergeHomepageSections(sdkSections, htmlSections),
    tagCategories,
  };
}

export async function fetchWorkshopItems(input: FetchWorkshopItemsInput) {
  const capabilities = await getWorkshopCapabilities().catch(() => null);
  if (capabilities?.canQueryItems) {
    const creatorId = input.creatorId?.trim();
    const creatorNumeric = !creatorId || /^\d+$/.test(creatorId);
    if (creatorNumeric) {
      try {
        const data = await invoke<WorkshopItemsResponse>('query_workshop_items', {
          query: {
            query: input.query || undefined,
            sort: input.sort || undefined,
            section: input.section || undefined,
            page: input.page || 1,
            creatorId: creatorId || undefined,
            activeTag: input.activeTag || undefined,
            activeTagName: input.activeTagName || undefined,
          },
        });
        return {
          source: data.source,
          items: data.items.map((item) => mapSteamDetailToWorkshopItem(item, data.source)),
        };
      } catch (err) {
        console.warn('Steam SDK browse query failed, falling back to HTML:', err);
      }
    }
  }

  const url = buildBrowseUrl(input);
  const html: string = await invoke('fetch_workshop_html', {
    url,
    source: input.creatorId ? 'workshop-creator' : input.query ? 'workshop-search' : 'workshop-browse',
  });
  return {
    source: 'web-fallback',
    items: parseSSRItems(html, 'workshop_query'),
  };
}

export async function fetchWorkshopItem(workshopId: string) {
  const capabilities = await getWorkshopCapabilities().catch(() => null);
  if (capabilities?.canQueryItems) {
    try {
      const data = await invoke<WorkshopItemResponse>('query_workshop_item', { workshopId });
      return {
        source: data.source,
        item: mapSteamDetailToWorkshopItem(data.item, data.source),
      };
    } catch (err) {
      console.warn('Steam SDK item query failed, falling back to existing collection command:', err);
    }
  }

  const data: any = await invoke('fetch_collection', { collectionId: workshopId });
  return {
    source: 'web-fallback',
    item: mapSteamDetailToWorkshopItem(data.collection, 'web-fallback'),
  };
}

export async function fetchWorkshopCollection(workshopId: string) {
  const capabilities = await getWorkshopCapabilities().catch(() => null);
  if (capabilities?.canQueryItems) {
    try {
      const data = await invoke<WorkshopCollectionResponse>('query_workshop_collection', { workshopId });
      return {
        source: data.source,
        collection: data.collection,
        items: data.items.map((item) => mapSteamDetailToWorkshopItem(item, data.source)),
      };
    } catch (err) {
      console.warn('Steam SDK collection query failed, falling back to existing collection command:', err);
    }
  }

  const data: any = await invoke('fetch_collection', { collectionId: workshopId });
  return {
    source: 'web-fallback',
    collection: data.collection,
    items: (data.items || []).map((item: any) => mapSteamDetailToWorkshopItem(item, 'web-fallback')),
  };
}

export async function fetchWorkshopHtml(url: string, source: string) {
  return invoke<string>('fetch_workshop_html', { url, source });
}

export async function fetchWorkshopPageDetails(workshopId: string, source: string) {
  const url = `https://steamcommunity.com/sharedfiles/filedetails/?id=${workshopId}`;
  const html = await fetchWorkshopHtml(url, source);
  return parseWorkshopPageDetails(html);
}

export async function persistWorkshopPageDetails(workshopId: string, details: any, source: string) {
  return invoke('persist_workshop_page_details', {
    workshopId,
    details,
    source,
  });
}

function buildBrowseUrl(input: FetchWorkshopItemsInput) {
  let url = 'https://steamcommunity.com/workshop/browse/?appid=550';
  if (input.creatorId) {
    url += `&browsesort=myfiles&creatorid=${input.creatorId}&p=${input.page || 1}`;
  } else {
    if (input.query) {
      url += `&searchtext=${encodeURIComponent(input.query)}`;
    }
    url += `&browsesort=${input.sort || 'trend'}&section=${input.section || 'readytouseitems'}&p=${input.page || 1}`;
    if (input.activeTag) {
      url += `&requiredtags[]=${encodeURIComponent(input.activeTagName || input.activeTag)}`;
    }
  }
  return url;
}

function mergeHomepageSections(primary: HomepageSection[], fallback: HomepageSection[]) {
  const primaryMap = new Map(primary.map((section) => [section.id, section]));
  const merged: HomepageSection[] = [];

  for (const section of fallback) {
    merged.push(primaryMap.get(section.id) || section);
    primaryMap.delete(section.id);
  }

  for (const section of primaryMap.values()) {
    merged.push(section);
  }

  return merged;
}

export function mapSteamDetailToWorkshopItem(detail: any, source = 'steam-sdk'): WorkshopItem {
  const ownerSteamId = String(
    detail.creator_steam_id ||
    detail.creatorSteamId ||
    detail.owner_steam_id ||
    detail.ownerSteamId ||
    '',
  );
  const ownerAccountId = String(
    detail.creator_account_id ||
    detail.creatorAccountId ||
    detail.creator ||
    detail.owner_account_id ||
    detail.ownerAccountId ||
    '',
  );
  const authorSteamId = ownerSteamId || undefined;
  const authorAccountId = ownerAccountId || undefined;
  const authorId = ownerAccountId || ownerSteamId || String(detail.creator || '');
  const authorName = String(detail.creator_name || detail.creatorName || detail.authorName || authorId || '');
  const authorUrl = authorSteamId
    ? `https://steamcommunity.com/profiles/${authorSteamId}`
    : '';

  return {
    workshopId: String(detail.publishedfileid || detail.workshopId || ''),
    title: String(detail.title || '').trim(),
    imagePath: String(detail.preview_url || detail.previewUrl || detail.imagePath || ''),
    authorName,
    authorId,
    authorUrl,
    authorSteamId,
    authorAccountId,
    ownerSteamId: authorSteamId,
    ownerAccountId: authorAccountId,
    stars: Number(detail.star_rating ?? detail.score ?? 0),
    shortDescription: detail.short_description || detail.shortDescription || detail.description || '',
    fileSize: detail.file_size
      ? `${(parseInt(String(detail.file_size), 10) / 1024 / 1024).toFixed(1)} MB`
      : detail.fileSizeDisplay,
    tags: Array.isArray(detail.tags)
      ? detail.tags.map((tag: any) => String(tag.display_name || tag.tag || tag.name || '')).filter(Boolean)
      : [],
    subscriptions: detail.subscriptions ? parseInt(String(detail.subscriptions), 10) : undefined,
    favorites: detail.favorites ? parseInt(String(detail.favorites), 10) : undefined,
    lifetimeSubscriptions: detail.lifetime_subscriptions ? parseInt(String(detail.lifetime_subscriptions), 10) : undefined,
    lifetimeFavorites: detail.lifetime_favorites ? parseInt(String(detail.lifetime_favorites), 10) : undefined,
    views: detail.views ? parseInt(String(detail.views), 10) : undefined,
    comments: detail.comments ? parseInt(String(detail.comments), 10) : undefined,
    totalVotes: detail.total_votes ? parseInt(String(detail.total_votes), 10) : undefined,
    timeCreated: detail.time_created ? parseInt(String(detail.time_created), 10) : undefined,
    timeUpdated: detail.time_updated ? parseInt(String(detail.time_updated), 10) : undefined,
    childCount: detail.num_children !== undefined ? parseInt(String(detail.num_children), 10) : undefined,
    previewCount: detail.preview_count !== undefined ? parseInt(String(detail.preview_count), 10) : undefined,
    childItemIds: detail.child_item_ids || detail.childItemIds || undefined,
    galleryPreviewUrls: detail.gallery_preview_urls || detail.galleryPreviewUrls || undefined,
    source,
    isSubscribed: detail.isSubscribed,
    isInstalled: detail.isInstalled,
    installState: detail.installState || detail.itemState,
  };
}
