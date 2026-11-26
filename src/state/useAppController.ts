import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import * as ipc from '../api/ipc';
import type { AppConfig, DashboardSnapshot, SteamCapabilities, SyncReport } from '../api/ipc';
import {
  computeFilterPolicySignature,
  isLinkedWorkshopTitleRedacted,
  type FilterLookupSource,
} from '../utils/contentFilter';
import type { BatchDialogPayload, NotifyOptions, SnackbarItem } from './controller/types';
import { getClient } from './controller/types';
import { useAddonActions } from './controller/useAddonActions';
import { useContentFilter } from './controller/useContentFilter';
import { useLiveDownloads } from './controller/useLiveDownloads';
import { useOrganizeActions } from './controller/useOrganizeActions';

export type {
  BatchDialogPayload,
  DesktopClient,
  LiveDownloadState,
  SnackbarItem,
} from './controller/types';

function samePolicy(a: AppConfig, b: AppConfig): boolean {
  return (
    computeFilterPolicySignature(a.filter, {
      catalogServerUrl: a.catalogServerUrl,
      httpsOnly: a.httpsOnly,
    }) ===
    computeFilterPolicySignature(b.filter, {
      catalogServerUrl: b.catalogServerUrl,
      httpsOnly: b.httpsOnly,
    })
  );
}

export function useAppController(onOpenReconcile?: () => void) {
  const { t } = useTranslation();
  const isTauri = ipc.isTauriRuntime();
  const client = useMemo(() => getClient(isTauri), [isTauri]);

  const [snapshot, setSnapshot] = useState<DashboardSnapshot | null>(null);
  const [snapshotVersion, setSnapshotVersion] = useState(0);
  const [initError, setInitError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [scanning, setScanning] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [syncReport, setSyncReport] = useState<SyncReport | null>(null);
  const [steamCaps, setSteamCaps] = useState<SteamCapabilities | null>(null);
  const [checkingSteamCaps, setCheckingSteamCaps] = useState(false);
  const [batchReport, setBatchReport] = useState<BatchDialogPayload | null>(null);
  const [snackbar, setSnackbar] = useState<SnackbarItem | null>(null);

  const snackTimerRef = useRef<number | null>(null);
  const snackIdRef = useRef(0);
  const snapshotSeqRef = useRef(0);
  const lastCommittedSeqRef = useRef(0);
  const lastSavedConfigSeqRef = useRef(0);
  const refreshPromiseRef = useRef<Promise<boolean> | null>(null);
  const refreshQueuedRef = useRef(false);
  const snapshotRef = useRef<DashboardSnapshot | null>(null);
  const tRef = useRef(t);
  const onOpenReconcileRef = useRef(onOpenReconcile);

  useEffect(() => {
    tRef.current = t;
  }, [t]);

  useEffect(() => {
    onOpenReconcileRef.current = onOpenReconcile;
  }, [onOpenReconcile]);

  useEffect(() => {
    return () => {
      if (snackTimerRef.current !== null) {
        window.clearTimeout(snackTimerRef.current);
      }
    };
  }, []);

  const publishSnapshot = useCallback((next: DashboardSnapshot) => {
    snapshotRef.current = next;
    setSnapshot(next);
    setSnapshotVersion((v) => v + 1);
    setInitError(null);
  }, []);

  const commitSnapshot = useCallback(
    (seq: number, next: DashboardSnapshot): boolean => {
      if (seq < lastCommittedSeqRef.current) {
        return false;
      }
      lastCommittedSeqRef.current = seq;
      const current = snapshotRef.current;
      // A refresh started before a config save can still contain fresh addon data.
      publishSnapshot(
        current && seq < lastSavedConfigSeqRef.current
          ? {
              ...next,
              config: current.config,
              cachedFilterVerdicts: samePolicy(current.config, next.config)
                ? next.cachedFilterVerdicts
                : current.cachedFilterVerdicts,
              filterPolicySignature: samePolicy(current.config, next.config)
                ? (next.filterPolicySignature ?? current.filterPolicySignature)
                : current.filterPolicySignature,
            }
          : next,
      );
      return true;
    },
    [publishSnapshot],
  );

  const nextSnapshotSeq = useCallback(() => {
    snapshotSeqRef.current += 1;
    return snapshotSeqRef.current;
  }, []);

  const commitSavedConfig = useCallback(
    (config: AppConfig) => {
      const current = snapshotRef.current;
      if (!current) return;
      lastSavedConfigSeqRef.current = nextSnapshotSeq();
      const policyUnchanged = samePolicy(current.config, config);
      publishSnapshot({
        ...current,
        config,
        cachedFilterVerdicts: policyUnchanged ? current.cachedFilterVerdicts : [],
        filterPolicySignature: policyUnchanged
          ? current.filterPolicySignature
          : computeFilterPolicySignature(config.filter, {
              catalogServerUrl: config.catalogServerUrl,
              httpsOnly: config.httpsOnly,
            }),
      });
    },
    [nextSnapshotSeq, publishSnapshot],
  );

  const notify = useCallback((message: string, options?: NotifyOptions) => {
    if (snackTimerRef.current !== null) {
      window.clearTimeout(snackTimerRef.current);
    }
    snackIdRef.current += 1;
    setSnackbar({
      id: snackIdRef.current,
      message,
      tone: options?.tone ?? 'default',
      actionLabel: options?.actionLabel,
      onAction: options?.onAction,
    });
    snackTimerRef.current = window.setTimeout(() => {
      setSnackbar(null);
    }, 4500);
  }, []);

  const dismissSnackbar = useCallback(() => {
    if (snackTimerRef.current !== null) {
      window.clearTimeout(snackTimerRef.current);
    }
    setSnackbar(null);
  }, []);

  const refresh = useCallback(async (): Promise<boolean> => {
    if (refreshPromiseRef.current) {
      refreshQueuedRef.current = true;
      return refreshPromiseRef.current;
    }
    const run = (async () => {
      let ok = true;
      try {
        do {
          refreshQueuedRef.current = false;
          ok = true;
          const seq = nextSnapshotSeq();
          try {
            const next = await client.getDashboardSnapshot();
            commitSnapshot(seq, next);
          } catch (err) {
            ok = false;
            notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
          }
        } while (refreshQueuedRef.current);
      } finally {
        refreshPromiseRef.current = null;
      }
      return ok;
    })();
    refreshPromiseRef.current = run;
    return run;
  }, [client, commitSnapshot, nextSnapshotSeq, notify]);

  const [bootstrapNonce, setBootstrapNonce] = useState(0);

  const bootstrap = useCallback(() => {
    setLoading(true);
    setInitError(null);
    setBootstrapNonce((n) => n + 1);
  }, []);

  const refreshSteamCapabilities = useCallback(async () => {
    setCheckingSteamCaps(true);
    try {
      const caps = await client.getSteamCapabilities();
      setSteamCaps(caps);
    } catch (err) {
      setSteamCaps({
        bridgeAvailable: false,
        bridgeInitialized: false,
        lastError: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setCheckingSteamCaps(false);
    }
  }, [client]);

  useEffect(() => {
    let active = true;
    const snapSeq = nextSnapshotSeq();

    void client
      .getDashboardSnapshot()
      .then(async (initial) => {
        if (!active) return;
        commitSnapshot(snapSeq, initial);
        setLoading(false);

        if (isTauri) {
          setScanning(true);
          const scanSeq = nextSnapshotSeq();
          try {
            const scanned = await client.scanWorkspace();
            if (active) commitSnapshot(scanSeq, scanned);
          } catch (err) {
            if (active) {
              notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
            }
          } finally {
            if (active) setScanning(false);
          }
        }
      })
      .catch((err) => {
        if (!active) return;
        const msg = err instanceof Error ? err.message : String(err);
        setInitError(msg);
        setLoading(false);
        notify(msg, { tone: 'error' });
      });

    return () => {
      active = false;
    };
  }, [bootstrapNonce, client, commitSnapshot, isTauri, nextSnapshotSeq, notify]);

  const contentFilter = useContentFilter({
    client,
    snapshot,
    snapshotRef,
    tRef,
    commitSnapshot,
    commitSavedConfig,
    nextSnapshotSeq,
    notify,
    setBusy,
  });

  const isWorkshopTitleRedacted = useCallback(
    (
      workshopId: string,
      rawTitle?: string,
      addonId?: string,
      options?: { transferKind?: 'download' | 'update' },
    ): boolean => {
      const liveSnapshot = snapshotRef.current;
      const freshExclusions = liveSnapshot
        ? new Map(liveSnapshot.filterExclusions.map((e) => [e.addonId, e]))
        : contentFilter.exclusionsByAddonId;
      const lookup: FilterLookupSource = {
        filterConfig: liveSnapshot?.config.filter ?? contentFilter.filterConfig,
        verdicts: contentFilter.filterVerdicts,
        evaluatedRevisions: contentFilter.evaluatedRevisions,
        exclusionsByAddonId: freshExclusions,
        revealedCardIds: contentFilter.revealedCardIds,
        failedTargetKeys: contentFilter.failedTargetKeys,
      };
      const addons = liveSnapshot?.addons ?? [];
      const found =
        (addonId ? addons.find((a) => a.id === addonId) : undefined) ??
        addons.find((a) => a.workshopId === workshopId);
      return isLinkedWorkshopTitleRedacted(workshopId, found, rawTitle, lookup, options);
    },
    [
      contentFilter.evaluatedRevisions,
      contentFilter.exclusionsByAddonId,
      contentFilter.failedTargetKeys,
      contentFilter.filterConfig,
      contentFilter.filterVerdicts,
      contentFilter.revealedCardIds,
    ],
  );

  const liveDownloadController = useLiveDownloads({
    client,
    snapshotRef,
    tRef,
    onOpenReconcileRef,
    notify,
    refresh,
    isWorkshopTitleRedacted,
    onFilterVerdictsUpdated: contentFilter.handleFilterVerdictsUpdated,
    onLocalFilterStatusChanged: contentFilter.handleLocalFilterStatusChanged,
  });

  const addonActions = useAddonActions({
    client,
    isTauri,
    snapshotRef,
    tRef,
    commitSnapshot,
    commitSavedConfig,
    nextSnapshotSeq,
    notify,
    refresh,
    setScanning,
    setSyncing,
    setBusy,
    setSyncReport,
    setSteamCaps,
    setBatchReport,
    onClearFailedFilterEvaluations: contentFilter.retryFailedEvaluations,
  });

  const organizeActions = useOrganizeActions({
    client,
    tRef,
    notify,
    refresh,
    setBusy,
    setBatchReport,
  });

  return {
    isTauri,
    snapshot,
    snapshotVersion,
    initError,
    loading,
    scanning,
    syncing,
    busy,
    syncReport,
    steamCaps,
    checkingSteamCaps,
    liveDownloads: liveDownloadController.liveDownloads,
    batchReport,
    setBatchReport,
    snackbar,
    notify,
    dismissSnackbar,
    bootstrap,
    refresh,
    refreshSteamCapabilities,
    scan: addonActions.scan,
    sync: addonActions.sync,
    saveConfig: addonActions.saveConfig,
    detectGamePath: addonActions.detectGamePath,
    toggleAddons: addonActions.toggleAddons,
    moveAddons: addonActions.moveAddons,
    previewRename: addonActions.previewRename,
    applyRenames: addonActions.applyRenames,
    deleteAddons: addonActions.deleteAddons,
    safeDeleteObsoleteDependency: addonActions.safeDeleteObsoleteDependency,
    resolveIssue: addonActions.resolveIssue,
    resolveIssuesBatch: addonActions.resolveIssuesBatch,
    downloadWorkshopItem: liveDownloadController.downloadWorkshopItem,
    updateWorkshopItem: liveDownloadController.updateWorkshopItem,
    cancelDownload: liveDownloadController.cancelDownload,
    rebuildBundles: organizeActions.rebuildBundles,
    createManualBundle: organizeActions.createManualBundle,
    deleteBundle: organizeActions.deleteBundle,
    setBundleLocked: organizeActions.setBundleLocked,
    trackCollection: organizeActions.trackCollection,
    untrackCollection: organizeActions.untrackCollection,
    saveUserGroup: organizeActions.saveUserGroup,
    deleteUserGroup: organizeActions.deleteUserGroup,
    saveProfile: organizeActions.saveProfile,
    deleteProfile: organizeActions.deleteProfile,
    applyProfile: organizeActions.applyProfile,
    searchWorkshop: addonActions.searchWorkshop,
    fetchWorkshopItem: addonActions.fetchWorkshopItem,
    getSubscribedWorkshopIds: addonActions.getSubscribedWorkshopIds,
    searchByMapCode: addonActions.searchByMapCode,
    openExternalUrl: addonActions.openExternalUrl,
    openInFileManager: addonActions.openInFileManager,
    filterConfig: contentFilter.filterConfig,
    filterVerdicts: contentFilter.filterVerdicts,
    evaluatedRevisions: contentFilter.evaluatedRevisions,
    exclusionsByAddonId: contentFilter.exclusionsByAddonId,
    localFilterStatus: contentFilter.localFilterStatus,
    filterStatusBusy: contentFilter.filterStatusBusy,
    filterTokenStatus: contentFilter.filterTokenStatus,
    filterEvaluationLog: contentFilter.filterEvaluationLog,
    revealedCardIds: contentFilter.revealedCardIds,
    failedTargetKeys: contentFilter.failedTargetKeys,
    revealCard: contentFilter.revealCard,
    retryFailedEvaluations: contentFilter.retryFailedEvaluations,
    registerWorkshopItems: contentFilter.registerWorkshopItems,
    refreshLocalFilterStatus: contentFilter.refreshLocalFilterStatus,
    checkLocalFilterUpdates: contentFilter.checkLocalFilterUpdates,
    deployLocalFilter: contentFilter.deployLocalFilter,
    startLocalFilterService: contentFilter.startLocalFilterService,
    stopLocalFilterService: contentFilter.stopLocalFilterService,
    refreshFilterTokenStatus: contentFilter.refreshFilterTokenStatus,
    setFilterLlmToken: contentFilter.setFilterLlmToken,
    clearFilterCache: contentFilter.clearFilterCache,
    evaluateSandboxTarget: contentFilter.evaluateSandboxTarget,
    upsertFilterExclusion: contentFilter.upsertFilterExclusion,
    deleteFilterExclusion: contentFilter.deleteFilterExclusion,
  };
}

export type AppController = ReturnType<typeof useAppController>;
