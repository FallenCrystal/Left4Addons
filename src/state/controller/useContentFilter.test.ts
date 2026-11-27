import { StrictMode } from 'react';
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
      const releases: Array<() => void> = [];
      const evaluate = vi.fn(async (targets: FilterEvaluationTarget[]) => {
        expect(targets).toHaveLength(1);
        await new Promise<void>((resolve) => { releases.push(resolve); });
        return { items: [], policySignature: sig };
      });
      const { snapshotRef } = setup(snapshot, { evaluateFilterItems: evaluate });
      await act(() => vi.advanceTimersByTimeAsync(50));
      expect(evaluate).toHaveBeenCalledTimes(4);

      snapshotRef.current = {
        ...snapshot,
        config: { ...snapshot.config, filter: { ...snapshot.config.filter, [setting]: false } },
      };
      await act(async () => {
        for (const release of releases) release();
      });
      expect(evaluate).toHaveBeenCalledTimes(4);
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

  it('commits completed and failed cards immediately without waiting for slow in-flight cards', async () => {
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.applyToLibrary = true;
    snapshot.addons = Array.from({ length: 6 }, (_, i) => ({
      ...snapshot.addons[0],
      id: `addon-stream-${i}`,
    }));
    snapshot.filterPolicySignature = policy(snapshot);
    const sig = snapshot.filterPolicySignature;

    let releaseSlow: (() => void) | undefined;
    const evaluate = vi.fn(async (targets: FilterEvaluationTarget[]) => {
      const target = targets[0];
      if (target.targetKey === 'addon:addon-stream-0') {
        await new Promise<void>((resolve) => { releaseSlow = resolve; });
      }
      if (target.targetKey === 'addon:addon-stream-2') {
        return {
          policySignature: sig,
          items: [{ targetKey: target.targetKey, status: 'failed' as const, error: 'boom' }],
        };
      }
      return {
        policySignature: sig,
        items: [{
          targetKey: target.targetKey,
          status: 'completed' as const,
          complete: true,
          verdict: safeVerdict(target),
        }],
      };
    });

    const { result } = setup(snapshot, { evaluateFilterItems: evaluate });
    await act(() => vi.advanceTimersByTimeAsync(50));

    // Cards 1, 3, 4, 5 complete and Card 2 fails immediately while Card 0 is still blocked!
    expect(result.current.filterVerdicts['addon:addon-stream-0']).toBeUndefined();
    for (const idx of [1, 3, 4, 5]) {
      expect(result.current.filterVerdicts[`addon:addon-stream-${idx}`]).toBeDefined();
    }
    expect(result.current.failedTargetKeys.has('addon:addon-stream-2')).toBe(true);

    await act(async () => { releaseSlow?.(); });
    expect(result.current.filterVerdicts['addon:addon-stream-0']).toBeDefined();
  });

  it('evaluates urgent and front-prioritized targets ahead of queued background cards', async () => {
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.applyToLibrary = true;
    snapshot.config.filter.applyToWorkshop = true;
    snapshot.addons = Array.from({ length: 8 }, (_, i) => ({
      ...snapshot.addons[0],
      id: `addon-bg-${i}`,
    }));
    snapshot.filterPolicySignature = policy(snapshot);
    const sig = snapshot.filterPolicySignature;

    const dispatchedKeys: string[] = [];
    const resolvers = new Map<string, () => void>();
    const evaluate = vi.fn(async (targets: FilterEvaluationTarget[]) => {
      const target = targets[0];
      dispatchedKeys.push(target.targetKey);
      await new Promise<void>((resolve) => { resolvers.set(target.targetKey, resolve); });
      return {
        policySignature: sig,
        items: [{
          targetKey: target.targetKey,
          status: 'completed' as const,
          complete: true,
          verdict: safeVerdict(target),
        }],
      };
    });

    const { result } = setup(snapshot, { evaluateFilterItems: evaluate });
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(dispatchedKeys).toEqual([
      'addon:addon-bg-0',
      'addon:addon-bg-1',
      'addon:addon-bg-2',
      'addon:addon-bg-3',
    ]);

    // Promote addon-bg-7 urgently and register a front Workshop card while slots 0..3 are busy
    act(() => {
      result.current.prioritizeFilterTargets(['addon:addon-bg-7'], { urgent: true });
      result.current.registerWorkshopItems(
        [{
          workshopId: '9999001',
          fileType: 'item',
          status: 'active',
          title: 'Active Workshop Page Item',
          tags: [],
          dependencies: [],
          collectionChildren: [],
          source: 'catalog_server',
          fetchedAt: new Date().toISOString(),
        }],
        { priority: 'front' },
      );
    });

    // Free two slots -> urgent addon-bg-7 runs first, then front ws:9999001, ahead of addon-bg-4..6!
    await act(async () => {
      resolvers.get('addon:addon-bg-0')?.();
      resolvers.get('addon:addon-bg-1')?.();
    });
    expect(dispatchedKeys.slice(4, 6)).toEqual(['addon:addon-bg-7', 'ws:9999001']);
  });

  it('adopts a new Catalog policy across staggered concurrent calls without duplicating in-flight siblings', async () => {
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.mode = 'catalog';
    snapshot.addons = Array.from({ length: 3 }, (_, i) => ({
      ...snapshot.addons[0],
      id: `addon-cat-${i}`,
    }));
    const oldPolicy = policy(snapshot, 'a');
    const nextPolicy = policy(snapshot, 'b');
    snapshot.filterPolicySignature = oldPolicy;

    const resolvers = new Map<string, (pol: string) => void>();
    const evaluate = vi.fn(async (targets: FilterEvaluationTarget[]) => {
      const target = targets[0];
      const responsePolicy = await new Promise<string>((resolve) => {
        resolvers.set(`${target.targetKey}#${evaluate.mock.calls.length}`, resolve);
      });
      return {
        policySignature: responsePolicy,
        items: [{
          targetKey: target.targetKey,
          status: 'completed' as const,
          complete: true,
          verdict: safeVerdict(target),
        }],
      };
    });

    const { result } = setup(snapshot, { evaluateFilterItems: evaluate });
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(evaluate).toHaveBeenCalledTimes(3);

    // Call 1 adopts pol_b while Call 2 and Call 3 are STILL in flight.
    await act(async () => {
      resolvers.get('addon:addon-cat-0#1')?.(nextPolicy);
    });
    expect(result.current.filterVerdicts['addon:addon-cat-0']).toBeDefined();

    // Advancing past the debounce timer must NOT dispatch duplicate calls for in-flight siblings.
    await act(() => vi.advanceTimersByTimeAsync(100));
    expect(evaluate).toHaveBeenCalledTimes(3);

    // Call 2 returns pol_b (accepted), Call 3 returns stale pol_a (dropped and re-queued once).
    await act(async () => {
      resolvers.get('addon:addon-cat-1#2')?.(nextPolicy);
      resolvers.get('addon:addon-cat-2#3')?.(oldPolicy);
    });
    expect(result.current.filterVerdicts['addon:addon-cat-1']).toBeDefined();
    expect(result.current.filterVerdicts['addon:addon-cat-2']).toBeUndefined();

    // Dropped stale sibling addon-cat-2 re-dispatches cleanly under pol_b.
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(evaluate).toHaveBeenCalledTimes(4);
    expect(evaluate.mock.calls[3][0][0].targetKey).toBe('addon:addon-cat-2');
    await act(async () => {
      resolvers.get('addon:addon-cat-2#4')?.(nextPolicy);
    });
    expect(result.current.filterVerdicts['addon:addon-cat-2']).toBeDefined();
  });

  it('ignores a late failed or incomplete response once a card already completed via streaming event', async () => {
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.addons = [snapshot.addons[0]];
    const target = buildAddonFilterTarget(snapshot.addons[0]);
    const sig = policy(snapshot);
    snapshot.filterPolicySignature = sig;

    let releaseCall: (() => void) | undefined;
    const evaluate = vi.fn(async () => {
      await new Promise<void>((resolve) => { releaseCall = resolve; });
      return {
        policySignature: sig,
        items: [{ targetKey: target.targetKey, status: 'failed' as const, error: 'late error' }],
      };
    });

    const { result } = setup(snapshot, { evaluateFilterItems: evaluate });
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(evaluate).toHaveBeenCalledTimes(1);

    // Card finishes via streamed event first
    act(() => result.current.handleFilterVerdictsUpdated([safeVerdict(target)], sig, 0));
    expect(result.current.filterVerdicts[target.targetKey]).toBeDefined();

    // Late failed IPC response arrives afterwards -> must NOT mark failed or log an error
    await act(async () => { releaseCall?.(); });
    expect(result.current.failedTargetKeys.has(target.targetKey)).toBe(false);
    expect(result.current.filterEvaluationLog).toHaveLength(0);
    expect(result.current.filterVerdicts[target.targetKey]).toBeDefined();
  });

  it('preserves owner-scoped urgent and front priority across retryFailedEvaluations and cache clear', async () => {
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.applyToLibrary = true;
    snapshot.config.filter.applyToWorkshop = true;
    snapshot.addons = Array.from({ length: 6 }, (_, i) => ({
      ...snapshot.addons[0],
      id: `addon-persist-${i}`,
    }));
    const sig = policy(snapshot);
    snapshot.filterPolicySignature = sig;

    let currentGen = 0;
    let shouldFail = true;
    const dispatchedPerRound: string[] = [];
    const evaluate = vi.fn(async (targets: FilterEvaluationTarget[]) => {
      const target = targets[0];
      dispatchedPerRound.push(target.targetKey);
      if (shouldFail) {
        return {
          policySignature: sig,
          cacheGeneration: currentGen,
          items: [{ targetKey: target.targetKey, status: 'failed' as const, error: 'offline' }],
        };
      }
      return {
        policySignature: sig,
        cacheGeneration: currentGen,
        items: [{
          targetKey: target.targetKey,
          status: 'completed' as const,
          complete: true,
          verdict: safeVerdict(target),
        }],
      };
    });

    const { result } = setup(snapshot, {
      evaluateFilterItems: evaluate,
      clearFilterCache: async () => {
        currentGen = 1;
        return { count: 8, cacheGeneration: currentGen };
      },
    });

    // Register Workshop page + active detail item with persistent owners
    act(() => {
      result.current.registerWorkshopItems([
        {
          workshopId: '88001',
          fileType: 'item',
          status: 'active',
          title: 'Workshop Page Item 1',
          tags: [],
          dependencies: [],
          collectionChildren: [],
          source: 'catalog_server',
          fetchedAt: new Date().toISOString(),
        },
        {
          workshopId: '88002',
          fileType: 'item',
          status: 'active',
          title: 'Workshop Active Detail Item',
          tags: [],
          dependencies: [],
          collectionChildren: [],
          source: 'catalog_server',
          fetchedAt: new Date().toISOString(),
        },
      ]);
      result.current.prioritizeFilterTargets(['ws:88001'], { owner: 'workshop-visible' });
      result.current.prioritizeFilterTargets(['ws:88002'], {
        urgent: true,
        owner: 'workshop-detail',
      });
    });

    // Round 1 fails (simulating offline service)
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(result.current.failedTargetKeys.size).toBe(8);

    // Retry once service recovers -> Workshop urgent detail & visible page MUST precede Library
    shouldFail = false;
    dispatchedPerRound.length = 0;
    act(() => { result.current.retryFailedEvaluations(); });
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(dispatchedPerRound.slice(0, 2)).toEqual(['ws:88002', 'ws:88001']);

    // Clear cache -> all targets re-queue; Workshop urgent detail & visible page STILL precede Library
    dispatchedPerRound.length = 0;
    await act(async () => {
      await result.current.clearFilterCache();
    });
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(dispatchedPerRound.slice(0, 2)).toEqual(['ws:88002', 'ws:88001']);
  });

  it('prepends a newly eligible front-scoped card ahead of existing items in pendingTargetsRef', async () => {
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.applyToLibrary = true;
    snapshot.addons = Array.from({ length: 8 }, (_, i) => ({
      ...snapshot.addons[0],
      id: `addon-front-${i}`,
    }));
    // Exclude addon-front-7 initially so it is not queued yet while 0..3 are in-flight and 4..6 are pending
    snapshot.filterExclusions = [{
      addonId: 'addon-front-7',
      displayLevel: 'normal',
      bindFingerprint: false,
      fingerprintValid: true,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
    }];
    const sig = policy(snapshot);
    snapshot.filterPolicySignature = sig;

    const dispatchedKeys: string[] = [];
    const resolvers = new Map<string, () => void>();
    const evaluate = vi.fn(async (targets: FilterEvaluationTarget[]) => {
      const target = targets[0];
      dispatchedKeys.push(target.targetKey);
      await new Promise<void>((resolve) => { resolvers.set(target.targetKey, resolve); });
      return {
        policySignature: sig,
        items: [{
          targetKey: target.targetKey,
          status: 'completed' as const,
          complete: true,
          verdict: safeVerdict(target),
        }],
      };
    });

    const { result, rerender, snapshotRef } = setup(snapshot, { evaluateFilterItems: evaluate });
    act(() => {
      result.current.prioritizeFilterTargets(['addon:addon-front-7'], {
        owner: 'library-visible',
      });
    });
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(dispatchedKeys).toEqual([
      'addon:addon-front-0',
      'addon:addon-front-1',
      'addon:addon-front-2',
      'addon:addon-front-3',
    ]);

    // Remove exclusion on addon-front-7 -> enqueueTargets adds it while addon-front-4..6 are already pending
    const nextSnapshot: DashboardSnapshot = { ...snapshot, filterExclusions: [] };
    snapshotRef.current = nextSnapshot;
    rerender({ snapshot: nextSnapshot });

    // Free one slot -> addon-front-7 must jump to the front ahead of addon-front-4..6
    await act(async () => { resolvers.get('addon:addon-front-0')?.(); });
    expect(dispatchedKeys[4]).toBe('addon:addon-front-7');
  });

  it('demotes queued urgent targets when their owner scope closes before dispatch', async () => {
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.applyToLibrary = true;
    snapshot.addons = Array.from({ length: 10 }, (_, i) => ({
      ...snapshot.addons[0],
      id: `addon-demote-${i}`,
    }));
    const sig = policy(snapshot);
    snapshot.filterPolicySignature = sig;

    const dispatchedKeys: string[] = [];
    const resolvers = new Map<string, () => void>();
    const evaluate = vi.fn(async (targets: FilterEvaluationTarget[]) => {
      const target = targets[0];
      dispatchedKeys.push(target.targetKey);
      await new Promise<void>((resolve) => { resolvers.set(target.targetKey, resolve); });
      return {
        policySignature: sig,
        items: [{
          targetKey: target.targetKey,
          status: 'completed' as const,
          complete: true,
          verdict: safeVerdict(target),
        }],
      };
    });

    const { result } = setup(snapshot, { evaluateFilterItems: evaluate });
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(dispatchedKeys).toHaveLength(4);

    // Open bundle-detail urgent scope for 8..9 and library-visible front scope for 5
    act(() => {
      result.current.prioritizeFilterTargets(['addon:addon-demote-5'], {
        owner: 'library-visible',
      });
      result.current.prioritizeFilterTargets(['addon:addon-demote-8', 'addon:addon-demote-9'], {
        urgent: true,
        owner: 'bundle-detail',
      });
    });

    // Close bundle-detail before any slot frees -> 8..9 must demote below visible 5
    act(() => {
      result.current.prioritizeFilterTargets([], {
        urgent: true,
        owner: 'bundle-detail',
      });
    });

    await act(async () => { resolvers.get('addon:addon-demote-0')?.(); });
    expect(dispatchedKeys[4]).toBe('addon:addon-demote-5');
  });

  it('resets flushTimerRef on cleanup so StrictMode remounts flush queued evaluations normally', async () => {
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.config.filter.applyToLibrary = true;
    snapshot.addons = [snapshot.addons[0]];
    const target = buildAddonFilterTarget(snapshot.addons[0]);
    const sig = policy(snapshot);
    snapshot.filterPolicySignature = sig;

    const evaluate = vi.fn(async (targets: FilterEvaluationTarget[]) => ({
      policySignature: sig,
      items: targets.map((item) => ({
        targetKey: item.targetKey,
        status: 'completed' as const,
        complete: true,
        verdict: safeVerdict(item),
      })),
    }));

    const snapshotRef = { current: snapshot };
    const { result } = renderHook(
      () =>
        useContentFilter({
          client: { ...getClient(false), evaluateFilterItems: evaluate },
          snapshot,
          snapshotRef,
          tRef: { current: i18n.t },
          commitSnapshot: () => true,
          commitSavedConfig: () => {},
          nextSnapshotSeq: () => 1,
          notify: () => {},
          setBusy: () => {},
        }),
      { wrapper: StrictMode },
    );

    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(evaluate).toHaveBeenCalledTimes(1);
    expect(result.current.filterVerdicts[target.targetKey]).toBeDefined();
  });

  it('clears failedTargetKeys immediately when a streamed filter_verdicts_updated event arrives', async () => {
    const snapshot = await previewEngine.getDashboardSnapshot();
    snapshot.config.filter.enabled = true;
    snapshot.addons = [snapshot.addons[0]];
    const target = buildAddonFilterTarget(snapshot.addons[0]);
    const sig = policy(snapshot);
    snapshot.filterPolicySignature = sig;

    const evaluate = vi.fn(async () => ({
      policySignature: sig,
      items: [{ targetKey: target.targetKey, status: 'failed' as const, error: 'transient' }],
    }));

    const { result } = setup(snapshot, { evaluateFilterItems: evaluate });
    await act(() => vi.advanceTimersByTimeAsync(50));
    expect(result.current.failedTargetKeys.has(target.targetKey)).toBe(true);

    act(() => result.current.handleFilterVerdictsUpdated([safeVerdict(target)], sig, 0));
    expect(result.current.failedTargetKeys.has(target.targetKey)).toBe(false);
    expect(result.current.filterVerdicts[target.targetKey]).toBeDefined();
  });
});
