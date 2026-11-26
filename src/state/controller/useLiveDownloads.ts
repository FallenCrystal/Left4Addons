import type { RefObject } from 'react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  DashboardSnapshot,
  FilterItemVerdict,
  LocalFilterDeploymentStatus,
} from '../../api/ipc';
import { IpcError } from '../../api/ipc';
import type {
  DesktopClient,
  DownloadSpeedTracker,
  LiveDownloadState,
  NotifyFn,
  TranslateFn,
} from './types';

export interface UseLiveDownloadsParams {
  client: DesktopClient;
  snapshotRef: RefObject<DashboardSnapshot | null>;
  tRef: RefObject<TranslateFn>;
  onOpenReconcileRef: RefObject<(() => void) | undefined>;
  notify: NotifyFn;
  refresh: () => Promise<boolean>;
  onFilterVerdictsUpdated?: (
    verdicts: FilterItemVerdict[],
    policySignature?: string,
    cacheGeneration?: number,
  ) => void;
  onLocalFilterStatusChanged?: (status: LocalFilterDeploymentStatus) => void;
}

export function useLiveDownloads({
  client,
  snapshotRef,
  tRef,
  onOpenReconcileRef,
  notify,
  refresh,
  onFilterVerdictsUpdated,
  onLocalFilterStatusChanged,
}: UseLiveDownloadsParams) {
  const [liveDownloads, setLiveDownloads] = useState<Record<string, LiveDownloadState>>({});
  const downloadSeqRef = useRef(0);
  const liveDownloadsRef = useRef<Record<string, LiveDownloadState>>({});
  const speedTrackersRef = useRef<Map<string, DownloadSpeedTracker>>(new Map());

  useEffect(() => {
    liveDownloadsRef.current = liveDownloads;
  }, [liveDownloads]);

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
          if (ev.percent >= 100) {
            speedTrackersRef.current.delete(ev.workshopId);
            setLiveDownloads((prev) => {
              if (!(ev.workshopId in prev)) return prev;
              const copy = { ...prev };
              delete copy[ev.workshopId];
              return copy;
            });
            return;
          }
          const hasBackendSpeed =
            typeof ev.speedBps === 'number' && Number.isFinite(ev.speedBps) && ev.speedBps >= 0;
          const prevTracker = speedTrackersRef.current.get(ev.workshopId);
          if (!prevTracker || ev.downloaded < prevTracker.lastSampleBytes) {
            speedTrackersRef.current.set(ev.workshopId, {
              latestDownloaded: ev.downloaded,
              lastSampleBytes: ev.downloaded,
              lastSampleAt: now,
              lastEventAt: now,
              backendSpeed: hasBackendSpeed,
            });
          } else {
            prevTracker.latestDownloaded = ev.downloaded;
            prevTracker.lastEventAt = now;
            prevTracker.backendSpeed = hasBackendSpeed;
          }
          setLiveDownloads((prev) => {
            const existing = prev[ev.workshopId];
            const fallbackTitle = snapshotRef.current?.addons.find(
              (a) => a.workshopId === ev.workshopId,
            )?.title;
            const title = ev.title?.trim() || existing?.title || fallbackTitle;
            return {
              ...prev,
              [ev.workshopId]: {
                requestId: existing?.requestId ?? 0,
                taskId: ev.taskId,
                workshopId: ev.workshopId,
                title,
                downloaded: ev.downloaded,
                total: ev.total ?? undefined,
                percent: ev.percent,
                speedBps: hasBackendSpeed ? Math.round(ev.speedBps!) : (existing?.speedBps ?? 0),
                hasProgress: true,
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
        } else if (ev.type === 'filter_verdicts_updated') {
          onFilterVerdictsUpdated?.(ev.verdicts, ev.policySignature ?? undefined, ev.cacheGeneration);
        } else if (ev.type === 'local_filter_status_changed') {
          onLocalFilterStatusChanged?.(ev.status);
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
    };
  }, [
    client,
    notify,
    onFilterVerdictsUpdated,
    onLocalFilterStatusChanged,
    onOpenReconcileRef,
    refresh,
    snapshotRef,
    tRef,
  ]);

  const hasLiveDownloads = Object.keys(liveDownloads).length > 0;

  useEffect(() => {
    if (!hasLiveDownloads) return;
    const timer = window.setInterval(() => {
      const now = Date.now();
      const current = liveDownloadsRef.current;
      const updates = new Map<string, number>();

      for (const [wid, dl] of Object.entries(current)) {
        const tracker = speedTrackersRef.current.get(wid);
        if (!dl.hasProgress || !tracker) continue;

        if (tracker.backendSpeed) {
          if (now - tracker.lastEventAt >= 1500 && dl.speedBps !== 0) {
            updates.set(wid, 0);
          }
        } else {
          const elapsedSec = (now - tracker.lastSampleAt) / 1000;
          if (elapsedSec > 0) {
            const delta = Math.max(0, tracker.latestDownloaded - tracker.lastSampleBytes);
            const nextSpeed = Math.round(delta / elapsedSec);
            tracker.lastSampleBytes = tracker.latestDownloaded;
            tracker.lastSampleAt = now;
            if (nextSpeed !== dl.speedBps) {
              updates.set(wid, nextSpeed);
            }
          }
        }
      }

      if (updates.size === 0) return;

      setLiveDownloads((prev) => {
        let changed = false;
        const next: Record<string, LiveDownloadState> = {};
        for (const [wid, dl] of Object.entries(prev)) {
          const updatedSpeed = updates.get(wid);
          if (updatedSpeed !== undefined && updatedSpeed !== dl.speedBps) {
            changed = true;
            next[wid] = { ...dl, speedBps: updatedSpeed };
          } else {
            next[wid] = dl;
          }
        }
        return changed ? next : prev;
      });
    }, 1000);

    return () => {
      window.clearInterval(timer);
    };
  }, [hasLiveDownloads]);

  const runTransfer = useCallback(
    async (workshopId: string, kind: 'download' | 'update', titleHint?: string) => {
      if (liveDownloadsRef.current[workshopId]) {
        return null;
      }
      downloadSeqRef.current += 1;
      const requestId = downloadSeqRef.current;
      const initialTitle =
        titleHint?.trim() ||
        snapshotRef.current?.addons.find((a) => a.workshopId === workshopId)?.title;

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
          title: initialTitle,
          downloaded: 0,
          percent: 0,
          speedBps: 0,
          hasProgress: false,
          cancelling: false,
          updatedAt: Date.now(),
        },
      }));

      try {
        const summary =
          kind === 'update'
            ? await client.updateWorkshopItem(workshopId, initialTitle)
            : await client.downloadWorkshopItem(workshopId, initialTitle);
        speedTrackersRef.current.delete(workshopId);
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
        speedTrackersRef.current.delete(workshopId);
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
    [client, notify, refresh, snapshotRef, tRef],
  );

  const downloadWorkshopItem = useCallback(
    (workshopId: string, title?: string) => runTransfer(workshopId, 'download', title),
    [runTransfer],
  );

  const updateWorkshopItem = useCallback(
    (workshopId: string, title?: string) => runTransfer(workshopId, 'update', title),
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
          speedTrackersRef.current.delete(workshopId);
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
    [client, notify, refresh, tRef],
  );

  return {
    liveDownloads,
    downloadWorkshopItem,
    updateWorkshopItem,
    cancelDownload,
  };
}
