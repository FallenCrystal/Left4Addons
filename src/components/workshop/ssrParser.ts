/** SSR parsing utilities for Steam Workshop pages */

import { formatBytes } from '../../utils/addonHelpers';
import { WorkshopItem, HomepageSection, TagCategory, WorkshopPageDetails } from './types';

interface AuthorInfo {
  authorName: string;
  authorUrl: string;
  stars: number;
  authorSteamId?: string;
  authorVanityId?: string;
  authorAccountId?: string;
}

function parseCount(text: string): number | undefined {
  const normalized = (text || '').replace(/,/g, '').trim();
  const match = normalized.match(/\d+/);
  if (!match) return undefined;
  const value = parseInt(match[0], 10);
  return Number.isFinite(value) ? value : undefined;
}

function parseProfileIdentifiers(url: string): {
  profileUrl: string;
  steamId?: string;
  vanityId?: string;
} {
  const trimmed = (url || '').trim();
  if (!trimmed) {
    return { profileUrl: '' };
  }

  const steamId = trimmed.match(/\/profiles\/(\d+)/)?.[1];
  const vanityId = trimmed.match(/\/id\/([^/?#]+)/)?.[1];
  return {
    profileUrl: trimmed,
    steamId,
    vanityId,
  };
}

function accountIdToSteamId(accountId?: string): string | undefined {
  const normalized = (accountId || '').trim();
  if (!/^\d+$/.test(normalized)) return undefined;
  try {
    return (BigInt(normalized) + 76561197960265728n).toString();
  } catch {
    return undefined;
  }
}

// ── Helper: find balanced {…} from a start position in HTML ───────────────────

function extractBalancedJson(html: string, startIdx: number): string | null {
  if (html[startIdx] !== '{') return null;
  let depth = 0;
  let inStr = false;
  let escaped = false;
  for (let i = startIdx; i < html.length; i++) {
    const ch = html[i];
    if (escaped) { escaped = false; continue; }
    if (ch === '\\') { escaped = true; continue; }
    if (ch === '"') { inStr = !inStr; continue; }
    if (inStr) continue;
    if (ch === '{') depth++;
    if (ch === '}') { depth--; if (depth === 0) return html.slice(startIdx, i + 1); }
  }
  return null;
}

// ── Helper: extract SSR renderContext & queryData ─────────────────────────────

function extractSSRQueryData(html: string): any | null {
  try {
    // Pattern 1: window.SSR.renderContext=JSON.parse("…");
    // Steam Workshop uses this format. The JSON string contains escaped quotes.
    const jpPrefix = 'window.SSR.renderContext=JSON.parse("';
    const jpIdx = html.indexOf(jpPrefix);
    if (jpIdx !== -1) {
      const jsonStart = jpIdx + jpPrefix.length;
      // Find closing ");  — scan for unescaped " followed by );
      let jsonEnd = -1;
      let esc = false;
      for (let i = jsonStart; i < html.length; i++) {
        if (esc) { esc = false; continue; }
        if (html[i] === '\\') { esc = true; continue; }
        if (html[i] === '"' && html[i + 1] === ')' && html[i + 2] === ';') {
          jsonEnd = i;
          break;
        }
      }
      if (jsonEnd !== -1) {
        const rawJson = html.slice(jsonStart, jsonEnd);
        // Unescape JS string escapes: \" → ", \\ → \
        const rcJson = rawJson.replace(/\\"/g, '"').replace(/\\\\/g, '\\');
        const rc = JSON.parse(rcJson);
        const qd = rc?.queryData;
        return typeof qd === 'string' ? JSON.parse(qd) : qd;
      }
    }

    // Pattern 2: window.SSR.renderContext={…};
    const directPrefix = 'window.SSR.renderContext=';
    const directIdx = html.indexOf(directPrefix);
    if (directIdx !== -1) {
      const braceStart = directIdx + directPrefix.length;
      if (html[braceStart] === '{') {
        const jsonStr = extractBalancedJson(html, braceStart);
        if (jsonStr) {
          const rc = new Function(`return ${jsonStr}`)();
          const qd = rc?.queryData;
          return typeof qd === 'string' ? JSON.parse(qd) : qd;
        }
      }
    }

    return null;
  } catch {
    return null;
  }
}

// ── Helper: extract items from a single SSR query entry ───────────────────────

function extractItemsFromQuery(query: any, domMap: Map<string, AuthorInfo>): WorkshopItem[] {
  const items: WorkshopItem[] = [];
  const data = query?.state?.data;
  if (!data) return items;

  // Two data formats:
  //   Homepage (workshop_query): numeric keys → { "0": {publishedfileid, ...}, "1": ... }
  //   Browse   (workshop_browse): data.results → [{publishedfileid, ...}, ...]
  const rawItems: any[] = [];
  if (Array.isArray(data.results)) {
    rawItems.push(...data.results);
  } else {
    const keys = Object.keys(data).filter((k) => !isNaN(Number(k)));
    for (const k of keys) {
      if (data[k]) rawItems.push(data[k]);
    }
  }

  for (const item of rawItems) {
    if (!item?.publishedfileid) continue;
    const workshopId = item.publishedfileid;
    const domData = domMap.get(workshopId) || { authorName: '', authorUrl: '', stars: 0 };
    let authorId = domData.authorVanityId || domData.authorSteamId || domData.authorAccountId || '';
    let authorSteamId = domData.authorSteamId || '';
    let authorVanityId = domData.authorVanityId || '';
    let authorAccountId = domData.authorAccountId || '';
    // Prefer SSR creator_player_link_details for author info (browse page)
    let authorName = domData.authorName;
    let authorUrl = domData.authorUrl;
    if (item.creator) {
      authorSteamId = String(item.creator);
      if (!authorUrl) {
        authorUrl = `https://steamcommunity.com/profiles/${item.creator}`;
      }
      if (!authorId) {
        authorId = authorSteamId;
      }
    }
    // Check query-level creator_player_link_details
    const creatorDetails = query?.state?.data?.creator_player_link_details;
    if (creatorDetails && item.creator) {
      const cd = creatorDetails[item.creator];
      const publicData = cd?.public_data || cd;
      const privateData = cd?.private_data || {};
      if (!authorName && publicData?.persona_name) {
        authorName = String(publicData.persona_name);
      }
      if (!authorSteamId && publicData?.steamid) {
        authorSteamId = String(publicData.steamid);
      }
      if (!authorVanityId && publicData?.profile_url) {
        authorVanityId = String(publicData.profile_url);
      }
      if (!authorAccountId && privateData?.account_name) {
        authorAccountId = String(privateData.account_name);
      }
      if (authorVanityId) {
        authorUrl = `https://steamcommunity.com/id/${authorVanityId}`;
      } else if (authorSteamId) {
        authorUrl = `https://steamcommunity.com/profiles/${authorSteamId}`;
      }
    }
    items.push({
      workshopId,
      title: (item.title || '').trim(),
      imagePath: item.preview_url || '',
      authorName,
      authorId,
      authorUrl,
      authorSteamId: authorSteamId || undefined,
      authorVanityId: authorVanityId || undefined,
      authorAccountId: authorAccountId || undefined,
      stars: item.star_rating !== undefined ? item.star_rating : domData.stars,
      shortDescription: item.short_description || '',
      fileSize: item.file_size ? formatBytes(parseInt(item.file_size)) : undefined,
      tags: item.tags ? item.tags.map((t: any) => t.display_name || t.tag) : [],
      subscriptions: item.subscriptions,
      favorites: item.favorited,
      lifetimeSubscriptions: item.lifetime_subscriptions,
      lifetimeFavorites: item.lifetime_favorited,
      views: item.views,
      comments: item.num_comments_public,
      totalVotes: item.total_votes,
      timeCreated: item.time_created,
      timeUpdated: item.time_updated,
      childCount: item.num_children !== undefined ? parseInt(item.num_children) : undefined,
      previewCount: Array.isArray(item.previews) ? item.previews.length : undefined,
      childItemIds: Array.isArray(item.children)
        ? item.children.map((child: any) => String(child?.publishedfileid || child?.publishedfile_id || '')).filter(Boolean)
        : undefined,
      galleryPreviewUrls: Array.isArray(item.previews)
        ? item.previews.map((preview: any) => preview?.url || preview?.preview_url || '').filter(Boolean)
        : undefined,
    });
  }
  return items;
}

// ── Helper: parse DOM cards into a lookup map ─────────────────────────────────

function parseDOMCardMap(html: string): Map<string, AuthorInfo> {
  const map = new Map<string, AuthorInfo>();
  const parser = new DOMParser();
  const doc = parser.parseFromString(html, 'text/html');
  const cardXPath =
    "//div[contains(@class, ' Panel') and starts-with(./div/a/@href, 'https://steamcommunity.com/sharedfiles/filedetails/?id=')]";
  const snapshot = doc.evaluate(cardXPath, doc, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);

  for (let i = 0; i < snapshot.snapshotLength; i++) {
    const card = snapshot.snapshotItem(i) as HTMLElement;
    const linkVal = doc.evaluate(
      "(.//a[contains(@href, 'filedetails/?id=')])[1]/@href",
      card, null, XPathResult.STRING_TYPE, null,
    ).stringValue;
    const m = linkVal.match(/id=(\d+)/);
    if (!m) continue;
    const workshopId = m[1];
    const authorUrl = doc.evaluate(
      ".//a[contains(@href, 'myworkshopfiles')]/@href",
      card, null, XPathResult.STRING_TYPE, null,
    ).stringValue;
    let authorName = doc.evaluate(
      ".//a[contains(@href, 'myworkshopfiles')]/text()",
      card, null, XPathResult.STRING_TYPE, null,
    ).stringValue.trim();
    if (/^by\s+/i.test(authorName)) authorName = authorName.replace(/^by\s+/i, '');
    const { steamId, vanityId } = parseProfileIdentifiers(authorUrl);
    const stars = parseInt(
      doc.evaluate("count(.//svg[contains(@class, 'SVGIcon_Star_Filled')])",
        card, null, XPathResult.NUMBER_TYPE, null,
      ).numberValue.toString(),
    ) || 0;
    map.set(workshopId, {
      authorName,
      authorUrl,
      stars,
      authorSteamId: steamId,
      authorVanityId: vanityId,
    });
  }
  return map;
}

// ── Helper: parse workshop items from a single DOM card ───────────────────────

function parseDOMFallbackItems(html: string): WorkshopItem[] {
  const items: WorkshopItem[] = [];
  const parser = new DOMParser();
  const doc = parser.parseFromString(html, 'text/html');
  const cardXPath =
    "//div[contains(@class, ' Panel') and starts-with(./div/a/@href, 'https://steamcommunity.com/sharedfiles/filedetails/?id=')]";
  const snapshot = doc.evaluate(cardXPath, doc, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);

  for (let i = 0; i < snapshot.snapshotLength; i++) {
    const card = snapshot.snapshotItem(i) as HTMLElement;
    const linkVal = doc.evaluate(
      "(.//a[contains(@href, 'filedetails/?id=')])[1]/@href",
      card, null, XPathResult.STRING_TYPE, null,
    ).stringValue;
    const m = linkVal.match(/id=(\d+)/);
    if (!m) continue;
    const workshopId = m[1];
    let title = doc.evaluate(
      "(.//a[contains(@href, 'filedetails/?id=')])[2]",
      card, null, XPathResult.STRING_TYPE, null,
    ).stringValue.trim();
    if (!title) {
      title = doc.evaluate(
        "(.//a[contains(@href, 'filedetails/?id=')])[1]",
        card, null, XPathResult.STRING_TYPE, null,
      ).stringValue.trim();
    }
    const imagePath = doc.evaluate(".//img/@src", card, null, XPathResult.STRING_TYPE, null).stringValue;
    const authorUrl = doc.evaluate(
      ".//a[contains(@href, 'myworkshopfiles')]/@href",
      card, null, XPathResult.STRING_TYPE, null,
    ).stringValue;
    let authorName = doc.evaluate(
      ".//a[contains(@href, 'myworkshopfiles')]/text()",
      card, null, XPathResult.STRING_TYPE, null,
    ).stringValue.trim();
    if (/^by\s+/i.test(authorName)) authorName = authorName.replace(/^by\s+/i, '');
    const { steamId, vanityId } = parseProfileIdentifiers(authorUrl);
    const authorId = vanityId || steamId || '';
    const stars = parseInt(
      doc.evaluate("count(.//svg[contains(@class, 'SVGIcon_Star_Filled')])",
        card, null, XPathResult.NUMBER_TYPE, null,
      ).numberValue.toString(),
    ) || 0;
    items.push({
      workshopId,
      title: title || '',
      imagePath,
      authorName: authorName || '',
      authorId,
      authorUrl,
      authorSteamId: steamId,
      authorVanityId: vanityId,
      stars,
    });
  }
  return items;
}

// ── Main: parse SSR workshop items (for browse page) ─────────────────────────

export function parseSSRItems(
  html: string,
  queryKeyPrefix: string,
): WorkshopItem[] {
  const domMap = parseDOMCardMap(html);

  // Try SSR queryData — also try 'workshop_browse' as fallback
  const queryDataObj = extractSSRQueryData(html);
  if (queryDataObj?.queries) {
    for (const prefix of [queryKeyPrefix, 'workshop_browse', 'workshop_query']) {
      const matchingQueries = queryDataObj.queries.filter(
        (q: any) => q?.queryKey?.[0] === prefix,
      );
      if (matchingQueries.length > 0) {
        const items = extractItemsFromQuery(matchingQueries[0], domMap);
        if (items.length > 0) return items;
      }
    }
  }

  // Fallback to pure DOM
  return parseDOMFallbackItems(html);
}

/** Parse the homepage SSR to extract all homepage sections.
 *  Parses the SSR queryData once, then distributes items to each section. */
export function parseHomepageSections(html: string): HomepageSection[] {
  const sectionDefs: Array<{
    id: string;
    titleKey: string;
    subtitleKey: string;
    icon: string;
    queryIndex: number;
    browseParams: HomepageSection['browseParams'];
  }> = [
    {
      id: 'past-week',
      titleKey: 'workshop.home.pastWeek',
      subtitleKey: 'workshop.home.pastWeekDesc',
      icon: 'Clock',
      queryIndex: 0,
      browseParams: { sort: 'trend', section: 'readytouseitems', days: 7 },
    },
    {
      id: 'trending',
      titleKey: 'workshop.home.trending',
      subtitleKey: 'workshop.home.trendingDesc',
      icon: 'Flame',
      queryIndex: 1,
      browseParams: { sort: 'trend', section: 'readytouseitems', days: 90 },
    },
    {
      id: 'most-required',
      titleKey: 'workshop.home.mostRequired',
      subtitleKey: 'workshop.home.mostRequiredDesc',
      icon: 'Package',
      queryIndex: 4,
      browseParams: { sort: 'trend', section: 'readytouseitems' },
    },
    {
      id: 'most-subscribed',
      titleKey: 'workshop.home.mostSubscribed',
      subtitleKey: 'workshop.home.mostSubscribedDesc',
      icon: 'Users',
      queryIndex: 2,
      browseParams: { sort: 'totalprofiles', section: 'readytouseitems' },
    },
    {
      id: 'recently-updated',
      titleKey: 'workshop.home.recentlyUpdated',
      subtitleKey: 'workshop.home.recentlyUpdatedDesc',
      icon: 'RefreshCw',
      queryIndex: 3,
      browseParams: { sort: 'mostrecent', section: 'readytouseitems' },
    },
    {
      id: 'newest',
      titleKey: 'workshop.home.newest',
      subtitleKey: 'workshop.home.newestDesc',
      icon: 'Star',
      queryIndex: 5,
      browseParams: { sort: 'mostrecent', section: 'readytouseitems' },
    },
  ];

  // Parse SSR data once
  const domMap = parseDOMCardMap(html);
  const queryDataObj = extractSSRQueryData(html);

  // Collect all workshop_query entries from SSR
  const allQueries: any[] = [];
  if (queryDataObj?.queries) {
    for (const q of queryDataObj.queries) {
      if (q?.queryKey?.[0] === 'workshop_query') {
        allQueries.push(q);
      }
    }
  }

  // Distribute items to sections by queryIndex
  const sections: HomepageSection[] = [];
  for (const def of sectionDefs) {
    const query = allQueries[def.queryIndex];
    const items = query ? extractItemsFromQuery(query, domMap) : [];
    // Skip sections with no data
    if (items.length === 0) continue;
    sections.push({
      id: def.id,
      title: def.titleKey,
      subtitle: def.subtitleKey,
      icon: def.icon,
      items,
      browseParams: def.browseParams,
    });
  }

  return sections;
}

/** Parse declared_tags_v5 from homepage SSR to get the category tree */
export function parseTagCategories(html: string): TagCategory[] {
  try {
    const queryDataObj = extractSSRQueryData(html);
    if (!queryDataObj?.queries) return [];

    const tagsQuery = queryDataObj.queries.find(
      (q: any) => q?.queryKey?.[0] === 'declared_tags_v5',
    );
    if (!tagsQuery?.state?.data?.readytouse_tags) return [];

    return tagsQuery.state.data.readytouse_tags.map((cat: any) => ({
      name: cat.name,
      tags: cat.tags.map((t: any) => ({
        id: t.id,
        name: t.name,
        display_name: t.display_name,
      })),
    }));
  } catch {
    return [];
  }
}

/** Convert a Steam thumbnail URL to a full-size image URL by removing size/fit params */
function toFullSizeUrl(thumbUrl: string): string {
  if (!thumbUrl) return thumbUrl;
  // Remove imw/imh/ima/impolicy/imcolor/letterbox params to get the full-size image
  return thumbUrl.replace(/\?imw=.*$/, '');
}

function appendLineBreak(output: string[]): void {
  const last = output[output.length - 1] || '';
  if (!last.endsWith('\n')) {
    output.push('\n');
  }
}

function appendBlockBreak(output: string[]): void {
  const text = output.join('');
  if (!text.endsWith('\n\n')) {
    if (!text.endsWith('\n')) output.push('\n');
    output.push('\n');
  }
}

function descriptionNodeToText(node: Element): string {
  const blockTags = new Set(['DIV', 'P', 'UL', 'OL', 'TABLE', 'TR', 'H1', 'H2', 'H3', 'H4']);
  const output: string[] = [];

  const walk = (current: Node) => {
    if (current.nodeType === Node.TEXT_NODE) {
      output.push(current.textContent || '');
      return;
    }
    if (current.nodeType !== Node.ELEMENT_NODE) {
      return;
    }

    const element = current as Element;
    const tag = element.tagName;

    if (tag === 'BR') {
      appendLineBreak(output);
      return;
    }

    if (tag === 'LI') {
      appendLineBreak(output);
      output.push('* ');
    } else if (blockTags.has(tag) && output.join('').trim()) {
      appendBlockBreak(output);
    }

    element.childNodes.forEach(walk);

    if (tag === 'LI') {
      appendLineBreak(output);
    } else if (blockTags.has(tag)) {
      appendBlockBreak(output);
    }
  };

  node.childNodes.forEach(walk);

  return output
    .join('')
    .replace(/\u00a0/g, ' ')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n[ \t]+/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

/** Parse extra details from a Steam Workshop item/collection page HTML */
export function parseWorkshopPageDetails(html: string): WorkshopPageDetails {
  const result: WorkshopPageDetails = {
    imageGallery: [],
    tags: [],
    requiredItems: [],
    parentCollections: [],
  };

  try {
    const parser = new DOMParser();
    const doc = parser.parseFromString(html, 'text/html');

    const text = (selector: string) => doc.querySelector(selector)?.textContent?.trim() || '';
    const attr = (selector: string, name: string) => doc.querySelector(selector)?.getAttribute(name)?.trim() || '';
    const all = (selector: string) => Array.from(doc.querySelectorAll(selector));

    result.title = text('.workshopItemTitle');
    // Steam currently uses #previewImage. Keep the older selector for cached
    // or legacy page markup so a no-screenshot item still has its cover.
    result.previewUrl = toFullSizeUrl(attr('#previewImage, #previewImageMain', 'src'));

    const descriptionNode = doc.querySelector('.workshopItemDescription');
    if (descriptionNode) {
      result.description = descriptionNodeToText(descriptionNode);
      result.descriptionHtml = descriptionNode.innerHTML.trim();
    }

    result.creatorProfileUrl = attr('.friendBlockLinkOverlay', 'href');
    if (result.creatorProfileUrl) {
      const ids = parseProfileIdentifiers(result.creatorProfileUrl);
      result.creatorSteamId = ids.steamId;
      result.creatorVanityId = ids.vanityId;
    }
    const creatorNameNode = doc.querySelector('.friendBlockContent');
    if (creatorNameNode?.childNodes?.[0]?.textContent) {
      result.creatorName = creatorNameNode.childNodes[0].textContent.trim();
    }
    result.creatorAccountId = attr('.friendBlock', 'data-miniprofile') || undefined;
    result.creatorSteamId = result.creatorSteamId || accountIdToSteamId(result.creatorAccountId);

    const detailLabels = all('.detailsStatsContainerLeft .detailsStatLeft').map((el) => el.textContent?.trim().toLowerCase() || '');
    const detailValues = all('.detailsStatsContainerRight .detailsStatRight').map((el) => el.textContent?.trim() || '');
    detailLabels.forEach((label, index) => {
      const value = detailValues[index] || '';
      if (!value) return;
      if (label.includes('file size')) result.fileSizeDisplay = value;
      if (label.includes('posted')) result.postedDateText = value;
      if (label.includes('updated')) result.updatedDateText = value;
    });
    result.changeNoteCount = parseCount(text('.detailsStatNumChangeNotes'));
    result.ratingCount = parseCount(text('.numRatings'));
    if (doc.querySelector('.fileRatingDetails img')?.getAttribute('src')?.includes('5-star')) {
      result.ratingStars = 5;
    }

    all('.stats_table tr').forEach((row) => {
      const cells = Array.from(row.querySelectorAll('td')).map((td) => td.textContent?.trim() || '');
      if (cells.length < 2) return;
      const value = parseCount(cells[0]);
      const label = cells[1].toLowerCase();
      if (value === undefined) return;
      if (label.includes('unique visitor')) result.uniqueVisitors = value;
      if (label.includes('current subscriber')) result.currentSubscribers = value;
      if (label.includes('current favorite')) result.currentFavorites = value;
    });

    const fullGalleryMatch = html.match(/var\s+rgFullScreenshotURLs\s*=\s*(\[[\s\S]*?\]);/);
    if (fullGalleryMatch) {
      try {
        const normalized = fullGalleryMatch[1]
          .replace(/([{,]\s*)([A-Za-z_][A-Za-z0-9_]*)(\s*:)/g, '$1"$2"$3')
          .replace(/'/g, '"');
        const gallery = JSON.parse(normalized) as Array<{ url?: string }>;
        gallery.forEach((entry) => {
          if (entry?.url) {
            result.imageGallery.push(toFullSizeUrl(entry.url));
          }
        });
      } catch {
        // Fallback to DOM below.
      }
    }
    if (result.imageGallery.length === 0) {
      const galleryImgs = doc.evaluate(
        '//div[@id="highlight_strip_scroll"]/div[@role="button"]/img',
        doc, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null,
      );
      for (let i = 0; i < galleryImgs.snapshotLength; i++) {
        const img = galleryImgs.snapshotItem(i) as HTMLImageElement;
        const src = img.getAttribute('src') || '';
        if (src) {
          result.imageGallery.push(toFullSizeUrl(src));
        }
      }
    }

    all('.rightDetailsBlock .workshopTags').forEach((node) => {
      const category = (node.querySelector('.workshopTagsTitle')?.textContent || '').replace(/:$/, '').trim();
      node.querySelectorAll('a').forEach((a) => {
        const name = (a.textContent || '').trim();
        if (name) {
          result.tags.push({ category, name });
        }
      });
    });

    // 3. Required items
    const requiredContainer = doc.querySelector('.requiredItemsContainer');
    if (requiredContainer) {
      const links = requiredContainer.querySelectorAll('a');
      links.forEach((a) => {
        const title = (a.textContent || '').trim();
        const href = a.getAttribute('href') || '';
        const idMatch = href.match(/id=(\d+)/);
        if (title && idMatch) {
          result.requiredItems.push({ title, workshopId: idMatch[1] });
        }
      });
    }

    const collectionItems: WorkshopItem[] = [];
    all('.collectionChildren .collectionItem').forEach((node) => {
      const itemLink = node.querySelector('a[href*="filedetails/?id="]') as HTMLAnchorElement | null;
      const workshopId = itemLink?.href?.match(/id=(\d+)/)?.[1] || node.id?.match(/sharedfile_(\d+)/)?.[1] || '';
      if (!workshopId) return;

      const title = (node.querySelector('.workshopItemTitle')?.textContent || '').trim();
      const imagePath = toFullSizeUrl((node.querySelector('.workshopItemPreviewImage') as HTMLImageElement | null)?.src || '');
      const authorLink = node.querySelector('.workshopItemAuthorName a') as HTMLAnchorElement | null;
      const authorName = (authorLink?.textContent || '').trim();
      const authorUrl = authorLink?.href || '';
      const authorIds = parseProfileIdentifiers(authorUrl);
      const shortDescription = (node.querySelector('.workshopItemShortDesc')?.textContent || '').trim();
      const ratingSrc = (node.querySelector('.fileRating') as HTMLImageElement | null)?.src || '';
      const ratingStars = parseInt(ratingSrc.match(/(\d+)-star/)?.[1] || '0', 10) || 0;

      collectionItems.push({
        workshopId,
        title,
        imagePath,
        authorName,
        authorId: authorIds.vanityId || authorIds.steamId || '',
        authorUrl,
        authorSteamId: authorIds.steamId,
        authorVanityId: authorIds.vanityId,
        stars: ratingStars,
        shortDescription,
      });
    });
    if (collectionItems.length > 0) {
      result.collectionItems = collectionItems;
      result.childItemIds = collectionItems.map((item) => item.workshopId);
    }

    // 4. Parent collections
    const parentCollectionsDiv = doc.querySelector('.parentCollections');
    if (parentCollectionsDiv) {
      const collectionDivs = parentCollectionsDiv.querySelectorAll(':scope > div');
      collectionDivs.forEach((div) => {
        const title = (div.querySelector('.parentCollectionTitle')?.textContent || '').trim();
        const onclick = div.getAttribute('onclick') || '';
        const idMatch = onclick.match(/id=(\d+)/);
        if (title && idMatch) {
          result.parentCollections.push({ title, workshopId: idMatch[1] });
        }
      });
    }

    // 5. Collection background image (large hero image)
    const bgImg = doc.querySelector('img.collectionBackgroundImage') as HTMLImageElement | null;
    if (bgImg) {
      const src = bgImg.getAttribute('src') || '';
      if (src) {
        result.backgroundImageUrl = toFullSizeUrl(src);
      }
    }

    result.imageGallery = [...new Set(result.imageGallery.filter(Boolean))];
    result.tags = result.tags.filter((tag, index, tags) => (
      !!tag.name && tags.findIndex((candidate) => candidate.category === tag.category && candidate.name === tag.name) === index
    ));
  } catch (e) {
    console.error('Failed to parse workshop page details:', e);
  }

  return result;
}
