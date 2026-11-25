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
  buildAddonFilterTarget,
  buildWorkshopFilterTarget,
  computeFilterPolicySignature,
  createDefaultFilterConfig,
} from '../../utils/contentFilter';
import type { DesktopClient, NotifyFn, TranslateFn } from './types';

const EVAL_BATCH_CHUNK_SIZE = 16;
const EVAL_DEBOUNCE_MS = 50;

export interface UseContentFilterParams {
  client: DesktopClient;
  snapshot: DashboardSnapshot | null;
  tRef: RefObject<TranslateFn>;
  commitSnapshot: (seq: number, next: DashboardSnapshot) => boolean;
  commitSavedConfig: (config: AppConfig) => void;
  nextSnapshotSeq: () => number;
  notify: NotifyFn;
  setBusy: Dispatch<SetStateAction<boolean>>;
}

const EMPTY_STRING_SET = new Set<string>();

export function useContentFilter({
  client,
  snapshot,
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
  const policySignature = useMemo(
    () =>
      computeFilterPolicySignature(filterConfig, {
        catalogServerUrl,
        httpsOnly,
      }),
    [filterConfig, catalogServerUrl, httpsOnly],
  );

  const [verdictStore, setVerdictStore] = useState<{
    policySig: string;
    map: Record<string, FilterItemVerdict>;
    revisions: Record<string, string>;
  }>(() => ({
    policySig: policySignature,
    map: {},
    revisions: {},
  }));

  const [lastSeededCached, setLastSeededCached] = useState<FilterItemVerdict[] | undefined>(
    undefined,
  );

  // Immediately hydrate snapshot.cachedFilterVerdicts at t = 0ms before first paint
  if (snapshot?.cachedFilterVerdicts !== lastSeededCached) {
    setLastSeededCached(snapshot?.cachedFilterVerdicts);
    if (snapshot?.cachedFilterVerdicts && snapshot.cachedFilterVerdicts.length > 0) {
      const addonRevByKey = new Map<string, string>();
      for (const addon of snapshot.addons) {
        const target = buildAddonFilterTarget(addon);
        addonRevByKey.set(target.targetKey, target.revisionHint ?? '');
      }
      const cached = snapshot.cachedFilterVerdicts;
      setVerdictStore((prev) => {
        const baseMap = prev.policySig === policySignature ? prev.map : {};
        const baseRevs = prev.policySig === policySignature ? prev.revisions : {};
        const nextMap = { ...baseMap };
        const nextRevs = { ...baseRevs };
        let changed = prev.policySig !== policySignature;
        for (const v of cached) {
          if (v.targetKey.startsWith('sandbox:')) continue;
          if (!nextMap[v.targetKey]) {
            nextMap[v.targetKey] = v;
            changed = true;
          }
          const rev = addonRevByKey.get(v.targetKey);
          if (rev !== undefined && nextRevs[v.targetKey] !== rev) {
            nextRevs[v.targetKey] = rev;
            changed = true;
          }
        }
        return changed ? { policySig: policySignature, map: nextMap, revisions: nextRevs } : prev;
      });
    }
  }

  const [localFilterStatus, setLocalFilterStatus] = useState<LocalFilterDeploymentStatus | null>(
    null,
  );
  const [filterStatusBusy, setFilterStatusBusy] = useState(false);
  const [filterTokenStatus, setFilterTokenStatus] = useState<FilterTokenStatus | null>(null);
  const [filterEvalError, setFilterEvalError] = useState<string | null>(null);
  const [revealedStore, setRevealedStore] = useState<{
    policySig: string;
    keys: Set<string>;
  }>(() => ({ policySig: policySignature, keys: new Set() }));
  const [failedStore, setFailedStore] = useState<{
    policySig: string;
    keys: Set<string>;
  }>(() => ({ policySig: policySignature, keys: new Set() }));
  const [evalRetryNonce, setEvalRetryNonce] = useState(0);

  const filterVerdicts = useMemo(
    () => (verdictStore.policySig === policySignature ? verdictStore.map : {}),
    [verdictStore.policySig, verdictStore.map, policySignature],
  );
  const evaluatedRevisions = useMemo(
    () => (verdictStore.policySig === policySignature ? verdictStore.revisions : {}),
    [verdictStore.policySig, verdictStore.revisions, policySignature],
  );
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

  const policySigRef = useRef(policySignature);
  const prevEnabledRef = useRef(filterConfig.enabled);
  const evalEpochRef = useRef(0);
  const pendingTargetsRef = useRef<Map<string, FilterEvaluationTarget>>(new Map());
  const inFlightSigsRef = useRef<Set<string>>(new Set());
  const failedSigsRef = useRef<Set<string>>(new Set());
  const evaluatedRevisionsRef = useRef<Record<string, string>>({});
  const filterVerdictsRef = useRef<Record<string, FilterItemVerdict>>({});
  const workshopRegistryRef = useRef<Map<string, WorkshopItemRecord>>(new Map());
  const flushTimerRef = useRef<number | null>(null);
  const prevServiceStateRef = useRef<string | undefined>(undefined);

  useEffect(() => {
    if (policySigRef.current !== policySignature) {
      policySigRef.current = policySignature;
      evalEpochRef.current += 1;
      pendingTargetsRef.current.clear();
      inFlightSigsRef.current.clear();
      failedSigsRef.current.clear();
      evaluatedRevisionsRef.current = {};
      filterVerdictsRef.current = {};
      setFilterEvalError(null);
    }
  }, [policySignature]);

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
      setFilterEvalError(null);
      setEvalRetryNonce((n) => n + 1);
    }
  }, [filterConfig.enabled]);

  useEffect(() => {
    return () => {
      if (flushTimerRef.current !== null) {
        window.clearTimeout(flushTimerRef.current);
      }
    };
  }, []);

  const retryFailedEvaluations = useCallback(() => {
    failedSigsRef.current.clear();
    setFailedStore({ policySig: policySigRef.current, keys: new Set() });
    setFilterEvalError(null);
    setEvalRetryNonce((n) => n + 1);
  }, []);

  const mergeVerdicts = useCallback(
    (incoming: FilterItemVerdict[], targetRevisions?: Map<string, string>) => {
      const valid = incoming.filter((v) => !v.targetKey.startsWith('sandbox:'));
      const hasRevs = Boolean(targetRevisions && targetRevisions.size > 0);
      if (valid.length === 0 && !hasRevs) return;
      const sig = policySigRef.current;

      // Eagerly update refs so microtasks never double-dispatch or re-enqueue
      for (const v of valid) {
        filterVerdictsRef.current[v.targetKey] = v;
      }
      if (targetRevisions) {
        for (const [key, rev] of targetRevisions) {
          if (!key.startsWith('sandbox:')) {
            evaluatedRevisionsRef.current[key] = rev;
          }
        }
      }

      setVerdictStore((prev) => {
        const baseMap = prev.policySig === sig ? prev.map : {};
        const baseRevs = prev.policySig === sig ? prev.revisions : {};
        let mapChanged = prev.policySig !== sig;
        let revsChanged = prev.policySig !== sig;
        const nextMap = { ...baseMap };
        const nextRevs = { ...baseRevs };

        for (const v of valid) {
          const existing = baseMap[v.targetKey];
          if (
            !existing ||
            existing.evaluatedAt !== v.evaluatedAt ||
            existing.flagged !== v.flagged ||
            existing.evaluatedScore !== v.evaluatedScore ||
            existing.decisionSource !== v.decisionSource
          ) {
            nextMap[v.targetKey] = v;
            mapChanged = true;
          }
        }
        if (targetRevisions) {
          for (const [key, rev] of targetRevisions) {
            if (!key.startsWith('sandbox:') && baseRevs[key] !== rev) {
              nextRevs[key] = rev;
              revsChanged = true;
            }
          }
        }

        if (!mapChanged && !revsChanged) {
          return prev;
        }
        return {
          policySig: sig,
          map: mapChanged ? nextMap : baseMap,
          revisions: revsChanged ? nextRevs : baseRevs,
        };
      });
    },
    [],
  );

  const flushEvaluationQueue = useCallback(() => {
    flushTimerRef.current = null;
    if (pendingTargetsRef.current.size === 0) return;

    const targets = Array.from(pendingTargetsRef.current.values());
    pendingTargetsRef.current.clear();

    const sig = policySigRef.current;
    const epoch = evalEpochRef.current;

    for (let i = 0; i < targets.length; i += EVAL_BATCH_CHUNK_SIZE) {
      const chunk = targets.slice(i, i + EVAL_BATCH_CHUNK_SIZE);
      const chunkSigs = chunk.map((t) => `${sig}::${t.targetKey}::${t.revisionHint ?? ''}`);
      for (const s of chunkSigs) {
        inFlightSigsRef.current.add(s);
      }

      void client
        .evaluateFilterItems(chunk)
        .then((verdicts) => {
          for (const s of chunkSigs) {
            inFlightSigsRef.current.delete(s);
          }
          if (evalEpochRef.current !== epoch || policySigRef.current !== sig) {
            return;
          }
          setFilterEvalError(null);
          setFailedStore((prev) => {
            if (prev.policySig !== sig || prev.keys.size === 0) return prev;
            const next = new Set(prev.keys);
            let removed = false;
            for (const t of chunk) {
              if (next.delete(t.targetKey)) removed = true;
            }
            return removed ? { policySig: sig, keys: next } : prev;
          });
          const revMap = new Map<string, string>();
          for (const t of chunk) {
            revMap.set(t.targetKey, t.revisionHint ?? '');
          }
          mergeVerdicts(verdicts, revMap);
        })
        .catch((err) => {
          for (const s of chunkSigs) {
            inFlightSigsRef.current.delete(s);
          }
          if (evalEpochRef.current !== epoch || policySigRef.current !== sig) return;
          for (const s of chunkSigs) {
            failedSigsRef.current.add(s);
          }
          setFailedStore((prev) => {
            const base = prev.policySig === sig ? prev.keys : new Set<string>();
            const next = new Set(base);
            for (const t of chunk) {
              next.add(t.targetKey);
            }
            return { policySig: sig, keys: next };
          });
          setFilterEvalError(err instanceof Error ? err.message : String(err));
        });
    }
  }, [client, mergeVerdicts]);

  const enqueueTargets = useCallback(
    (targets: FilterEvaluationTarget[]) => {
      if (targets.length === 0) return;
      const sig = policySigRef.current;
      let added = false;

      for (const target of targets) {
        const rev = target.revisionHint ?? '';
        const itemSig = `${sig}::${target.targetKey}::${rev}`;
        if (
          (filterVerdictsRef.current[target.targetKey] ||
            target.targetKey in evaluatedRevisionsRef.current) &&
          evaluatedRevisionsRef.current[target.targetKey] === rev
        ) {
          continue;
        }
        if (inFlightSigsRef.current.has(itemSig) || failedSigsRef.current.has(itemSig)) {
          continue;
        }
        pendingTargetsRef.current.set(target.targetKey, target);
        added = true;
      }

      if (added && flushTimerRef.current === null) {
        flushTimerRef.current = window.setTimeout(flushEvaluationQueue, EVAL_DEBOUNCE_MS);
      }
    },
    [flushEvaluationQueue],
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
    const items = Array.from(workshopRegistryRef.current.values());
    if (items.length === 0) return;
    enqueueTargets(items.map((item) => buildWorkshopFilterTarget(item)));
  }, [
    enqueueTargets,
    evalRetryNonce,
    filterConfig.applyToWorkshop,
    filterConfig.enabled,
    policySignature,
  ]);

  const registerWorkshopItems = useCallback(
    (items: WorkshopItemRecord[]) => {
      if (items.length === 0) return;
      for (const item of items) {
        workshopRegistryRef.current.set(item.workshopId, item);
      }
      if (workshopRegistryRef.current.size > 160) {
        const keys = Array.from(workshopRegistryRef.current.keys());
        for (let i = 0; i < keys.length - 120; i += 1) {
          workshopRegistryRef.current.delete(keys[i]);
        }
      }
      if (filterConfig.enabled && filterConfig.applyToWorkshop) {
        enqueueTargets(items.map((item) => buildWorkshopFilterTarget(item)));
      }
    },
    [enqueueTargets, filterConfig.applyToWorkshop, filterConfig.enabled],
  );

  const handleFilterVerdictsUpdated = useCallback(
    (verdicts: FilterItemVerdict[], eventPolicySignature?: string) => {
      if (!eventPolicySignature || eventPolicySignature !== policySigRef.current) {
        return;
      }
      mergeVerdicts(verdicts);
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

  const revealCard = useCallback(
    (cardKey: string) => {
      setRevealedStore((prev) => {
        const base = prev.policySig === policySignature ? prev.keys : new Set<string>();
        if (base.has(cardKey)) return prev;
        const next = new Set(base);
        next.add(cardKey);
        return { policySig: policySignature, keys: next };
      });
    },
    [policySignature],
  );

  const resetLocalVerdicts = useCallback(() => {
    evalEpochRef.current += 1;
    pendingTargetsRef.current.clear();
    inFlightSigsRef.current.clear();
    failedSigsRef.current.clear();
    evaluatedRevisionsRef.current = {};
    filterVerdictsRef.current = {};
    setFilterEvalError(null);
    setVerdictStore({ policySig: policySigRef.current, map: {}, revisions: {} });
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
        if (snapshot) {
          const nextPinned = request.trackLatest ? null : (request.targetCommit?.trim() || null);
          const nextRepoUrl = request.repoUrl?.trim() || snapshot.config.filter.repoUrl;
          commitSavedConfig({
            ...snapshot.config,
            filter: {
              ...snapshot.config.filter,
              repoUrl: nextRepoUrl,
              trackLatest: request.trackLatest,
              pinnedCommit: nextPinned,
            },
          });
        }
        resetLocalVerdicts();
        notify(tRef.current('filter.toastDeployed'));
        return status;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      } finally {
        setFilterStatusBusy(false);
      }
    },
    [client, commitSavedConfig, notify, resetLocalVerdicts, snapshot, tRef],
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
        if (filterConfig.llmEnabled) {
          resetLocalVerdicts();
        }
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
    [client, filterConfig.llmEnabled, notify, resetLocalVerdicts, setBusy, tRef],
  );

  const clearFilterCache = useCallback(async () => {
    setBusy(true);
    try {
      const count = await client.clearFilterCache();
      resetLocalVerdicts();
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
        const results = await client.evaluateFilterItems([{ ...target, bypassCache: true }]);
        return results[0] ?? null;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      }
    },
    [client, notify],
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
    filterEvalError,
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
