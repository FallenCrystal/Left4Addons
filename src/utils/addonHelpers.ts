import { Addon, RenameSettings } from '../types/addon';

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

export function cleanAuthorName(value: unknown): string {
  if (typeof value !== 'string') return '';
  let cleaned = value.trim();
  if (!cleaned) return '';

  return cleaned
    .replace(/^(by|von|par|de|por|от)\s+/i, '')
    .replace(/^(creado por|criado por|créé par)\s+/i, '')
    .replace(/^(创作者|創作者|作者|作成者|投稿者|제작자|작성자|Создатель)\s*[：:]\s*/i, '')
    .replace(/^由\s+/, '')
    .replace(/\s+發表$/, '')
    .replace(/\s+创作$/, '')
    .trim();
}

export function isPlaceholderAuthorName(value: unknown, identities: string[] = []): boolean {
  const rawName = typeof value === 'string' ? value.trim() : '';
  if (!rawName) return true;

  const cleaned = cleanAuthorName(rawName);
  if (!cleaned) return true;

  const normalized = cleaned.toLowerCase();
  if (
    /^\d+$/.test(cleaned) ||
    normalized === 'author_name' ||
    normalized === '[unknown]' ||
    normalized === 'unknown author' ||
    normalized === '未知作者' ||
    normalized === '未知'
  ) {
    return true;
  }

  return identities.some((identity) => {
    const normId = String(identity || '').trim().toLowerCase();
    return normId !== '' && (normalized === normId || rawName.toLowerCase() === normId);
  });
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
    return cleanAuthorName(author);
  }
  const workshopAuthor = addon.workshopDetails?.creatorName || addon.workshopDetails?.authorName;
  if (typeof workshopAuthor === 'string' && !isPlaceholderAuthorName(workshopAuthor)) {
    return cleanAuthorName(workshopAuthor);
  }
  const steamAuthor = addon.steamDetails?.creator_name;
  if (typeof steamAuthor === 'string' && !isPlaceholderAuthorName(steamAuthor)) {
    return cleanAuthorName(steamAuthor);
  }
  const steamCreator = addon.steamDetails?.creator;
  if (typeof steamCreator === 'string' && !isPlaceholderAuthorName(steamCreator)) {
    return cleanAuthorName(steamCreator);
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
/**
 * Helper to sanitize a VPK filename based on RenameSettings rules.
 */
export function sanitizeVpkName(
  name: string,
  renameSettings?: RenameSettings
): string {
  const settings = renameSettings || {
    enableWorkshopIdPrefix: true,
    enableGroupPrefix: true,
    cleanSpecialChars: false,
    invalidCharReplace: 'underscore' as const,
    maxFilenameLength: 0,
    enableTrim: true,
    enableRemoveDoubleSpaces: true,
  };

  // Strip extension
  let baseName = name;
  if (name.toLowerCase().endsWith('.vpk.disabled')) {
    baseName = name.slice(0, -13);
  } else if (name.toLowerCase().endsWith('.disabled')) {
    baseName = name.slice(0, -9);
  } else if (name.toLowerCase().endsWith('.vpk')) {
    baseName = name.slice(0, -4);
  }

  // Windows invalid chars
  let baseSuggestedName = baseName.replace(/[\\/:*?"<>|]/g, '_');

  // Clean special characters (only allow 0-9, [], a-z, A-Z, _, space) if cleanSpecialChars is true
  if (settings.cleanSpecialChars) {
    const replaceWith = settings.invalidCharReplace === 'space' 
      ? ' ' 
      : (settings.invalidCharReplace === 'empty' ? '' : '_');
    
    let nextStr = '';
    for (let i = 0; i < baseSuggestedName.length; i++) {
      const c = baseSuggestedName[i];
      if (/^[a-zA-Z0-9_\[\] ]$/.test(c)) {
        nextStr += c;
      } else {
        nextStr += replaceWith;
      }
    }
    baseSuggestedName = nextStr;
  }

  // Trim if enabled
  if (settings.enableTrim) {
    baseSuggestedName = baseSuggestedName.trim();
  }

  // Replace double spaces if enabled
  if (settings.enableRemoveDoubleSpaces) {
    while (baseSuggestedName.indexOf('  ') !== -1) {
      baseSuggestedName = baseSuggestedName.replace(/  /g, ' ');
    }
  }

  // Max filename length check
  if (settings.maxFilenameLength && settings.maxFilenameLength > 0) {
    const maxLen = settings.maxFilenameLength;
    const maxBaseLen = maxLen > 4 ? maxLen - 4 : 1;
    const chars = Array.from(baseSuggestedName);
    if (chars.length > maxBaseLen) {
      baseSuggestedName = chars.slice(0, maxBaseLen).join('');
    }
  }

  // Re-trim after truncation just in case
  if (settings.enableTrim) {
    baseSuggestedName = baseSuggestedName.trim();
  }

  return `${baseSuggestedName}.vpk`;
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
  addons: Record<string, Addon>,
  renameSettings?: RenameSettings
): string {
  const settings = renameSettings || {
    enableWorkshopIdPrefix: true,
    enableGroupPrefix: true,
    cleanSpecialChars: false,
    invalidCharReplace: 'underscore' as const,
    maxFilenameLength: 0,
    enableTrim: true,
    enableRemoveDoubleSpaces: true,
  };

  let cleanTitle = '';
  let prefix = '';

  if (addon.workshopId) {
    // Workshop item: use Steam title or addon title
    const steamTitle = addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName;
    cleanTitle = steamTitle;
    if (cleanTitle.toLowerCase().endsWith('.vpk')) {
      cleanTitle = cleanTitle.slice(0, -4);
    }
    cleanTitle = cleanTitle.replace(/^(?:\[[^\]]+\])*/g, '');

    if (settings.enableWorkshopIdPrefix) {
      prefix += `[${addon.workshopId}]`;
    }
    if (groupName && settings.enableGroupPrefix) {
      prefix += `[${groupName}]`;
    }
  } else {
    // Non-workshop item: use original VPK filename (stripped of old bracket prefixes)
    let origBase = addon.vpkName.replace(/\.vpk(\.disabled)?$/i, '');
    origBase = origBase.replace(/\.disabled$/i, '');
    // Strip leading bracket prefixes like [Group] or [OldGroup]
    origBase = origBase.replace(/^(?:\[[^\]]+\])*/g, '');
    
    if (groupName && settings.enableGroupPrefix) {
      prefix += `[${groupName}]`;
    }
    cleanTitle = origBase;
  }

  const rawSuggestedName = `${prefix}${cleanTitle}.vpk`;
  const baseSuggestedName = sanitizeVpkName(rawSuggestedName, settings).slice(0, -4);

  // De-duplicate if the name conflicts with other addons
  let counter = 1;
  let finalName = `${baseSuggestedName}.vpk`;
  while (
    Object.keys(addons).some(key => {
      const existingAddon = addons[key];
      return existingAddon.vpkName.toLowerCase() === finalName.toLowerCase() && existingAddon.id !== addon.id;
    })
  ) {
    const counterStr = `_${counter}`;
    let truncatedBase = baseSuggestedName;

    if (settings.maxFilenameLength && settings.maxFilenameLength > 0) {
      const maxLen = settings.maxFilenameLength;
      const maxBaseLen = maxLen > (counterStr.length + 4) ? maxLen - (counterStr.length + 4) : 1;
      const chars = Array.from(baseSuggestedName);
      if (chars.length > maxBaseLen) {
        truncatedBase = chars.slice(0, maxBaseLen).join('');
      }
    }

    finalName = `${truncatedBase}_${counter}.vpk`;
    counter++;
  }

  return finalName;
}
