import {
  DEFAULT_FILTER_REPO_URL,
  DEFAULT_LLM_ENDPOINT,
  DEFAULT_LLM_MODEL,
  DEFAULT_LLM_SYSTEM_PROMPT,
  DEFAULT_LLM_TOKEN_ENV_VAR,
  type AddonNamePreference,
  type AddonSummary,
  type BundleRecord,
  type ClientFilterConfig,
  type ExclusionDisplayLevel,
  type FilterEvaluationTarget,
  type FilterExclusionRecord,
  type FilterGalleryAggregation,
  type FilterItemVerdict,
  type FilterModelParams,
  type FilterPresetId,
  type FilterProbabilities,
  type FilterScoreTarget,
  type FlaggedItemAction,
  type MapEntry,
  type WorkshopItemRecord,
} from '../api/ipc';
import {
  formatAddonDisplayName,
  formatBundleMemberLabel,
  stripVpkExtension,
  type AddonSearchRedactionOptions,
} from './format';

export type CoverFilterMode = 'none' | 'lazy' | 'blur' | 'hide';

export const FILTER_REMOTE_VERDICT_MAX_AGE_MS = 60 * 60 * 1000;

export function filterVerdictExpiresAt(
  verdict: FilterItemVerdict,
  mode: ClientFilterConfig['mode'],
): number {
  if (mode === 'local') return Infinity;
  const evaluatedAt = Date.parse(verdict.evaluatedAt);
  return Number.isFinite(evaluatedAt) ? evaluatedAt + FILTER_REMOTE_VERDICT_MAX_AGE_MS : 0;
}

export function isAutomaticFilterTargetEnabled(
  targetKey: string,
  config: Pick<ClientFilterConfig, 'enabled' | 'applyToLibrary' | 'applyToWorkshop'>,
): boolean {
  return config.enabled &&
    (targetKey.startsWith('addon:') ? config.applyToLibrary : config.applyToWorkshop);
}

export type FilterTranslateFn = (key: string, options?: Record<string, unknown>) => string;

export interface EffectiveModalities {
  imageSensitive: boolean;
  textSensitive: boolean;
  titleSensitive: boolean;
  descriptionSensitive: boolean;
  sensitiveImageIndices: number[];
}

export interface CardFilterState extends EffectiveModalities {
  scopeEnabled: boolean;
  pendingEvaluation: boolean;
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
  pendingEvaluation: boolean;
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
    threads: Number.isFinite(params.threads)
      ? Math.min(16, Math.max(1, Math.round(params.threads)))
      : 4,
    enableText: Boolean(params.enableText),
    enableImage: Boolean(params.enableImage),
    maxImages: Number.isFinite(params.maxImages)
      ? Math.min(8, Math.max(1, Math.round(params.maxImages)))
      : 2,
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
  const rawFusion = raw.fusionProb ?? (
    raw.corpseReviewFloor !== undefined && raw.prob <= raw.corpseReviewFloor ? 0 : raw.prob
  );
  const [fusion, goreProb, nsfwProb] = adjustTriplet(params, rawFusion, raw.goreProb, raw.nsfwProb);
  const corpseEnabled = sanitizeModelParams(params).goreWeight > 1e-6;
  const corpseReviewFloor = corpseEnabled ? raw.corpseReviewFloor : undefined;
  const prob = Math.max(fusion, corpseReviewFloor ?? 0);

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
    ...raw,
    prob,
    goreProb,
    nsfwProb,
    fusionProb: corpseReviewFloor !== undefined && fusion <= corpseReviewFloor ? fusion : undefined,
    corpseReviewFloor,
    corpseImageIndices: corpseEnabled ? raw.corpseImageIndices : undefined,
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
  const primaryCover = addon.coverImage?.trim() || addon.localCoverImage?.trim();
  pushUnique(primaryCover);
  for (const m of addon.maps) {
    pushUnique(m.imagePath);
  }
  pushUnique(addon.localCoverImage);
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

export function hasFilterableAddonContent(addon: AddonSummary): boolean {
  const desc = addon.addonInfo.addondescription ?? addon.addonInfo.description;
  return Boolean(
    addon.title?.trim() ||
      addon.addonTitle?.trim() ||
      addon.workshopTitle?.trim() ||
      addon.currentFileName?.trim() ||
      desc?.trim() ||
      addon.categories.length > 0 ||
      collectAddonImageSources(addon).length > 0,
  );
}

export function hasFilterableWorkshopContent(item: WorkshopItemRecord): boolean {
  return Boolean(
    item.title?.trim() ||
      item.description?.trim() ||
      item.tags.length > 0 ||
      collectWorkshopImageUrls(item).length > 0,
  );
}

function addonFilterTitle(addon: AddonSummary): string {
  const fileStem = addon.currentFileName && stripVpkExtension(addon.currentFileName);
  const titles = [addon.workshopTitle, addon.addonTitle, addon.title, fileStem];
  return [...new Set(titles.map((title) => title?.trim()).filter(Boolean))].join('\n');
}

export function buildAddonRevisionHint(addon: AddonSummary): string {
  const imageSources = collectAddonImageSources(addon);
  const description = (addon.addonInfo.addondescription ?? addon.addonInfo.description ?? '').trim();
  return [
    'v2',
    addon.contentHash ?? '',
    addon.localTimeUpdated ?? 0,
    addon.remoteTimeUpdated ?? 0,
    fnv1aHex(addonFilterTitle(addon)),
    fnv1aHex(description),
    addon.categories.join(','),
    imageSources.join('|'),
  ].join(':');
}

export function buildAddonFilterTarget(addon: AddonSummary): FilterEvaluationTarget {
  const imageSources = collectAddonImageSources(addon);
  const title = addonFilterTitle(addon);
  const description = addon.addonInfo.addondescription ?? addon.addonInfo.description;
  const revisionHint = buildAddonRevisionHint(addon);

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

export function buildWorkshopRevisionHint(
  item: WorkshopItemRecord,
  imageSources = collectWorkshopImageUrls(item),
): string {
  return fnv1aHex(JSON.stringify([
    item.timeUpdated ?? 0, item.title, item.description?.trim() ?? '', item.tags, imageSources,
  ]));
}

export function buildWorkshopFilterTarget(item: WorkshopItemRecord): FilterEvaluationTarget {
  const imageSources = collectWorkshopImageUrls(item);
  return {
    targetKey: workshopFilterKey(item.workshopId),
    workshopId: item.workshopId,
    title: item.title,
    description: item.description?.trim() || undefined,
    tags: [...item.tags],
    imageSources,
    revisionHint: buildWorkshopRevisionHint(item, imageSources),
  };
}

const UTF8_ENCODER = new TextEncoder();
const FNV_PRIME = 0x100000001b3n;
const MASK_64 = 0xffffffffffffffffn;

function fnv1aHex(input: string): string {
  const bytes = UTF8_ENCODER.encode(input);
  let h1 = 0xcbf29ce484222325n;
  let h2 = 0x84222325cbf29ce4n;
  for (let idx = 0; idx < bytes.length; idx += 1) {
    const b = bytes[idx];
    h1 = ((h1 ^ BigInt(b)) * FNV_PRIME) & MASK_64;
    const shift = idx % 7;
    const rot = shift === 0 ? b : ((b << shift) | (b >>> (8 - shift))) & 0xff;
    h2 = ((h2 ^ BigInt(rot)) * FNV_PRIME) & MASK_64;
  }
  return `${h1.toString(16).padStart(16, '0')}${h2.toString(16).padStart(16, '0')}`;
}

function fmtF32Fixed4(v: number): string {
  const f = Math.fround(Number.isFinite(v) ? v : 0);
  const normalized = Object.is(f, -0) ? 0 : f;
  return normalized.toFixed(4);
}

function trimTrailingSlashes(s: string): string {
  return s.trim().replace(/\/+$/, '');
}

function rustDebugGalleryAgg(agg: FilterGalleryAggregation): string {
  switch (agg) {
    case 'top2_mean':
      return 'Top2Mean';
    case 'mean':
      return 'Mean';
    case 'max':
    default:
      return 'Max';
  }
}

function rustDebugScoreTarget(target: FilterScoreTarget): string {
  switch (target) {
    case 'gore':
      return 'Gore';
    case 'nsfw':
      return 'Nsfw';
    case 'max_category':
      return 'MaxCategory';
    case 'overall':
    default:
      return 'Overall';
  }
}

export interface FilterPolicySignatureOptions {
  catalogServerUrl?: string | null;
  httpsOnly?: boolean;
  catalogScoringRevision?: string | null;
}

export function computeFilterPolicySignature(
  filter: ClientFilterConfig,
  options?: FilterPolicySignatureOptions,
): string {
  const p = sanitizeModelParams(filter.localParams);
  const httpsOnly = options?.httpsOnly ?? true;

  let modePart: string;
  switch (filter.mode) {
    case 'catalog':
      modePart = `catalog:url=${trimTrailingSlashes(options?.catalogServerUrl ?? '')}:https=${httpsOnly}:rev=${options?.catalogScoringRevision ?? 'unverified'}`;
      break;
    case 'custom_endpoint':
      modePart = `custom:url=${trimTrailingSlashes(filter.customEndpointUrl ?? '')}:https=${httpsOnly}:max=${p.maxImages}`;
      break;
    case 'local':
    default:
      modePart = `local:txt=${p.enableText}:img=${p.enableImage}:max=${p.maxImages}:agg=${rustDebugGalleryAgg(p.galleryAggregation)}:gw=${fmtF32Fixed4(p.goreWeight)}:nw=${fmtF32Fixed4(p.nsfwWeight)}:t=${fmtF32Fixed4(p.temperature)}:b=${fmtF32Fixed4(p.bias)}:commit=${filter.trackLatest ? 'latest' : (filter.pinnedCommit ?? 'latest')}`;
      break;
  }

  const llmPart = filter.llmEnabled
    ? `1:t=${fmtF32Fixed4(filter.llmThreshold)}:ep=${trimTrailingSlashes(filter.llmEndpoint)}:m=${filter.llmModel.trim()}:img=${filter.llmSendImages}:sys=${fnv1aHex(filter.llmSystemPrompt.trim())}:token=${filter.llmTokenSource}:env=${filter.llmTokenSource === 'env_var' ? filter.llmTokenEnvVar.trim() : ''}`
    : '0';

  const raw = `v4|${modePart}|st=${rustDebugScoreTarget(filter.scoreTarget)}|df=${filter.enableDirectFlag}:${fmtF32Fixed4(filter.flagThreshold)}|llm=${llmPart}|sub=${filter.subdivideModalities}:${fmtF32Fixed4(filter.modalityThreshold)}`;
  return `pol_${fnv1aHex(raw)}`;
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
        (verdict.evaluatedImageIndices?.length ?? 0) > 0
          ? Math.max(...verdict.evaluatedImageIndices) + 1
          : 0,
        verdict.sensitiveImageIndices.length > 0
          ? Math.max(...verdict.sensitiveImageIndices) + 1
          : 1,
      );
    return {
      imageSensitive: true,
      textSensitive: true,
      titleSensitive: true,
      descriptionSensitive: true,
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
  hasTargetFailed?: boolean;
  hasContent?: boolean;
}

export function resolveCardFilterState({
  config,
  scope,
  verdict,
  exclusion,
  revealed = false,
  totalImages,
  forceShowHiddenCards = false,
  hasTargetFailed = false,
  hasContent = true,
}: ResolveCardFilterOptions): CardFilterState {
  const scopeEnabled =
    config.enabled && (scope === 'library' ? config.applyToLibrary : config.applyToWorkshop);

  const neutral: CardFilterState = {
    scopeEnabled,
    pendingEvaluation: false,
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

  if (!scopeEnabled) {
    return {
      ...neutral,
      exclusionInvalidated,
    };
  }

  const isPendingEvaluation = verdict === undefined && !hasTargetFailed && hasContent;
  if (isPendingEvaluation) {
    if (config.flaggedAction === 'mark_only') {
      return {
        ...neutral,
        exclusionInvalidated,
        pendingEvaluation: true,
      };
    }
    const isHideImage = config.flaggedAction === 'hide_image';
    return {
      ...neutral,
      exclusionInvalidated,
      pendingEvaluation: true,
      coverMode: 'hide',
      redactTitle: isHideImage,
      redactDescription: isHideImage,
      blurWholeCard:
        config.flaggedAction === 'blur_card' || config.flaggedAction === 'hide_card',
    };
  }

  if (verdict === undefined && hasTargetFailed && hasContent) {
    if (config.flaggedAction === 'mark_only') {
      return {
        ...neutral,
        exclusionInvalidated,
      };
    }
    const isHideImage = config.flaggedAction === 'hide_image';
    const shouldBlur =
      config.flaggedAction === 'blur_card' || config.flaggedAction === 'hide_card';
    return {
      ...neutral,
      exclusionInvalidated,
      coverMode: 'hide',
      redactTitle: isHideImage,
      redactDescription: isHideImage,
      blurWholeCard: shouldBlur && !revealed,
      isBlurCardRevealed: shouldBlur && revealed,
    };
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
  > & { flaggedAction?: FlaggedItemAction };
  scope: 'library' | 'workshop';
  verdict?: FilterItemVerdict | null;
  exclusion?: FilterExclusionRecord | null;
  revealed?: boolean;
  totalImages?: number;
  hasTargetFailed?: boolean;
  hasContent?: boolean;
}

export function resolveDetailFilterState({
  config,
  scope,
  verdict,
  exclusion,
  revealed = false,
  totalImages,
  hasTargetFailed = false,
  hasContent = true,
}: ResolveDetailFilterOptions): DetailFilterState {
  const scopeEnabled =
    config.enabled && (scope === 'library' ? config.applyToLibrary : config.applyToWorkshop);

  const safeFn = () => false;
  const noneModeFn = (): CoverFilterMode => 'none';

  const neutral: DetailFilterState = {
    scopeEnabled,
    pendingEvaluation: false,
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

  if (!scopeEnabled) {
    return {
      ...neutral,
      exclusionInvalidated,
    };
  }

  const isPendingEvaluation = verdict === undefined && !hasTargetFailed && hasContent;
  if (isPendingEvaluation) {
    if (config.flaggedAction === 'mark_only') {
      return {
        ...neutral,
        exclusionInvalidated,
        pendingEvaluation: true,
      };
    }
    return {
      ...neutral,
      exclusionInvalidated,
      pendingEvaluation: true,
      coverMode: 'hide',
      mapCoverMode: 'hide',
      redactText: true,
      isImageIndexSensitive: () => true,
      imageModeAt: () => 'hide',
    };
  }

  if (verdict === undefined && hasTargetFailed && hasContent) {
    if (config.flaggedAction === 'mark_only') {
      return {
        ...neutral,
        exclusionInvalidated,
      };
    }
    return {
      ...neutral,
      exclusionInvalidated,
      coverMode: 'blur',
      mapCoverMode: 'blur',
      redactText: !revealed,
      isImageIndexSensitive: () => true,
      imageModeAt: () => 'blur',
    };
  }

  if (!verdict?.flagged) {
    return {
      ...neutral,
      exclusionInvalidated,
    };
  }

  const modalities = resolveEffectiveModalities(verdict, config.subdivideModalities, totalImages);
  const isImageIndexSensitive = (slotIndex: number): boolean => {
    if (!config.subdivideModalities) return true;
    if (!modalities.imageSensitive) return false;
    if (modalities.sensitiveImageIndices.length === 0) {
      return true;
    }
    if (modalities.sensitiveImageIndices.includes(slotIndex)) {
      return true;
    }
    const evaluatedIndices = verdict.evaluatedImageIndices ?? [];
    if (evaluatedIndices.length > 0) {
      return !evaluatedIndices.includes(slotIndex);
    }
    const evaluatedCount = verdict.probabilities.perImageProbs?.length ?? 0;
    if (evaluatedCount === 0 || slotIndex >= evaluatedCount) {
      return true;
    }
    return false;
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

export interface FilterLookupSource {
  filterConfig: Pick<
    ClientFilterConfig,
    'enabled' | 'applyToLibrary' | 'applyToWorkshop' | 'flaggedAction' | 'subdivideModalities'
  >;
  verdicts: Record<string, FilterItemVerdict>;
  evaluatedRevisions?: Record<string, string>;
  exclusionsByAddonId: ReadonlyMap<string, FilterExclusionRecord>;
  revealedCardIds?: ReadonlySet<string>;
  failedTargetKeys?: ReadonlySet<string>;
}

export function resolveLookupVerdict(
  targetKey: string,
  filterCtx: Pick<FilterLookupSource, 'verdicts' | 'evaluatedRevisions'>,
  expectedRevision?: string,
): FilterItemVerdict | null | undefined {
  const verdict = filterCtx.verdicts[targetKey];
  if (verdict && (expectedRevision === undefined || verdict.revisionHint === expectedRevision)) {
    return verdict;
  }
  const completedRevision = filterCtx.evaluatedRevisions?.[targetKey];
  if (completedRevision !== undefined && (expectedRevision === undefined || completedRevision === expectedRevision)) {
    return null;
  }
  return undefined;
}

export function resolveWorkshopVerdict(
  item: WorkshopItemRecord,
  filterCtx: Pick<FilterLookupSource, 'verdicts' | 'evaluatedRevisions'>,
): FilterItemVerdict | null | undefined {
  return resolveLookupVerdict(
    workshopFilterKey(item.workshopId),
    filterCtx,
    buildWorkshopRevisionHint(item),
  );
}

export function resolveAddonVerdict(
  addon: AddonSummary,
  filterCtx: Pick<FilterLookupSource, 'verdicts' | 'evaluatedRevisions'>,
): FilterItemVerdict | null | undefined {
  const targetKey = addonFilterKey(addon.id);
  const expectedRevision = buildAddonRevisionHint(addon);
  const verdict = filterCtx.verdicts[targetKey];
  if (verdict) {
    if (verdict.revisionHint === undefined || verdict.revisionHint === expectedRevision) {
      return verdict;
    }
    return undefined;
  }
  const completedRevision = filterCtx.evaluatedRevisions?.[targetKey];
  if (completedRevision !== undefined && completedRevision === expectedRevision) {
    return null;
  }
  return undefined;
}

export function resolveLibraryAddonCardFilter(
  addon: AddonSummary,
  filterCtx: FilterLookupSource,
  options?: { forceShowHiddenCards?: boolean },
): CardFilterState {
  const key = addonFilterKey(addon.id);
  return resolveCardFilterState({
    config: filterCtx.filterConfig,
    scope: 'library',
    verdict: resolveAddonVerdict(addon, filterCtx),
    exclusion: filterCtx.exclusionsByAddonId.get(addon.id),
    revealed: filterCtx.revealedCardIds?.has(key) ?? false,
    forceShowHiddenCards: options?.forceShowHiddenCards ?? false,
    hasTargetFailed: filterCtx.failedTargetKeys?.has(key) ?? false,
    hasContent: hasFilterableAddonContent(addon),
  });
}

export function resolveLibraryAddonDetailFilter(
  addon: AddonSummary,
  filterCtx: FilterLookupSource,
): DetailFilterState {
  const key = addonFilterKey(addon.id);
  const imageSources = collectAddonImageSources(addon);
  return resolveDetailFilterState({
    config: filterCtx.filterConfig,
    scope: 'library',
    verdict: resolveAddonVerdict(addon, filterCtx),
    exclusion: filterCtx.exclusionsByAddonId.get(addon.id),
    revealed: filterCtx.revealedCardIds?.has(key) ?? false,
    totalImages: imageSources.length,
    hasTargetFailed: filterCtx.failedTargetKeys?.has(key) ?? false,
    hasContent: hasFilterableAddonContent(addon),
  });
}

export function getFilteredAddonFallbackCover(
  addon: AddonSummary,
  filterCtx: FilterLookupSource,
): string | undefined {
  const local = addon.localCoverImage?.trim();
  const primary = addon.coverImage?.trim();
  if (!local || !primary || primary === local) return local;
  const config = filterCtx.filterConfig;
  if (!config.enabled || !config.applyToLibrary || config.flaggedAction === 'mark_only' ||
      filterCtx.exclusionsByAddonId.get(addon.id)?.fingerprintValid) {
    return local;
  }
  const slot = collectAddonImageSources(addon).indexOf(local);
  const verdict = resolveLookupVerdict(addonFilterKey(addon.id), filterCtx, buildAddonRevisionHint(addon));
  if (!verdict?.evaluatedImageIndices?.includes(slot)) return undefined;
  return resolveLibraryAddonDetailFilter(addon, filterCtx).imageModeAt(slot) === 'none'
    ? local
    : undefined;
}

export function getAddonSearchRedaction(
  addon: AddonSummary,
  filterCtx: FilterLookupSource,
): AddonSearchRedactionOptions {
  const state = resolveLibraryAddonCardFilter(addon, filterCtx);
  const unverifiedCardShielded =
    (state.blurWholeCard || state.hiddenCard) && !state.flagged && !state.isBlurCardRevealed;
  const unverifiedTitleShielded = !state.flagged && state.redactTitle;
  const redactTitle =
    state.redactTitle ||
    ((state.blurWholeCard || state.hiddenCard) && state.titleSensitive) ||
    unverifiedCardShielded;
  const redactDescription =
    state.redactDescription ||
    ((state.blurWholeCard || state.hiddenCard) && state.descriptionSensitive) ||
    unverifiedCardShielded;
  return {
    redactTitle,
    redactDescription,
    redactMeta: state.redactMeta || unverifiedCardShielded || unverifiedTitleShielded,
  };
}

export function isAddonTitleRedacted(addon: AddonSummary, filterCtx: FilterLookupSource): boolean {
  return Boolean(getAddonSearchRedaction(addon, filterCtx).redactTitle);
}

export function isWorkshopKeyTitleRedacted(
  workshopId: string,
  rawTitle: string | undefined,
  filterCtx: FilterLookupSource,
): boolean {
  const wsKey = workshopFilterKey(workshopId);
  const wsState = resolveCardFilterState({
    config: filterCtx.filterConfig,
    scope: 'workshop',
    verdict: resolveLookupVerdict(wsKey, filterCtx),
    revealed: filterCtx.revealedCardIds?.has(wsKey) ?? false,
    hasTargetFailed: filterCtx.failedTargetKeys?.has(wsKey) ?? false,
    hasContent: Boolean(rawTitle?.trim()),
  });
  const unverifiedCardShielded =
    (wsState.blurWholeCard || wsState.hiddenCard) &&
    !wsState.flagged &&
    !wsState.isBlurCardRevealed;
  return (
    wsState.redactTitle ||
    ((wsState.blurWholeCard || wsState.hiddenCard) && wsState.titleSensitive) ||
    unverifiedCardShielded
  );
}

export function matchesEvaluatedAddonTitle(addon: AddonSummary, candidate: string): boolean {
  const trimmed = candidate.trim();
  if (!trimmed) return true;
  const known = [
    addon.title,
    addon.workshopTitle,
    addon.addonTitle,
    addon.currentFileName ? stripVpkExtension(addon.currentFileName) : undefined,
  ]
    .map((s) => s?.trim())
    .filter((s): s is string => Boolean(s));
  return known.includes(trimmed);
}

export function isLinkedWorkshopTitleRedacted(
  workshopId: string,
  addon: AddonSummary | undefined,
  rawTitle: string | undefined,
  filterCtx: FilterLookupSource,
  options?: { transferKind?: 'download' | 'update' },
): boolean {
  if (!addon) {
    return isWorkshopKeyTitleRedacted(workshopId, rawTitle, filterCtx);
  }

  const addonKey = addonFilterKey(addon.id);
  const localState = resolveLibraryAddonCardFilter(addon, filterCtx);
  if (localState.exclusionActive) {
    return localState.redactTitle;
  }

  const addonVerdict = resolveAddonVerdict(addon, filterCtx);
  const addonFailed = filterCtx.failedTargetKeys?.has(addonKey) ?? false;
  const wsKey = workshopFilterKey(workshopId);
  const wsVerdict = resolveLookupVerdict(wsKey, filterCtx);

  // Freshly downloaded Workshop item already verified safe in Workshop before library eval runs:
  // avoid falsely redacting the completion toast when the title matches the verified Workshop title.
  if (
    options?.transferKind === 'download' &&
    addonVerdict === undefined &&
    !addonFailed &&
    (wsVerdict === null ||
      (wsVerdict !== undefined && !wsVerdict.flagged && wsVerdict.complete !== false))
  ) {
    const candidate = (rawTitle ?? addon.title).trim();
    const trustedWsTitle = addon.workshopTitle?.trim();
    if (!candidate || (trustedWsTitle && candidate === trustedWsTitle)) {
      return false;
    }
  }

  if (isAddonTitleRedacted(addon, filterCtx)) {
    return true;
  }

  if (!filterCtx.filterConfig.applyToLibrary) {
    return isWorkshopKeyTitleRedacted(workshopId, rawTitle ?? addon.title, filterCtx);
  }

  const trimmedRawTitle = rawTitle?.trim();
  if (
    trimmedRawTitle &&
    filterCtx.filterConfig.applyToWorkshop &&
    !matchesEvaluatedAddonTitle(addon, trimmedRawTitle)
  ) {
    return isWorkshopKeyTitleRedacted(workshopId, trimmedRawTitle, filterCtx);
  }

  if (wsVerdict?.flagged) {
    const wsState = resolveCardFilterState({
      config: filterCtx.filterConfig,
      scope: 'workshop',
      verdict: wsVerdict,
      revealed: filterCtx.revealedCardIds?.has(wsKey) ?? false,
      hasTargetFailed: filterCtx.failedTargetKeys?.has(wsKey) ?? false,
      hasContent: Boolean((rawTitle ?? addon.title).trim()),
    });
    return (
      wsState.redactTitle ||
      ((wsState.blurWholeCard || wsState.hiddenCard) && wsState.titleSensitive)
    );
  }

  return false;
}

export function formatRedactedAddonLabel(
  workshopId: string | undefined,
  t: FilterTranslateFn,
): string {
  return workshopId
    ? t('filter.redactedTitleWithId', { id: workshopId })
    : t('filter.redactedTitle');
}

export function formatFilteredAddonTitle(
  addon: AddonSummary,
  namePreference: AddonNamePreference | undefined,
  filterCtx: FilterLookupSource,
  t: FilterTranslateFn,
): string {
  if (isAddonTitleRedacted(addon, filterCtx)) {
    return formatRedactedAddonLabel(addon.workshopId, t);
  }
  return formatAddonDisplayName(addon, namePreference);
}

export function formatFilteredBundleMemberLabel(
  addon: AddonSummary,
  siblings: readonly AddonSummary[],
  namePreference: AddonNamePreference | undefined,
  bundleName: string | undefined,
  filterCtx: FilterLookupSource,
  t: FilterTranslateFn,
): string {
  if (isAddonTitleRedacted(addon, filterCtx)) {
    return formatRedactedAddonLabel(addon.workshopId, t);
  }
  return formatBundleMemberLabel(addon, siblings, namePreference, bundleName);
}

export interface BundleCoverPresentation {
  ownerMember?: AddonSummary;
  src?: string;
  fallbackSrc?: string;
  coverMode: CoverFilterMode;
  pendingEvaluation: boolean;
  redactTitle: boolean;
  redactAuthor: boolean;
  author?: string;
  blurWholeCard: boolean;
}

function memberTitleOverlapsBundleName(member: AddonSummary, bundleNameLower: string): boolean {
  const candidates = [member.title, member.workshopTitle, member.addonTitle]
    .map((s) => s?.trim().toLowerCase())
    .filter((s): s is string => Boolean(s));
  return candidates.some(
    (c) => c === bundleNameLower || c.includes(bundleNameLower) || bundleNameLower.includes(c),
  );
}

export function resolveBundleCoverPresentation(
  visibleMembers: readonly AddonSummary[],
  filterCtx: FilterLookupSource,
  options?: {
    allMembers?: readonly AddonSummary[];
    bundleCoverImage?: string;
    bundleName?: string;
    forceShowHiddenCards?: boolean;
  },
): BundleCoverPresentation {
  const entries = visibleMembers.map((member) => ({
    member,
    state: resolveLibraryAddonCardFilter(member, filterCtx, {
      forceShowHiddenCards: options?.forceShowHiddenCards,
    }),
  }));
  const eligible = entries.filter(
    (entry) => !entry.state.hiddenCard || options?.forceShowHiddenCards,
  );

  const memberNeedsTitleRedaction = (state: CardFilterState): boolean =>
    state.redactTitle ||
    ((state.blurWholeCard || state.hiddenCard) && (!state.flagged || state.titleSensitive));

  let redactTitle =
    eligible.length > 0 && eligible.some((e) => memberNeedsTitleRedaction(e.state));

  if (!redactTitle && options?.allMembers && options.allMembers.length > 0) {
    const bundleNameLower = options.bundleName?.trim().toLowerCase() ?? '';
    const hiddenTitleSensitiveMembers = options.allMembers.filter((member) => {
      const state = resolveLibraryAddonCardFilter(member, filterCtx, {
        forceShowHiddenCards: options.forceShowHiddenCards,
      });
      return state.hiddenCard && memberNeedsTitleRedaction(state);
    });
    if (hiddenTitleSensitiveMembers.length > 0 && bundleNameLower) {
      const hiddenMatches = hiddenTitleSensitiveMembers.some((m) =>
        memberTitleOverlapsBundleName(m, bundleNameLower),
      );
      const safeVisibleMatches = eligible.some(
        (e) =>
          !memberNeedsTitleRedaction(e.state) &&
          memberTitleOverlapsBundleName(e.member, bundleNameLower),
      );
      if (hiddenMatches || !safeVisibleMatches) {
        redactTitle = true;
      }
    }
  }

  const author = eligible.find((e) => !e.state.redactMeta && e.member.author?.trim())?.member.author;
  const redactAuthor = !author && eligible.some((e) => e.state.redactMeta && e.member.author?.trim());
  const effectiveBlurWholeCard =
    eligible.length > 0 && eligible.every((e) => e.state.blurWholeCard);
  const anyPendingBlur =
    effectiveBlurWholeCard && eligible.some((e) => e.state.pendingEvaluation);

  const withCover = eligible.filter((e) =>
    Boolean(e.member.coverImage?.trim() || e.member.localCoverImage?.trim()),
  );
  const ownerEntry =
    withCover.find((e) => e.member.maps.length > 0 && e.state.coverMode === 'none') ??
    withCover.find((e) => e.state.coverMode === 'none') ??
    withCover.find((e) => e.member.maps.length > 0) ??
    withCover[0];

  if (ownerEntry) {
    return {
      ownerMember: ownerEntry.member,
      src: ownerEntry.member.coverImage,
      fallbackSrc: getFilteredAddonFallbackCover(ownerEntry.member, filterCtx),
      coverMode: effectiveBlurWholeCard ? 'hide' : ownerEntry.state.coverMode,
      pendingEvaluation: effectiveBlurWholeCard ? anyPendingBlur : ownerEntry.state.pendingEvaluation,
      redactTitle,
      redactAuthor,
      author,
      blurWholeCard: effectiveBlurWholeCard,
    };
  }

  const fallbackBundleCover = options?.bundleCoverImage?.trim();
  if (fallbackBundleCover && eligible.length > 0) {
    const candidateAllMembers = options?.allMembers ?? visibleMembers;
    const bundleOwner = candidateAllMembers.find(
      (m) =>
        m.coverImage?.trim() === fallbackBundleCover ||
        m.localCoverImage?.trim() === fallbackBundleCover,
    );
    if (bundleOwner) {
      const bundleOwnerState = resolveLibraryAddonCardFilter(bundleOwner, filterCtx, {
        forceShowHiddenCards: options?.forceShowHiddenCards,
      });
      if (!bundleOwnerState.hiddenCard || options?.forceShowHiddenCards) {
        return {
          ownerMember: bundleOwner,
          src: fallbackBundleCover,
          fallbackSrc:
            bundleOwner.localCoverImage?.trim() !== fallbackBundleCover
              ? getFilteredAddonFallbackCover(bundleOwner, filterCtx)
              : undefined,
          coverMode: effectiveBlurWholeCard ? 'hide' : bundleOwnerState.coverMode,
          pendingEvaluation: effectiveBlurWholeCard
            ? anyPendingBlur
            : bundleOwnerState.pendingEvaluation,
          redactTitle,
          redactAuthor,
          author,
          blurWholeCard: effectiveBlurWholeCard,
        };
      }
    }
  }

  return {
    ownerMember: eligible[0]?.member,
    src: undefined,
    fallbackSrc: undefined,
    coverMode: effectiveBlurWholeCard ? 'hide' : 'none',
    pendingEvaluation: anyPendingBlur,
    redactTitle,
    redactAuthor,
    author,
    blurWholeCard: effectiveBlurWholeCard,
  };
}

export interface FilteredBundleMapItem {
  map: MapEntry;
  coverMode: CoverFilterMode;
  pendingEvaluation: boolean;
  redactText: boolean;
}

export function buildFilteredBundleMaps(
  bundle: BundleRecord,
  visibleMembers: readonly AddonSummary[],
  allMembers: readonly AddonSummary[],
  filterCtx: FilterLookupSource,
): FilteredBundleMapItem[] {
  const anyMemberHasMaps = allMembers.some((m) => m.maps.length > 0);
  if (anyMemberHasMaps) {
    const seenCodes = new Set<string>();
    const out: FilteredBundleMapItem[] = [];
    for (const member of visibleMembers) {
      if (member.maps.length === 0) continue;
      const detail = resolveLibraryAddonDetailFilter(member, filterCtx);
      const imgSources = collectAddonImageSources(member);
      for (const map of member.maps) {
        const codeKey = map.code.toLowerCase();
        if (seenCodes.has(codeKey)) continue;
        seenCodes.add(codeKey);
        const trimmedImg = map.imagePath?.trim();
        const imgIdx = trimmedImg ? imgSources.indexOf(trimmedImg) : -1;
        const coverMode: CoverFilterMode = trimmedImg
          ? imgIdx >= 0
            ? detail.imageModeAt(imgIdx)
            : detail.mapCoverMode
          : 'none';
        out.push({
          map,
          coverMode,
          pendingEvaluation: detail.pendingEvaluation,
          redactText: detail.redactText,
        });
      }
    }
    return out;
  }

  if (bundle.maps.length > 0 && visibleMembers.length > 0) {
    const detail = resolveLibraryAddonDetailFilter(visibleMembers[0], filterCtx);
    return bundle.maps.map((map) => ({
      map,
      coverMode: map.imagePath?.trim() ? detail.mapCoverMode : 'none',
      pendingEvaluation: detail.pendingEvaluation,
      redactText: detail.redactText,
    }));
  }

  return [];
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
