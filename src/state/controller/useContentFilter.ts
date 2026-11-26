import type { Dispatch, RefObject, SetStateAction } from 'react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type {
  AddonId,
  AppConfig,
  ClientFilterConfig,
  DashboardSnapshot,
  DeployLocalFilterRequest,
  FilterEvaluationTarget,
  FilterExclusionRecord,
  FilterItemVerdict,
  FilterTokenStatus,
  LocalFilterDeploymentStatus,
  UpsertFilterExclusionRequest,
  WorkshopItemRecord,
} from '../../api/ipc';
import {
  addonFilterKey,
  buildAddonFilterTarget,
  buildAddonRevisionHint,
  buildWorkshopFilterTarget,
  filterVerdictExpiresAt,
  isAutomaticFilterTargetEnabled,
  computeFilterPolicySignature,
  createDefaultFilterConfig,
} from '../../utils/contentFilter';
import type { DesktopClient, NotifyFn, TranslateFn } from './types';

const EVAL_BATCH_CHUNK_SIZE = 16;
const MAX_CONCURRENT_EVAL_BATCHES = 1;
const EVAL_DEBOUNCE_MS = 50;
const MAX_EVALUATION_LOG_ENTRIES = 100;

type FilterEvaluationIssue =
  | { targetKey: string; status: 'failed'; error?: string }
  | { targetKey: string; status: 'incomplete' };

export type FilterEvaluationLogEntry = FilterEvaluationIssue & {
  id: number;
  at: string;
};

interface FilterTargetKeys {
  policySig: string;
  keys: Set<string>;
}

function removeTargetKeys(
  state: FilterTargetKeys,
  policySig: string,
  staleKeys: ReadonlySet<string>,
): FilterTargetKeys {
  if (state.policySig !== policySig || state.keys.size === 0) return state;
  const keys = new Set(state.keys);
  for (const key of staleKeys) keys.delete(key);
  return keys.size === state.keys.size ? state : { policySig, keys };
}

export interface UseContentFilterParams {
  client: DesktopClient;
  snapshot: DashboardSnapshot | null;
  snapshotRef?: RefObject<DashboardSnapshot | null>;
  tRef: RefObject<TranslateFn>;
  commitSnapshot: (seq: number, next: DashboardSnapshot) => boolean;
  commitSavedConfig: (config: AppConfig) => void;
  nextSnapshotSeq: () => number;
  notify: NotifyFn;
  setBusy: Dispatch<SetStateAction<boolean>>;
}

const EMPTY_STRING_SET = new Set<string>();
const EMPTY_VERDICTS: Record<string, FilterItemVerdict> = {};
const EMPTY_REVISIONS: Record<string, string> = {};

export function useContentFilter({
  client,
  snapshot,
  snapshotRef,
  tRef,
  commitSnapshot,
  commitSavedConfig,
  nextSnapshotSeq,
  notify,
  setBusy,
}: UseContentFilterParams) {
  const defaultConfig = useMemo(() => createDefaultFilterConfig(), []);
  const filterConfig: ClientFilterConfig = snapshot?.config.filter ?? defaultConfig;
  const catalogServerUrl = snapshot?.config.catalogServerUrl;
  const httpsOnly = snapshot?.config.httpsOnly;

  const computedPolicySignature = useMemo(
    () =>
      computeFilterPolicySignature(filterConfig, {
        catalogServerUrl,
        httpsOnly,
      }),
    [filterConfig, catalogServerUrl, httpsOnly],
  );
  const basePolicySignature = snapshot?.filterPolicySignature?.trim() || computedPolicySignature;
  const [cacheGenerationFloor, setCacheGenerationFloor] = useState(0);
  const [verdictsExpiredThrough, setVerdictsExpiredThrough] = useState(() => Date.now());
  const snapshotCacheGeneration = snapshot?.filterCacheGeneration ?? 0;
  const cacheGeneration = Math.max(cacheGenerationFloor, snapshotCacheGeneration);

  const [runtimePolicyOverride, setRuntimePolicyOverride] = useState<{
    baseSig: string;
    sig: string;
  } | null>(null);

  const policySignature =
    runtimePolicyOverride && runtimePolicyOverride.baseSig === basePolicySignature
      ? runtimePolicyOverride.sig
      : basePolicySignature;

  const [verdictStore, setVerdictStore] = useState<{
    policySig: string;
    cacheGeneration: number;
    map: Record<string, FilterItemVerdict>;
    revisions: Record<string, string>;
  }>(() => ({
    policySig: policySignature,
    cacheGeneration,
    map: {},
    revisions: {},
  }));

  const [localFilterStatus, setLocalFilterStatus] = useState<LocalFilterDeploymentStatus | null>(
    null,
  );
  const [filterStatusBusy, setFilterStatusBusy] = useState(false);
  const [filterTokenStatus, setFilterTokenStatus] = useState<FilterTokenStatus | null>(null);
  const [filterEvaluationLog, setFilterEvaluationLog] = useState<FilterEvaluationLogEntry[]>([]);
  const evaluationLogId = useRef(0);
  const [revealedStore, setRevealedStore] = useState<FilterTargetKeys>(
    () => ({ policySig: policySignature, keys: new Set() }),
  );
  const [failedStore, setFailedStore] = useState<FilterTargetKeys>(
    () => ({ policySig: policySignature, keys: new Set() }),
  );
  const [evalRetryNonce, setEvalRetryNonce] = useState(0);

  const [hydrationTracker, setHydrationTracker] = useState<{
    addons: DashboardSnapshot['addons'] | undefined;
    addonRevs: Map<string, string>;
    cached: FilterItemVerdict[] | undefined;
    policySig: string;
    cacheGeneration: number;
  }>({
    addons: undefined,
    addonRevs: new Map(),
    cached: undefined,
    policySig: policySignature,
    cacheGeneration,
  });

  // Reconcile addon revisions and hydrate complete revision-matching cachedFilterVerdicts at t = 0ms
  if (
    snapshot?.addons !== hydrationTracker.addons ||
    snapshot?.cachedFilterVerdicts !== hydrationTracker.cached ||
    policySignature !== hydrationTracker.policySig ||
    cacheGeneration !== hydrationTracker.cacheGeneration
  ) {
    const currentRevByKey = new Map<string, string>();
    if (snapshot) {
      for (const addon of snapshot.addons) {
        currentRevByKey.set(addonFilterKey(addon.id), buildAddonRevisionHint(addon));
      }
    }
    const staleAddonKeys = new Set<string>();
    for (const [key, prevRev] of hydrationTracker.addonRevs) {
      if (currentRevByKey.get(key) !== prevRev) {
        staleAddonKeys.add(key);
      }
    }

    setHydrationTracker({
      addons: snapshot?.addons,
      addonRevs: currentRevByKey,
      cached: snapshot?.cachedFilterVerdicts,
      policySig: policySignature,
      cacheGeneration,
    });

    if (staleAddonKeys.size > 0) {
      setFailedStore((prev) => removeTargetKeys(prev, policySignature, staleAddonKeys));
      setRevealedStore((prev) => removeTargetKeys(prev, policySignature, staleAddonKeys));
    }

    const cached =
      basePolicySignature === policySignature && snapshotCacheGeneration === cacheGeneration
        ? snapshot?.cachedFilterVerdicts ?? []
        : [];

    setVerdictStore((prev) => {
      const samePolicy =
        prev.policySig === policySignature && prev.cacheGeneration === cacheGeneration;
      const baseMap = samePolicy ? prev.map : {};
      const baseRevs = samePolicy ? prev.revisions : {};
      const nextMap = { ...baseMap };
      const nextRevs = { ...baseRevs };
      let changed = !samePolicy;

      if (snapshot) {
        for (const key of Object.keys(nextMap)) {
          if (
            filterVerdictExpiresAt(nextMap[key], filterConfig.mode) <= verdictsExpiredThrough ||
            (key.startsWith('addon:') && nextMap[key].revisionHint !== currentRevByKey.get(key))
          ) {
            delete nextMap[key];
            delete nextRevs[key];
            changed = true;
          }
        }
        for (const key of Object.keys(nextRevs)) {
          if (!key.startsWith('addon:')) continue;
          const currentRev = currentRevByKey.get(key);
          if (currentRev === undefined || nextRevs[key] !== currentRev) {
            delete nextRevs[key];
            delete nextMap[key];
            changed = true;
          }
        }
      }

      for (const v of cached) {
        if (v.targetKey.startsWith('sandbox:')) continue;
        if (
          v.complete === false ||
          v.revisionHint == null ||
          filterVerdictExpiresAt(v, filterConfig.mode) <= verdictsExpiredThrough
        ) continue;
        const currentRev = currentRevByKey.get(v.targetKey);
        if (currentRev === undefined || v.revisionHint !== currentRev) {
          continue;
        }
        if (!nextMap[v.targetKey] || nextMap[v.targetKey].complete === false) {
          nextMap[v.targetKey] = v;
          changed = true;
        }
        if (nextRevs[v.targetKey] !== v.revisionHint) {
          nextRevs[v.targetKey] = v.revisionHint;
          changed = true;
        }
      }

      return changed
        ? { policySig: policySignature, cacheGeneration, map: nextMap, revisions: nextRevs }
        : prev;
    });
  }

  const verdictStoreIsCurrent =
    verdictStore.policySig === policySignature && verdictStore.cacheGeneration === cacheGeneration;
  const filterVerdicts = verdictStoreIsCurrent ? verdictStore.map : EMPTY_VERDICTS;
  const evaluatedRevisions = verdictStoreIsCurrent ? verdictStore.revisions : EMPTY_REVISIONS;
  const revealedCardIds = useMemo(
    () => (revealedStore.policySig === policySignature ? revealedStore.keys : EMPTY_STRING_SET),
    [revealedStore, policySignature],
  );
  const failedTargetKeys = useMemo(
    () => (failedStore.policySig === policySignature ? failedStore.keys : EMPTY_STRING_SET),
    [failedStore, policySignature],
  );

  const exclusionsByAddonId = useMemo(() => {
    const map = new Map<string, FilterExclusionRecord>();
    for (const excl of snapshot?.filterExclusions ?? []) {
      map.set(excl.addonId, excl);
    }
    return map;
  }, [snapshot?.filterExclusions]);

  const basePolicySigRef = useRef(basePolicySignature);
  const policySigRef = useRef(policySignature);
  const filterConfigRef = useRef(filterConfig);
  const cacheGenerationRef = useRef(cacheGeneration);
  const latestConfigRef = useRef<AppConfig | null>(snapshot?.config ?? null);
  const currentRevByKeyRef = useRef<Map<string, string>>(new Map());
  const prevEnabledRef = useRef(filterConfig.enabled);
  const evalEpochRef = useRef(0);
  const activeBatchCountRef = useRef(0);
  const pendingTargetsRef = useRef<Map<string, FilterEvaluationTarget>>(new Map());
  const inFlightSigsRef = useRef<Set<string>>(new Set());
  const failedSigsRef = useRef<Set<string>>(new Set());
  const evaluatedRevisionsRef = useRef<Record<string, string>>({});
  const filterVerdictsRef = useRef<Record<string, FilterItemVerdict>>({});
  const workshopRegistryRef = useRef<Map<string, FilterEvaluationTarget>>(new Map());
  const flushTimerRef = useRef<number | null>(null);
  const prevServiceStateRef = useRef<string | undefined>(undefined);

  const appendEvaluationLog = useCallback((issues: FilterEvaluationIssue[]) => {
    if (issues.length === 0) return;
    const at = new Date().toISOString();
    const entries = issues.map((issue) => ({ ...issue, at, id: ++evaluationLogId.current }));
    setFilterEvaluationLog((prev) => [...entries, ...prev].slice(0, MAX_EVALUATION_LOG_ENTRIES));
  }, []);

  const updateFailedTargets = useCallback((
    sig: string,
    issues: FilterEvaluationIssue[],
    completedKeys: string[] = [],
  ) => {
    if (issues.length === 0 && completedKeys.length === 0) return;
    setFailedStore((prev) => {
      const next = new Set(prev.policySig === sig ? prev.keys : []);
      let changed = prev.policySig !== sig;
      for (const key of completedKeys) {
        if (next.delete(key)) changed = true;
      }
      for (const { targetKey } of issues) {
        if (!next.has(targetKey)) {
          next.add(targetKey);
          changed = true;
        }
      }
      return changed ? { policySig: sig, keys: next } : prev;
    });
    appendEvaluationLog(issues);
  }, [appendEvaluationLog]);

  useEffect(() => {
    basePolicySigRef.current = basePolicySignature;
    filterConfigRef.current = filterConfig;
  }, [basePolicySignature, filterConfig]);

  useEffect(() => {
    if (snapshot?.config) {
      latestConfigRef.current = snapshot.config;
    }
  }, [snapshot?.config]);

  useEffect(() => {
    const prevMap = currentRevByKeyRef.current;
    const map = new Map<string, string>();
    for (const addon of snapshot?.addons ?? []) {
      map.set(addonFilterKey(addon.id), buildAddonRevisionHint(addon));
    }
    const staleKeys = new Set<string>();
    for (const [key, prevRev] of prevMap) {
      if (map.get(key) !== prevRev) {
        staleKeys.add(key);
      }
    }
    if (staleKeys.size > 0) {
      for (const sigEntry of Array.from(failedSigsRef.current)) {
        const parts = sigEntry.split('::');
        if (parts.length >= 2 && staleKeys.has(parts[1])) {
          failedSigsRef.current.delete(sigEntry);
        }
      }
    }
    currentRevByKeyRef.current = map;
  }, [snapshot?.addons]);

  useEffect(() => {
    if (policySigRef.current !== policySignature || cacheGenerationRef.current !== cacheGeneration) {
      policySigRef.current = policySignature;
      cacheGenerationRef.current = cacheGeneration;
      evalEpochRef.current += 1;
      activeBatchCountRef.current = 0;
      pendingTargetsRef.current.clear();
      inFlightSigsRef.current.clear();
      failedSigsRef.current.clear();
      evaluatedRevisionsRef.current = {};
      filterVerdictsRef.current = {};
      setFailedStore({ policySig: policySignature, keys: new Set() });
      setRevealedStore({ policySig: policySignature, keys: new Set() });
    }
  }, [policySignature, cacheGeneration]);

  useEffect(() => {
    evaluatedRevisionsRef.current = { ...evaluatedRevisions };
    filterVerdictsRef.current = { ...filterVerdicts };
  }, [evaluatedRevisions, filterVerdicts]);

  useEffect(() => {
    const wasEnabled = prevEnabledRef.current;
    prevEnabledRef.current = filterConfig.enabled;
    if (filterConfig.enabled && !wasEnabled) {
      failedSigsRef.current.clear();
      setFailedStore({ policySig: policySigRef.current, keys: new Set() });
      setEvalRetryNonce((n) => n + 1);
    }
  }, [filterConfig.enabled]);

  useEffect(() => {
    return () => {
      evalEpochRef.current += 1;
      activeBatchCountRef.current = 0;
      if (flushTimerRef.current !== null) {
        window.clearTimeout(flushTimerRef.current);
      }
    };
  }, []);

  const adoptRuntimePolicySignature = useCallback((nextSig: string) => {
    if (!nextSig || nextSig === policySigRef.current) return;
    policySigRef.current = nextSig;
    pendingTargetsRef.current.clear();
    failedSigsRef.current.clear();
    evaluatedRevisionsRef.current = {};
    filterVerdictsRef.current = {};
    setRuntimePolicyOverride({ baseSig: basePolicySigRef.current, sig: nextSig });
    setVerdictStore({
      policySig: nextSig,
      cacheGeneration: cacheGenerationRef.current,
      map: {},
      revisions: {},
    });
    setRevealedStore({ policySig: nextSig, keys: new Set() });
    setFailedStore({ policySig: nextSig, keys: new Set() });
  }, []);

  const retryFailedEvaluations = useCallback(() => {
    failedSigsRef.current.clear();
    setFailedStore({ policySig: policySigRef.current, keys: new Set() });
    setEvalRetryNonce((n) => n + 1);
  }, []);

  const targetStillEligible = useCallback((targetKey: string, revision?: string) => {
    const latest = snapshotRef?.current;
    if (!isAutomaticFilterTargetEnabled(targetKey, latest?.config.filter ?? filterConfigRef.current)) {
      return false;
    }
    if (targetKey.startsWith('addon:')) {
      return revision !== undefined && currentRevByKeyRef.current.get(targetKey) === revision &&
        !latest?.filterExclusions.some((entry) =>
          entry.fingerprintValid && addonFilterKey(entry.addonId) === targetKey,
        );
    }
    const registered = workshopRegistryRef.current.get(targetKey);
    return !registered || registered.revisionHint === revision;
  }, [snapshotRef]);

  const mergeVerdicts = useCallback((incoming: FilterItemVerdict[]) => {
    const mode = filterConfigRef.current.mode;
    const now = Date.now();
    const relevant = incoming.filter((verdict) =>
      !verdict.targetKey.startsWith('sandbox:') &&
      targetStillEligible(verdict.targetKey, verdict.revisionHint) &&
      (verdict.complete !== false || verdict.flagged) &&
      filterVerdictExpiresAt(verdict, mode) > now,
    );
    if (relevant.length === 0) return;

    const sig = policySigRef.current;
    const generation = cacheGenerationRef.current;
    const accepted: FilterItemVerdict[] = [];
    for (const verdict of relevant) {
      const existing = filterVerdictsRef.current[verdict.targetKey];
      const complete = verdict.complete !== false;
      if (existing && existing.revisionHint === verdict.revisionHint) {
        const existingComplete = existing.complete !== false;
        if (existingComplete && !complete && filterVerdictExpiresAt(existing, mode) > now) continue;
        if (
          existingComplete === complete &&
          Date.parse(existing.evaluatedAt) > Date.parse(verdict.evaluatedAt)
        ) continue;
      }
      filterVerdictsRef.current[verdict.targetKey] = verdict;
      accepted.push(verdict);
      if (complete && verdict.revisionHint != null) {
        evaluatedRevisionsRef.current[verdict.targetKey] = verdict.revisionHint;
      } else {
        delete evaluatedRevisionsRef.current[verdict.targetKey];
      }
    }

    setVerdictStore((prev) => {
      if (policySigRef.current !== sig || cacheGenerationRef.current !== generation) return prev;
      const samePolicy = prev.policySig === sig && prev.cacheGeneration === generation;
      const map = { ...(samePolicy ? prev.map : {}) };
      const revisions = { ...(samePolicy ? prev.revisions : {}) };
      for (const verdict of accepted) {
        const { targetKey } = verdict;
        map[targetKey] = verdict;
        if (verdict.complete !== false && verdict.revisionHint != null) {
          revisions[targetKey] = verdict.revisionHint;
        } else {
          delete revisions[targetKey];
        }
      }
      return { policySig: sig, cacheGeneration: generation, map, revisions };
    });
  }, [targetStillEligible]);

  const isCurrentEvaluation = useCallback((targetKey: string, revision: string) => {
    const verdict = filterVerdictsRef.current[targetKey];
    return evaluatedRevisionsRef.current[targetKey] === revision &&
      verdict && verdict.complete !== false &&
      filterVerdictExpiresAt(verdict, filterConfigRef.current.mode) > Date.now();
  }, []);

  useEffect(() => {
    if (filterConfig.mode === 'local') return;
    const expireVerdicts = () => {
      const now = Date.now();
      const expired = Object.keys(filterVerdictsRef.current).filter((key) =>
        filterVerdictExpiresAt(filterVerdictsRef.current[key], filterConfig.mode) <= now,
      );
      if (expired.length === 0) return;
      for (const key of expired) {
        const verdict = filterVerdictsRef.current[key];
        failedSigsRef.current.delete(`${policySigRef.current}::${key}::${verdict.revisionHint ?? ''}`);
        delete filterVerdictsRef.current[key];
        delete evaluatedRevisionsRef.current[key];
      }
      setVerdictsExpiredThrough(now);
      const sig = policySigRef.current;
      const generation = cacheGenerationRef.current;
      const expiredKeys = new Set(expired);
      setFailedStore((prev) => removeTargetKeys(prev, sig, expiredKeys));
      setVerdictStore((prev) => {
        if (prev.policySig !== sig || prev.cacheGeneration !== generation) return prev;
        const map = { ...prev.map };
        const revisions = { ...prev.revisions };
        for (const key of expired) {
          if (!map[key] || filterVerdictExpiresAt(map[key], filterConfig.mode) > now) continue;
          delete map[key];
          delete revisions[key];
        }
        return { ...prev, map, revisions };
      });
      setEvalRetryNonce((n) => n + 1);
    };

    const deadline = Object.values(filterVerdicts).reduce(
      (soonest, verdict) => Math.min(soonest, filterVerdictExpiresAt(verdict, filterConfig.mode)),
      Infinity,
    );
    if (!Number.isFinite(deadline)) return;
    const timer = window.setTimeout(expireVerdicts, Math.max(0, deadline - Date.now()));
    document.addEventListener('visibilitychange', expireVerdicts);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener('visibilitychange', expireVerdicts);
    };
  }, [filterConfig.mode, filterVerdicts]);

  const flushEvaluationQueue = useCallback(() => {
    if (flushTimerRef.current !== null) {
      window.clearTimeout(flushTimerRef.current);
      flushTimerRef.current = null;
    }

    const pump = () => {
      while (
        activeBatchCountRef.current < MAX_CONCURRENT_EVAL_BATCHES &&
        pendingTargetsRef.current.size > 0
      ) {
        const chunk: FilterEvaluationTarget[] = [];
        const sigNow = policySigRef.current;

        for (const [key, target] of pendingTargetsRef.current) {
          pendingTargetsRef.current.delete(key);
          const rev = target.revisionHint ?? '';
          const itemSig = `${sigNow}::${target.targetKey}::${rev}`;
          if (!targetStillEligible(key, target.revisionHint) || isCurrentEvaluation(key, rev)) {
            continue;
          }
          if (inFlightSigsRef.current.has(itemSig) || failedSigsRef.current.has(itemSig)) {
            continue;
          }
          chunk.push(target);
          if (chunk.length >= EVAL_BATCH_CHUNK_SIZE) {
            break;
          }
        }

        if (chunk.length === 0) {
          continue;
        }

        const dispatchSig = policySigRef.current;
        const epoch = evalEpochRef.current;
        const chunkSigs = chunk.map(
          (t) => `${dispatchSig}::${t.targetKey}::${t.revisionHint ?? ''}`,
        );
        const requestedRevByKey = new Map<string, string>();
        for (const t of chunk) {
          requestedRevByKey.set(t.targetKey, t.revisionHint ?? '');
        }
        for (const s of chunkSigs) {
          inFlightSigsRef.current.add(s);
        }
        activeBatchCountRef.current += 1;

        void client
          .evaluateFilterItems(chunk, true)
          .then((response) => {
            if (evalEpochRef.current !== epoch || dispatchSig !== policySigRef.current ||
                (response.cacheGeneration ?? 0) !== cacheGenerationRef.current) {
              return;
            }
            if (response.policySignature && response.policySignature !== policySigRef.current) {
              if (filterConfigRef.current.mode !== 'catalog') return;
              adoptRuntimePolicySignature(response.policySignature);
            }
            const sig = policySigRef.current;

            const verdicts: FilterItemVerdict[] = [];
            const clearedKeys: string[] = [];
            const issues: FilterEvaluationIssue[] = [];

            for (const item of response.items) {
              const reqRev = requestedRevByKey.get(item.targetKey);
              if (reqRev === undefined || !targetStillEligible(item.targetKey, reqRev)) continue;
              const itemSig = `${sig}::${item.targetKey}::${reqRev}`;
              const dispatchItemSig = `${dispatchSig}::${item.targetKey}::${reqRev}`;

              if (
                item.status === 'completed' &&
                item.complete !== false &&
                item.verdict.complete !== false
              ) {
                verdicts.push(item.verdict);
                failedSigsRef.current.delete(itemSig);
                failedSigsRef.current.delete(dispatchItemSig);
                clearedKeys.push(item.targetKey);
                continue;
              }

              failedSigsRef.current.add(itemSig);
              if (item.status === 'failed') {
                issues.push({ targetKey: item.targetKey, status: 'failed', error: item.error });
              } else {
                issues.push({ targetKey: item.targetKey, status: 'incomplete' });
                if (item.verdict.flagged) {
                  verdicts.push({ ...item.verdict, complete: false });
                }
              }
            }

            updateFailedTargets(sig, issues, clearedKeys);
            mergeVerdicts(verdicts);
          })
          .catch((err) => {
            if (evalEpochRef.current !== epoch) return;
            const sig = policySigRef.current;
            const error = err instanceof Error ? err.message : String(err);
            const issues: FilterEvaluationIssue[] = [];
            for (const t of chunk) {
              if (!targetStillEligible(t.targetKey, t.revisionHint)) continue;
              failedSigsRef.current.add(`${sig}::${t.targetKey}::${t.revisionHint ?? ''}`);
              issues.push({ targetKey: t.targetKey, status: 'failed', error });
            }
            updateFailedTargets(sig, issues);
          })
          .finally(() => {
            if (evalEpochRef.current === epoch) {
              for (const s of chunkSigs) {
                inFlightSigsRef.current.delete(s);
              }
              activeBatchCountRef.current = Math.max(0, activeBatchCountRef.current - 1);
              pump();
            }
          });
      }

      if (pendingTargetsRef.current.size === 0 && flushTimerRef.current !== null) {
        window.clearTimeout(flushTimerRef.current);
        flushTimerRef.current = null;
      }
    };

    pump();
  }, [
    adoptRuntimePolicySignature, client, isCurrentEvaluation, mergeVerdicts, targetStillEligible,
    updateFailedTargets,
  ]);

  const enqueueTargets = useCallback(
    (targets: FilterEvaluationTarget[]) => {
      if (targets.length === 0) return;
      const sig = policySigRef.current;
      let added = false;

      for (const target of targets) {
        const rev = target.revisionHint ?? '';
        const itemSig = `${sig}::${target.targetKey}::${rev}`;
        if (isCurrentEvaluation(target.targetKey, rev)) {
          continue;
        }
        if (inFlightSigsRef.current.has(itemSig) || failedSigsRef.current.has(itemSig)) {
          continue;
        }
        pendingTargetsRef.current.set(target.targetKey, target);
        added = true;
      }

      if (
        added &&
        flushTimerRef.current === null &&
        activeBatchCountRef.current < MAX_CONCURRENT_EVAL_BATCHES
      ) {
        flushTimerRef.current = window.setTimeout(flushEvaluationQueue, EVAL_DEBOUNCE_MS);
      }
    },
    [flushEvaluationQueue, isCurrentEvaluation],
  );

  // Automatic Library evaluation (skipping addons with a valid fingerprint exclusion)
  useEffect(() => {
    if (!filterConfig.enabled || !filterConfig.applyToLibrary || !snapshot) {
      return;
    }
    const targets: FilterEvaluationTarget[] = [];
    for (const addon of snapshot.addons) {
      const excl = exclusionsByAddonId.get(addon.id);
      if (excl?.fingerprintValid) {
        continue;
      }
      targets.push(buildAddonFilterTarget(addon));
    }
    enqueueTargets(targets);
  }, [
    enqueueTargets,
    cacheGeneration,
    evalRetryNonce,
    exclusionsByAddonId,
    filterConfig.applyToLibrary,
    filterConfig.enabled,
    policySignature,
    snapshot,
  ]);

  // Re-evaluate registered Workshop items when filter settings or policy change
  useEffect(() => {
    if (!filterConfig.enabled || !filterConfig.applyToWorkshop) {
      return;
    }
    enqueueTargets(Array.from(workshopRegistryRef.current.values()));
  }, [
    enqueueTargets,
    cacheGeneration,
    evalRetryNonce,
    filterConfig.applyToWorkshop,
    filterConfig.enabled,
    policySignature,
    snapshot?.cachedFilterVerdicts,
  ]);

  const registerWorkshopItems = useCallback(
    (items: WorkshopItemRecord[], options?: { onlyIfMissing?: boolean }) => {
      if (items.length === 0) return;
      const onlyIfMissing = options?.onlyIfMissing ?? false;
      const sig = policySigRef.current;
      const staleKeys = new Set<string>();
      const targets: FilterEvaluationTarget[] = [];
      for (const item of items) {
        const target = buildWorkshopFilterTarget(item);
        const previous = workshopRegistryRef.current.get(target.targetKey);
        if (previous) {
          if (onlyIfMissing) continue;
          if (previous.revisionHint !== target.revisionHint) {
            staleKeys.add(target.targetKey);
            failedSigsRef.current.delete(`${sig}::${target.targetKey}::${previous.revisionHint ?? ''}`);
          }
        }
        workshopRegistryRef.current.set(target.targetKey, target);
        targets.push(target);
      }
      if (staleKeys.size > 0) {
        setFailedStore((prev) => removeTargetKeys(prev, sig, staleKeys));
        setRevealedStore((prev) => removeTargetKeys(prev, sig, staleKeys));
      }
      if (workshopRegistryRef.current.size > 160) {
        const keys = Array.from(workshopRegistryRef.current.keys());
        for (let i = 0; i < keys.length - 120; i += 1) {
          workshopRegistryRef.current.delete(keys[i]);
        }
      }
      if (targets.length > 0 && filterConfig.enabled && filterConfig.applyToWorkshop) {
        enqueueTargets(targets);
      }
    },
    [enqueueTargets, filterConfig.applyToWorkshop, filterConfig.enabled],
  );

  const handleFilterVerdictsUpdated = useCallback(
    (verdicts: FilterItemVerdict[], eventPolicySignature?: string, generation = 0) => {
      if (eventPolicySignature === policySigRef.current && generation === cacheGenerationRef.current) {
        mergeVerdicts(verdicts);
      }
    },
    [mergeVerdicts],
  );

  const handleLocalFilterStatusChanged = useCallback(
    (status: LocalFilterDeploymentStatus) => {
      const prev = prevServiceStateRef.current;
      prevServiceStateRef.current = status.serviceState;
      setLocalFilterStatus(status);
      if (status.serviceState === 'running' && prev !== 'running') {
        retryFailedEvaluations();
      }
    },
    [retryFailedEvaluations],
  );

  const revealCard = useCallback((cardKey: string) => {
    const sig = policySigRef.current;
    setRevealedStore((prev) => {
      const base = prev.policySig === sig ? prev.keys : new Set<string>();
      if (base.has(cardKey)) return prev;
      const next = new Set(base);
      next.add(cardKey);
      return { policySig: sig, keys: next };
    });
  }, []);

  const resetLocalVerdicts = useCallback((generation?: number) => {
    if (generation != null) {
      cacheGenerationRef.current = Math.max(cacheGenerationRef.current, generation);
      setCacheGenerationFloor((floor) => Math.max(floor, generation));
    }
    evalEpochRef.current += 1;
    activeBatchCountRef.current = 0;
    pendingTargetsRef.current.clear();
    inFlightSigsRef.current.clear();
    failedSigsRef.current.clear();
    evaluatedRevisionsRef.current = {};
    filterVerdictsRef.current = {};
    setVerdictStore({
      policySig: policySigRef.current,
      cacheGeneration: cacheGenerationRef.current,
      map: {},
      revisions: {},
    });
    setRevealedStore({ policySig: policySigRef.current, keys: new Set() });
    setFailedStore({ policySig: policySigRef.current, keys: new Set() });
    setEvalRetryNonce((n) => n + 1);
  }, []);

  const refreshLocalFilterStatus = useCallback(async () => {
    try {
      const status = await client.getLocalFilterStatus();
      prevServiceStateRef.current = status.serviceState;
      setLocalFilterStatus(status);
      return status;
    } catch (err) {
      notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      return null;
    }
  }, [client, notify]);

  const checkLocalFilterUpdates = useCallback(async () => {
    setFilterStatusBusy(true);
    try {
      const status = await client.checkLocalFilterUpdates();
      prevServiceStateRef.current = status.serviceState;
      setLocalFilterStatus(status);
      notify(
        status.updateAvailable
          ? tRef.current('filter.toastUpdateFound', {
              sha: status.remoteLatestCommit?.shortSha ?? '',
            })
          : tRef.current('filter.toastUpToDate'),
      );
      return status;
    } catch (err) {
      notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      return null;
    } finally {
      setFilterStatusBusy(false);
    }
  }, [client, notify, tRef]);

  const deployLocalFilter = useCallback(
    async (request: DeployLocalFilterRequest) => {
      setFilterStatusBusy(true);
      try {
        const status = await client.deployLocalFilter(request);
        prevServiceStateRef.current = status.serviceState;
        setLocalFilterStatus(status);
        const latestConfig = snapshotRef?.current?.config ?? latestConfigRef.current;
        if (latestConfig) {
          const nextPinned = request.trackLatest ? null : (request.targetCommit?.trim() || null);
          const nextRepoUrl = request.repoUrl?.trim() || latestConfig.filter.repoUrl;
          const mergedConfig: AppConfig = {
            ...latestConfig,
            filter: {
              ...latestConfig.filter,
              repoUrl: nextRepoUrl,
              trackLatest: request.trackLatest,
              pinnedCommit: nextPinned,
            },
          };
          latestConfigRef.current = mergedConfig;
          commitSavedConfig(mergedConfig);
        }
        resetLocalVerdicts(status.cacheGeneration);
        notify(tRef.current('filter.toastDeployed'));
        return status;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      } finally {
        setFilterStatusBusy(false);
      }
    },
    [client, commitSavedConfig, notify, resetLocalVerdicts, snapshotRef, tRef],
  );

  const startLocalFilterService = useCallback(async () => {
    setFilterStatusBusy(true);
    try {
      const status = await client.startLocalFilterService();
      prevServiceStateRef.current = status.serviceState;
      setLocalFilterStatus(status);
      retryFailedEvaluations();
      notify(tRef.current('filter.toastServiceStarted'));
      return status;
    } catch (err) {
      notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      return null;
    } finally {
      setFilterStatusBusy(false);
    }
  }, [client, notify, retryFailedEvaluations, tRef]);

  const stopLocalFilterService = useCallback(async () => {
    setFilterStatusBusy(true);
    try {
      const status = await client.stopLocalFilterService();
      prevServiceStateRef.current = status.serviceState;
      setLocalFilterStatus(status);
      notify(tRef.current('filter.toastServiceStopped'));
      return status;
    } catch (err) {
      notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      return null;
    } finally {
      setFilterStatusBusy(false);
    }
  }, [client, notify, tRef]);

  const refreshFilterTokenStatus = useCallback(async () => {
    try {
      const status = await client.getFilterTokenStatus();
      setFilterTokenStatus(status);
      return status;
    } catch (err) {
      notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      return null;
    }
  }, [client, notify]);

  const setFilterLlmToken = useCallback(
    async (token: string | null) => {
      setBusy(true);
      try {
        const status = await client.setFilterLlmToken(token);
        setFilterTokenStatus(status);
        resetLocalVerdicts(status.cacheGeneration);
        notify(
          token ? tRef.current('filter.toastTokenSaved') : tRef.current('filter.toastTokenCleared'),
        );
        return status;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      } finally {
        setBusy(false);
      }
    },
    [client, notify, resetLocalVerdicts, setBusy, tRef],
  );

  const clearFilterCache = useCallback(async () => {
    setBusy(true);
    try {
      const { count, cacheGeneration: generation } = await client.clearFilterCache();
      resetLocalVerdicts(generation);
      notify(tRef.current('filter.toastCacheCleared', { count }));
      return count;
    } catch (err) {
      notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      return null;
    } finally {
      setBusy(false);
    }
  }, [client, notify, resetLocalVerdicts, setBusy, tRef]);

  const evaluateSandboxTarget = useCallback(
    async (target: FilterEvaluationTarget): Promise<FilterItemVerdict | null> => {
      try {
        const response = await client.evaluateFilterItems([{ ...target, bypassCache: true }]);
        const first = response.items[0];
        if (!first) return null;
        if (first.status === 'failed') {
          appendEvaluationLog([{ targetKey: target.targetKey, status: 'failed', error: first.error }]);
          notify(first.error, { tone: 'error' });
          return null;
        }
        if (first.complete === false || first.verdict.complete === false) {
          appendEvaluationLog([{ targetKey: target.targetKey, status: 'incomplete' }]);
          notify(tRef.current('filter.evalIncompleteDetail'), { tone: 'error' });
          return null;
        }
        return first.verdict;
      } catch (err) {
        const error = err instanceof Error ? err.message : String(err);
        appendEvaluationLog([{ targetKey: target.targetKey, status: 'failed', error }]);
        notify(error, { tone: 'error' });
        return null;
      }
    },
    [appendEvaluationLog, client, notify, tRef],
  );

  const upsertFilterExclusion = useCallback(
    async (request: UpsertFilterExclusionRequest): Promise<boolean> => {
      setBusy(true);
      const seq = nextSnapshotSeq();
      try {
        const next = await client.upsertFilterExclusion(request);
        commitSnapshot(seq, next);
        notify(tRef.current('filter.toastExclusionSaved'));
        return true;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return false;
      } finally {
        setBusy(false);
      }
    },
    [client, commitSnapshot, nextSnapshotSeq, notify, setBusy, tRef],
  );

  const deleteFilterExclusion = useCallback(
    async (addonId: AddonId): Promise<boolean> => {
      setBusy(true);
      const seq = nextSnapshotSeq();
      try {
        const next = await client.deleteFilterExclusion(addonId);
        commitSnapshot(seq, next);
        retryFailedEvaluations();
        notify(tRef.current('filter.toastExclusionRemoved'));
        return true;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return false;
      } finally {
        setBusy(false);
      }
    },
    [client, commitSnapshot, nextSnapshotSeq, notify, retryFailedEvaluations, setBusy, tRef],
  );

  return {
    filterConfig,
    filterVerdicts,
    evaluatedRevisions,
    exclusionsByAddonId,
    localFilterStatus,
    filterStatusBusy,
    filterTokenStatus,
    filterEvaluationLog,
    revealedCardIds,
    failedTargetKeys,
    revealCard,
    retryFailedEvaluations,
    registerWorkshopItems,
    handleFilterVerdictsUpdated,
    handleLocalFilterStatusChanged,
    refreshLocalFilterStatus,
    checkLocalFilterUpdates,
    deployLocalFilter,
    startLocalFilterService,
    stopLocalFilterService,
    refreshFilterTokenStatus,
    setFilterLlmToken,
    clearFilterCache,
    evaluateSandboxTarget,
    upsertFilterExclusion,
    deleteFilterExclusion,
  };
}
