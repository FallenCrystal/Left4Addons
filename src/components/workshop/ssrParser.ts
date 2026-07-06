/** SSR parsing utilities for Steam Workshop pages */

import { formatBytes } from '../../utils/addonHelpers';
import { WorkshopItem, HomepageSection, TagCategory, WorkshopPageDetails } from './types';

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

function extractItemsFromQuery(query: any, domMap: Map<string, { authorName: string; authorUrl: string; stars: number }>): WorkshopItem[] {
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
    let authorId = '';
    if (domData.authorUrl) {
      const idMatch = domData.authorUrl.match(/(?:profiles|id)\/([^\/]+)/);
      if (idMatch) authorId = idMatch[1];
    }
    // Prefer SSR creator_player_link_details for author info (browse page)
    let authorName = domData.authorName;
    let authorUrl = domData.authorUrl;
    if (!authorName && item.creator) {
      // creator is steamid64, build profile URL
      authorUrl = `https://steamcommunity.com/profiles/${item.creator}`;
    }
    // Check query-level creator_player_link_details
    const creatorDetails = query?.state?.data?.creator_player_link_details;
    if (!authorName && creatorDetails && item.creator) {
      const cd = creatorDetails[item.creator];
      if (cd?.persona_name) {
        authorName = cd.persona_name;
      }
    }
    items.push({
      workshopId,
      title: (item.title || '').trim(),
      imagePath: item.preview_url || '',
      authorName,
      authorId,
      authorUrl,
      stars: item.star_rating !== undefined ? item.star_rating : domData.stars,
      shortDescription: item.short_description || '',
      fileSize: item.file_size ? formatBytes(parseInt(item.file_size)) : undefined,
      tags: item.tags ? item.tags.map((t: any) => t.display_name || t.tag) : [],
      subscriptions: item.subscriptions,
      timeCreated: item.time_created,
      timeUpdated: item.time_updated,
      childCount: item.num_children !== undefined ? parseInt(item.num_children) : undefined,
    });
  }
  return items;
}

// ── Helper: parse DOM cards into a lookup map ─────────────────────────────────

function parseDOMCardMap(html: string): Map<string, { authorName: string; authorUrl: string; stars: number }> {
  const map = new Map<string, { authorName: string; authorUrl: string; stars: number }>();
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
    const stars = parseInt(
      doc.evaluate("count(.//svg[contains(@class, 'SVGIcon_Star_Filled')])",
        card, null, XPathResult.NUMBER_TYPE, null,
      ).numberValue.toString(),
    ) || 0;
    map.set(workshopId, { authorName, authorUrl, stars });
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
    let authorId = '';
    if (authorUrl) {
      const idMatch = authorUrl.match(/(?:profiles|id)\/([^\/]+)/);
      if (idMatch) authorId = idMatch[1];
    }
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

    // 1. Image gallery from highlight strip
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

    // 2. Tags (Game Content, Game Modes, etc.)
    const tagGroupXPaths = [
      '//div[contains(@data-panel, "PanelGroup") and starts-with(./span/text(), "Game Content")]',
      '//div[contains(@data-panel, "PanelGroup") and starts-with(./span/text(), "Game Modes")]',
    ];
    for (const xpath of tagGroupXPaths) {
      const nodes = doc.evaluate(xpath, doc, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
      for (let i = 0; i < nodes.snapshotLength; i++) {
        const node = nodes.snapshotItem(i) as HTMLElement;
        const span = node.querySelector('span');
        const category = (span?.textContent || '').replace(/:$/, '').trim();
        const links = node.querySelectorAll('a');
        links.forEach((a) => {
          const name = (a.textContent || '').trim();
          if (name) {
            result.tags.push({ category, name });
          }
        });
      }
    }

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
  } catch (e) {
    console.error('Failed to parse workshop page details:', e);
  }

  return result;
}
