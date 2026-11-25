import { describe, expect, it } from 'vitest';
import type {
  AddonSummary,
  ClientFilterConfig,
  ExclusionDisplayLevel,
  FilterExclusionRecord,
  FilterItemVerdict,
} from '../api/ipc';
import { en } from '../i18n/en';
import { zh } from '../i18n/zh';
import {
  adjustProbabilities,
  adjustTriplet,
  collectWorkshopImageUrls,
  computeFilterPolicySignature,
  createDefaultFilterConfig,
  formatDiagnosticVerdictBadge,
  getPresetDefaultParams,
  resolveBundleCoverPresentation,
  resolveCardFilterState,
  resolveDetailFilterState,
  resolveLookupVerdict,
} from './contentFilter';

function makeEnabledConfig(overrides: Partial<ClientFilterConfig> = {}): ClientFilterConfig {
  return {
    ...createDefaultFilterConfig(),
    enabled: true,
    applyToLibrary: true,
    applyToWorkshop: true,
    ...overrides,
  };
}

function makeFlaggedVerdict(overrides: Partial<FilterItemVerdict> = {}): FilterItemVerdict {
  return {
    targetKey: 'addon:ws:2814104912',
    flagged: true,
    decisionSource: 'model_direct_flag',
    evaluatedScore: 0.86,
    probabilities: {
      prob: 0.86,
      goreProb: 0.04,
      nsfwProb: 0.85,
      textProb: 0.08,
      titleProb: 0.05,
      descriptionProb: 0.08,
      imageProb: 0.85,
      perImageProbs: [0.85, 0.05],
    },
    textSensitive: false,
    titleSensitive: false,
    descriptionSensitive: false,
    imageSensitive: true,
    sensitiveImageIndices: [0],
    evaluatedAt: '2026-11-25T00:00:00Z',
    ...overrides,
  };
}

function makeExclusion(
  displayLevel: ExclusionDisplayLevel,
  fingerprintValid = true,
): FilterExclusionRecord {
  return {
    addonId: 'ws:2814104912',
    displayLevel,
    bindFingerprint: true,
    boundFingerprint: '9f8e7d6c5b4a3210',
    currentFingerprint: fingerprintValid ? '9f8e7d6c5b4a3210' : 'changed_hash_9999',
    fingerprintValid,
    createdAt: '2026-11-25T00:00:00Z',
    updatedAt: '2026-11-25T00:00:00Z',
  };
}

function flattenKeys(obj: Record<string, unknown>, prefix = ''): string[] {
  const keys: string[] = [];
  for (const [k, v] of Object.entries(obj)) {
    const next = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === 'object' && !Array.isArray(v)) {
      keys.push(...flattenKeys(v as Record<string, unknown>, next));
    } else {
      keys.push(next);
    }
  }
  return keys.sort();
}

describe('contentFilter pure resolution & calibration logic', () => {
  it('supports all 4 flagged card actions including blur_card click-once reveal while keeping image hidden', () => {
    const verdict = makeFlaggedVerdict({
      imageSensitive: true,
      textSensitive: true,
      titleSensitive: true,
    });

    // 1. mark_only: no hiding or blurring, shows diagnostic badge
    const markOnly = resolveCardFilterState({
      config: makeEnabledConfig({ flaggedAction: 'mark_only' }),
      scope: 'library',
      verdict,
    });
    expect(markOnly.hiddenCard).toBe(false);
    expect(markOnly.coverMode).toBe('none');
    expect(markOnly.blurWholeCard).toBe(false);
    expect(markOnly.redactTitle).toBe(false);
    expect(markOnly.showDiagnosticBadge).toBe(true);
    expect(
      formatDiagnosticVerdictBadge(verdict, markOnly, { imageTag: 'IMG', textTag: 'TXT' }),
    ).toContain('P=0.86 · NSFW · [IMG][TXT]');

    // 2. hide_image: hides sensitive image and redacts sensitive title when subdivideModalities=true
    const hideImage = resolveCardFilterState({
      config: makeEnabledConfig({ flaggedAction: 'hide_image', subdivideModalities: true }),
      scope: 'library',
      verdict,
    });
    expect(hideImage.coverMode).toBe('hide');
    expect(hideImage.redactTitle).toBe(true);
    expect(hideImage.blurWholeCard).toBe(false);
    expect(hideImage.hiddenCard).toBe(false);

    // 3. blur_card: initial state blurs whole card AND hides image; first click unblurs text while STILL hiding image!
    const blurInitial = resolveCardFilterState({
      config: makeEnabledConfig({ flaggedAction: 'blur_card' }),
      scope: 'workshop',
      verdict,
      revealed: false,
    });
    expect(blurInitial.blurWholeCard).toBe(true);
    expect(blurInitial.isBlurCardRevealed).toBe(false);
    expect(blurInitial.coverMode).toBe('hide');
    expect(blurInitial.redactTitle).toBe(false);

    const blurRevealed = resolveCardFilterState({
      config: makeEnabledConfig({ flaggedAction: 'blur_card' }),
      scope: 'workshop',
      verdict,
      revealed: true,
    });
    expect(blurRevealed.blurWholeCard).toBe(false);
    expect(blurRevealed.isBlurCardRevealed).toBe(true);
    expect(blurRevealed.coverMode).toBe('hide');
    expect(blurRevealed.redactTitle).toBe(false);
    expect(blurRevealed.redactDescription).toBe(false);

    // 4. hide_card: omits card from grid/list unless forceShowHiddenCards is toggled
    const hideCard = resolveCardFilterState({
      config: makeEnabledConfig({ flaggedAction: 'hide_card' }),
      scope: 'library',
      verdict,
    });
    expect(hideCard.hiddenCard).toBe(true);

    const forceShown = resolveCardFilterState({
      config: makeEnabledConfig({ flaggedAction: 'hide_card' }),
      scope: 'library',
      verdict,
      forceShowHiddenCards: true,
    });
    expect(forceShown.hiddenCard).toBe(false);
    expect(forceShown.blurWholeCard).toBe(true);
  });

  it('distinguishes image-only vs pure-text sensitive items when subdivideModalities is enabled vs disabled', () => {
    const pureTextVerdict = makeFlaggedVerdict({
      imageSensitive: false,
      textSensitive: true,
      titleSensitive: true,
      descriptionSensitive: true,
      sensitiveImageIndices: [],
    });

    // With subdivideModalities=true under hide_image: safe cover image stays visible ('none'), sensitive text redacted!
    const subdivided = resolveCardFilterState({
      config: makeEnabledConfig({ flaggedAction: 'hide_image', subdivideModalities: true }),
      scope: 'workshop',
      verdict: pureTextVerdict,
    });
    expect(subdivided.coverMode).toBe('none');
    expect(subdivided.redactTitle).toBe(true);
    expect(subdivided.redactDescription).toBe(true);

    // With subdivideModalities=false: treats BOTH image and text modalities as sensitive!
    const coarseFromText = resolveCardFilterState({
      config: makeEnabledConfig({ flaggedAction: 'hide_image', subdivideModalities: false }),
      scope: 'workshop',
      verdict: pureTextVerdict,
      totalImages: 3,
    });
    expect(coarseFromText.imageSensitive).toBe(true);
    expect(coarseFromText.textSensitive).toBe(true);
    expect(coarseFromText.titleSensitive).toBe(true);
    expect(coarseFromText.descriptionSensitive).toBe(true);
    expect(coarseFromText.coverMode).toBe('hide');
    expect(coarseFromText.redactTitle).toBe(true);
    expect(coarseFromText.redactDescription).toBe(true);
    expect(coarseFromText.sensitiveImageIndices).toEqual([0, 1, 2]);

    const coarseFromImageOnly = resolveCardFilterState({
      config: makeEnabledConfig({ flaggedAction: 'hide_image', subdivideModalities: false }),
      scope: 'library',
      verdict: makeFlaggedVerdict({
        imageSensitive: true,
        textSensitive: false,
        titleSensitive: false,
        descriptionSensitive: false,
      }),
    });
    expect(coarseFromImageOnly.imageSensitive).toBe(true);
    expect(coarseFromImageOnly.textSensitive).toBe(true);
    expect(coarseFromImageOnly.redactTitle).toBe(true);
    expect(coarseFromImageOnly.redactDescription).toBe(true);
  });

  it('prevents raw cover flash while evaluation is pending and clears pending state once evaluated', () => {
    // Pending evaluation (verdict === undefined, not failed, has content) hides covers immediately
    const pendingHideImage = resolveCardFilterState({
      config: makeEnabledConfig({ flaggedAction: 'hide_image' }),
      scope: 'workshop',
      verdict: undefined,
      hasTargetFailed: false,
      hasContent: true,
    });
    expect(pendingHideImage.pendingEvaluation).toBe(true);
    expect(pendingHideImage.coverMode).toBe('hide');
    expect(pendingHideImage.blurWholeCard).toBe(false);

    const pendingBlurCard = resolveCardFilterState({
      config: makeEnabledConfig({ flaggedAction: 'blur_card' }),
      scope: 'library',
      verdict: undefined,
      hasTargetFailed: false,
      hasContent: true,
    });
    expect(pendingBlurCard.pendingEvaluation).toBe(true);
    expect(pendingBlurCard.coverMode).toBe('hide');
    expect(pendingBlurCard.blurWholeCard).toBe(true);

    const pendingDetail = resolveDetailFilterState({
      config: makeEnabledConfig({ flaggedAction: 'hide_card' }),
      scope: 'workshop',
      verdict: undefined,
      hasTargetFailed: false,
      hasContent: true,
    });
    expect(pendingDetail.pendingEvaluation).toBe(true);
    expect(pendingDetail.coverMode).toBe('hide');
    expect(pendingDetail.imageModeAt(0)).toBe('hide');
    expect(pendingDetail.imageModeAt(2)).toBe('hide');

    // Once evaluated safe (present in evaluatedRevisions without flagged verdict), resolveLookupVerdict returns null -> not pending!
    const lookupSafe = resolveLookupVerdict('workshop:999', {
      verdicts: {},
      evaluatedRevisions: { 'workshop:999': 'rev1' },
    });
    expect(lookupSafe).toBeNull();
    const settledSafe = resolveCardFilterState({
      config: makeEnabledConfig({ flaggedAction: 'blur_card' }),
      scope: 'workshop',
      verdict: lookupSafe,
    });
    expect(settledSafe.pendingEvaluation).toBe(false);
    expect(settledSafe.coverMode).toBe('none');
    expect(settledSafe.blurWholeCard).toBe(false);
  });

  it('enforces image-only blurring in Detail view, subdivides gallery screenshots, and blurs unscanned screenshots beyond perImageProbs', () => {
    const galleryVerdict = makeFlaggedVerdict({
      imageSensitive: true,
      textSensitive: true,
      titleSensitive: true,
      descriptionSensitive: true,
      sensitiveImageIndices: [1],
      probabilities: {
        prob: 0.91,
        goreProb: 0.9,
        nsfwProb: 0.02,
        // Suppose maxImages=2 scanned only indices 0 and 1, while gallery has 4 total images (indices 0..3)
        perImageProbs: [0.03, 0.9],
      },
    });

    // Subdivided detail view: never redacts text; safe cover (0) stays 'none', flagged shot (1) is 'blur',
    // and unscanned shots (index >= perImageProbs.length: 2, 3) are blurred defensively!
    const detailSubdivided = resolveDetailFilterState({
      config: makeEnabledConfig({ flaggedAction: 'hide_card', subdivideModalities: true }),
      scope: 'workshop',
      verdict: galleryVerdict,
      totalImages: 4,
    });
    expect(detailSubdivided.flagged).toBe(true);
    expect(detailSubdivided.redactText).toBe(false);
    expect(detailSubdivided.coverMode).toBe('none');
    expect(detailSubdivided.imageModeAt(0)).toBe('none');
    expect(detailSubdivided.imageModeAt(1)).toBe('blur');
    expect(detailSubdivided.imageModeAt(2)).toBe('blur');
    expect(detailSubdivided.imageModeAt(3)).toBe('blur');

    // Non-subdivided detail view: all images (0, 1, 2, 3) are blurred while text remains unredacted
    const detailCoarse = resolveDetailFilterState({
      config: makeEnabledConfig({ flaggedAction: 'hide_card', subdivideModalities: false }),
      scope: 'workshop',
      verdict: galleryVerdict,
      totalImages: 4,
    });
    expect(detailCoarse.redactText).toBe(false);
    expect(detailCoarse.coverMode).toBe('blur');
    expect(detailCoarse.imageModeAt(0)).toBe('blur');
    expect(detailCoarse.imageModeAt(1)).toBe('blur');
    expect(detailCoarse.imageModeAt(2)).toBe('blur');
    expect(detailCoarse.imageModeAt(3)).toBe('blur');
  });

  it('applies all 5 progressive Library Exclusion display levels (even when filter.enabled is false) and invalidates when fingerprintValid is false', () => {
    const verdict = makeFlaggedVerdict();
    const config = makeEnabledConfig({ flaggedAction: 'hide_card' });
    const disabledConfig = makeEnabledConfig({
      enabled: false,
      applyToLibrary: false,
      flaggedAction: 'hide_card',
    });

    const expectedByLevel: Record<
      ExclusionDisplayLevel,
      { coverMode: string; redactTitle: boolean; redactMeta: boolean; detailRedactText: boolean }
    > = {
      normal: { coverMode: 'none', redactTitle: false, redactMeta: false, detailRedactText: false },
      lazy_load: {
        coverMode: 'lazy',
        redactTitle: false,
        redactMeta: false,
        detailRedactText: false,
      },
      blur_image: {
        coverMode: 'blur',
        redactTitle: false,
        redactMeta: false,
        detailRedactText: false,
      },
      hide_image: {
        coverMode: 'hide',
        redactTitle: false,
        redactMeta: false,
        detailRedactText: false,
      },
      hide_text: {
        coverMode: 'hide',
        redactTitle: true,
        redactMeta: true,
        detailRedactText: true,
      },
    };

    for (const [level, expected] of Object.entries(expectedByLevel) as Array<
      [ExclusionDisplayLevel, (typeof expectedByLevel)[ExclusionDisplayLevel]]
    >) {
      const excl = makeExclusion(level, true);
      for (const cfg of [config, disabledConfig]) {
        const card = resolveCardFilterState({
          config: cfg,
          scope: 'library',
          verdict,
          exclusion: excl,
        });
        expect(card.exclusionActive).toBe(true);
        expect(card.exclusionInvalidated).toBe(false);
        expect(card.hiddenCard).toBe(false);
        expect(card.flagged).toBe(false);
        expect(card.coverMode).toBe(expected.coverMode);
        expect(card.redactTitle).toBe(expected.redactTitle);
        expect(card.redactMeta).toBe(expected.redactMeta);

        const detail = resolveDetailFilterState({
          config: cfg,
          scope: 'library',
          verdict,
          exclusion: excl,
        });
        expect(detail.exclusionActive).toBe(true);
        expect(detail.coverMode).toBe(expected.coverMode);
        expect(detail.mapCoverMode).toBe(expected.coverMode);
        expect(detail.redactText).toBe(expected.detailRedactText);
      }
    }

    // Expired fingerprint (fingerprintValid === false) invalidates exclusion and falls through to automatic filter!
    const invalidExcl = makeExclusion('normal', false);
    const invalidCard = resolveCardFilterState({
      config,
      scope: 'library',
      verdict,
      exclusion: invalidExcl,
    });
    expect(invalidCard.exclusionActive).toBe(false);
    expect(invalidCard.exclusionInvalidated).toBe(true);
    expect(invalidCard.flagged).toBe(true);
    expect(invalidCard.hiddenCard).toBe(true);

    const invalidDetail = resolveDetailFilterState({
      config,
      scope: 'library',
      verdict,
      exclusion: invalidExcl,
    });
    expect(invalidDetail.exclusionActive).toBe(false);
    expect(invalidDetail.exclusionInvalidated).toBe(true);
    expect(invalidDetail.flagged).toBe(true);
    expect(invalidDetail.coverMode).toBe('blur');
    expect(invalidDetail.redactText).toBe(false);
  });

  it('never allows bundle.coverImage to bypass member filterMode on collapsed bundles', () => {
    const makeAddon = (id: string, coverImage?: string): AddonSummary => ({
      id,
      workshopId: id.replace('ws:', ''),
      title: `Title ${id}`,
      installState: 'installed',
      enabled: true,
      fileSize: 1024,
      filesCount: 1,
      categories: ['Campaign'],
      maps: [{ code: 'c1m1', name: 'Chapter 1' }],
      hasDummyBypass: false,
      remoteStatus: 'active',
      updateAvailable: false,
      addonInfo: {},
      coverImage,
    });

    const memberWithCover = makeAddon('ws:100', 'https://img.example.com/member100.jpg');
    const memberWithoutCover = makeAddon('ws:200', undefined);
    const flaggedVerdict100 = makeFlaggedVerdict({
      targetKey: 'addon:ws:100',
      imageSensitive: true,
      textSensitive: true,
      titleSensitive: true,
    });
    const flaggedVerdict200 = makeFlaggedVerdict({
      targetKey: 'addon:ws:200',
      imageSensitive: true,
      textSensitive: true,
      titleSensitive: true,
    });

    // Case 1: Member has cover AND bundle.coverImage is populated -> still enforces 'hide' & redactTitle!
    const presWithMemberCover = resolveBundleCoverPresentation(
      [memberWithCover],
      {
        filterConfig: makeEnabledConfig({ flaggedAction: 'hide_image', subdivideModalities: true }),
        verdicts: { 'addon:ws:100': flaggedVerdict100 },
        evaluatedRevisions: { 'addon:ws:100': 'r1' },
        exclusionsByAddonId: new Map(),
      },
      { bundleCoverImage: 'https://img.example.com/explicit_bundle_cover.jpg' },
    );
    expect(presWithMemberCover.coverMode).toBe('hide');
    expect(presWithMemberCover.redactTitle).toBe(true);

    // Case 2: Members have NO individual cover, falling back to bundle.coverImage -> still enforces strictest member coverMode ('hide' / 'blur')!
    const presFallbackBundleCover = resolveBundleCoverPresentation(
      [memberWithoutCover],
      {
        filterConfig: makeEnabledConfig({ enabled: false }),
        verdicts: { 'addon:ws:200': flaggedVerdict200 },
        evaluatedRevisions: {},
        exclusionsByAddonId: new Map([['ws:200', { ...makeExclusion('blur_image'), addonId: 'ws:200' }]]),
      },
      { bundleCoverImage: 'https://img.example.com/explicit_bundle_cover.jpg' },
    );
    expect(presFallbackBundleCover.src).toBe('https://img.example.com/explicit_bundle_cover.jpg');
    expect(presFallbackBundleCover.coverMode).toBe('blur');
  });

  it('matches Rust FilterPresetId calibration math, deduplicates Workshop gallery URLs, and tracks policy signatures', () => {
    const identity = getPresetDefaultParams('recommended');
    const raw = {
      prob: 0.721234,
      goreProb: 0.013712,
      nsfwProb: 0.718001,
      perImageProbs: [0.719, 0.018],
    };
    expect(adjustProbabilities(identity, raw)).toEqual(raw);

    const nsfwOnly = getPresetDefaultParams('nsfw_only');
    const [ov, g, n] = adjustTriplet(nsfwOnly, 0.95, 0.94, 0.002);
    expect(g).toBe(0);
    expect(ov).toBeLessThan(0.01);
    expect(Math.abs(ov - n)).toBeLessThan(1e-5);

    const urls = collectWorkshopImageUrls({
      workshopId: '1',
      fileType: 'item',
      status: 'active',
      title: 'Gallery Test',
      previewUrl: 'https://img.example.com/0.jpg',
      previews: [
        { url: 'https://img.example.com/0.jpg', previewType: 0 },
        { url: 'https://img.example.com/1.jpg', previewType: 0 },
        { youtubeVideoId: 'yt123', previewType: 1 },
        { url: 'https://img.example.com/2.jpg', previewType: 0 },
      ],
      tags: [],
      dependencies: [],
      collectionChildren: [],
      source: 'catalog_server',
      fetchedAt: '2026-11-25T00:00:00Z',
    });
    expect(urls).toEqual([
      'https://img.example.com/0.jpg',
      'https://img.example.com/1.jpg',
      'https://img.example.com/2.jpg',
    ]);

    const c1 = makeEnabledConfig({ flaggedAction: 'blur_card' });
    const c2 = makeEnabledConfig({ flaggedAction: 'hide_image' });
    const c3 = makeEnabledConfig({ flagThreshold: 0.55 });
    // Changing UI-only flaggedAction preserves cached verdict policy signature; changing threshold invalidates it!
    expect(computeFilterPolicySignature(c1)).toBe(computeFilterPolicySignature(c2));
    expect(computeFilterPolicySignature(c1)).not.toBe(computeFilterPolicySignature(c3));
  });

  it('maintains 1:1 translation key parity between zh and en dictionaries', () => {
    expect(flattenKeys(zh)).toEqual(flattenKeys(en));
  });
});
