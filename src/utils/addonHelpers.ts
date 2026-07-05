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

// Category mappings from keys
export function getAddonCategories(addon: Addon): string[] {
  const categories = new Set<string>();
  const info = addon.addonInfo || {};
  
  if (info.addonContent_Campaign === '1' || info.addonContent_Campaign === 1) categories.add('Campaign');
  if (info.addonContent_Map === '1' || info.addonContent_Map === 1) categories.add('Map');
  if (info.addonContent_Survivor === '1' || info.addonContent_Survivor === 1) categories.add('Survivor');
  if (
    info.addonContent_WeaponModel === '1' || info.addonContent_WeaponModel === 1 ||
    info.Content_WeaponModel === '1' || info.Content_WeaponModel === 1 ||
    info.Content_weapon === '1' || info.Content_weapon === 1
  ) {
    categories.add('Weapon Model');
  }
  if (info.addonContent_Skin === '1' || info.addonContent_Skin === 1) categories.add('Skin');
  if (info.addonContent_Script === '1' || info.addonContent_Script === 1) categories.add('Script');
  if (
    info.addonContent_Music === '1' || info.addonContent_Music === 1 ||
    info.addonContent_Sound === '1' || info.addonContent_Sound === 1
  ) {
    categories.add('Sound/Music');
  }
  if (
    info.addonContent_BossInfected === '1' || info.addonContent_BossInfected === 1 ||
    info.addonContent_CommonInfected === '1' || info.addonContent_CommonInfected === 1
  ) {
    categories.add('Infected');
  }
  if (
    info.addonContent_UI === '1' || info.addonContent_UI === 1 ||
    info.addonContent_Spray === '1' || info.addonContent_Spray === 1 ||
    info.addonContent_BackgroundMovie === '1' || info.addonContent_BackgroundMovie === 1
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
  
  if (categories.size === 0) {
    categories.add('Other');
  }
  
  return Array.from(categories);
}

export const getImageUrl = (path?: string): string => {
  if (!path) return '';
  if (path.startsWith('http://') || path.startsWith('https://')) {
    return path;
  }
  if (path.startsWith('/cache/')) {
    const filename = path.slice(7);
    const isApple = navigator.userAgent.includes('Mac OS X') || navigator.userAgent.includes('iPhone') || navigator.userAgent.includes('iPad');
    if (!isApple) {
      return `http://cache.localhost/${filename}`;
    }
    return `cache://localhost/${filename}`;
  }
  return path;
};

export const getAddonUrl = (addon: Addon): string | null => {
  if (!addon || !addon.addonInfo) return null;
  const info = addon.addonInfo;
  let url = info.addonurl0 || info.addonURL0 || info.addonurl || info.addonURL || info.addonUrl0 || info.addonUrl;
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
  const info = addon.addonInfo || {};
  const author = info.addonauthor || info.addonAuthor || info.author;
  if (author && typeof author === 'string' && author.trim()) {
    return author.trim();
  }
  if (addon.steamDetails?.creator_name) {
    return addon.steamDetails.creator_name;
  }
  if (addon.steamDetails?.creator && addon.steamDetails.creator !== '0') {
    return addon.steamDetails.creator;
  }
  return 'Unknown Author';
};

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
    Object.keys(addons).some(key => key.toLowerCase() === finalName.toLowerCase() && key !== addon.vpkName)
  ) {
    const dotIndex = baseSuggestedName.lastIndexOf('.vpk');
    const nameWithoutExt = baseSuggestedName.slice(0, dotIndex);
    finalName = `${nameWithoutExt}_${counter}.vpk`;
    counter++;
  }

  return finalName;
}

