export interface AddonInfo {
  addonContent_Campaign?: string | number;
  addonContent_Map?: string | number;
  addonContent_Survivor?: string | number;
  addonContent_WeaponModel?: string | number;
  Content_WeaponModel?: string | number;
  Content_weapon?: string | number;
  addonContent_Skin?: string | number;
  addonContent_Script?: string | number;
  addonContent_Music?: string | number;
  addonContent_Sound?: string | number;
  addonContent_BossInfected?: string | number;
  addonContent_CommonInfected?: string | number;
  addonContent_UI?: string | number;
  addonContent_Spray?: string | number;
  addonContent_BackgroundMovie?: string | number;
  addonurl0?: string;
  addonURL0?: string;
  addonurl?: string;
  addonURL?: string;
  addonUrl0?: string;
  addonUrl?: string;
  addonauthor?: string;
  addonAuthor?: string;
  author?: string;
  addonauthorSteamID?: string;
  addonversion?: string;
  addontitle?: string;
  addondescription?: string;
  addonDescription?: string;
  addontagline?: string;
}

export interface SteamTag {
  tag: string;
}

export interface SteamDetails {
  title?: string;
  description?: string;
  creator_name?: string;
  creator?: string;
  tags?: SteamTag[];
  file_size?: string;
}

export interface WorkshopDetails {
  workshopId?: string;
  title?: string;
  previewUrl?: string;
  imagePath?: string;
  creatorName?: string;
  authorName?: string;
  creatorId?: string;
  authorId?: string;
  creatorSteamId?: string;
  creatorProfileUrl?: string;
  authorUrl?: string;
  shortDescription?: string;
  fileSizeDisplay?: string;
  tags?: string[];
  pageTags?: { category: string; name: string }[];
  imageGallery?: string[];
  galleryUrls?: string[];
  requiredItems?: { title: string; workshopId: string }[];
  parentCollections?: { title: string; workshopId: string }[];
  backgroundImageUrl?: string;
  lastSeenAt?: string;
  lastPageFetchedAt?: string;
}

export interface Addon {
  id: string;
  vpkName: string;
  dirType: 'workshop' | 'loading' | 'none';
  isEnabled: boolean;
  fileSize: number;
  filesCount: number;
  imagePath?: string;
  workshopId?: string;
  addonInfo?: AddonInfo;
  steamDetails?: SteamDetails;
  workshopDetails?: WorkshopDetails;
  isDummy?: boolean;
}

export interface BackgroundTask {
  id: string;
  kind: 'download' | 'workshop-crawl';
  status: 'queued' | 'running' | 'completed' | 'failed' | 'cancelled';
  source?: string;
  targetIds: string[];
  progress: number;
  error?: string;
  createdAt: string;
  startedAt?: string;
  finishedAt?: string;
}

export interface Group {
  id: string;
  name: string;
  addons: string[];
  tags?: string[];
  workshopCollectionId?: string;
  masterCollectionIds?: string[];
  source?: 'auto-group' | 'workshop-import' | 'manual' | 'user';
}

export interface MasterCollection {
  id: string;
  name: string;
  nameKey?: string; // i18n key for system collections
  groupIds: string[];
  isSystem: boolean;
  icon?: string;
}

export interface Settings {
  workshopDir: string;
  loadingDir: string;
  enableDummyBypass: boolean;
}

export interface DatabasePayload {
  settings: Settings;
  addons: Record<string, Addon>;
  groups: Group[];
  knownUninstalledAddons: Record<string, Addon>;
  masterCollections: MasterCollection[];
}

export interface Toast {
  id: string;
  message: string;
  type: 'success' | 'error';
}
