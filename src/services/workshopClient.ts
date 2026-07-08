import { invoke } from '@tauri-apps/api/core';
import {
  HomepageSection,
  WorkshopCapabilities,
  WorkshopItem,
  WorkshopPageDetails,
} from '../components/workshop/types';
import type { WorkshopSourceSettings } from '../types/addon';
import {
  parseHomepageSections,
  parseSSRItems,
  parseTagCategories,
  parseWorkshopPageDetails,
} from '../components/workshop/ssrParser';
import {
  rememberWorkshopItems,
  rememberWorkshopPageDetails,
  resolveWorkshopItemAuthor,
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
let workshopSourceSettings: WorkshopSourceSettings = {
  preset: 'conservative',
  allowSteamworksSdk: true,
  allowSteamWebApi: true,
  allowSteamCommunityHtml: true,
  allowSdkHtmlHybrid: false,
  sourceOrder: ['steamworks-sdk', 'steam-web-api', 'steamcommunity-html'],
  cacheRetention: 'keep',
};

export function setSteamworksSdkDisabled(disabled: boolean) {
  steamworksSdkDisabled = disabled;
}

export function setWorkshopSourceSettings(settings: WorkshopSourceSettings | undefined) {
  workshopSourceSettings = {
    ...workshopSourceSettings,
    ...(settings || {}),
    sourceOrder: settings?.sourceOrder?.length ? settings.sourceOrder : workshopSourceSettings.sourceOrder,
    cacheRetention: 'keep',
  };
}

export function setWorkshopWarningReporter(reporter: ((message: string) => void) | null) {
  workshopWarningReporter = reporter;
}

function shouldUseSteamworksSdk() {
  return !steamworksSdkDisabled &&
    workshopSourceSettings.preset !== 'offline' &&
    workshopSourceSettings.allowSteamworksSdk;
}

function shouldUseSteamCommunityHtml(capabilities: WorkshopCapabilities | null) {
  if (workshopSourceSettings.preset === 'offline' || !workshopSourceSettings.allowSteamCommunityHtml) {
    return false;
  }
  const sdkAvailable = shouldUseSteamworksSdk() && !!(capabilities?.canQueryHome || capabilities?.canQueryItems);
  if (workshopSourceSettings.preset === 'hybrid' || workshopSourceSettings.allowSdkHtmlHybrid) {
    return true;
  }
  return !sdkAvailable;
}

function listCacheKey(kind: string, input: unknown) {
  return `l4a.workshop.${kind}.${JSON.stringify(input)}`;
}

function readListCache<T>(key: string): T | null {
  try {
    const raw = localStorage.getItem(key);
    return raw ? JSON.parse(raw) as T : null;
  } catch {
    return null;
  }
}

function writeListCache(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify({ savedAt: new Date().toISOString(), value }));
  } catch (err) {
    console.warn('Failed to save workshop list cache:', err);
  }
}

function readCachedValue<T>(key: string): T | null {
  const cached = readListCache<{ value: T }>(key);
  return cached?.value || null;
}

async function readWorkshopItemCache(): Promise<Record<string, any>> {
  try {
    return await invoke<Record<string, any>>('get_workshop_cache');
  } catch {
    return {};
  }
}

function cachedDetailToWorkshopItem(workshopId: string, detail: any): WorkshopItem {
  return mapSteamDetailToWorkshopItem({
    ...detail,
    publishedfileid: detail?.publishedfileid || detail?.workshopId || workshopId,
    preview_url: detail?.preview_url || detail?.previewUrl || detail?.imagePath,
    creator_name: detail?.creator_name || detail?.creatorName || detail?.authorName,
    creator: detail?.creator || detail?.creatorId || detail?.authorId,
    creator_steam_id: detail?.creator_steam_id || detail?.creatorSteamId,
    creator_account_id: detail?.creator_account_id || detail?.creatorAccountId,
  }, detail?.lastSeenSource || detail?.lastPageSource || 'cache');
}

function normalizeCachedTags(tags: any): { category: string; name: string }[] {
  if (!Array.isArray(tags)) return [];
  return tags
    .map((tag) => {
      if (typeof tag === 'string') {
        return { category: '', name: tag };
      }
      return {
        category: normalizeText(tag?.category),
        name: normalizeText(tag?.name || tag?.display_name || tag?.tag),
      };
    })
    .filter((tag) => tag.name);
}

function normalizeCachedRelations(value: any): { title: string; workshopId: string }[] {
  if (!Array.isArray(value)) return [];
  return value
    .map((item) => ({
      title: normalizeText(item?.title),
      workshopId: normalizeText(item?.workshopId || item?.publishedfileid),
    }))
    .filter((item) => item.workshopId);
}

function normalizeCachedWorkshopItems(value: any): WorkshopItem[] {
  if (!Array.isArray(value)) return [];
  return value
    .map((item) => {
      if (!item?.workshopId && !item?.publishedfileid) return null;
      return {
        workshopId: normalizeText(item.workshopId || item.publishedfileid),
        title: normalizeText(item.title),
        imagePath: normalizeText(item.imagePath || item.previewUrl || item.preview_url),
        authorName: normalizeText(item.authorName || item.creatorName || item.creator_name),
        authorId: normalizeText(item.authorId || item.creatorId || item.creator),
        authorUrl: normalizeText(item.authorUrl || item.creatorProfileUrl),
        authorSteamId: normalizeText(item.authorSteamId || item.creatorSteamId) || undefined,
        authorVanityId: normalizeText(item.authorVanityId || item.creatorVanityId) || undefined,
        authorAccountId: normalizeText(item.authorAccountId || item.creatorAccountId) || undefined,
        stars: Number(item.stars || item.ratingStars || 0),
        shortDescription: normalizeText(item.shortDescription || item.description) || undefined,
      };
    })
    .filter(Boolean) as WorkshopItem[];
}

function mapCachedDetailToPageDetails(detail: any): WorkshopPageDetails {
  const steamDetails = detail?.steamDetails || {};
  const gallery = detail?.imageGallery || detail?.galleryUrls || detail?.galleryPreviewUrls || [];
  return {
    title: normalizeText(detail?.title || steamDetails?.title) || undefined,
    previewUrl: normalizeText(detail?.previewUrl || detail?.imagePath || detail?.preview_url || steamDetails?.preview_url) || undefined,
    description: normalizeText(detail?.description || steamDetails?.description || detail?.shortDescription) || undefined,
    descriptionHtml: normalizeText(detail?.descriptionHtml) || undefined,
    creatorName: normalizeText(detail?.creatorName || detail?.authorName || detail?.creator_name || steamDetails?.creator_name) || undefined,
    creatorProfileUrl: normalizeText(detail?.creatorProfileUrl || detail?.authorUrl) || undefined,
    creatorSteamId: normalizeText(detail?.creatorSteamId || detail?.creator_steam_id || steamDetails?.creator) || undefined,
    creatorVanityId: normalizeText(detail?.creatorVanityId || steamDetails?.creator_vanity_id) || undefined,
    creatorAccountId: normalizeText(detail?.creatorAccountId || detail?.creator_account_id || steamDetails?.creator_account_id) || undefined,
    imageGallery: Array.isArray(gallery) ? gallery.map(String).filter(Boolean) : [],
    tags: normalizeCachedTags(detail?.pageTags || detail?.tags || steamDetails?.tags),
    requiredItems: normalizeCachedRelations(detail?.requiredItems),
    collectionItems: normalizeCachedWorkshopItems(detail?.collectionItems),
    childItemIds: Array.isArray(detail?.childItemIds) ? detail.childItemIds.map(String).filter(Boolean) : undefined,
    parentCollections: normalizeCachedRelations(detail?.parentCollections),
    fileSizeDisplay: normalizeText(detail?.fileSizeDisplay || detail?.fileSize || steamDetails?.file_size) || undefined,
    postedDateText: normalizeText(detail?.postedDateText) || undefined,
    updatedDateText: normalizeText(detail?.updatedDateText) || undefined,
    changeNoteCount: toNumericOrUndefined(detail?.changeNoteCount),
    ratingStars: toNumericOrUndefined(detail?.ratingStars),
    ratingCount: toNumericOrUndefined(detail?.ratingCount),
    uniqueVisitors: toNumericOrUndefined(detail?.uniqueVisitors),
    currentSubscribers: toNumericOrUndefined(detail?.currentSubscribers),
    currentFavorites: toNumericOrUndefined(detail?.currentFavorites),
    backgroundImageUrl: normalizeText(detail?.backgroundImageUrl) || undefined,
  };
}

function normalizePageDetails(details: Partial<WorkshopPageDetails> | null | undefined): WorkshopPageDetails {
  return {
    ...details,
    imageGallery: Array.isArray(details?.imageGallery) ? details.imageGallery.filter(Boolean) : [],
    tags: Array.isArray(details?.tags) ? details.tags.filter((tag) => normalizeText(tag?.name)) : [],
    requiredItems: Array.isArray(details?.requiredItems)
      ? details.requiredItems.filter((item) => normalizeText(item?.workshopId))
      : [],
    collectionItems: Array.isArray(details?.collectionItems)
      ? details.collectionItems.filter((item) => normalizeText(item?.workshopId))
      : [],
    childItemIds: Array.isArray(details?.childItemIds)
      ? details.childItemIds.map(String).filter(Boolean)
      : undefined,
    parentCollections: Array.isArray(details?.parentCollections)
      ? details.parentCollections.filter((item) => normalizeText(item?.workshopId))
      : [],
  };
}

function chooseDescription(current?: string, incoming?: string): string | undefined {
  const existing = normalizeText(current);
  const next = normalizeText(incoming);
  return existing || next || undefined;
}

function chooseArray<T>(current: T[] | undefined, incoming: T[] | undefined): T[] {
  const existing = Array.isArray(current) ? current : [];
  const next = Array.isArray(incoming) ? incoming : [];
  return next.length > 0 ? next : existing;
}

function chooseNumber(current: number | undefined, incoming: number | undefined): number | undefined {
  return incoming !== undefined ? incoming : current;
}

function resolvePageDetailsAuthor(details: WorkshopPageDetails): WorkshopPageDetails {
  const resolved = resolveWorkshopItemAuthor({
    workshopId: '',
    title: details.title || '',
    imagePath: details.previewUrl || '',
    authorName: details.creatorName || '',
    authorId: details.creatorSteamId || details.creatorVanityId || details.creatorAccountId || '',
    authorUrl: details.creatorProfileUrl || '',
    authorSteamId: details.creatorSteamId,
    authorVanityId: details.creatorVanityId,
    authorAccountId: details.creatorAccountId,
    stars: 0,
  });

  return {
    ...details,
    creatorName: resolved.authorName || details.creatorName,
    creatorProfileUrl: resolved.authorUrl || details.creatorProfileUrl,
    creatorSteamId: resolved.authorSteamId || details.creatorSteamId,
    creatorVanityId: resolved.authorVanityId || details.creatorVanityId,
    creatorAccountId: resolved.authorAccountId || details.creatorAccountId,
  };
}

export function mergeWorkshopPageDetails(
  current: Partial<WorkshopPageDetails> | null | undefined,
  incoming: Partial<WorkshopPageDetails> | null | undefined,
): WorkshopPageDetails {
  const existing = normalizePageDetails(current);
  const next = normalizePageDetails(incoming);
  const merged = resolvePageDetailsAuthor(normalizePageDetails({
    title: normalizeText(next.title) || existing.title,
    previewUrl: normalizeText(next.previewUrl) || existing.previewUrl,
    description: chooseDescription(existing.description, next.description),
    descriptionHtml: normalizeText(next.descriptionHtml) || existing.descriptionHtml,
    creatorName: normalizeText(next.creatorName) || existing.creatorName,
    creatorProfileUrl: normalizeText(next.creatorProfileUrl) || existing.creatorProfileUrl,
    creatorSteamId: normalizeText(next.creatorSteamId) || existing.creatorSteamId,
    creatorVanityId: normalizeText(next.creatorVanityId) || existing.creatorVanityId,
    creatorAccountId: normalizeText(next.creatorAccountId) || existing.creatorAccountId,
    imageGallery: chooseArray(existing.imageGallery, next.imageGallery),
    tags: chooseArray(existing.tags, next.tags),
    requiredItems: chooseArray(existing.requiredItems, next.requiredItems),
    collectionItems: chooseArray(existing.collectionItems, next.collectionItems),
    childItemIds: chooseArray(existing.childItemIds, next.childItemIds),
    parentCollections: chooseArray(existing.parentCollections, next.parentCollections),
    fileSizeDisplay: normalizeText(next.fileSizeDisplay) || existing.fileSizeDisplay,
    postedDateText: normalizeText(next.postedDateText) || existing.postedDateText,
    updatedDateText: normalizeText(next.updatedDateText) || existing.updatedDateText,
    changeNoteCount: chooseNumber(existing.changeNoteCount, next.changeNoteCount),
    ratingStars: chooseNumber(existing.ratingStars, next.ratingStars),
    ratingCount: chooseNumber(existing.ratingCount, next.ratingCount),
    uniqueVisitors: chooseNumber(existing.uniqueVisitors, next.uniqueVisitors),
    currentSubscribers: chooseNumber(existing.currentSubscribers, next.currentSubscribers),
    currentFavorites: chooseNumber(existing.currentFavorites, next.currentFavorites),
    backgroundImageUrl: normalizeText(next.backgroundImageUrl) || existing.backgroundImageUrl,
  }));
  rememberWorkshopPageDetails(merged);
  return merged;
}

function hasUsefulPageSnapshot(details: WorkshopPageDetails): boolean {
  return Boolean(
    details.title ||
    details.creatorName ||
    details.description ||
    details.imageGallery.length > 0 ||
    details.requiredItems.length > 0 ||
    (details.collectionItems?.length || 0) > 0 ||
    (details.childItemIds?.length || 0) > 0 ||
    details.parentCollections.length > 0 ||
    details.tags.length > 0,
  );
}

function cachedCollectionResponse(workshopId: string, collection: any, cache: Record<string, any>, source: string) {
  const resolvedCollection = cachedDetailToWorkshopItem(workshopId, collection);
  const childIds = collection.childItemIds || [];
  const cachedItems = collection.collectionItems || [];
  return {
    source,
    collection: {
      ...collection,
      publishedfileid: workshopId,
      title: resolvedCollection.title,
      preview_url: resolvedCollection.imagePath,
      creator_name: resolvedCollection.authorName,
      creator: resolvedCollection.authorId,
      creator_steam_id: resolvedCollection.authorSteamId,
      creator_account_id: resolvedCollection.authorAccountId,
    },
    items: rememberWorkshopItems(
      (cachedItems.length > 0
        ? cachedItems.map((item: any) => ({
          ...cachedDetailToWorkshopItem(item.workshopId || item.publishedfileid, item),
          ...item,
        }))
        : childIds.map((childId: string) => cache[childId] ? cachedDetailToWorkshopItem(childId, cache[childId]) : null))
        .filter(Boolean),
    ),
  };
}

async function enrichItemsFromSnapshot(items: WorkshopItem[]): Promise<WorkshopItem[]> {
  if (items.length === 0) return items;
  const cache = await readWorkshopItemCache();
  if (Object.keys(cache).length === 0) return items;
  const cachedItems = items
    .map((item) => cache[item.workshopId] ? cachedDetailToWorkshopItem(item.workshopId, cache[item.workshopId]) : null)
    .filter((item): item is WorkshopItem => Boolean(item));
  return enrichWorkshopItems(items, cachedItems);
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

function accountIdToSteamId(accountId: unknown): string {
  const normalized = normalizeText(accountId);
  if (!/^\d+$/.test(normalized)) return '';
  try {
    return (BigInt(normalized) + 76561197960265728n).toString();
  } catch {
    return '';
  }
}

function isPlaceholderAuthorName(name: unknown, ids: string[]): boolean {
  const normalized = normalizeText(name);
  if (!normalized) return true;
  const lowered = normalized.toLowerCase();
  if (/^\d+$/.test(normalized)) return true;
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
  return isPlaceholderAuthorName(item.authorName, [
    item.authorSteamId || '',
    item.authorAccountId || '',
    item.authorId || '',
    item.ownerSteamId || '',
    item.ownerAccountId || '',
  ]) || !normalizeText(item.authorUrl);
}

function enrichWorkshopItems(primary: WorkshopItem[], fallback: WorkshopItem[]): WorkshopItem[] {
  const fallbackMap = new Map(fallback.map((item) => [item.workshopId, item]));

  return primary.map((item) => {
    const fallbackItem = fallbackMap.get(item.workshopId);
    if (!fallbackItem) {
      return item;
    }

    const primaryAuthorIsPlaceholder = isPlaceholderAuthorName(item.authorName, [
      item.authorSteamId || '',
      item.authorAccountId || '',
      item.authorId || '',
      item.ownerSteamId || '',
      item.ownerAccountId || '',
    ]);
    const fallbackAuthorIsUsable = !isPlaceholderAuthorName(fallbackItem.authorName, [
      fallbackItem.authorSteamId || '',
      fallbackItem.authorAccountId || '',
      fallbackItem.authorId || '',
      fallbackItem.ownerSteamId || '',
      fallbackItem.ownerAccountId || '',
    ]);

    return {
      ...item,
      authorName: primaryAuthorIsPlaceholder && fallbackAuthorIsUsable
        ? fallbackItem.authorName
        : normalizeText(item.authorName) || fallbackItem.authorName,
      authorId: item.authorId || fallbackItem.authorId,
      authorUrl: normalizeText(item.authorUrl) || fallbackItem.authorUrl,
      authorSteamId: item.authorSteamId || fallbackItem.authorSteamId,
      authorVanityId: item.authorVanityId || fallbackItem.authorVanityId,
      authorAccountId: item.authorAccountId || fallbackItem.authorAccountId,
      ownerSteamId: item.ownerSteamId || fallbackItem.ownerSteamId,
      ownerAccountId: item.ownerAccountId || fallbackItem.ownerAccountId,
      stars: item.stars > 0 ? item.stars : fallbackItem.stars,
      imagePath: item.imagePath || fallbackItem.imagePath,
      shortDescription: chooseDescription(item.shortDescription, fallbackItem.shortDescription),
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
  const cacheKey = listCacheKey('home', { version: 1 });
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
      sdkSections = await Promise.all(sdkSections.map(async (section) => ({
        ...section,
        items: rememberWorkshopItems(await enrichItemsFromSnapshot(section.items)),
      })));
      source = data.source;
    } catch (err) {
      console.warn('Steam SDK home query failed, falling back to HTML:', err);
    }
  }

  const allowHtml = shouldUseSteamCommunityHtml(capabilities);

  if (sdkSections.length === 0) {
    if (!allowHtml) {
      const cached = readCachedValue<any>(cacheKey);
      if (cached) return { ...cached, source: 'cache' };
      return {
        source: 'cache',
        sections: [],
        tagCategories: [],
      };
    }
    const html = await invoke<string>('fetch_workshop_html', {
      url: 'https://steamcommunity.com/app/550/workshop/',
      source: 'workshop-home',
    });
    const htmlSections = await Promise.all(parseHomepageSections(html).map(async (section) => ({
      ...section,
      items: rememberWorkshopItems(await enrichItemsFromSnapshot(section.items)),
    })));
    const tagCategories = parseTagCategories(html);
    const result = {
      source: 'web-fallback',
      sections: htmlSections,
      tagCategories,
    };
    writeListCache(cacheKey, result);
    return result;
  }

  let tagCategories = [];
  let htmlSections: HomepageSection[] = [];
  if (allowHtml) try {
    const html = await invoke<string>('fetch_workshop_html', {
      url: 'https://steamcommunity.com/app/550/workshop/',
      source: 'workshop-home',
    });
    htmlSections = await Promise.all(parseHomepageSections(html).map(async (section) => ({
      ...section,
      items: rememberWorkshopItems(await enrichItemsFromSnapshot(section.items)),
    })));
    tagCategories = parseTagCategories(html);
  } catch (err) {
    console.warn('Workshop home HTML enrichment failed:', err);
  }

  const result = {
    source: htmlSections.length > 0 && source === 'steam-sdk' ? 'hybrid' : source,
    sections: enrichHomepageSections(sdkSections, htmlSections),
    tagCategories,
  };
  writeListCache(cacheKey, result);
  return result;
}

export async function fetchWorkshopItems(input: FetchWorkshopItemsInput) {
  const capabilities = shouldUseSteamworksSdk()
    ? await getWorkshopCapabilities().catch(() => null)
    : null;
  const resolvedSort = resolveBrowseSort(input);
  const cacheKey = listCacheKey('items', { ...input, sort: resolvedSort || '' });
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
        items = await enrichItemsFromSnapshot(items);
        if (items.some(needsHtmlAuthorEnrichment) && shouldUseSteamCommunityHtml(capabilities)) {
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
        const result = {
          source: data.source,
          items: rememberWorkshopItems(items),
        };
        writeListCache(cacheKey, result);
        return result;
      } catch (err) {
        console.warn('Steam SDK browse query failed, falling back to HTML:', err);
      }
    }
  }

  if (!shouldUseSteamCommunityHtml(capabilities)) {
    const cached = readCachedValue<any>(cacheKey);
    if (cached) return { ...cached, source: 'cache' };
    return { source: 'cache', items: [] };
  }

  const url = buildBrowseUrl(input);
  try {
    const html: string = await invoke('fetch_workshop_html', {
      url,
      source: input.creatorId ? 'workshop-creator' : input.query ? 'workshop-search' : 'workshop-browse',
    });
    const items = await enrichItemsFromSnapshot(parseSSRItems(html, 'workshop_query'));
    const result = {
      source: 'web-fallback',
      items: rememberWorkshopItems(items),
    };
    writeListCache(cacheKey, result);
    return result;
  } catch (err) {
    const cached = readCachedValue<any>(cacheKey);
    if (cached) return { ...cached, source: 'cache' };
    throw err;
  }
}

export async function fetchWorkshopItem(workshopId: string) {
  const cache = await readWorkshopItemCache();
  if (cache[workshopId]) {
    return {
      source: 'snapshot',
      item: rememberWorkshopItems([cachedDetailToWorkshopItem(workshopId, cache[workshopId])])[0],
    };
  }

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

  try {
    const data: any = await invoke('fetch_collection', { collectionId: workshopId });
    return {
      source: 'web-fallback',
      item: rememberWorkshopItems([mapSteamDetailToWorkshopItem(data.collection, 'web-fallback')])[0],
    };
  } catch (err) {
    if (cache[workshopId]) {
      return {
        source: 'cache',
        item: rememberWorkshopItems([cachedDetailToWorkshopItem(workshopId, cache[workshopId])])[0],
      };
    }
    throw err;
  }
}

export async function fetchWorkshopCollection(workshopId: string) {
  const cache = await readWorkshopItemCache();
  const cachedCollection = cache[workshopId];
  if (cachedCollection && (cachedCollection.childItemIds?.length || cachedCollection.collectionItems?.length)) {
    return cachedCollectionResponse(workshopId, cachedCollection, cache, 'snapshot');
  }

  const capabilities = shouldUseSteamworksSdk()
    ? await getWorkshopCapabilities().catch(() => null)
    : null;
  if (capabilities?.canQueryItems) {
    try {
      const data = await invoke<WorkshopCollectionResponse>('query_workshop_collection', { workshopId });
      reportWorkshopWarnings(data.warnings);
      if (!data?.collection?.publishedfileid) {
        throw new Error('Steam SDK returned empty collection details');
      }
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
          await enrichItemsFromSnapshot(data.items.map((item) => mapSteamDetailToWorkshopItem(item, data.source))),
        ),
      };
    } catch (err) {
      console.warn('Steam SDK collection query failed, falling back to existing collection command:', err);
    }
  }

  try {
    const data: any = await invoke('fetch_collection', { collectionId: workshopId });
    if (!data?.collection?.publishedfileid && cachedCollection) {
      return cachedCollectionResponse(workshopId, cachedCollection, cache, 'cache');
    }
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
        await enrichItemsFromSnapshot((data.items || []).map((item: any) => mapSteamDetailToWorkshopItem(item, 'web-fallback'))),
      ),
    };
  } catch (err) {
    const collection = cache[workshopId];
    if (collection) {
      return cachedCollectionResponse(workshopId, collection, cache, 'cache');
    }
    throw err;
  }
}

export async function fetchWorkshopHtml(url: string, source: string) {
  return invoke<string>('fetch_workshop_html', { url, source });
}

export async function fetchWorkshopPageDetails(workshopId: string, source: string) {
  const url = `https://steamcommunity.com/sharedfiles/filedetails/?id=${workshopId}`;
  const snapshot = await getWorkshopPageSnapshot(workshopId);
  try {
    const html = await fetchWorkshopHtml(url, source);
    const details = mergeWorkshopPageDetails(snapshot, parseWorkshopPageDetails(html));
    return details;
  } catch (err) {
    if (snapshot) return snapshot;
    throw err;
  }
}

export async function getWorkshopPageSnapshot(workshopId: string): Promise<WorkshopPageDetails | null> {
  const cache = await readWorkshopItemCache();
  const detail = cache[workshopId];
  if (!detail) return null;
  const snapshot = mergeWorkshopPageDetails(null, mapCachedDetailToPageDetails(detail));
  if (!hasUsefulPageSnapshot(snapshot)) return null;
  return snapshot;
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
    accountIdToSteamId(detail.creator_account_id || detail.creatorAccountId || detail.owner_account_id || detail.ownerAccountId) ||
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
