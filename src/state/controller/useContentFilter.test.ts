import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DashboardSnapshot, FilterEvaluationTarget, FilterItemVerdict } from '../../api/ipc';
import { previewEngine } from '../../api/previewStore';
import i18n from '../../i18n';
import {
  buildAddonFilterTarget,
  computeFilterPolicySignature,
  FILTER_REMOTE_VERDICT_MAX_AGE_MS,
  resolveDetailFilterState,
} from '../../utils/contentFilter';
import { getClient, type DesktopClient } from './types';
import { useContentFilter } from './useContentFilter';

function safeVerdict(target: FilterEvaluationTarget, evaluatedAt = Date.now()): FilterItemVerdict {
  return {
    targetKey: target.targetKey,
    revisionHint: target.revisionHint,
    flagged: false,
    decisionSource: 'model_safe',
    evaluatedScore: 0.1,
    probabilities: { prob: 0.1, goreProb: 0.05, nsfwProb: 0.05 },
    textSensitive: false,
    titleSensitive: false,
    descriptionSensitive: false,
    imageSensitive: false,
    evaluatedImageIndices: [],
    sensitiveImageIndices: [],
    complete: true,
    evaluatedAt: new Date(evaluatedAt).toISOString(),
  };
}

function policy(snapshot: DashboardSnapshot, catalogScoringRevision?: string): string {
  return computeFilterPolicySignature(snapshot.config.filter, {
    catalogServerUrl: snapshot.config.catalogServerUrl,
    httpsOnly: snapshot.config.httpsOnly,
    catalogScoringRevision,
  });
}

function setup(snapshot: DashboardSnapshot, clientOverrides: Partial<DesktopClient>) {
  const snapshotRef = { current: snapshot };
  const params = {
    client: { ...getClient(false), ...clientOverrides },
    snapshotRef,
    tRef: { current: i18n.t },
    commitSnapshot: () => true,
    commitSavedConfig: () => {},
    nextSnapshotSeq: () => 1,
    notify: () => {},
    setBusy: () => {},
  };
  const hook = renderHook(
    ({ snapshot }) => useContentFilter({ ...params, snapshot }),
    { initialProps: { snapshot } },
  );
  return { ...hook, snapshotRef };
}

describe('automatic content filter lifecycle', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    previewEngine.reset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it.each(['enabled', 'applyToLibrary'] as const)(
    'discards queued targets once the saved %s switch is off',
    async (setting) => {
      const snapshot = await previewEngine.getDashboardSnapshot();
      snapshot.config.filter.enabled = true;
      snapshot.config.filter.applyToLibrary = true;
      snapshot.addons = Array.from({ length: 18 }, (_, i) => ({
        ...snapshot.addons[0],
        id: `addon-fixture-${i}`,
      }));
      snapshot.filterPolicySignature = policy(snapshot);
      const sig = snapshot.filterPolicySignature;
      let release: (() => void) | undefined;
      const evaluate = vi.fn(async () => {
        await new Promise<void>((resolve) => { release = resolve; });
        return { items: [], policySignature: sig };
      });
      const { snapshotRef } = setup(snapshot, { evaluateFilterItems: evaluate });
      await act(() => vi.advanceTimersByTimeAsync(50));
      expect(evaluate).toHaveBeenCalledTimes(1);

      snapshotRef.current = {
        ...snapshot,
        config: { ...snapshot.config, filter: { ...snapshot.config.filter, [setting]: false } },
      };
      await act(async () => { release?.(); });
      expect(evaluate).toHaveBeenCalledTimes(1);
    },
  );

  it('discards and requeues prior verdicts when a batch adopts a new Catalog policy', async () => {
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.mode = 'catalog';
    snapshot.addons = snapshot.addons.slice(0, 2);
    snapshot.filterPolicySignature = policy(snapshot, 'a');
    const existing = buildAddonFilterTarget(snapshot.addons[0]);
    snapshot.cachedFilterVerdicts = [safeVerdict(existing)];
    const nextPolicy = policy(snapshot, 'b');
    const evaluate = vi.fn(async (targets: FilterEvaluationTarget[]) => ({
      policySignature: nextPolicy,
      items: targets.map((target) => ({
        targetKey: target.targetKey,
        status: 'completed' as const,
        complete: true,
        verdict: safeVerdict(target),
      })),
    }));
    const { result } = setup(snapshot, { evaluateFilterItems: evaluate });
    expect(result.current.filterVerdicts[existing.targetKey]).toBeDefined();
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(evaluate.mock.calls[0][0].map((target) => target.targetKey)).not.toContain(existing.targetKey);
    expect(result.current.filterVerdicts[existing.targetKey]).toBeUndefined();
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(evaluate.mock.calls[1][0].map((target) => target.targetKey)).toEqual([existing.targetKey]);
  });

  it('expires an in-memory remote verdict and rejects events from before a cache clear', async () => {
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.mode = 'custom_endpoint';
    snapshot.addons = snapshot.addons.slice(0, 1);
    snapshot.filterPolicySignature = policy(snapshot);
    const target = buildAddonFilterTarget(snapshot.addons[0]);
    const saved = safeVerdict(target, Date.now() - FILTER_REMOTE_VERDICT_MAX_AGE_MS + 100);
    snapshot.cachedFilterVerdicts = [saved];
    const sig = snapshot.filterPolicySignature;
    const evaluate = vi.fn(async (targets: FilterEvaluationTarget[]) => ({
      policySignature: sig,
      items: targets.map((item) => ({
        targetKey: item.targetKey,
        status: 'completed' as const,
        complete: true,
        verdict: safeVerdict(item),
      })),
    }));
    const { result } = setup(snapshot, {
      evaluateFilterItems: evaluate,
      clearFilterCache: async () => ({ count: 1, cacheGeneration: 1 }),
    });
    expect(result.current.filterVerdicts[target.targetKey]).toEqual(saved);
    await act(() => vi.advanceTimersByTimeAsync(100));
    expect(result.current.filterVerdicts[target.targetKey]).toBeUndefined();
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(evaluate).toHaveBeenCalledTimes(1);
    expect(result.current.filterVerdicts[target.targetKey]).toBeDefined();

    await act(async () => { await result.current.clearFilterCache(); });
    act(() => result.current.handleFilterVerdictsUpdated([safeVerdict(target)], sig, 0));
    expect(result.current.filterVerdicts[target.targetKey]).toBeUndefined();
  });

  it('retains partial image findings in details while rechecking the target', async () => {
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.flaggedAction = 'mark_only';
    snapshot.addons = [snapshot.addons[0]];
    const target = buildAddonFilterTarget(snapshot.addons[0]);
    const { targetKey } = target;
    const sig = policy(snapshot);
    const partial: FilterItemVerdict = {
      ...safeVerdict(target),
      flagged: true,
      decisionSource: 'model_direct_flag',
      imageSensitive: true,
      evaluatedImageIndices: [0, 1],
      sensitiveImageIndices: [0],
    };
    const evaluate = vi.fn<DesktopClient['evaluateFilterItems']>()
      .mockResolvedValueOnce({
        policySignature: sig,
        items: [{ targetKey, status: 'completed', complete: false, verdict: partial }],
      })
      .mockResolvedValueOnce({
        policySignature: sig,
        items: [{ targetKey, status: 'completed', complete: true, verdict: safeVerdict(target) }],
      });
    const { result } = setup(snapshot, { evaluateFilterItems: evaluate });
    const detail = () => resolveDetailFilterState({
      config: snapshot.config.filter,
      scope: 'library',
      verdict: result.current.filterVerdicts[targetKey],
      hasTargetFailed: result.current.failedTargetKeys.has(targetKey),
    });

    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(result.current.filterVerdicts[targetKey].complete).toBe(false);
    expect(result.current.evaluatedRevisions[targetKey]).toBeUndefined();
    expect(detail()).toMatchObject({ failureDisplay: 'notice', flagged: false, coverMode: 'blur' });
    expect(detail().imageModeAt(1)).toBe('none');
    expect(detail().imageModeAt(2)).toBe('blur');

    act(() => result.current.retryFailedEvaluations());
    expect(detail()).toMatchObject({ pendingEvaluation: true, coverMode: 'blur' });
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(evaluate).toHaveBeenCalledTimes(2);
    expect(result.current.evaluatedRevisions[targetKey]).toBe(target.revisionHint);
    expect(detail()).toMatchObject({ pendingEvaluation: false, coverMode: 'none' });

    act(() => result.current.handleFilterVerdictsUpdated([{
      ...partial,
      complete: false,
      evaluatedAt: new Date().toISOString(),
    }], sig));
    expect(result.current.filterVerdicts[targetKey].complete).toBe(true);
  });

  it('clears failedTargetKeys and revealedCardIds for an addon when its revision changes', async () => {
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.mode = 'local';
    snapshot.config.filter.flaggedAction = 'blur_card';
    snapshot.addons = [snapshot.addons[0]];
    const target = buildAddonFilterTarget(snapshot.addons[0]);
    const sig = policy(snapshot);

    let failNext = true;
    const evaluate = vi.fn(async (targets: FilterEvaluationTarget[]) => {
      if (failNext) {
        return {
          policySignature: sig,
          cacheGeneration: 0,
          items: targets.map((item) => ({
            targetKey: item.targetKey,
            status: 'failed' as const,
            error: 'temporary error',
          })),
        };
      }
      return new Promise<never>(() => {});
    });

    const { result, rerender, snapshotRef } = setup(snapshot, { evaluateFilterItems: evaluate });
    await act(() => vi.advanceTimersByTimeAsync(100));
    expect(result.current.failedTargetKeys.has(target.targetKey)).toBe(true);

    act(() => result.current.revealCard(target.targetKey));
    expect(result.current.revealedCardIds.has(target.targetKey)).toBe(true);

    // Update the addon's contentHash (simulating an update on disk) -> stale failed/revealed flags must clear
    failNext = false;
    const updatedSnapshot: DashboardSnapshot = {
      ...snapshot,
      addons: [{ ...snapshot.addons[0], contentHash: 'updated-v2-hash' }],
    };
    snapshotRef.current = updatedSnapshot;
    rerender({ snapshot: updatedSnapshot });

    expect(result.current.failedTargetKeys.has(target.targetKey)).toBe(false);
    expect(result.current.revealedCardIds.has(target.targetKey)).toBe(false);
  });
});
