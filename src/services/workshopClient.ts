import { invoke } from '@tauri-apps/api/core';
import {
  HomepageSection,
  WorkshopCapabilities,
  WorkshopItem,
} from '../components/workshop/types';
import {
  parseHomepageSections,
  parseSSRItems,
  parseTagCategories,
  parseWorkshopPageDetails,
} from '../components/workshop/ssrParser';
import {
  rememberWorkshopItems,
  rememberWorkshopPageDetails,
} from '../components/workshop/authorDirectory';

interface WorkshopHomeResponse {
  source: string;
  warnings?: string[];
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
  warnings?: string[];
  items: any[];
}

interface WorkshopItemResponse {
  source: string;
  warnings?: string[];
  item: any;
}

interface WorkshopCollectionResponse {
  source: string;
  warnings?: string[];
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
let steamworksSdkDisabled = false;
let workshopWarningReporter: ((message: string) => void) | null = null;

export function setSteamworksSdkDisabled(disabled: boolean) {
  steamworksSdkDisabled = disabled;
}

export function setWorkshopWarningReporter(reporter: ((message: string) => void) | null) {
  workshopWarningReporter = reporter;
}

function shouldUseSteamworksSdk() {
  return !steamworksSdkDisabled;
}

function resolveBrowseSort(input: FetchWorkshopItemsInput): string | undefined {
  if (input.query?.trim()) {
    return input.sort || 'textsearch';
  }
  return input.sort || undefined;
}

function reportWorkshopWarnings(warnings?: string[]) {
  if (!Array.isArray(warnings) || warnings.length === 0) {
    return;
  }

  const uniqueWarnings = [...new Set(warnings.map((warning) => normalizeText(warning)).filter(Boolean))];
  for (const warning of uniqueWarnings) {
    if (workshopWarningReporter) {
      workshopWarningReporter(warning);
    } else {
      console.error('Workshop warning:', warning);
    }
  }
}

function normalizeText(value: unknown): string {
  return String(value || '').trim();
}

function isPlaceholderAuthorName(name: unknown, ids: string[]): boolean {
  const normalized = normalizeText(name);
  if (!normalized) return true;
  const lowered = normalized.toLowerCase();
  return ids.some((id) => lowered === normalizeText(id).toLowerCase());
}

function toNumericOrUndefined(value: unknown): number | undefined {
  const normalized = normalizeText(value);
  if (!normalized) return undefined;
  const parsed = Number(normalized);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function clampStars(value: number): number {
  return Math.max(0, Math.min(5, value));
}

function mapSteamScoreToStars(detail: any): number {
  const explicitStars = toNumericOrUndefined(detail.star_rating ?? detail.ratingStars);
  if (explicitStars !== undefined) {
    return clampStars(explicitStars);
  }

  const score = toNumericOrUndefined(detail.score);
  if (score === undefined) {
    return 0;
  }

  if (score <= 1) {
    return clampStars(Math.round(score * 5));
  }

  return clampStars(Math.round(score));
}

function needsHtmlAuthorEnrichment(item: WorkshopItem): boolean {
  return !normalizeText(item.authorName) || !normalizeText(item.authorUrl);
}

function enrichWorkshopItems(primary: WorkshopItem[], fallback: WorkshopItem[]): WorkshopItem[] {
  const fallbackMap = new Map(fallback.map((item) => [item.workshopId, item]));

  return primary.map((item) => {
    const fallbackItem = fallbackMap.get(item.workshopId);
    if (!fallbackItem) {
      return item;
    }

    return {
      ...item,
      authorName: normalizeText(item.authorName) || fallbackItem.authorName,
      authorId: item.authorId || fallbackItem.authorId,
      authorUrl: normalizeText(item.authorUrl) || fallbackItem.authorUrl,
      authorSteamId: item.authorSteamId || fallbackItem.authorSteamId,
      authorVanityId: item.authorVanityId || fallbackItem.authorVanityId,
      authorAccountId: item.authorAccountId || fallbackItem.authorAccountId,
      ownerSteamId: item.ownerSteamId || fallbackItem.ownerSteamId,
      ownerAccountId: item.ownerAccountId || fallbackItem.ownerAccountId,
      stars: item.stars > 0 ? item.stars : fallbackItem.stars,
      imagePath: item.imagePath || fallbackItem.imagePath,
      shortDescription: item.shortDescription || fallbackItem.shortDescription,
    };
  });
}

function enrichHomepageSections(primary: HomepageSection[], fallback: HomepageSection[]) {
  const primaryMap = new Map(primary.map((section) => [section.id, section]));
  const merged: HomepageSection[] = [];

  for (const section of fallback) {
    const primarySection = primaryMap.get(section.id);
    if (primarySection) {
      merged.push({
        ...primarySection,
        items: enrichWorkshopItems(primarySection.items, section.items),
      });
      primaryMap.delete(section.id);
      continue;
    }
    merged.push(section);
  }

  for (const section of primaryMap.values()) {
    merged.push(section);
  }

  return merged;
}

export async function getWorkshopCapabilities(): Promise<WorkshopCapabilities> {
  capabilitiesPromise ??= invoke<WorkshopCapabilities>('get_workshop_capabilities').catch((err) => {
    capabilitiesPromise = null;
    throw err;
  });
  return capabilitiesPromise;
}

export async function fetchWorkshopHome() {
  const capabilities = shouldUseSteamworksSdk()
    ? await getWorkshopCapabilities().catch(() => null)
    : null;
  let sdkSections: HomepageSection[] = [];
  let source = 'web-fallback';

  if (capabilities?.canQueryHome) {
    try {
      const data = await invoke<WorkshopHomeResponse>('query_workshop_home');
      reportWorkshopWarnings(data.warnings);
      sdkSections = data.sections.map((section) => ({
        id: section.id,
        title: section.titleKey,
        subtitle: section.subtitleKey,
        icon: section.icon,
        items: rememberWorkshopItems(
          section.items.map((item) => mapSteamDetailToWorkshopItem(item, data.source)),
        ),
        browseParams: section.browseParams,
      }));
      source = data.source;
    } catch (err) {
      console.warn('Steam SDK home query failed, falling back to HTML:', err);
    }
  }

  if (sdkSections.length === 0) {
    const html = await invoke<string>('fetch_workshop_html', {
      url: 'https://steamcommunity.com/app/550/workshop/',
      source: 'workshop-home',
    });
    const htmlSections = parseHomepageSections(html).map((section) => ({
      ...section,
      items: rememberWorkshopItems(section.items),
    }));
    const tagCategories = parseTagCategories(html);
    return {
      source: 'web-fallback',
      sections: htmlSections,
      tagCategories,
    };
  }

  let tagCategories = [];
  let htmlSections: HomepageSection[] = [];
  try {
    const html = await invoke<string>('fetch_workshop_html', {
      url: 'https://steamcommunity.com/app/550/workshop/',
      source: 'workshop-home',
    });
    htmlSections = parseHomepageSections(html).map((section) => ({
      ...section,
      items: rememberWorkshopItems(section.items),
    }));
    tagCategories = parseTagCategories(html);
  } catch (err) {
    console.warn('Workshop home HTML enrichment failed:', err);
  }

  return {
    source: htmlSections.length > 0 && source === 'steam-sdk' ? 'hybrid' : source,
    sections: enrichHomepageSections(sdkSections, htmlSections),
    tagCategories,
  };
}

export async function fetchWorkshopItems(input: FetchWorkshopItemsInput) {
  const capabilities = shouldUseSteamworksSdk()
    ? await getWorkshopCapabilities().catch(() => null)
    : null;
  const resolvedSort = resolveBrowseSort(input);
  if (capabilities?.canQueryItems) {
    const creatorId = input.creatorId?.trim();
    const creatorNumeric = !creatorId || /^\d+$/.test(creatorId);
    if (creatorNumeric) {
      try {
        const data = await invoke<WorkshopItemsResponse>('query_workshop_items', {
          query: {
            query: input.query || undefined,
            sort: resolvedSort,
            section: input.section || undefined,
            page: input.page || 1,
            creatorId: creatorId || undefined,
            activeTag: input.activeTag || undefined,
            activeTagName: input.activeTagName || undefined,
          },
        });
        reportWorkshopWarnings(data.warnings);
        let items = data.items.map((item) => mapSteamDetailToWorkshopItem(item, data.source));
        if (items.some(needsHtmlAuthorEnrichment)) {
          try {
            const html: string = await invoke('fetch_workshop_html', {
              url: buildBrowseUrl(input),
              source: input.creatorId ? 'workshop-creator' : input.query ? 'workshop-search' : 'workshop-browse',
            });
            items = enrichWorkshopItems(items, parseSSRItems(html, 'workshop_query'));
          } catch (err) {
            console.warn('Steam SDK browse HTML enrichment failed:', err);
          }
        }
        return {
          source: data.source,
          items: rememberWorkshopItems(items),
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
    items: rememberWorkshopItems(parseSSRItems(html, 'workshop_query')),
  };
}

export async function fetchWorkshopItem(workshopId: string) {
  const capabilities = shouldUseSteamworksSdk()
    ? await getWorkshopCapabilities().catch(() => null)
    : null;
  if (capabilities?.canQueryItems) {
    try {
      const data = await invoke<WorkshopItemResponse>('query_workshop_item', { workshopId });
      reportWorkshopWarnings(data.warnings);
      return {
        source: data.source,
        item: rememberWorkshopItems([mapSteamDetailToWorkshopItem(data.item, data.source)])[0],
      };
    } catch (err) {
      console.warn('Steam SDK item query failed, falling back to existing collection command:', err);
    }
  }

  const data: any = await invoke('fetch_collection', { collectionId: workshopId });
  return {
    source: 'web-fallback',
    item: rememberWorkshopItems([mapSteamDetailToWorkshopItem(data.collection, 'web-fallback')])[0],
  };
}

export async function fetchWorkshopCollection(workshopId: string) {
  const capabilities = shouldUseSteamworksSdk()
    ? await getWorkshopCapabilities().catch(() => null)
    : null;
  if (capabilities?.canQueryItems) {
    try {
      const data = await invoke<WorkshopCollectionResponse>('query_workshop_collection', { workshopId });
      reportWorkshopWarnings(data.warnings);
      const resolvedCollection = rememberWorkshopItems([
        mapSteamDetailToWorkshopItem(data.collection, data.source),
      ])[0];
      return {
        source: data.source,
        collection: {
          ...data.collection,
          title: resolvedCollection.title,
          preview_url: resolvedCollection.imagePath,
          creator_name: resolvedCollection.authorName,
          creator: resolvedCollection.authorId,
          creator_steam_id: resolvedCollection.authorSteamId,
          creator_account_id: resolvedCollection.authorAccountId,
        },
        items: rememberWorkshopItems(
          data.items.map((item) => mapSteamDetailToWorkshopItem(item, data.source)),
        ),
      };
    } catch (err) {
      console.warn('Steam SDK collection query failed, falling back to existing collection command:', err);
    }
  }

  const data: any = await invoke('fetch_collection', { collectionId: workshopId });
  const resolvedCollection = rememberWorkshopItems([
    mapSteamDetailToWorkshopItem(data.collection, 'web-fallback'),
  ])[0];
  return {
    source: 'web-fallback',
    collection: {
      ...data.collection,
      title: resolvedCollection.title,
      preview_url: resolvedCollection.imagePath,
      creator_name: resolvedCollection.authorName,
      creator: resolvedCollection.authorId,
      creator_steam_id: resolvedCollection.authorSteamId,
      creator_account_id: resolvedCollection.authorAccountId,
    },
    items: rememberWorkshopItems(
      (data.items || []).map((item: any) => mapSteamDetailToWorkshopItem(item, 'web-fallback')),
    ),
  };
}

export async function fetchWorkshopHtml(url: string, source: string) {
  return invoke<string>('fetch_workshop_html', { url, source });
}

export async function fetchWorkshopPageDetails(workshopId: string, source: string) {
  const url = `https://steamcommunity.com/sharedfiles/filedetails/?id=${workshopId}`;
  const html = await fetchWorkshopHtml(url, source);
  const details = parseWorkshopPageDetails(html);
  rememberWorkshopPageDetails(details);
  return details;
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
    const resolvedSort = resolveBrowseSort(input) || 'trend';
    if (input.query) {
      url += `&searchtext=${encodeURIComponent(input.query)}`;
    }
    url += `&browsesort=${resolvedSort}&section=${input.section || 'readytouseitems'}&p=${input.page || 1}`;
    if (input.activeTag) {
      url += `&requiredtags[]=${encodeURIComponent(input.activeTagName || input.activeTag)}`;
    }
  }
  return url;
}

export function mapSteamDetailToWorkshopItem(detail: any, source = 'steam-sdk'): WorkshopItem {
  const ownerSteamId = normalizeText(
    detail.creator_steam_id ||
    detail.creatorSteamId ||
    detail.owner_steam_id ||
    detail.ownerSteamId ||
    (detail.creator && /^\d{17,20}$/.test(String(detail.creator)) ? detail.creator : '') ||
    '',
  );
  const ownerAccountId = normalizeText(
    detail.creator_account_id ||
    detail.creatorAccountId ||
    detail.owner_account_id ||
    detail.ownerAccountId ||
    '',
  );
  const authorSteamId = ownerSteamId || undefined;
  const authorAccountId = ownerAccountId || undefined;
  const authorId = ownerSteamId || ownerAccountId || normalizeText(detail.creator);
  const rawAuthorName = normalizeText(detail.creator_name || detail.creatorName || detail.authorName);
  const authorName = isPlaceholderAuthorName(rawAuthorName, [ownerSteamId, ownerAccountId, normalizeText(detail.creator)])
    ? ''
    : rawAuthorName;
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
    stars: mapSteamScoreToStars(detail),
    shortDescription: detail.short_description || detail.shortDescription || detail.description || '',
    fileSize: detail.file_size
      ? `${(parseInt(String(detail.file_size), 10) / 1024 / 1024).toFixed(1)} MB`
      : detail.fileSizeDisplay,
    tags: Array.isArray(detail.tags)
      ? detail.tags.map((tag: any) => String(tag.display_name || tag.tag || tag.name || '')).filter(Boolean)
      : [],
    subscriptions: detail.subscriptions ? parseInt(String(detail.subscriptions), 10) : undefined,
    favorites: (detail.favorites ?? detail.favorited) ? parseInt(String(detail.favorites ?? detail.favorited), 10) : undefined,
    lifetimeSubscriptions: detail.lifetime_subscriptions ? parseInt(String(detail.lifetime_subscriptions), 10) : undefined,
    lifetimeFavorites: (detail.lifetime_favorites ?? detail.lifetime_favorited)
      ? parseInt(String(detail.lifetime_favorites ?? detail.lifetime_favorited), 10)
      : undefined,
    views: detail.views ? parseInt(String(detail.views), 10) : undefined,
    comments: (detail.comments ?? detail.num_comments_public)
      ? parseInt(String(detail.comments ?? detail.num_comments_public), 10)
      : undefined,
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
