import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import * as ipc from '../api/ipc';
import type {
  AddonBatchReport,
  AddonDirType,
  AddonSummary,
  AppConfig,
  BundleRecord,
  CoreEvent,
  DashboardSnapshot,
  LoadoutProfile,
  ProfileApplyReport,
  ReconcileAction,
  ReconcileBatchItem,
  ReconcileBatchReport,
  RenamePreviewItem,
  RenameRuleConfig,
  SearchQuery,
  SearchResponse,
  SteamCapabilities,
  SyncReport,
  TrackedCollectionRecord,
  UserGroup,
  WorkshopItemRecord,
} from '../api/ipc';
import { IpcError } from '../api/ipc';
import { previewEngine } from '../api/previewStore';
import { summarizeBatchReport } from '../utils/format';

export interface LiveDownloadState {
  requestId: number;
  taskId: string;
  workshopId: string;
  downloaded: number;
  total?: number;
  percent: number;
  speedBps: number;
  cancelling: boolean;
  updatedAt: number;
}

export interface SnackbarItem {
  id: number;
  message: string;
  tone?: 'default' | 'error';
  actionLabel?: string;
  onAction?: () => void;
}

export interface BatchDialogPayload {
  batch: AddonBatchReport;
  profileSummary?: {
    enabledCount: number;
    disabledCount: number;
    skippedWorkshopOnly: string[];
    missingAddons: string[];
  };
}

export interface DesktopClient {
  getDashboardSnapshot: () => Promise<DashboardSnapshot>;
  scanWorkspace: () => Promise<DashboardSnapshot>;
  saveConfig: (config: AppConfig) => Promise<AppConfig>;
  detectGameAddonsPath: () => Promise<string | null>;
  getSteamCapabilities: () => Promise<SteamCapabilities>;
  syncWorkshop: () => Promise<SyncReport>;
  searchWorkshop: (query: SearchQuery) => Promise<SearchResponse>;
  fetchWorkshopItem: (workshopId: string) => Promise<WorkshopItemRecord>;
  getSubscribedWorkshopIds: () => Promise<string[]>;
  downloadWorkshopItem: (workshopId: string) => Promise<AddonSummary>;
  updateWorkshopItem: (workshopId: string) => Promise<AddonSummary>;
  cancelDownload: (workshopId: string) => Promise<boolean>;
  resolveReconcileIssue: (
    issueId: string,
    action: ReconcileAction,
  ) => Promise<DashboardSnapshot>;
  resolveReconcileIssuesBatch: (
    items: ReconcileBatchItem[],
  ) => Promise<ReconcileBatchReport>;
  moveAddons: (addonIds: string[], targetDirType: AddonDirType) => Promise<AddonBatchReport>;
  toggleAddons: (
    addonIds: string[],
    enabled: boolean,
    allowWorkshopDir?: boolean,
  ) => Promise<AddonBatchReport>;
  previewRename: (rules: RenameRuleConfig, addonIds?: string[]) => Promise<RenamePreviewItem[]>;
  applyRenames: (renames: [string, string][]) => Promise<AddonBatchReport>;
  deleteAddons: (
    addonIds: string[],
    deleteDiskFile: boolean,
    purgeKnownRecord: boolean,
  ) => Promise<AddonBatchReport>;
  safeDeleteObsoleteDependency: (
    workshopId: string,
    deleteDiskFile: boolean,
    purgeKnownRecord: boolean,
  ) => Promise<AddonBatchReport>;
  rebuildBundles: () => Promise<BundleRecord[]>;
  createManualBundle: (name: string, memberAddonIds: string[]) => Promise<BundleRecord>;
  deleteBundle: (bundleId: string) => Promise<void>;
  setBundleLocked: (bundleId: string, locked: boolean) => Promise<void>;
  trackCollection: (collectionId: string, autoSync: boolean) => Promise<TrackedCollectionRecord>;
  untrackCollection: (collectionId: string) => Promise<void>;
  saveUserGroup: (group: UserGroup) => Promise<UserGroup>;
  deleteUserGroup: (groupId: string) => Promise<void>;
  saveProfile: (profile: LoadoutProfile) => Promise<LoadoutProfile>;
  deleteProfile: (profileId: string) => Promise<void>;
  applyProfile: (profileId: string) => Promise<ProfileApplyReport>;
  searchByMapCode: (query: string) => Promise<AddonSummary[]>;
  openExternalUrl: (url: string) => Promise<void>;
  openInFileManager: (path: string) => Promise<void>;
  subscribeCoreEvents: (handler: (ev: CoreEvent) => void) => Promise<() => void>;
}

function getClient(tauri: boolean): DesktopClient {
  if (tauri) {
    return {
      getDashboardSnapshot: ipc.getDashboardSnapshot,
      scanWorkspace: ipc.scanWorkspace,
      saveConfig: ipc.saveConfig,
      detectGameAddonsPath: ipc.detectGameAddonsPath,
      getSteamCapabilities: ipc.getSteamCapabilities,
      syncWorkshop: ipc.syncWorkshop,
      searchWorkshop: ipc.searchWorkshop,
      fetchWorkshopItem: ipc.fetchWorkshopItem,
      getSubscribedWorkshopIds: ipc.getSubscribedWorkshopIds,
      downloadWorkshopItem: ipc.downloadWorkshopItem,
      updateWorkshopItem: ipc.updateWorkshopItem,
      cancelDownload: ipc.cancelDownload,
      resolveReconcileIssue: ipc.resolveReconcileIssue,
      resolveReconcileIssuesBatch: ipc.resolveReconcileIssuesBatch,
      moveAddons: ipc.moveAddons,
      toggleAddons: ipc.toggleAddons,
      previewRename: ipc.previewRename,
      applyRenames: ipc.applyRenames,
      deleteAddons: ipc.deleteAddons,
      safeDeleteObsoleteDependency: ipc.safeDeleteObsoleteDependency,
      rebuildBundles: ipc.rebuildBundles,
      createManualBundle: ipc.createManualBundle,
      deleteBundle: ipc.deleteBundle,
      setBundleLocked: ipc.setBundleLocked,
      trackCollection: ipc.trackCollection,
      untrackCollection: ipc.untrackCollection,
      saveUserGroup: ipc.saveUserGroup,
      deleteUserGroup: ipc.deleteUserGroup,
      saveProfile: ipc.saveProfile,
      deleteProfile: ipc.deleteProfile,
      applyProfile: ipc.applyProfile,
      searchByMapCode: ipc.searchByMapCode,
      openExternalUrl: ipc.openExternalUrl,
      openInFileManager: ipc.openInFileManager,
      subscribeCoreEvents: ipc.subscribeCoreEvents,
    };
  }
  return {
    getDashboardSnapshot: () => previewEngine.getDashboardSnapshot(),
    scanWorkspace: () => previewEngine.scanWorkspace(),
    saveConfig: (c) => previewEngine.saveConfig(c),
    detectGameAddonsPath: () => previewEngine.detectGameAddonsPath(),
    getSteamCapabilities: () => previewEngine.getSteamCapabilities(),
    syncWorkshop: () => previewEngine.syncWorkshop(),
    searchWorkshop: (q) => previewEngine.searchWorkshop(q),
    fetchWorkshopItem: (id) => previewEngine.fetchWorkshopItem(id),
    getSubscribedWorkshopIds: () => previewEngine.getSubscribedWorkshopIds(),
    downloadWorkshopItem: (id) => previewEngine.downloadWorkshopItem(id),
    updateWorkshopItem: (id) => previewEngine.updateWorkshopItem(id),
    cancelDownload: (id) => previewEngine.cancelDownload(id),
    resolveReconcileIssue: (id, act) => previewEngine.resolveReconcileIssue(id, act),
    resolveReconcileIssuesBatch: (items) => previewEngine.resolveReconcileIssuesBatch(items),
    moveAddons: (ids, dir) => previewEngine.moveAddons(ids, dir),
    toggleAddons: (ids, en, allow) => previewEngine.toggleAddons(ids, en, allow),
    previewRename: (rules, ids) => previewEngine.previewRename(rules, ids),
    applyRenames: (r) => previewEngine.applyRenames(r),
    deleteAddons: (ids, d, p) => previewEngine.deleteAddons(ids, d, p),
    safeDeleteObsoleteDependency: (wid, d, p) =>
      previewEngine.safeDeleteObsoleteDependency(wid, d, p),
    rebuildBundles: () => previewEngine.rebuildBundles(),
    createManualBundle: (n, m) => previewEngine.createManualBundle(n, m),
    deleteBundle: (id) => previewEngine.deleteBundle(id),
    setBundleLocked: (id, l) => previewEngine.setBundleLocked(id, l),
    trackCollection: (id, a) => previewEngine.trackCollection(id, a),
    untrackCollection: (id) => previewEngine.untrackCollection(id),
    saveUserGroup: (g) => previewEngine.saveUserGroup(g),
    deleteUserGroup: (id) => previewEngine.deleteUserGroup(id),
    saveProfile: (p) => previewEngine.saveProfile(p),
    deleteProfile: (id) => previewEngine.deleteProfile(id),
    applyProfile: (id) => previewEngine.applyProfile(id),
    searchByMapCode: (q) => previewEngine.searchByMapCode(q),
    openExternalUrl: (u) => previewEngine.openExternalUrl(u),
    openInFileManager: (p) => previewEngine.openInFileManager(p),
    subscribeCoreEvents: async (h) => previewEngine.subscribe(h),
  };
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
  const [liveDownloads, setLiveDownloads] = useState<Record<string, LiveDownloadState>>({});
  const [batchReport, setBatchReport] = useState<BatchDialogPayload | null>(null);
  const [snackbar, setSnackbar] = useState<SnackbarItem | null>(null);

  const snackTimerRef = useRef<number | null>(null);
  const snackIdRef = useRef(0);
  const snapshotSeqRef = useRef(0);
  const lastCommittedSeqRef = useRef(0);
  const refreshInFlightRef = useRef(false);
  const refreshQueuedRef = useRef(false);
  const downloadSeqRef = useRef(0);
  const liveDownloadsRef = useRef<Record<string, LiveDownloadState>>({});
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
    liveDownloadsRef.current = liveDownloads;
  }, [liveDownloads]);

  useEffect(() => {
    snapshotRef.current = snapshot;
  }, [snapshot]);

  const commitSnapshot = useCallback((seq: number, next: DashboardSnapshot): boolean => {
    if (seq < lastCommittedSeqRef.current) {
      return false;
    }
    lastCommittedSeqRef.current = seq;
    snapshotRef.current = next;
    setSnapshot(next);
    setSnapshotVersion((v) => v + 1);
    setInitError(null);
    return true;
  }, []);

  const nextSnapshotSeq = useCallback(() => {
    snapshotSeqRef.current += 1;
    return snapshotSeqRef.current;
  }, []);

  const notify = useCallback(
    (
      message: string,
      options?: { tone?: 'default' | 'error'; actionLabel?: string; onAction?: () => void },
    ) => {
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
    },
    [],
  );

  const dismissSnackbar = useCallback(() => {
    if (snackTimerRef.current !== null) {
      window.clearTimeout(snackTimerRef.current);
    }
    setSnackbar(null);
  }, []);

  const refresh = useCallback(async (): Promise<boolean> => {
    if (refreshInFlightRef.current) {
      refreshQueuedRef.current = true;
      return true;
    }
    refreshInFlightRef.current = true;
    let ok = true;
    try {
      do {
        refreshQueuedRef.current = false;
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
      refreshInFlightRef.current = false;
    }
    return ok;
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

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    void client
      .subscribeCoreEvents((ev) => {
        if (!active) return;
        if (ev.type === 'snapshot_updated') {
          void refresh();
        } else if (ev.type === 'download_progress') {
          const now = Date.now();
          setLiveDownloads((prev) => {
            if (ev.percent >= 100) {
              if (!(ev.workshopId in prev)) return prev;
              const copy = { ...prev };
              delete copy[ev.workshopId];
              return copy;
            }
            const existing = prev[ev.workshopId];
            let speedBps = existing?.speedBps ?? 0;
            if (existing && now > existing.updatedAt) {
              const dt = (now - existing.updatedAt) / 1000;
              const delta = ev.downloaded - existing.downloaded;
              if (dt > 0.05 && delta >= 0) {
                speedBps = Math.round(delta / dt);
              }
            }
            return {
              ...prev,
              [ev.workshopId]: {
                requestId: existing?.requestId ?? 0,
                taskId: ev.taskId,
                workshopId: ev.workshopId,
                downloaded: ev.downloaded,
                total: ev.total ?? undefined,
                percent: ev.percent,
                speedBps,
                cancelling: existing?.cancelling ?? false,
                updatedAt: now,
              },
            };
          });
        } else if (ev.type === 'reconcile_issues_detected') {
          notify(tRef.current('toast.reconcileDetected', { count: ev.count }), {
            actionLabel: tRef.current('common.view'),
            onAction: () => onOpenReconcileRef.current?.(),
          });
        } else if (ev.type === 'watch_error') {
          notify(ev.message, { tone: 'error' });
        }
      })
      .then((fn) => {
        if (!active) {
          fn();
          return;
        }
        unlisten = fn;
      })
      .catch((err) => {
        if (!active) return;
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      });

    return () => {
      active = false;
      unlisten?.();
      if (snackTimerRef.current !== null) {
        window.clearTimeout(snackTimerRef.current);
      }
    };
  }, [client, notify, refresh]);

  const handleBatchReport = useCallback(
    (report: AddonBatchReport, refreshOk: boolean) => {
      const counts = summarizeBatchReport(report);
      if (counts.hasIssue) {
        setBatchReport({ batch: report });
        return false;
      }
      if (!refreshOk) {
        return true;
      }
      if (counts.applied > 0) {
        notify(tRef.current('toast.appliedCount', { count: counts.applied }));
      } else {
        notify(tRef.current('toast.unchangedAll'));
      }
      return true;
    },
    [notify],
  );

  const scan = useCallback(async (): Promise<boolean> => {
    setScanning(true);
    const seq = nextSnapshotSeq();
    try {
      const next = await client.scanWorkspace();
      commitSnapshot(seq, next);
      notify(tRef.current('toast.scanDone', { count: next.addons.length }));
      return true;
    } catch (err) {
      notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      return false;
    } finally {
      setScanning(false);
    }
  }, [client, commitSnapshot, nextSnapshotSeq, notify]);

  const sync = useCallback(async () => {
    setSyncing(true);
    try {
      const report = await client.syncWorkshop();
      setSyncReport(report);
      await refresh();
      return report;
    } catch (err) {
      await refresh();
      notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      return null;
    } finally {
      setSyncing(false);
    }
  }, [client, notify, refresh]);

  const saveConfig = useCallback(
    async (config: AppConfig, forceRescan = false): Promise<boolean> => {
      setBusy(true);
      const prevConfig = snapshotRef.current?.config;
      const dirsChanged =
        forceRescan ||
        !prevConfig ||
        prevConfig.loadingDir !== config.loadingDir ||
        prevConfig.workshopDir !== config.workshopDir;
      const sdkFlagChanged =
        !prevConfig || prevConfig.disableSteamworksSdk !== config.disableSteamworksSdk;

      try {
        const saved = await client.saveConfig(config);
        const cfgSeq = nextSnapshotSeq();
        if (snapshotRef.current) {
          commitSnapshot(cfgSeq, { ...snapshotRef.current, config: saved });
        }
        if (sdkFlagChanged) {
          setSteamCaps(null);
        }

        if (dirsChanged) {
          setScanning(true);
          const scanSeq = nextSnapshotSeq();
          try {
            const next = await client.scanWorkspace();
            commitSnapshot(scanSeq, next);
            notify(tRef.current('toast.scanDone', { count: next.addons.length }));
          } catch (scanErr) {
            notify(
              `${tRef.current('settings.savedScanFailed')}: ${scanErr instanceof Error ? scanErr.message : String(scanErr)}`,
              {
                tone: 'error',
                actionLabel: tRef.current('common.retry'),
                onAction: () => void scan(),
              },
            );
          } finally {
            setScanning(false);
          }
        } else {
          notify(tRef.current('settings.saved'));
        }
        return true;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return false;
      } finally {
        setBusy(false);
      }
    },
    [client, commitSnapshot, nextSnapshotSeq, notify, scan],
  );

  const detectGamePath = useCallback(async () => {
    try {
      return await client.detectGameAddonsPath();
    } catch (err) {
      notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      return null;
    }
  }, [client, notify]);

  const toggleAddons = useCallback(
    async (addonIds: string[], enabled: boolean, allowWorkshopDir = false) => {
      setBusy(true);
      try {
        const report = await client.toggleAddons(addonIds, enabled, allowWorkshopDir);
        const refreshed = await refresh();
        handleBatchReport(report, refreshed);
        return report;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      } finally {
        setBusy(false);
      }
    },
    [client, handleBatchReport, notify, refresh],
  );

  const moveAddons = useCallback(
    async (addonIds: string[], targetDirType: AddonDirType) => {
      setBusy(true);
      try {
        const report = await client.moveAddons(addonIds, targetDirType);
        const refreshed = await refresh();
        handleBatchReport(report, refreshed);
        return report;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      } finally {
        setBusy(false);
      }
    },
    [client, handleBatchReport, notify, refresh],
  );

  const previewRename = useCallback(
    async (rules: RenameRuleConfig, addonIds: string[] = []) => {
      return client.previewRename(rules, addonIds);
    },
    [client],
  );

  const applyRenames = useCallback(
    async (renames: [string, string][]) => {
      setBusy(true);
      try {
        const report = await client.applyRenames(renames);
        const refreshed = await refresh();
        handleBatchReport(report, refreshed);
        return report;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      } finally {
        setBusy(false);
      }
    },
    [client, handleBatchReport, notify, refresh],
  );

  const deleteAddons = useCallback(
    async (addonIds: string[], deleteDiskFile: boolean, purgeKnownRecord: boolean) => {
      setBusy(true);
      try {
        const report = await client.deleteAddons(addonIds, deleteDiskFile, purgeKnownRecord);
        const refreshed = await refresh();
        handleBatchReport(report, refreshed);
        return report;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      } finally {
        setBusy(false);
      }
    },
    [client, handleBatchReport, notify, refresh],
  );

  const safeDeleteObsoleteDependency = useCallback(
    async (workshopId: string, deleteDiskFile: boolean, purgeKnownRecord: boolean) => {
      setBusy(true);
      try {
        const report = await client.safeDeleteObsoleteDependency(
          workshopId,
          deleteDiskFile,
          purgeKnownRecord,
        );
        const refreshed = await refresh();
        handleBatchReport(report, refreshed);
        return report;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      } finally {
        setBusy(false);
      }
    },
    [client, handleBatchReport, notify, refresh],
  );

  const resolveIssue = useCallback(
    async (issueId: string, action: ReconcileAction) => {
      setBusy(true);
      const seq = nextSnapshotSeq();
      try {
        const next = await client.resolveReconcileIssue(issueId, action);
        commitSnapshot(seq, next);
        notify(tRef.current('common.done'));
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      } finally {
        setBusy(false);
      }
    },
    [client, commitSnapshot, nextSnapshotSeq, notify],
  );

  const resolveIssuesBatch = useCallback(
    async (items: ReconcileBatchItem[]) => {
      if (items.length === 0) return null;
      setBusy(true);
      const seq = nextSnapshotSeq();
      try {
        const report = await client.resolveReconcileIssuesBatch(items);
        commitSnapshot(seq, report.snapshot);
        const okCount = report.items.filter((item) => item.ok).length;
        const failedCount = report.items.length - okCount;
        if (failedCount > 0) {
          const firstErr = report.items.find((item) => !item.ok)?.error?.message;
          const summary = tRef.current('reconcile.batchPartial', {
            ok: okCount,
            failed: failedCount,
          });
          notify(firstErr ? `${summary} · ${firstErr}` : summary, { tone: 'error' });
        } else {
          notify(tRef.current('reconcile.batchDone', { ok: okCount }));
        }
        return report;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      } finally {
        setBusy(false);
      }
    },
    [client, commitSnapshot, nextSnapshotSeq, notify],
  );

  const runTransfer = useCallback(
    async (workshopId: string, kind: 'download' | 'update') => {
      if (liveDownloadsRef.current[workshopId]) {
        return null;
      }
      downloadSeqRef.current += 1;
      const requestId = downloadSeqRef.current;

      notify(
        tRef.current(kind === 'update' ? 'toast.updateStarted' : 'toast.downloadStarted', {
          id: workshopId,
        }),
      );

      setLiveDownloads((prev) => ({
        ...prev,
        [workshopId]: {
          requestId,
          taskId: `pending-${requestId}-${workshopId}`,
          workshopId,
          downloaded: 0,
          percent: 0,
          speedBps: 0,
          cancelling: false,
          updatedAt: Date.now(),
        },
      }));

      try {
        const summary =
          kind === 'update'
            ? await client.updateWorkshopItem(workshopId)
            : await client.downloadWorkshopItem(workshopId);
        setLiveDownloads((prev) => {
          if (prev[workshopId]?.requestId !== requestId) return prev;
          const copy = { ...prev };
          delete copy[workshopId];
          return copy;
        });
        await refresh();
        notify(
          tRef.current(kind === 'update' ? 'toast.updateDone' : 'toast.downloadDone', {
            title: summary.title,
          }),
        );
        return summary;
      } catch (err) {
        setLiveDownloads((prev) => {
          if (prev[workshopId]?.requestId !== requestId) return prev;
          const copy = { ...prev };
          delete copy[workshopId];
          return copy;
        });
        await refresh();
        if (err instanceof IpcError && err.code === 'cancelled') {
          notify(tRef.current('toast.downloadCancelled', { id: workshopId }));
        } else {
          notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        }
        return null;
      }
    },
    [client, notify, refresh],
  );

  const downloadWorkshopItem = useCallback(
    (workshopId: string) => runTransfer(workshopId, 'download'),
    [runTransfer],
  );

  const updateWorkshopItem = useCallback(
    (workshopId: string) => runTransfer(workshopId, 'update'),
    [runTransfer],
  );

  const cancelDownload = useCallback(
    async (workshopId: string) => {
      setLiveDownloads((prev) => {
        const current = prev[workshopId];
        if (!current) return prev;
        return { ...prev, [workshopId]: { ...current, cancelling: true } };
      });
      try {
        const accepted = await client.cancelDownload(workshopId);
        if (!accepted) {
          setLiveDownloads((prev) => {
            if (!(workshopId in prev)) return prev;
            const copy = { ...prev };
            delete copy[workshopId];
            return copy;
          });
          notify(tRef.current('toast.cancelUnavailable', { id: workshopId }));
          await refresh();
        }
      } catch (err) {
        setLiveDownloads((prev) => {
          const current = prev[workshopId];
          if (!current) return prev;
          return { ...prev, [workshopId]: { ...current, cancelling: false } };
        });
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      }
    },
    [client, notify, refresh],
  );

  const rebuildBundles = useCallback(async () => {
    setBusy(true);
    try {
      const bundles = await client.rebuildBundles();
      const refreshed = await refresh();
      if (refreshed) {
        notify(tRef.current('toast.bundleRebuilt', { count: bundles.length }));
      }
    } catch (err) {
      notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
    } finally {
      setBusy(false);
    }
  }, [client, notify, refresh]);

  const createManualBundle = useCallback(
    async (name: string, memberAddonIds: string[]) => {
      setBusy(true);
      try {
        await client.createManualBundle(name, memberAddonIds);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
        return true;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return false;
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh],
  );

  const deleteBundle = useCallback(
    async (bundleId: string) => {
      setBusy(true);
      try {
        await client.deleteBundle(bundleId);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh],
  );

  const setBundleLocked = useCallback(
    async (bundleId: string, locked: boolean) => {
      setBusy(true);
      try {
        await client.setBundleLocked(bundleId, locked);
        await refresh();
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh],
  );

  const trackCollection = useCallback(
    async (collectionId: string, autoSync: boolean) => {
      setBusy(true);
      try {
        const record = await client.trackCollection(collectionId, autoSync);
        const refreshed = await refresh();
        if (refreshed) {
          notify(tRef.current('toast.collectionTracked', { title: record.title }));
        }
        return record;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh],
  );

  const untrackCollection = useCallback(
    async (collectionId: string) => {
      setBusy(true);
      try {
        await client.untrackCollection(collectionId);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh],
  );

  const saveUserGroup = useCallback(
    async (group: UserGroup) => {
      setBusy(true);
      try {
        await client.saveUserGroup(group);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
        return true;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return false;
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh],
  );

  const deleteUserGroup = useCallback(
    async (groupId: string) => {
      setBusy(true);
      try {
        await client.deleteUserGroup(groupId);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh],
  );

  const saveProfile = useCallback(
    async (profile: LoadoutProfile) => {
      setBusy(true);
      try {
        await client.saveProfile(profile);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
        return true;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return false;
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh],
  );

  const deleteProfile = useCallback(
    async (profileId: string) => {
      setBusy(true);
      try {
        await client.deleteProfile(profileId);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh],
  );

  const applyProfile = useCallback(
    async (profileId: string) => {
      setBusy(true);
      try {
        const report = await client.applyProfile(profileId);
        const refreshed = await refresh();
        const counts = summarizeBatchReport(report.batch);
        if (counts.hasIssue) {
          setBatchReport({
            batch: report.batch,
            profileSummary: {
              enabledCount: report.enabledCount,
              disabledCount: report.disabledCount,
              skippedWorkshopOnly: report.skippedWorkshopOnly,
              missingAddons: report.missingAddons,
            },
          });
        } else if (refreshed) {
          const parts = [
            tRef.current('organize.profileApplied', {
              enabled: report.enabledCount,
              disabled: report.disabledCount,
            }),
          ];
          if (report.skippedWorkshopOnly.length > 0) {
            parts.push(
              tRef.current('organize.profileSkipped', {
                count: report.skippedWorkshopOnly.length,
              }),
            );
          }
          if (report.missingAddons.length > 0) {
            parts.push(
              tRef.current('organize.profileMissing', { count: report.missingAddons.length }),
            );
          }
          notify(parts.join(' · '));
        }
        return report;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh],
  );

  const searchWorkshop = useCallback(
    async (query: SearchQuery) => {
      return client.searchWorkshop(query);
    },
    [client],
  );

  const fetchWorkshopItem = useCallback(
    async (workshopId: string) => {
      try {
        return await client.fetchWorkshopItem(workshopId);
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      }
    },
    [client, notify],
  );

  const getSubscribedWorkshopIds = useCallback(async () => {
    try {
      return await client.getSubscribedWorkshopIds();
    } catch (err) {
      notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      return null;
    }
  }, [client, notify]);

  const searchByMapCode = useCallback(
    async (query: string) => {
      return client.searchByMapCode(query);
    },
    [client],
  );

  const openExternalUrl = useCallback(
    async (url: string): Promise<boolean> => {
      try {
        await client.openExternalUrl(url);
        return true;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return false;
      }
    },
    [client, notify],
  );

  const openInFileManager = useCallback(
    async (path: string): Promise<boolean> => {
      try {
        await client.openInFileManager(path);
        if (!isTauri) {
          notify(tRef.current('toast.previewFileManager'));
        }
        return true;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return false;
      }
    },
    [client, isTauri, notify],
  );

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
    liveDownloads,
    batchReport,
    setBatchReport,
    snackbar,
    notify,
    dismissSnackbar,
    bootstrap,
    refresh,
    refreshSteamCapabilities,
    scan,
    sync,
    saveConfig,
    detectGamePath,
    toggleAddons,
    moveAddons,
    previewRename,
    applyRenames,
    deleteAddons,
    safeDeleteObsoleteDependency,
    resolveIssue,
    resolveIssuesBatch,
    downloadWorkshopItem,
    updateWorkshopItem,
    cancelDownload,
    rebuildBundles,
    createManualBundle,
    deleteBundle,
    setBundleLocked,
    trackCollection,
    untrackCollection,
    saveUserGroup,
    deleteUserGroup,
    saveProfile,
    deleteProfile,
    applyProfile,
    searchWorkshop,
    fetchWorkshopItem,
    getSubscribedWorkshopIds,
    searchByMapCode,
    openExternalUrl,
    openInFileManager,
  };
}

export type AppController = ReturnType<typeof useAppController>;
