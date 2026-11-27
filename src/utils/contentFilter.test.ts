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
  addonFilterKey,
  adjustProbabilities,
  adjustTriplet,
  buildAddonFilterTarget,
  buildAddonRevisionHint,
  buildWorkshopFilterTarget,
  collectWorkshopImageUrls,
  computeFilterPolicySignature,
  createDefaultFilterConfig,
  formatDiagnosticVerdictBadge,
  getAddonSearchRedaction,
  getFilteredAddonFallbackCover,
  getPresetDefaultParams,
  isAddonTitleRedacted,
  isLinkedWorkshopTitleRedacted,
  isWorkshopKeyTitleRedacted,
  resolveBundleCoverPresentation,
  resolveCardFilterState,
  resolveDetailFilterState,
  resolveLookupVerdict,
  resolveWorkshopVerdict,
  workshopFilterKey,
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
    evaluatedImageIndices: [0, 1],
    sensitiveImageIndices: [0],
    complete: true,
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

    const failedBlurCard = resolveCardFilterState({
      config: makeEnabledConfig({ flaggedAction: 'blur_card' }),
      scope: 'library',
      verdict: undefined,
      hasTargetFailed: true,
      hasContent: true,
    });
    expect(failedBlurCard.pendingEvaluation).toBe(false);
    expect(failedBlurCard.failureDisplay).toBe('notice');
    expect(failedBlurCard.flagged).toBe(false);
    expect(failedBlurCard.coverMode).toBe('hide');
    expect(failedBlurCard.blurWholeCard).toBe(true);

    const failedDetail = resolveDetailFilterState({
      config: makeEnabledConfig({ flaggedAction: 'hide_image' }),
      scope: 'workshop',
      verdict: undefined,
      hasTargetFailed: true,
      hasContent: true,
    });
    expect(failedDetail.pendingEvaluation).toBe(false);
    expect(failedDetail.failureDisplay).toBe('notice');
    expect(failedDetail.flagged).toBe(false);
    expect(failedDetail.coverMode).toBe('blur');
    expect(failedDetail.imageModeAt(0)).toBe('blur');
  });

  it('hides failed cards only when they are configured to follow the hide-card action', () => {
    const options = {
      scope: 'library' as const,
      verdict: makeFlaggedVerdict({ complete: false }),
    };
    const config = makeEnabledConfig({ flaggedAction: 'hide_card' });

    expect(resolveCardFilterState({ ...options, config })).toMatchObject({
      flagged: false,
      hiddenCard: false,
      blurWholeCard: true,
      failureDisplay: 'notice',
    });

    const asFlagged = makeEnabledConfig({ flaggedAction: 'hide_card', failureDisplay: 'as_flagged' });
    expect(resolveCardFilterState({ ...options, config: asFlagged })).toMatchObject({
      flagged: false,
      hiddenCard: true,
      failureDisplay: 'as_flagged',
    });
    expect(resolveCardFilterState({ ...options, config: asFlagged, forceShowHiddenCards: true }))
      .toMatchObject({ hiddenCard: false, blurWholeCard: true });
  });

  it('enforces image-only blurring in Detail view, maps evaluatedImageIndices to original gallery slots, and blurs unscanned or failed-to-load slots', () => {
    const galleryVerdict = makeFlaggedVerdict({
      imageSensitive: true,
      textSensitive: true,
      titleSensitive: true,
      descriptionSensitive: true,
      evaluatedImageIndices: [0, 1],
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
    // and unscanned shots (2, 3) are blurred defensively!
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

    // Original gallery slot mapping when slot 0 failed to load and slot 3 exceeded maxImages:
    // evaluatedImageIndices: [1, 2], sensitiveImageIndices: [2] -> blurs slots 0, 2, 3 and leaves slot 1 clear!
    const sparseSlotVerdict = makeFlaggedVerdict({
      imageSensitive: true,
      evaluatedImageIndices: [1, 2],
      sensitiveImageIndices: [2],
      probabilities: {
        prob: 0.92,
        goreProb: 0.91,
        nsfwProb: 0.03,
        perImageProbs: [0.02, 0.91],
      },
    });
    const detailSparseSlots = resolveDetailFilterState({
      config: makeEnabledConfig({ flaggedAction: 'blur_card', subdivideModalities: true }),
      scope: 'workshop',
      verdict: sparseSlotVerdict,
      totalImages: 4,
    });
    expect(detailSparseSlots.coverMode).toBe('blur');
    expect(detailSparseSlots.imageModeAt(0)).toBe('blur');
    expect(detailSparseSlots.imageModeAt(1)).toBe('none');
    expect(detailSparseSlots.imageModeAt(2)).toBe('blur');
    expect(detailSparseSlots.imageModeAt(3)).toBe('blur');

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

  it('omits bundle.coverImage when owned by a hiddenCard member or unverified, and enforces the owning visible member filterMode', () => {
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
    const safeMemberWithoutCover = makeAddon('ws:200', undefined);
    const hiddenOwnerMember = makeAddon('ws:300', 'https://img.example.com/hidden_nsfw.jpg');
    const visibleOwnerMember = makeAddon('ws:400', 'https://img.example.com/visible_blurred.jpg');

    const flaggedVerdict100 = makeFlaggedVerdict({
      targetKey: 'addon:ws:100',
      imageSensitive: true,
      textSensitive: true,
      titleSensitive: true,
    });
    const hiddenVerdict300 = makeFlaggedVerdict({
      targetKey: 'addon:ws:300',
      imageSensitive: true,
    });

    // Case 1: Visible member has cover -> enforces member's 'hide' & redactTitle!
    const presWithMemberCover = resolveBundleCoverPresentation(
      [memberWithCover],
      {
        filterConfig: makeEnabledConfig({ flaggedAction: 'hide_image', subdivideModalities: true }),
        verdicts: { 'addon:ws:100': flaggedVerdict100 },
        evaluatedRevisions: { 'addon:ws:100': 'r1' },
        exclusionsByAddonId: new Map(),
      },
      {
        allMembers: [memberWithCover],
        bundleCoverImage: 'https://img.example.com/explicit_bundle_cover.jpg',
      },
    );
    expect(presWithMemberCover.coverMode).toBe('hide');
    expect(presWithMemberCover.redactTitle).toBe(true);

    const bundleWithFailedMember = resolveBundleCoverPresentation(
      [memberWithCover, safeMemberWithoutCover],
      {
        filterConfig: makeEnabledConfig({ flaggedAction: 'hide_image' }),
        verdicts: { 'addon:ws:100': flaggedVerdict100 },
        failedTargetKeys: new Set(['addon:ws:200']),
        exclusionsByAddonId: new Map(),
      },
    );
    expect(bundleWithFailedMember).toMatchObject({
      ownerMember: memberWithCover,
      coverMode: 'hide',
      failureDisplay: 'notice',
      coverEvaluation: { pendingEvaluation: false, failureDisplay: undefined },
    });

    // Case 2: bundle.coverImage belongs to a hiddenCard member -> omitted completely!
    const presHiddenOwner = resolveBundleCoverPresentation(
      [safeMemberWithoutCover],
      {
        filterConfig: makeEnabledConfig({ flaggedAction: 'hide_card' }),
        verdicts: { 'addon:ws:300': hiddenVerdict300 },
        evaluatedRevisions: { 'addon:ws:200': 'r1', 'addon:ws:300': 'r1' },
        exclusionsByAddonId: new Map(),
      },
      {
        allMembers: [hiddenOwnerMember, safeMemberWithoutCover],
        bundleCoverImage: 'https://img.example.com/hidden_nsfw.jpg',
      },
    );
    expect(presHiddenOwner.src).toBeUndefined();
    expect(presHiddenOwner.fallbackSrc).toBeUndefined();

    // Case 3: bundle.coverImage cannot be verified against any member in allMembers -> omitted completely!
    const presUnverified = resolveBundleCoverPresentation(
      [safeMemberWithoutCover],
      {
        filterConfig: makeEnabledConfig({ flaggedAction: 'hide_card' }),
        verdicts: {},
        evaluatedRevisions: { 'addon:ws:200': 'r1' },
        exclusionsByAddonId: new Map(),
      },
      {
        allMembers: [safeMemberWithoutCover],
        bundleCoverImage: 'https://img.example.com/orphan_unverified.jpg',
      },
    );
    expect(presUnverified.src).toBeUndefined();
    expect(presUnverified.fallbackSrc).toBeUndefined();

    // Case 4: bundle.coverImage is owned by a visible member in allMembers -> uses that exact owner's coverMode ('blur')!
    const presVisibleOwner = resolveBundleCoverPresentation(
      [safeMemberWithoutCover],
      {
        filterConfig: makeEnabledConfig({ enabled: false }),
        verdicts: {},
        evaluatedRevisions: {},
        exclusionsByAddonId: new Map([
          ['ws:400', { ...makeExclusion('blur_image'), addonId: 'ws:400' }],
        ]),
      },
      {
        allMembers: [visibleOwnerMember, safeMemberWithoutCover],
        bundleCoverImage: 'https://img.example.com/visible_blurred.jpg',
      },
    );
    expect(presVisibleOwner.src).toBe('https://img.example.com/visible_blurred.jpg');
    expect(presVisibleOwner.ownerMember?.id).toBe('ws:400');
    expect(presVisibleOwner.coverMode).toBe('blur');

    const hiddenAuthor = { ...memberWithCover, id: 'ws:2814104912', author: 'Redacted author' };
    const publicAuthor = { ...safeMemberWithoutCover, author: 'Visible author' };
    const authorCtx = {
      filterConfig: makeEnabledConfig(),
      verdicts: {},
      exclusionsByAddonId: new Map([[hiddenAuthor.id, makeExclusion('hide_text')]]),
    };
    expect(resolveBundleCoverPresentation([hiddenAuthor, publicAuthor], authorCtx).author)
      .toBe('Visible author');
    expect(resolveBundleCoverPresentation([hiddenAuthor, safeMemberWithoutCover], authorCtx).author)
      .toBeUndefined();

    const withFallback = {
      ...memberWithCover,
      workshopTitle: 'Workshop title',
      addonTitle: 'VPK title',
      currentFileName: 'Alternate filename.vpk.disabled',
      localCoverImage: 'local-cover.jpg',
    };
    const target = buildAddonFilterTarget(withFallback);
    expect(target.title.split('\n')).toEqual([
      'Workshop title', 'VPK title', withFallback.title, 'Alternate filename',
    ]);
    expect(buildAddonRevisionHint({ ...withFallback, addonTitle: 'Changed title' }))
      .not.toBe(target.revisionHint);
    expect(target.imageSources).toEqual([withFallback.coverImage, withFallback.localCoverImage]);

    const safe = makeFlaggedVerdict({
      targetKey: target.targetKey,
      revisionHint: target.revisionHint,
      flagged: false,
      evaluatedImageIndices: [0],
    });
    const coverCtx = { ...authorCtx, verdicts: { [target.targetKey]: safe } };
    expect(getFilteredAddonFallbackCover(withFallback, coverCtx)).toBeUndefined();
    safe.evaluatedImageIndices = [0, 1];
    expect(getFilteredAddonFallbackCover(withFallback, coverCtx)).toBe(withFallback.localCoverImage);
  });

  it('matches Rust FilterPresetId calibration math, deduplicates Workshop gallery URLs, and computes canonical pol_<32hex> policy signatures', () => {
    const identity = getPresetDefaultParams('recommended');
    const raw = {
      prob: 0.721234,
      goreProb: 0.013712,
      nsfwProb: 0.718001,
      perImageProbs: [0.719, 0.018],
      scoringRevision: 'rev1',
    };
    expect(adjustProbabilities(identity, raw)).toEqual(raw);

    const nsfwOnly = getPresetDefaultParams('nsfw_only');
    const [ov, g, n] = adjustTriplet(nsfwOnly, 0.95, 0.94, 0.002);
    expect(g).toBe(0);
    expect(ov).toBeLessThan(0.01);
    expect(Math.abs(ov - n)).toBeLessThan(1e-5);
    expect(adjustProbabilities(nsfwOnly, raw).scoringRevision).toBe('rev1');

    const workshopTarget = {
      workshopId: '1',
      fileType: 'item' as const,
      status: 'active' as const,
      title: 'Gallery Test',
      description: 'English description',
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
      source: 'catalog_server' as const,
      fetchedAt: '2026-11-25T00:00:00Z',
    };
    const urls = collectWorkshopImageUrls(workshopTarget);
    expect(urls).toEqual([
      'https://img.example.com/0.jpg',
      'https://img.example.com/1.jpg',
      'https://img.example.com/2.jpg',
    ]);
    const evaluated = buildWorkshopFilterTarget(workshopTarget);
    const verdict = makeFlaggedVerdict({
      targetKey: evaluated.targetKey,
      revisionHint: evaluated.revisionHint,
    });
    const lookup = { verdicts: { [evaluated.targetKey]: verdict } };
    expect(resolveWorkshopVerdict(workshopTarget, lookup)).toBe(verdict);
    expect(resolveWorkshopVerdict({ ...workshopTarget, description: '新描述' }, lookup)).toBeUndefined();
    expect(resolveWorkshopVerdict({ ...workshopTarget, tags: ['Campaign'] }, lookup)).toBeUndefined();

    const c1 = makeEnabledConfig({ flaggedAction: 'blur_card' });
    const c2 = makeEnabledConfig({ flaggedAction: 'hide_image' });
    const c3 = makeEnabledConfig({ flagThreshold: 0.55 });
    const sig1 = computeFilterPolicySignature(c1);
    expect(sig1).toMatch(/^pol_[0-9a-f]{32}$/);
    // Changing UI-only flaggedAction preserves cached verdict policy signature; changing threshold invalidates it!
    expect(sig1).toBe(computeFilterPolicySignature(c2));
    expect(sig1).not.toBe(computeFilterPolicySignature(c3));

    const catCfg = makeEnabledConfig({ mode: 'catalog' });
    const catSigR1 = computeFilterPolicySignature(catCfg, {
      catalogServerUrl: 'https://cat.example/',
      httpsOnly: true,
      catalogScoringRevision: 'r1',
    });
    const catSigR2 = computeFilterPolicySignature(catCfg, {
      catalogServerUrl: 'https://cat.example',
      httpsOnly: true,
      catalogScoringRevision: 'r2',
    });
    expect(catSigR1).toMatch(/^pol_[0-9a-f]{32}$/);
    expect(catSigR1).not.toBe(catSigR2);
  });

  it('redacts pending and failed unrevealed titles under blur_card/hide_card and preserves mixed bundle blur', () => {
    const makeAddon = (overrides: Partial<AddonSummary> & { id: string }): AddonSummary => ({
      title: `Title ${overrides.id}`,
      installState: 'installed',
      enabled: true,
      fileSize: 1024,
      filesCount: 1,
      categories: ['Campaign'],
      maps: [],
      hasDummyBypass: false,
      remoteStatus: 'active',
      updateAvailable: false,
      addonInfo: {},
      ...overrides,
    });
    const blurCfg = makeEnabledConfig({ flaggedAction: 'blur_card' });
    const pendingAddon = makeAddon({
      id: 'addon-pending',
      workshopId: '991001',
      title: 'Unverified Sensitive Title',
    });
    const flaggedAddon = makeAddon({
      id: 'addon-flagged',
      workshopId: '991002',
      title: 'Confirmed Flagged Title',
      coverImage: 'https://img.example.com/flagged.jpg',
    });

    const flaggedVerdict = makeFlaggedVerdict({
      targetKey: addonFilterKey(flaggedAddon.id),
      revisionHint: buildAddonRevisionHint(flaggedAddon),
    });

    const pendingCtx = {
      filterConfig: blurCfg,
      verdicts: { [addonFilterKey(flaggedAddon.id)]: flaggedVerdict },
      exclusionsByAddonId: new Map(),
      revealedCardIds: new Set<string>(),
      failedTargetKeys: new Set<string>(),
    };

    // Pending addon under blur_card must redact title/description/meta in unblurred consumers & search
    expect(getAddonSearchRedaction(pendingAddon, pendingCtx)).toEqual({
      redactTitle: true,
      redactDescription: true,
      redactMeta: true,
    });
    expect(isAddonTitleRedacted(pendingAddon, pendingCtx)).toBe(true);
    expect(isWorkshopKeyTitleRedacted('991009', 'Pending Workshop Title', pendingCtx)).toBe(true);

    // Failed unrevealed addon under blur_card is redacted until revealed
    const failedCtx = {
      ...pendingCtx,
      failedTargetKeys: new Set([addonFilterKey(pendingAddon.id), workshopFilterKey('991009')]),
    };
    expect(isAddonTitleRedacted(pendingAddon, failedCtx)).toBe(true);
    expect(isWorkshopKeyTitleRedacted('991009', 'Failed Workshop Title', failedCtx)).toBe(true);

    const revealedFailedCtx = {
      ...failedCtx,
      revealedCardIds: new Set([addonFilterKey(pendingAddon.id), workshopFilterKey('991009')]),
    };
    expect(isAddonTitleRedacted(pendingAddon, revealedFailedCtx)).toBe(false);
    expect(isWorkshopKeyTitleRedacted('991009', 'Failed Workshop Title', revealedFailedCtx)).toBe(
      false,
    );

    // Mixed flagged + pending members in a collapsed bundle must remain blurred and non-revealable (pendingEvaluation=true)
    const mixedBundle = resolveBundleCoverPresentation([flaggedAddon, pendingAddon], pendingCtx);
    expect(mixedBundle.blurWholeCard).toBe(true);
    expect(mixedBundle.pendingEvaluation).toBe(true);
    expect(mixedBundle.coverMode).toBe('hide');

    const checkedBundle = resolveBundleCoverPresentation([flaggedAddon, pendingAddon], {
      ...failedCtx,
      verdicts: { [addonFilterKey(flaggedAddon.id)]: { ...flaggedVerdict, flagged: false } },
    });
    expect(checkedBundle).toMatchObject({
      blurWholeCard: false,
      coverMode: 'none',
      redactTitle: true,
      failureDisplay: 'notice',
    });
  });

  it('handles optional lookup sets and linked workshop/library redaction transitions safely', () => {
    const blurCfg = {
      ...createDefaultFilterConfig(),
      enabled: true,
      applyToLibrary: true,
      applyToWorkshop: true,
      flaggedAction: 'blur_card' as const,
    };

    // Optional revealedCardIds / failedTargetKeys omitted must not throw
    const minimalCtx = {
      filterConfig: blurCfg,
      verdicts: {},
      exclusionsByAddonId: new Map(),
    };
    expect(isWorkshopKeyTitleRedacted('992001', 'Pending Title', minimalCtx)).toBe(true);

    const downloadedAddon: AddonSummary = {
      id: 'ws:992001',
      workshopId: '992001',
      title: 'Verified Safe Mod',
      workshopTitle: 'Verified Safe Mod',
      addonTitle: 'Verified Safe Mod',
      installState: 'installed',
      enabled: true,
      fileSize: 1024,
      filesCount: 1,
      primaryDir: 'workshop',
      categories: [],
      maps: [],
      hasDummyBypass: false,
      remoteStatus: 'active',
      updateAvailable: false,
      addonInfo: {},
      contentHash: 'hash-v1',
    };
    const safeWsVerdict: FilterItemVerdict = {
      targetKey: workshopFilterKey('992001'),
      flagged: false,
      decisionSource: 'model_safe',
      evaluatedScore: 0.02,
      probabilities: { prob: 0.02, goreProb: 0.01, nsfwProb: 0.01 },
      textSensitive: false,
      titleSensitive: false,
      descriptionSensitive: false,
      imageSensitive: false,
      evaluatedImageIndices: [],
      sensitiveImageIndices: [],
      complete: true,
      evaluatedAt: '2026-11-26T00:00:00Z',
    };
    const downloadCtx = {
      ...minimalCtx,
      verdicts: { [workshopFilterKey('992001')]: safeWsVerdict },
    };
    // Fresh download already verified safe in Workshop should not falsely redact matching toast title
    expect(
      isLinkedWorkshopTitleRedacted('992001', downloadedAddon, 'Verified Safe Mod', downloadCtx, {
        transferKind: 'download',
      }),
    ).toBe(false);
    // If local VPK title differs from verified Workshop title, still shield until library eval runs
    expect(
      isLinkedWorkshopTitleRedacted('992001', downloadedAddon, 'Different VPK Title', downloadCtx, {
        transferKind: 'download',
      }),
    ).toBe(true);

    // applyToLibrary=false + applyToWorkshop=true must still redact pending workshop titles
    const wsOnlyCtx = {
      filterConfig: { ...blurCfg, applyToLibrary: false, applyToWorkshop: true },
      verdicts: {},
      exclusionsByAddonId: new Map(),
    };
    expect(
      isLinkedWorkshopTitleRedacted('992001', downloadedAddon, 'Verified Safe Mod', wsOnlyCtx),
    ).toBe(true);

    // Post-update stale addon verdict (mismatched revisionHint) must be treated as pending & redacted
    const staleSafeAddonVerdict: FilterItemVerdict = {
      ...safeWsVerdict,
      targetKey: addonFilterKey(downloadedAddon.id),
      revisionHint: 'v2:old-hash-revision',
    };
    const updateCtx = {
      ...minimalCtx,
      verdicts: { [addonFilterKey(downloadedAddon.id)]: staleSafeAddonVerdict },
    };
    expect(
      isLinkedWorkshopTitleRedacted('992001', downloadedAddon, 'Verified Safe Mod', updateCtx, {
        transferKind: 'update',
      }),
    ).toBe(true);

    // Safe local addon verdict must NOT trust a different unverified upstream rawTitle when applyToWorkshop=true
    const currentSafeAddonVerdict: FilterItemVerdict = {
      ...safeWsVerdict,
      targetKey: addonFilterKey(downloadedAddon.id),
      revisionHint: buildAddonRevisionHint(downloadedAddon),
    };
    const safeLocalOnlyCtx = {
      ...minimalCtx,
      verdicts: { [addonFilterKey(downloadedAddon.id)]: currentSafeAddonVerdict },
    };
    expect(
      isLinkedWorkshopTitleRedacted('992001', downloadedAddon, 'Verified Safe Mod', safeLocalOnlyCtx),
    ).toBe(false);
    expect(
      isLinkedWorkshopTitleRedacted(
        '992001',
        downloadedAddon,
        'Unverified Upstream Renamed Title',
        safeLocalOnlyCtx,
      ),
    ).toBe(true);

    // Pending/failed under hide_image must redact title & description so unverified text never flashes
    const pendingHideImage = resolveCardFilterState({
      config: { ...blurCfg, flaggedAction: 'hide_image' },
      scope: 'workshop',
      verdict: undefined,
      hasTargetFailed: false,
      hasContent: true,
    });
    expect(pendingHideImage.redactTitle).toBe(true);
    expect(pendingHideImage.redactDescription).toBe(true);
    const failedHideImage = resolveCardFilterState({
      config: { ...blurCfg, flaggedAction: 'hide_image' },
      scope: 'workshop',
      verdict: undefined,
      hasTargetFailed: true,
      hasContent: true,
    });
    expect(failedHideImage.redactTitle).toBe(true);
    expect(failedHideImage.redactDescription).toBe(true);
  });

  it('shields pending/failed detail text, rejects stale library verdict revisions, redacts mixed/hidden sensitive bundle titles, and ignores pinnedCommit when trackLatest is true', () => {
    const cfgBlur = makeEnabledConfig({ flaggedAction: 'blur_card', subdivideModalities: true });
    const cfgHideImage = makeEnabledConfig({ flaggedAction: 'hide_image', subdivideModalities: true });
    const cfgHideCard = makeEnabledConfig({ flaggedAction: 'hide_card', subdivideModalities: true });

    // 1. Pending and failed unrevealed detail views must redact text (non-mark_only)
    const pendingDetail = resolveDetailFilterState({
      config: cfgBlur,
      scope: 'library',
      verdict: undefined,
      hasTargetFailed: false,
      hasContent: true,
    });
    expect(pendingDetail.pendingEvaluation).toBe(true);
    expect(pendingDetail.redactText).toBe(true);

    const failedDetailUnrevealed = resolveDetailFilterState({
      config: cfgHideImage,
      scope: 'library',
      verdict: undefined,
      hasTargetFailed: true,
      hasContent: true,
      revealed: false,
    });
    expect(failedDetailUnrevealed.redactText).toBe(true);

    const failedDetailRevealed = resolveDetailFilterState({
      config: cfgBlur,
      scope: 'library',
      verdict: undefined,
      hasTargetFailed: true,
      hasContent: true,
      revealed: true,
    });
    expect(failedDetailRevealed.redactText).toBe(false);

    // 2. Stale library addon verdict (mismatched revisionHint) must NOT be reused as safe
    const updatedAddon: AddonSummary = {
      id: 'ws:881100',
      workshopId: '881100',
      title: 'Updated Sensitive Mod',
      author: 'Sensitive Author',
      installState: 'installed',
      enabled: true,
      fileSize: 1024,
      filesCount: 1,
      categories: ['Campaign'],
      maps: [],
      hasDummyBypass: false,
      remoteStatus: 'active',
      updateAvailable: false,
      addonInfo: {},
      contentHash: 'new-hash-v2',
    };
    const staleSafeVerdict = makeFlaggedVerdict({
      targetKey: addonFilterKey(updatedAddon.id),
      revisionHint: 'v2:stale-old-revision',
      flagged: false,
    });
    const staleCtx = {
      filterConfig: cfgBlur,
      verdicts: { [addonFilterKey(updatedAddon.id)]: staleSafeVerdict },
      evaluatedRevisions: { [addonFilterKey(updatedAddon.id)]: 'v2:stale-old-revision' },
      exclusionsByAddonId: new Map(),
    };
    expect(isAddonTitleRedacted(updatedAddon, staleCtx)).toBe(true);
    expect(getAddonSearchRedaction(updatedAddon, staleCtx).redactMeta).toBe(true);

    // 3. Mixed visible bundle (1 title-redacted + 1 safe) and hidden-owner bundle must redact bundle title
    const part1Flagged: AddonSummary = {
      ...updatedAddon,
      id: 'ws:881101',
      workshopId: '881101',
      title: 'Explicit Campaign Part 1',
      contentHash: 'p1-hash',
    };
    const part2Safe: AddonSummary = {
      ...updatedAddon,
      id: 'ws:881102',
      workshopId: '881102',
      title: 'Companion Texture Pack',
      contentHash: 'p2-hash',
    };
    const flaggedTitleVerdict = makeFlaggedVerdict({
      targetKey: addonFilterKey(part1Flagged.id),
      revisionHint: buildAddonRevisionHint(part1Flagged),
      flagged: true,
      textSensitive: true,
      titleSensitive: true,
      imageSensitive: false,
    });
    const safePart2Verdict = makeFlaggedVerdict({
      targetKey: addonFilterKey(part2Safe.id),
      revisionHint: buildAddonRevisionHint(part2Safe),
      flagged: false,
    });

    const mixedHideImagePres = resolveBundleCoverPresentation(
      [part1Flagged, part2Safe],
      {
        filterConfig: cfgHideImage,
        verdicts: {
          [addonFilterKey(part1Flagged.id)]: flaggedTitleVerdict,
          [addonFilterKey(part2Safe.id)]: safePart2Verdict,
        },
        exclusionsByAddonId: new Map(),
      },
      { allMembers: [part1Flagged, part2Safe], bundleName: 'Explicit Campaign' },
    );
    expect(mixedHideImagePres.redactTitle).toBe(true);

    const hiddenPart1Pres = resolveBundleCoverPresentation(
      [part2Safe],
      {
        filterConfig: cfgHideCard,
        verdicts: {
          [addonFilterKey(part1Flagged.id)]: flaggedTitleVerdict,
          [addonFilterKey(part2Safe.id)]: safePart2Verdict,
        },
        exclusionsByAddonId: new Map(),
      },
      { allMembers: [part1Flagged, part2Safe], bundleName: 'Explicit Campaign' },
    );
    expect(hiddenPart1Pres.redactTitle).toBe(true);

    // When bundleName matches the safe visible member and not the hidden flagged member, keep title visible
    const safeBundleNamePres = resolveBundleCoverPresentation(
      [part2Safe],
      {
        filterConfig: cfgHideCard,
        verdicts: {
          [addonFilterKey(part1Flagged.id)]: flaggedTitleVerdict,
          [addonFilterKey(part2Safe.id)]: safePart2Verdict,
        },
        exclusionsByAddonId: new Map(),
      },
      { allMembers: [part1Flagged, part2Safe], bundleName: 'Companion Texture Pack' },
    );
    expect(safeBundleNamePres.redactTitle).toBe(false);

    // 4. trackLatest=true must ignore stale pinnedCommit in computeFilterPolicySignature
    const sigLatestNoPin = computeFilterPolicySignature(
      makeEnabledConfig({ mode: 'local', trackLatest: true, pinnedCommit: undefined }),
    );
    const sigLatestWithStalePin = computeFilterPolicySignature(
      makeEnabledConfig({ mode: 'local', trackLatest: true, pinnedCommit: 'abc1234567' }),
    );
    const sigPinned = computeFilterPolicySignature(
      makeEnabledConfig({ mode: 'local', trackLatest: false, pinnedCommit: 'abc1234567' }),
    );
    expect(sigLatestWithStalePin).toBe(sigLatestNoPin);
    expect(sigPinned).not.toBe(sigLatestNoPin);

    // 5. Pending hide_image addon must also redact metadata/author in getAddonSearchRedaction
    const pendingHideImageRedaction = getAddonSearchRedaction(updatedAddon, {
      filterConfig: cfgHideImage,
      verdicts: {},
      exclusionsByAddonId: new Map(),
    });
    expect(pendingHideImageRedaction).toEqual({
      redactTitle: true,
      redactDescription: true,
      redactMeta: true,
    });
  });

  it('maintains 1:1 translation key parity between zh and en dictionaries', () => {
    expect(flattenKeys(zh)).toEqual(flattenKeys(en));
  });
});
