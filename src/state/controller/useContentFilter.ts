import type { Dispatch, RefObject, SetStateAction } from 'react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type {
  AddonId,
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
  nextSnapshotSeq: () => number;
  notify: NotifyFn;
  setBusy: Dispatch<SetStateAction<boolean>>;
}

export function useContentFilter({
  client,
  snapshot,
  tRef,
  commitSnapshot,
  nextSnapshotSeq,
  notify,
  setBusy,
}: UseContentFilterParams) {
  const defaultConfig = useMemo(() => createDefaultFilterConfig(), []);
  const filterConfig: ClientFilterConfig = snapshot?.config.filter ?? defaultConfig;
  const policySignature = useMemo(
    () => computeFilterPolicySignature(filterConfig),
    [filterConfig],
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
  const [evalRetryNonce, setEvalRetryNonce] = useState(0);

  const filterVerdicts = useMemo(
    () => (verdictStore.policySig === policySignature ? verdictStore.map : {}),
    [verdictStore, policySignature],
  );
  const evaluatedRevisions = useMemo(
    () => (verdictStore.policySig === policySignature ? verdictStore.revisions : {}),
    [verdictStore, policySignature],
  );
  const revealedCardIds = useMemo(
    () => (revealedStore.policySig === policySignature ? revealedStore.keys : new Set<string>()),
    [revealedStore, policySignature],
  );

  const exclusionsByAddonId = useMemo(() => {
    const map = new Map<string, FilterExclusionRecord>();
    for (const excl of snapshot?.filterExclusions ?? []) {
      map.set(excl.addonId, excl);
    }
    return map;
  }, [snapshot?.filterExclusions]);

  const policySigRef = useRef(policySignature);
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
    }
  }, [policySignature]);

  useEffect(() => {
    evaluatedRevisionsRef.current = evaluatedRevisions;
    filterVerdictsRef.current = filterVerdicts;
  }, [evaluatedRevisions, filterVerdicts]);

  useEffect(() => {
    return () => {
      if (flushTimerRef.current !== null) {
        window.clearTimeout(flushTimerRef.current);
      }
    };
  }, []);

  const mergeVerdicts = useCallback(
    (incoming: FilterItemVerdict[], targetRevisions?: Map<string, string>) => {
      if (incoming.length === 0) return;
      const sig = policySigRef.current;
      setVerdictStore((prev) => {
        const baseMap = prev.policySig === sig ? prev.map : {};
        const baseRevs = prev.policySig === sig ? prev.revisions : {};
        const nextMap = { ...baseMap };
        const nextRevs = { ...baseRevs };
        for (const v of incoming) {
          if (v.targetKey.startsWith('sandbox:')) continue;
          nextMap[v.targetKey] = v;
          const rev = targetRevisions?.get(v.targetKey);
          if (rev !== undefined) {
            nextRevs[v.targetKey] = rev;
          }
        }
        return { policySig: sig, map: nextMap, revisions: nextRevs };
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
          const revMap = new Map<string, string>();
          for (const t of chunk) {
            revMap.set(t.targetKey, t.revisionHint ?? '');
          }
          mergeVerdicts(verdicts, revMap);
        })
        .catch((err) => {
          for (const s of chunkSigs) {
            inFlightSigsRef.current.delete(s);
            failedSigsRef.current.add(s);
          }
          if (evalEpochRef.current !== epoch) return;
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
          filterVerdictsRef.current[target.targetKey] &&
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
    (verdicts: FilterItemVerdict[]) => {
      mergeVerdicts(verdicts);
    },
    [mergeVerdicts],
  );

  const handleLocalFilterStatusChanged = useCallback((status: LocalFilterDeploymentStatus) => {
    const prev = prevServiceStateRef.current;
    prevServiceStateRef.current = status.serviceState;
    setLocalFilterStatus(status);
    if (status.serviceState === 'running' && prev !== 'running') {
      failedSigsRef.current.clear();
      setFilterEvalError(null);
      setEvalRetryNonce((n) => n + 1);
    }
  }, []);

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
    [client, notify, resetLocalVerdicts, tRef],
  );

  const startLocalFilterService = useCallback(async () => {
    setFilterStatusBusy(true);
    try {
      const status = await client.startLocalFilterService();
      prevServiceStateRef.current = status.serviceState;
      setLocalFilterStatus(status);
      failedSigsRef.current.clear();
      setFilterEvalError(null);
      setEvalRetryNonce((n) => n + 1);
      notify(tRef.current('filter.toastServiceStarted'));
      return status;
    } catch (err) {
      notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      return null;
    } finally {
      setFilterStatusBusy(false);
    }
  }, [client, notify, tRef]);

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
        failedSigsRef.current.clear();
        setEvalRetryNonce((n) => n + 1);
        notify(tRef.current('filter.toastExclusionRemoved'));
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

  return {
    filterConfig,
    filterVerdicts,
    exclusionsByAddonId,
    localFilterStatus,
    filterStatusBusy,
    filterTokenStatus,
    filterEvalError,
    revealedCardIds,
    revealCard,
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
