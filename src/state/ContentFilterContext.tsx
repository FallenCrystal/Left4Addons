import { createContext, useContext } from 'react';
import type {
  AddonId,
  ClientFilterConfig,
  FilterExclusionRecord,
  FilterItemVerdict,
  UpsertFilterExclusionRequest,
  WorkshopItemRecord,
} from '../api/ipc';
import type {
  FilterEvalPriority,
  PrioritizeFilterTargetsOptions,
} from './controller/useContentFilter';
import { createDefaultFilterConfig } from '../utils/contentFilter';

export interface ContentFilterContextValue {
  filterConfig: ClientFilterConfig;
  verdicts: Record<string, FilterItemVerdict>;
  evaluatedRevisions: Record<string, string>;
  exclusionsByAddonId: ReadonlyMap<string, FilterExclusionRecord>;
  revealedCardIds: ReadonlySet<string>;
  failedTargetKeys: ReadonlySet<string>;
  revealCard: (cardKey: string) => void;
  prioritizeFilterTargets: (
    targetKeys: string[],
    options?: PrioritizeFilterTargetsOptions,
  ) => void;
  registerWorkshopItems: (
    items: WorkshopItemRecord[],
    options?: { onlyIfMissing?: boolean; priority?: FilterEvalPriority },
  ) => void;
  upsertFilterExclusion: (request: UpsertFilterExclusionRequest) => Promise<boolean>;
  deleteFilterExclusion: (addonId: AddonId) => Promise<boolean>;
  onOpenFilterSettings?: () => void;
}

const DEFAULT_CONTEXT: ContentFilterContextValue = {
  filterConfig: createDefaultFilterConfig(),
  verdicts: {},
  evaluatedRevisions: {},
  exclusionsByAddonId: new Map(),
  revealedCardIds: new Set(),
  failedTargetKeys: new Set(),
  revealCard: () => {},
  prioritizeFilterTargets: () => {},
  registerWorkshopItems: () => {},
  upsertFilterExclusion: async () => false,
  deleteFilterExclusion: async () => false,
};

export const ContentFilterContext = createContext<ContentFilterContextValue>(DEFAULT_CONTEXT);

export function useContentFilterContext(): ContentFilterContextValue {
  return useContext(ContentFilterContext);
}
