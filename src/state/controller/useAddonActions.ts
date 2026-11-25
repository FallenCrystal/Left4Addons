import type { Dispatch, RefObject, SetStateAction } from 'react';
import { useCallback } from 'react';
import type {
  AddonBatchReport,
  AddonDirType,
  AppConfig,
  DashboardSnapshot,
  ReconcileAction,
  ReconcileBatchItem,
  RenameRuleConfig,
  SearchQuery,
  SteamCapabilities,
  SyncReport,
} from '../../api/ipc';
import { summarizeBatchReport } from '../../utils/format';
import type { BatchDialogPayload, DesktopClient, NotifyFn, TranslateFn } from './types';

export interface UseAddonActionsParams {
  client: DesktopClient;
  isTauri: boolean;
  snapshotRef: RefObject<DashboardSnapshot | null>;
  tRef: RefObject<TranslateFn>;
  commitSnapshot: (seq: number, next: DashboardSnapshot) => boolean;
  commitSavedConfig: (config: AppConfig) => void;
  nextSnapshotSeq: () => number;
  notify: NotifyFn;
  refresh: () => Promise<boolean>;
  setScanning: Dispatch<SetStateAction<boolean>>;
  setSyncing: Dispatch<SetStateAction<boolean>>;
  setBusy: Dispatch<SetStateAction<boolean>>;
  setSyncReport: Dispatch<SetStateAction<SyncReport | null>>;
  setSteamCaps: Dispatch<SetStateAction<SteamCapabilities | null>>;
  setBatchReport: Dispatch<SetStateAction<BatchDialogPayload | null>>;
  onClearFailedFilterEvaluations?: () => void;
}

export function useAddonActions({
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
  onClearFailedFilterEvaluations,
}: UseAddonActionsParams) {
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
    [notify, setBatchReport, tRef],
  );

  const scan = useCallback(async (): Promise<boolean> => {
    setScanning(true);
    onClearFailedFilterEvaluations?.();
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
  }, [
    client,
    commitSnapshot,
    nextSnapshotSeq,
    notify,
    onClearFailedFilterEvaluations,
    setScanning,
    tRef,
  ]);

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
  }, [client, notify, refresh, setSyncReport, setSyncing]);

  const saveConfig = useCallback(
    async (config: AppConfig, forceRescan = false, silent = false): Promise<boolean> => {
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
        commitSavedConfig(await client.saveConfig(config));
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
        } else if (!silent) {
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
    [
      client,
      commitSavedConfig,
      commitSnapshot,
      nextSnapshotSeq,
      notify,
      scan,
      setBusy,
      setScanning,
      setSteamCaps,
      snapshotRef,
      tRef,
    ],
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
    [client, handleBatchReport, notify, refresh, setBusy],
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
    [client, handleBatchReport, notify, refresh, setBusy],
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
    [client, handleBatchReport, notify, refresh, setBusy],
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
    [client, handleBatchReport, notify, refresh, setBusy],
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
    [client, handleBatchReport, notify, refresh, setBusy],
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
    [client, commitSnapshot, nextSnapshotSeq, notify, setBusy, tRef],
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
    [client, commitSnapshot, nextSnapshotSeq, notify, setBusy, tRef],
  );

  const searchWorkshop = useCallback(
    async (query: SearchQuery) => {
      onClearFailedFilterEvaluations?.();
      return client.searchWorkshop(query);
    },
    [client, onClearFailedFilterEvaluations],
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
    [client, isTauri, notify, tRef],
  );

  return {
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
    searchWorkshop,
    fetchWorkshopItem,
    getSubscribedWorkshopIds,
    searchByMapCode,
    openExternalUrl,
    openInFileManager,
  };
}
