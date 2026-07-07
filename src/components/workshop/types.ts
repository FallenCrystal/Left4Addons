/** Shared types for the Workshop browser */

import type { DatabasePayload } from '../../types/addon';

export interface WorkshopItem {
  workshopId: string;
  title: string;
  imagePath: string;
  authorName: string;
  authorId: string;
  authorUrl: string;
  stars: number;
  shortDescription?: string;
  fileSize?: string;
  tags?: string[];
  subscriptions?: number;
  timeCreated?: number;
  timeUpdated?: number;
  childCount?: number;
}

export interface CollectionData {
  collection: any;
  items: any[];
}

/** Extra details scraped from the Steam Community workshop page */
export interface WorkshopPageDetails {
  imageGallery: string[];
  tags: { category: string; name: string }[];
  requiredItems: { title: string; workshopId: string }[];
  parentCollections: { title: string; workshopId: string }[];
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
  onDownload: (workshopId: string) => void;
  onOpenLink: (url: string) => void;
  onImportCollection: (name: string, itemIds: string[]) => void;
  onRecordSeenItems?: (items: WorkshopItem[], source?: string) => void;
  onDatabaseUpdate?: (data: DatabasePayload) => void;
  isSubmitting: boolean;
  groups?: import('../../types/addon').Group[];
}
