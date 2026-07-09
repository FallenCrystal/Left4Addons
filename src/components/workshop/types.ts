/** Shared types for the Workshop browser */

import type { DatabasePayload } from '../../types/addon';

export interface WorkshopItem {
  workshopId: string;
  title: string;
  imagePath: string;
  authorName: string;
  authorId: string;
  authorUrl: string;
  authorSteamId?: string;
  authorVanityId?: string;
  authorAccountId?: string;
  ownerSteamId?: string;
  ownerAccountId?: string;
  stars: number;
  shortDescription?: string;
  fileSize?: string;
  tags?: string[];
  subscriptions?: number;
  favorites?: number;
  lifetimeSubscriptions?: number;
  lifetimeFavorites?: number;
  views?: number;
  comments?: number;
  totalVotes?: number;
  timeCreated?: number;
  timeUpdated?: number;
  childCount?: number;
  previewCount?: number;
  childItemIds?: string[];
  galleryPreviewUrls?: string[];
  source?: string;
  isSubscribed?: boolean;
  isInstalled?: boolean;
  installState?: string[];
}

export interface CollectionData {
  collection: any;
  items: any[];
}

/** Extra details scraped from the Steam Community workshop page */
export interface WorkshopPageDetails {
  fileType?: string;
  title?: string;
  previewUrl?: string;
  description?: string;
  descriptionHtml?: string;
  creatorName?: string;
  creatorProfileUrl?: string;
  creatorSteamId?: string;
  creatorVanityId?: string;
  creatorAccountId?: string;
  imageGallery: string[];
  tags: { category: string; name: string }[];
  requiredItems: { title: string; workshopId: string }[];
  collectionItems?: WorkshopItem[];
  childItemIds?: string[];
  parentCollections: { title: string; workshopId: string }[];
  fileSizeDisplay?: string;
  postedDateText?: string;
  updatedDateText?: string;
  changeNoteCount?: number;
  ratingStars?: number;
  ratingCount?: number;
  uniqueVisitors?: number;
  currentSubscribers?: number;
  currentFavorites?: number;
  backgroundImageUrl?: string;
}

/** A single homepage section (e.g. "一周内物品", "最热门") */
export interface HomepageSection {
  id: string;
  title: string;
  subtitle: string;
  icon: React.ReactNode;
  items: WorkshopItem[];
  browseParams: {
    sort: string;
    section: string;
    days?: number;
  };
}

/** Tag category from declared_tags_v5 */
export interface TagCategory {
  name: string;
  tags: { id: string; name: string; display_name: string }[];
}

export interface WorkshopBrowserProps {
  addons: Record<string, any>;
  knownUninstalledAddons: Record<string, any>;
  downloadProgress: Record<string, number>;
  onDownload: (workshopId: string, title?: string, imagePath?: string) => void;
  onOpenLink: (url: string) => void;
  onImportCollection: (name: string, itemIds: string[]) => void;
  onRecordSeenItems?: (items: WorkshopItem[], source?: string) => void;
  onDatabaseUpdate?: (data: DatabasePayload) => void;
  isSubmitting: boolean;
  groups?: import('../../types/addon').Group[];
  backgroundTasks: import('../../types/addon').BackgroundTask[];
  syncingSteam: boolean;
  onOpenTaskCenter: () => void;
  onWarning?: (message: string) => void;
}

export interface WorkshopCapabilities {
  bridgeAvailable: boolean;
  bridgeLoaded: boolean;
  bridgeInitialized: boolean;
  provider: string;
  bridgeVersion?: string;
  lastError?: string;
  currentUserSteamId?: string;
  currentUserAccountId?: string;
  canQueryItems: boolean;
  canQueryHome: boolean;
  canDownload: boolean;
  canEnumerateInstalled: boolean;
  canEnumerateSubscribed: boolean;
}
