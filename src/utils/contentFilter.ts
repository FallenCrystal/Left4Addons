import {
  DEFAULT_FILTER_REPO_URL,
  DEFAULT_LLM_ENDPOINT,
  DEFAULT_LLM_MODEL,
  DEFAULT_LLM_SYSTEM_PROMPT,
  DEFAULT_LLM_TOKEN_ENV_VAR,
  type AddonSummary,
  type ClientFilterConfig,
  type ExclusionDisplayLevel,
  type FilterEvaluationTarget,
  type FilterExclusionRecord,
  type FilterItemVerdict,
  type FilterModelParams,
  type FilterPresetId,
  type FilterProbabilities,
  type FilterScoreTarget,
  type WorkshopItemRecord,
} from '../api/ipc';

export type CoverFilterMode = 'none' | 'lazy' | 'blur' | 'hide';

export interface EffectiveModalities {
  imageSensitive: boolean;
  textSensitive: boolean;
  titleSensitive: boolean;
  descriptionSensitive: boolean;
  sensitiveImageIndices: number[];
}

export interface CardFilterState extends EffectiveModalities {
  scopeEnabled: boolean;
  flagged: boolean;
  hiddenCard: boolean;
  coverMode: CoverFilterMode;
  blurWholeCard: boolean;
  isBlurCardRevealed: boolean;
  redactTitle: boolean;
  redactDescription: boolean;
  redactMeta: boolean;
  showDiagnosticBadge: boolean;
  exclusionActive: boolean;
  exclusionDisplayLevel?: ExclusionDisplayLevel;
  exclusionInvalidated: boolean;
}

export interface DetailFilterState extends EffectiveModalities {
  scopeEnabled: boolean;
  flagged: boolean;
  coverMode: CoverFilterMode;
  mapCoverMode: CoverFilterMode;
  redactText: boolean;
  exclusionActive: boolean;
  exclusionDisplayLevel?: ExclusionDisplayLevel;
  exclusionInvalidated: boolean;
  isImageIndexSensitive: (index: number) => boolean;
  imageModeAt: (index: number) => CoverFilterMode;
}

export function createDefaultModelParams(): FilterModelParams {
  return {
    threads: 4,
    enableText: true,
    enableImage: true,
    maxImages: 2,
    galleryAggregation: 'max',
    goreWeight: 1.0,
    nsfwWeight: 1.0,
    temperature: 1.0,
    bias: 0.0,
  };
}

export function getPresetDefaultParams(preset: FilterPresetId): FilterModelParams {
  const base = createDefaultModelParams();
  switch (preset) {
    case 'strict':
      return {
        threads: 4,
        enableText: true,
        enableImage: true,
        maxImages: 4,
        galleryAggregation: 'max',
        goreWeight: 1.15,
        nsfwWeight: 1.15,
        temperature: 0.9,
        bias: 0.35,
      };
    case 'relaxed':
      return {
        threads: 4,
        enableText: true,
        enableImage: true,
        maxImages: 1,
        galleryAggregation: 'max',
        goreWeight: 0.9,
        nsfwWeight: 0.9,
        temperature: 1.15,
        bias: -0.3,
      };
    case 'nsfw_only':
      return {
        ...base,
        goreWeight: 0.0,
        nsfwWeight: 1.0,
      };
    case 'gore_only':
      return {
        ...base,
        goreWeight: 1.0,
        nsfwWeight: 0.0,
      };
    case 'low_cpu':
      return {
        ...base,
        threads: 2,
        maxImages: 1,
      };
    case 'recommended':
    case 'custom':
    default:
      return base;
  }
}

export function getPresetRecommendedThresholds(preset: FilterPresetId): {
  llmThreshold: number;
  flagThreshold: number;
} {
  switch (preset) {
    case 'strict':
      return { llmThreshold: 0.15, flagThreshold: 0.55 };
    case 'relaxed':
      return { llmThreshold: 0.3, flagThreshold: 0.82 };
    default:
      return { llmThreshold: 0.2, flagThreshold: 0.72 };
  }
}

export function createDefaultFilterConfig(): ClientFilterConfig {
  return {
    enabled: false,
    mode: 'local',
    customEndpointUrl: null,
    applyToWorkshop: true,
    applyToLibrary: true,
    repoUrl: DEFAULT_FILTER_REPO_URL,
    trackLatest: true,
    pinnedCommit: null,
    autoStartLocalService: true,
    preset: 'recommended',
    localParams: createDefaultModelParams(),
    scoreTarget: 'overall',
    enableDirectFlag: true,
    flagThreshold: 0.72,
    llmEnabled: false,
    llmThreshold: 0.2,
    llmEndpoint: DEFAULT_LLM_ENDPOINT,
    llmModel: DEFAULT_LLM_MODEL,
    llmSendImages: true,
    llmSystemPrompt: DEFAULT_LLM_SYSTEM_PROMPT,
    llmTokenSource: 'env_var',
    llmTokenEnvVar: DEFAULT_LLM_TOKEN_ENV_VAR,
    flaggedAction: 'blur_card',
    subdivideModalities: true,
    modalityThreshold: 0.3,
  };
}

function clampProb(p: number): number {
  if (!Number.isFinite(p)) return 1e-6;
  return Math.min(1 - 1e-6, Math.max(1e-6, p));
}

function toLogit(p: number): number {
  const c = clampProb(p);
  return Math.log(c / (1 - c));
}

function fromLogit(x: number): number {
  if (x >= 40) return 1;
  if (x <= -40) return 0;
  return 1 / (1 + Math.exp(-x));
}

function round6(v: number): number {
  if (!Number.isFinite(v)) return 0;
  const clamped = Math.min(1, Math.max(0, v));
  return Math.round(clamped * 1_000_000) / 1_000_000;
}

export function sanitizeModelParams(params: FilterModelParams): FilterModelParams {
  return {
    threads: Math.min(16, Math.max(1, Math.round(params.threads || 4))),
    enableText: Boolean(params.enableText),
    enableImage: Boolean(params.enableImage),
    maxImages: Math.min(8, Math.max(1, Math.round(params.maxImages || 2))),
    galleryAggregation: params.galleryAggregation ?? 'max',
    goreWeight: Number.isFinite(params.goreWeight)
      ? Math.min(3, Math.max(0, params.goreWeight))
      : 1,
    nsfwWeight: Number.isFinite(params.nsfwWeight)
      ? Math.min(3, Math.max(0, params.nsfwWeight))
      : 1,
    temperature: Number.isFinite(params.temperature)
      ? Math.min(4, Math.max(0.25, params.temperature))
      : 1,
    bias: Number.isFinite(params.bias) ? Math.min(5, Math.max(-5, params.bias)) : 0,
  };
}

export function isIdentityCalibration(params: FilterModelParams): boolean {
  const s = sanitizeModelParams(params);
  return (
    Math.abs(s.goreWeight - 1) < 1e-5 &&
    Math.abs(s.nsfwWeight - 1) < 1e-5 &&
    Math.abs(s.temperature - 1) < 1e-5 &&
    Math.abs(s.bias) < 1e-5
  );
}

export function adjustTriplet(
  params: FilterModelParams,
  ov: number,
  gore: number,
  nsfw: number,
): [number, number, number] {
  const s = sanitizeModelParams(params);
  if (isIdentityCalibration(s)) {
    return [round6(ov), round6(gore), round6(nsfw)];
  }
  const gw = s.goreWeight;
  const nw = s.nsfwWeight;
  const temp = s.temperature;
  const bias = s.bias;

  const adjSingle = (p: number, w: number): number => {
    if (w <= 1e-6) return 0;
    return fromLogit((toLogit(p) + Math.log(w)) / temp + bias);
  };

  const gAdj = adjSingle(gore, gw);
  const nAdj = adjSingle(nsfw, nw);

  let ovAdj: number;
  if (gw <= 1e-6 && nw <= 1e-6) {
    ovAdj = 0;
  } else if (gw <= 1e-6) {
    ovAdj = nAdj;
  } else if (nw <= 1e-6) {
    ovAdj = gAdj;
  } else if (Math.abs(gw - 1) < 1e-5 && Math.abs(nw - 1) < 1e-5) {
    ovAdj = fromLogit(toLogit(ov) / temp + bias);
  } else {
    const gW = fromLogit(toLogit(gore) + Math.log(gw));
    const nW = fromLogit(toLogit(nsfw) + Math.log(nw));
    const origOr = clampProb(1 - (1 - clampProb(gore)) * (1 - clampProb(nsfw)));
    const newOr = clampProb(1 - (1 - gW) * (1 - nW));
    const deltaScale = Math.min(gw, nw) / Math.max(gw, nw, 1);
    const deltaLogit = (toLogit(ov) - toLogit(origOr)) * deltaScale;
    ovAdj = fromLogit((toLogit(newOr) + deltaLogit) / temp + bias);
  }

  return [round6(ovAdj), round6(gAdj), round6(nAdj)];
}

export function adjustScalar(params: FilterModelParams, p: number): number {
  const s = sanitizeModelParams(params);
  const maxW = Math.max(s.goreWeight, s.nsfwWeight);
  if (maxW <= 1e-6) return 0;
  if (isIdentityCalibration(s)) return round6(p);
  return round6(fromLogit((toLogit(p) + Math.log(maxW)) / s.temperature + s.bias));
}

export function adjustProbabilities(
  params: FilterModelParams,
  raw: FilterProbabilities,
): FilterProbabilities {
  if (isIdentityCalibration(params)) {
    return structuredClone(raw);
  }
  const [prob, goreProb, nsfwProb] = adjustTriplet(params, raw.prob, raw.goreProb, raw.nsfwProb);

  let textProb = raw.textProb;
  let textGoreProb = raw.textGoreProb;
  let textNsfwProb = raw.textNsfwProb;
  if (textProb !== undefined && textGoreProb !== undefined && textNsfwProb !== undefined) {
    [textProb, textGoreProb, textNsfwProb] = adjustTriplet(
      params,
      textProb,
      textGoreProb,
      textNsfwProb,
    );
  } else if (textProb !== undefined) {
    textProb = adjustScalar(params, textProb);
  }

  let imageProb = raw.imageProb;
  let imageGoreProb = raw.imageGoreProb;
  let imageNsfwProb = raw.imageNsfwProb;
  if (imageProb !== undefined && imageGoreProb !== undefined && imageNsfwProb !== undefined) {
    [imageProb, imageGoreProb, imageNsfwProb] = adjustTriplet(
      params,
      imageProb,
      imageGoreProb,
      imageNsfwProb,
    );
  } else if (imageProb !== undefined) {
    imageProb = adjustScalar(params, imageProb);
  }

  return {
    prob,
    goreProb,
    nsfwProb,
    textProb,
    textGoreProb,
    textNsfwProb,
    titleProb: raw.titleProb !== undefined ? adjustScalar(params, raw.titleProb) : undefined,
    descriptionProb:
      raw.descriptionProb !== undefined ? adjustScalar(params, raw.descriptionProb) : undefined,
    imageProb,
    imageGoreProb,
    imageNsfwProb,
    perImageProbs: raw.perImageProbs?.map((v) => adjustScalar(params, v)),
  };
}

export function scoreForTarget(probs: FilterProbabilities, target: FilterScoreTarget): number {
  switch (target) {
    case 'gore':
      return probs.goreProb;
    case 'nsfw':
      return probs.nsfwProb;
    case 'max_category':
      return Math.max(probs.goreProb, probs.nsfwProb);
    case 'overall':
    default:
      return probs.prob;
  }
}

export function addonFilterKey(addonId: string): string {
  return `addon:${addonId}`;
}

export function workshopFilterKey(workshopId: string): string {
  return `ws:${workshopId}`;
}

export function collectAddonImageSources(addon: AddonSummary): string[] {
  const urls: string[] = [];
  const pushUnique = (value?: string) => {
    const trimmed = value?.trim();
    if (trimmed && !urls.includes(trimmed)) {
      urls.push(trimmed);
    }
  };
  pushUnique(addon.coverImage);
  pushUnique(addon.localCoverImage);
  for (const m of addon.maps) {
    pushUnique(m.imagePath);
  }
  return urls;
}

export function collectWorkshopImageUrls(item: WorkshopItemRecord): string[] {
  const urls: string[] = [];
  const pushUnique = (value?: string) => {
    const trimmed = value?.trim();
    if (trimmed && !urls.includes(trimmed)) {
      urls.push(trimmed);
    }
  };
  pushUnique(item.previewUrl);
  for (const preview of item.previews ?? []) {
    if (preview.previewType === 0 && preview.url) {
      pushUnique(preview.url);
    }
  }
  return urls;
}

export function buildAddonFilterTarget(addon: AddonSummary): FilterEvaluationTarget {
  const imageSources = collectAddonImageSources(addon);
  const title = addon.workshopTitle?.trim() || addon.addonTitle?.trim() || addon.title;
  const description = addon.addonInfo.addondescription ?? addon.addonInfo.description;
  const revisionHint = [
    addon.contentHash ?? '',
    addon.localTimeUpdated ?? 0,
    addon.remoteTimeUpdated ?? 0,
    title,
    imageSources.join('|'),
  ].join(':');

  return {
    targetKey: addonFilterKey(addon.id),
    addonId: addon.id,
    workshopId: addon.workshopId,
    title,
    description: description?.trim() || undefined,
    tags: [...addon.categories],
    imageSources,
    revisionHint,
  };
}

export function buildWorkshopFilterTarget(item: WorkshopItemRecord): FilterEvaluationTarget {
  const imageSources = collectWorkshopImageUrls(item);
  const revisionHint = [
    item.timeUpdated ?? 0,
    item.title,
    `imgs=${imageSources.length}`,
    imageSources.join('|'),
  ].join(':');

  return {
    targetKey: workshopFilterKey(item.workshopId),
    workshopId: item.workshopId,
    title: item.title,
    description: item.description?.trim() || undefined,
    tags: [...item.tags],
    imageSources,
    revisionHint,
  };
}

export function computeFilterPolicySignature(filter: ClientFilterConfig): string {
  return JSON.stringify({
    mode: filter.mode,
    customEndpointUrl: filter.customEndpointUrl ?? null,
    localParams: filter.mode === 'local' ? filter.localParams : null,
    scoreTarget: filter.scoreTarget,
    enableDirectFlag: filter.enableDirectFlag,
    flagThreshold: filter.flagThreshold,
    llmEnabled: filter.llmEnabled,
    llmThreshold: filter.llmThreshold,
    llmEndpoint: filter.llmEndpoint,
    llmModel: filter.llmModel,
    llmSendImages: filter.llmSendImages,
    llmSystemPrompt: filter.llmSystemPrompt,
    llmTokenSource: filter.llmTokenSource,
    llmTokenEnvVar: filter.llmTokenEnvVar,
    subdivideModalities: filter.subdivideModalities,
    modalityThreshold: filter.modalityThreshold,
  });
}

function resolveEffectiveModalities(
  verdict: FilterItemVerdict,
  subdivideModalities: boolean,
  totalImages?: number,
): EffectiveModalities {
  if (!subdivideModalities) {
    const inferredCount =
      totalImages ??
      Math.max(
        verdict.probabilities.perImageProbs?.length ?? 0,
        verdict.sensitiveImageIndices.length > 0
          ? Math.max(...verdict.sensitiveImageIndices) + 1
          : 1,
      );
    return {
      imageSensitive: true,
      textSensitive: false,
      titleSensitive: false,
      descriptionSensitive: false,
      sensitiveImageIndices: Array.from({ length: inferredCount }, (_, i) => i),
    };
  }
  return {
    imageSensitive: verdict.imageSensitive,
    textSensitive: verdict.textSensitive,
    titleSensitive: verdict.titleSensitive,
    descriptionSensitive: verdict.descriptionSensitive,
    sensitiveImageIndices: [...verdict.sensitiveImageIndices],
  };
}

function exclusionLevelToCoverMode(level: ExclusionDisplayLevel): CoverFilterMode {
  switch (level) {
    case 'normal':
      return 'none';
    case 'lazy_load':
      return 'lazy';
    case 'blur_image':
      return 'blur';
    case 'hide_image':
    case 'hide_text':
      return 'hide';
  }
}

export interface ResolveCardFilterOptions {
  config: Pick<
    ClientFilterConfig,
    'enabled' | 'applyToLibrary' | 'applyToWorkshop' | 'flaggedAction' | 'subdivideModalities'
  >;
  scope: 'library' | 'workshop';
  verdict?: FilterItemVerdict | null;
  exclusion?: FilterExclusionRecord | null;
  revealed?: boolean;
  totalImages?: number;
  forceShowHiddenCards?: boolean;
}

export function resolveCardFilterState({
  config,
  scope,
  verdict,
  exclusion,
  revealed = false,
  totalImages,
  forceShowHiddenCards = false,
}: ResolveCardFilterOptions): CardFilterState {
  const scopeEnabled =
    config.enabled && (scope === 'library' ? config.applyToLibrary : config.applyToWorkshop);

  const neutral: CardFilterState = {
    scopeEnabled,
    flagged: false,
    hiddenCard: false,
    coverMode: 'none',
    blurWholeCard: false,
    isBlurCardRevealed: false,
    redactTitle: false,
    redactDescription: false,
    redactMeta: false,
    showDiagnosticBadge: false,
    exclusionActive: false,
    exclusionInvalidated: false,
    imageSensitive: false,
    textSensitive: false,
    titleSensitive: false,
    descriptionSensitive: false,
    sensitiveImageIndices: [],
  };

  if (!scopeEnabled) {
    return neutral;
  }

  let exclusionInvalidated = false;
  if (scope === 'library' && exclusion) {
    if (exclusion.fingerprintValid) {
      const level = exclusion.displayLevel;
      const hideText = level === 'hide_text';
      return {
        ...neutral,
        exclusionActive: true,
        exclusionDisplayLevel: level,
        coverMode: exclusionLevelToCoverMode(level),
        redactTitle: hideText,
        redactDescription: hideText,
        redactMeta: hideText,
      };
    }
    exclusionInvalidated = true;
  }

  if (!verdict?.flagged) {
    return {
      ...neutral,
      exclusionInvalidated,
    };
  }

  const modalities = resolveEffectiveModalities(verdict, config.subdivideModalities, totalImages);
  const action =
    forceShowHiddenCards && config.flaggedAction === 'hide_card'
      ? 'blur_card'
      : config.flaggedAction;

  switch (action) {
    case 'mark_only':
      return {
        ...neutral,
        ...modalities,
        flagged: true,
        exclusionInvalidated,
        showDiagnosticBadge: true,
      };
    case 'hide_image': {
      const pureTextFallback =
        !modalities.imageSensitive &&
        modalities.textSensitive &&
        !modalities.titleSensitive &&
        !modalities.descriptionSensitive;
      return {
        ...neutral,
        ...modalities,
        flagged: true,
        exclusionInvalidated,
        coverMode: modalities.imageSensitive ? 'hide' : 'none',
        redactTitle: modalities.titleSensitive || pureTextFallback,
        redactDescription: modalities.descriptionSensitive || pureTextFallback,
      };
    }
    case 'blur_card':
      return {
        ...neutral,
        ...modalities,
        flagged: true,
        exclusionInvalidated,
        coverMode: 'hide',
        blurWholeCard: !revealed,
        isBlurCardRevealed: revealed,
      };
    case 'hide_card':
    default:
      return {
        ...neutral,
        ...modalities,
        flagged: true,
        exclusionInvalidated,
        hiddenCard: true,
        coverMode: 'hide',
      };
  }
}

export interface ResolveDetailFilterOptions {
  config: Pick<
    ClientFilterConfig,
    'enabled' | 'applyToLibrary' | 'applyToWorkshop' | 'subdivideModalities'
  >;
  scope: 'library' | 'workshop';
  verdict?: FilterItemVerdict | null;
  exclusion?: FilterExclusionRecord | null;
  totalImages?: number;
}

export function resolveDetailFilterState({
  config,
  scope,
  verdict,
  exclusion,
  totalImages,
}: ResolveDetailFilterOptions): DetailFilterState {
  const scopeEnabled =
    config.enabled && (scope === 'library' ? config.applyToLibrary : config.applyToWorkshop);

  const safeFn = () => false;
  const noneModeFn = (): CoverFilterMode => 'none';

  const neutral: DetailFilterState = {
    scopeEnabled,
    flagged: false,
    coverMode: 'none',
    mapCoverMode: 'none',
    redactText: false,
    exclusionActive: false,
    exclusionInvalidated: false,
    imageSensitive: false,
    textSensitive: false,
    titleSensitive: false,
    descriptionSensitive: false,
    sensitiveImageIndices: [],
    isImageIndexSensitive: safeFn,
    imageModeAt: noneModeFn,
  };

  if (!scopeEnabled) {
    return neutral;
  }

  let exclusionInvalidated = false;
  if (scope === 'library' && exclusion) {
    if (exclusion.fingerprintValid) {
      const level = exclusion.displayLevel;
      const mode = exclusionLevelToCoverMode(level);
      const isSensitive = mode === 'blur' || mode === 'hide';
      return {
        ...neutral,
        exclusionActive: true,
        exclusionDisplayLevel: level,
        coverMode: mode,
        mapCoverMode: mode,
        redactText: level === 'hide_text',
        isImageIndexSensitive: () => isSensitive,
        imageModeAt: () => mode,
      };
    }
    exclusionInvalidated = true;
  }

  if (!verdict?.flagged) {
    return {
      ...neutral,
      exclusionInvalidated,
    };
  }

  const modalities = resolveEffectiveModalities(verdict, config.subdivideModalities, totalImages);
  const isImageIndexSensitive = (index: number): boolean => {
    if (!config.subdivideModalities) return true;
    if (!verdict.imageSensitive) return false;
    if (verdict.sensitiveImageIndices.length > 0) {
      return verdict.sensitiveImageIndices.includes(index);
    }
    return true;
  };
  const imageModeAt = (index: number): CoverFilterMode =>
    isImageIndexSensitive(index) ? 'blur' : 'none';

  return {
    ...neutral,
    ...modalities,
    flagged: true,
    exclusionInvalidated,
    coverMode: imageModeAt(0),
    mapCoverMode: modalities.imageSensitive ? 'blur' : 'none',
    redactText: false,
    isImageIndexSensitive,
    imageModeAt,
  };
}

export function formatDiagnosticVerdictBadge(
  verdict: FilterItemVerdict,
  modalities: Pick<EffectiveModalities, 'imageSensitive' | 'textSensitive'>,
  labels: { imageTag: string; textTag: string },
): string {
  const scoreText = `P=${verdict.evaluatedScore.toFixed(2)}`;
  const categories: string[] = [];
  if (verdict.probabilities.nsfwProb >= 0.25 || verdict.probabilities.nsfwProb >= verdict.probabilities.goreProb) {
    if (verdict.probabilities.nsfwProb >= 0.15) categories.push('NSFW');
  }
  if (verdict.probabilities.goreProb >= 0.15) {
    categories.push('Gore');
  }
  const catText = categories.length > 0 ? categories.join('/') : 'Flag';
  const tags: string[] = [];
  if (modalities.imageSensitive) tags.push(`[${labels.imageTag}]`);
  if (modalities.textSensitive) tags.push(`[${labels.textTag}]`);
  return [scoreText, catText, tags.join('')].filter(Boolean).join(' · ');
}
