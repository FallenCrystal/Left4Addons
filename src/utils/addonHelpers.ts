import { Addon } from '../types/addon';

// Format bytes helper
export function formatBytes(bytes: number, decimals: number = 2): string {
  if (!bytes || bytes === 0) return '0 Bytes';
  const k = 1024;
  const dm = decimals < 0 ? 0 : decimals;
  const sizes = ['Bytes', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(dm)) + ' ' + sizes[i];
}

// Case-insensitive lookup helper for addonInfo
export function getAddonInfoValue(addon: Addon, key: string): any {
  if (!addon || !addon.addonInfo) return undefined;
  const info = addon.addonInfo;
  // Direct check first
  const val = info[key as keyof typeof info];
  if (val !== undefined) return val;
  // Case-insensitive check
  const lowerK = key.toLowerCase();
  const foundKey = Object.keys(info).find(k => k.toLowerCase() === lowerK);
  return foundKey ? info[foundKey as keyof typeof info] : undefined;
}

export function isPlaceholderAuthorName(value: unknown, identities: string[] = []): boolean {
  const name = typeof value === 'string' ? value.trim() : '';
  if (!name) return true;

  const normalized = name.toLowerCase();
  if (/^\d+$/.test(name) || normalized === 'author_name' || normalized === '[unknown]') {
    return true;
  }

  return identities.some((identity) => normalized === String(identity || '').trim().toLowerCase());
}

// Category mappings from keys
export function getAddonCategories(addon: Addon): string[] {
  const categories = new Set<string>();
  
  const checkKey = (k: string): boolean => {
    const val = getAddonInfoValue(addon, k);
    if (val === undefined || val === null) return false;
    const strVal = String(val).trim();
    return strVal === '1';
  };

  if (checkKey('addonContent_Campaign')) categories.add('Campaign');
  if (checkKey('addonContent_Map')) categories.add('Map');
  if (checkKey('addonContent_Survivor')) categories.add('Survivor');
  if (
    checkKey('addonContent_WeaponModel') ||
    checkKey('Content_WeaponModel') ||
    checkKey('Content_weapon')
  ) {
    categories.add('Weapon Model');
  }
  if (checkKey('addonContent_Skin')) categories.add('Skin');
  if (checkKey('addonContent_Script')) categories.add('Script');
  if (
    checkKey('addonContent_Music') ||
    checkKey('addonContent_Sound')
  ) {
    categories.add('Sound/Music');
  }
  if (
    checkKey('addonContent_BossInfected') ||
    checkKey('addonContent_CommonInfected')
  ) {
    categories.add('Infected');
  }
  if (
    checkKey('addonContent_UI') ||
    checkKey('addonContent_Spray') ||
    checkKey('addonContent_BackgroundMovie')
  ) {
    categories.add('UI/Textures');
  }
      
  if (addon.steamDetails?.tags && Array.isArray(addon.steamDetails.tags)) {
    addon.steamDetails.tags.forEach((t: any) => {
      const tagStr = typeof t === 'string' ? t : (t?.tag || '');
      if (typeof tagStr !== 'string' || !tagStr) return;
      const tag = tagStr.toLowerCase();
      if (tag.includes('campaign') || tag.includes('map')) categories.add('Campaign');
      if (tag.includes('survivor') || tag.includes('character')) categories.add('Survivor');
      if (tag.includes('weapon') || tag.includes('melee') || tag.includes('gun')) categories.add('Weapon Model');
      if (tag.includes('skin') || tag.includes('texture') || tag.includes('material')) categories.add('Skin');
      if (tag.includes('script') || tag.includes('mod')) categories.add('Script');
      if (tag.includes('sound') || tag.includes('music') || tag.includes('voice')) categories.add('Sound/Music');
      if (tag.includes('infected') || tag.includes('monster')) categories.add('Infected');
      if (tag.includes('ui') || tag.includes('hud') || tag.includes('icon')) categories.add('UI/Textures');
    });
  }

  const cachedTags = [
    ...(addon.workshopDetails?.tags || []),
    ...(addon.workshopDetails?.pageTags || []).map(t => t.name),
  ];
  cachedTags.forEach((tagStr) => {
    if (typeof tagStr !== 'string' || !tagStr) return;
    const tag = tagStr.toLowerCase();
    if (tag.includes('campaign') || tag.includes('map')) categories.add('Campaign');
    if (tag.includes('survivor') || tag.includes('character')) categories.add('Survivor');
    if (tag.includes('weapon') || tag.includes('melee') || tag.includes('gun')) categories.add('Weapon Model');
    if (tag.includes('skin') || tag.includes('texture') || tag.includes('material')) categories.add('Skin');
    if (tag.includes('script') || tag.includes('mod')) categories.add('Script');
    if (tag.includes('sound') || tag.includes('music') || tag.includes('voice')) categories.add('Sound/Music');
    if (tag.includes('infected') || tag.includes('monster')) categories.add('Infected');
    if (tag.includes('ui') || tag.includes('hud') || tag.includes('icon')) categories.add('UI/Textures');
  });
  
  // Don't show "Other" for uninstalled items with no metadata
  if (categories.size === 0 && addon.dirType !== 'none') {
    categories.add('Other');
  }

  return Array.from(categories);
}

export const getImageUrl = (path?: string): string => {
  if (!path) return '';
  if (path.startsWith('http://') || path.startsWith('https://')) {
    return path;
  }
  return path;
};

export const getAddonUrl = (addon: Addon): string | null => {
  if (!addon) return null;
  let url = getAddonInfoValue(addon, 'addonurl0') || getAddonInfoValue(addon, 'addonurl');
  if (url && typeof url === 'string') {
    url = url.trim();
    if (url.startsWith('http://') || url.startsWith('https://')) {
      return url;
    }
    if (url.includes('.') && url.length > 4) {
      return 'https://' + url;
    }
  }
  return null;
};

export const getAddonAuthor = (addon: Addon): string => {
  if (!addon) return 'Unknown Author';
  const author = getAddonInfoValue(addon, 'addonauthor') || getAddonInfoValue(addon, 'author');
  if (typeof author === 'string' && !isPlaceholderAuthorName(author)) {
    return author.trim();
  }
  const workshopAuthor = addon.workshopDetails?.creatorName || addon.workshopDetails?.authorName;
  if (typeof workshopAuthor === 'string' && !isPlaceholderAuthorName(workshopAuthor)) {
    return workshopAuthor.trim();
  }
  const steamAuthor = addon.steamDetails?.creator_name;
  if (typeof steamAuthor === 'string' && !isPlaceholderAuthorName(steamAuthor)) {
    return steamAuthor;
  }
  const steamCreator = addon.steamDetails?.creator;
  if (typeof steamCreator === 'string' && !isPlaceholderAuthorName(steamCreator)) {
    return steamCreator;
  }
  return 'Unknown Author';
};

export function sortAddonsDownloadedFirst(addons: Addon[]): Addon[] {
  const installed: Addon[] = [];
  const uninstalled: Addon[] = [];

  addons.forEach((addon) => {
    if (addon.dirType === 'none') {
      uninstalled.push(addon);
    } else {
      installed.push(addon);
    }
  });

  return [...installed, ...uninstalled];
}

/**
 * Helper to suggest a unique VPK name for an addon, avoiding name conflicts.
 * Workshop items use the Steam title / addonInfo title.
 * Non-workshop items keep their original filename (stripped of old bracket prefixes).
 * Both will prefix with group name if grouped, and workshop items prefix with workshop ID.
 * Appends a numeric counter if the name still conflicts with other addons.
 */
export function getSuggestedVpkName(
  addon: Addon,
  groupName: string | undefined,
  addons: Record<string, Addon>
): string {
  let cleanName = '';
  let prefix = '';

  if (addon.workshopId) {
    // Workshop item: use Steam title or addon title
    const steamTitle = addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName;
    let cleanTitle = steamTitle.replace(/[\\/:*?"<>|]/g, '_').trim();
    if (cleanTitle.endsWith('.vpk')) {
      cleanTitle = cleanTitle.slice(0, -4);
    }
    cleanTitle = cleanTitle.replace(/^(?:\[[^\]]+\])*/g, '').trim();
    
    prefix += `[${addon.workshopId}]`;
    if (groupName) {
      prefix += `[${groupName}]`;
    }
    cleanName = `${cleanTitle}.vpk`;
  } else {
    // Non-workshop item: use original VPK filename (stripped of old bracket prefixes)
    let origBase = addon.vpkName.replace(/\.vpk(\.disabled)?$/i, '');
    origBase = origBase.replace(/\.disabled$/i, '');
    // Strip leading bracket prefixes like [Group] or [OldGroup]
    origBase = origBase.replace(/^(?:\[[^\]]+\])*/g, '').trim();
    
    if (groupName) {
      prefix += `[${groupName}]`;
    }
    cleanName = `${origBase}.vpk`;
  }

  const baseSuggestedName = `${prefix}${cleanName}`;

  // De-duplicate if the name conflicts with other addons
  let counter = 1;
  let finalName = baseSuggestedName;
  while (
    Object.keys(addons).some(key => key.toLowerCase() === finalName.toLowerCase() && key !== addon.id)
  ) {
    const dotIndex = baseSuggestedName.lastIndexOf('.vpk');
    const nameWithoutExt = baseSuggestedName.slice(0, dotIndex);
    finalName = `${nameWithoutExt}_${counter}.vpk`;
    counter++;
  }

  return finalName;
}
