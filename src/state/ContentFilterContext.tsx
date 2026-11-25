import { createContext, useContext } from 'react';
import type {
  AddonId,
  ClientFilterConfig,
  FilterExclusionRecord,
  FilterItemVerdict,
  UpsertFilterExclusionRequest,
  WorkshopItemRecord,
} from '../api/ipc';
import { createDefaultFilterConfig } from '../utils/contentFilter';

export interface ContentFilterContextValue {
  filterConfig: ClientFilterConfig;
  verdicts: Record<string, FilterItemVerdict>;
  exclusionsByAddonId: ReadonlyMap<string, FilterExclusionRecord>;
  revealedCardIds: ReadonlySet<string>;
  revealCard: (cardKey: string) => void;
  registerWorkshopItems: (items: WorkshopItemRecord[]) => void;
  upsertFilterExclusion: (request: UpsertFilterExclusionRequest) => Promise<boolean>;
  deleteFilterExclusion: (addonId: AddonId) => Promise<boolean>;
  onOpenFilterSettings?: () => void;
}

const DEFAULT_CONTEXT: ContentFilterContextValue = {
  filterConfig: createDefaultFilterConfig(),
  verdicts: {},
  exclusionsByAddonId: new Map(),
  revealedCardIds: new Set(),
  revealCard: () => {},
  registerWorkshopItems: () => {},
  upsertFilterExclusion: async () => false,
  deleteFilterExclusion: async () => false,
};

export const ContentFilterContext = createContext<ContentFilterContextValue>(DEFAULT_CONTEXT);

export function useContentFilterContext(): ContentFilterContextValue {
  return useContext(ContentFilterContext);
}
