import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Addon, BackgroundTask, DatabasePayload } from '../types/addon';
import { fetchWorkshopItem } from '../services/workshopClient';

const WORKSHOP_CRAWL_COOLDOWN_MS = 6000;

interface UseBackgroundTasksArgs {
  enabled: boolean;
  downloadConcurrency: number;
  addons: Record<string, Addon>;
  knownUninstalledAddons: Record<string, Addon>;
  updateLocalState: (data: DatabasePayload) => void;
  onDownloadSuccess: (workshopId: string) => void;
  onDownloadCancelled: (workshopId: string) => void;
  onTaskError: (message: string, workshopId?: string) => void;
}

const nowIso = () => new Date().toISOString();
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

const taskId = (kind: BackgroundTask['kind'], targetId: string) =>
  `${kind}_${targetId}_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`;

const isDatabasePayload = (value: unknown): value is DatabasePayload => (
  !!value &&
  typeof value === 'object' &&
  (
    'addons' in value ||
    'knownUninstalledAddons' in value ||
    'groups' in value ||
    'settings' in value
  )
);

function trimTasks(tasks: BackgroundTask[]) {
  return tasks.slice(-500);
}

export function useBackgroundTasks({
  enabled,
  downloadConcurrency,
  addons,
  knownUninstalledAddons,
  updateLocalState,
  onDownloadSuccess,
  onDownloadCancelled,
  onTaskError,
}: UseBackgroundTasksArgs) {
  const [backgroundTasks, setBackgroundTasks] = useState<BackgroundTask[]>([]);
  const tasksRef = useRef<BackgroundTask[]>([]);
  const activeDownloadsRef = useRef(0);
  const crawlRunningRef = useRef(false);
  const lastCrawlAtRef = useRef(0);
  const processDownloadQueueRef = useRef<() => void>(() => {});
  const processCrawlQueueRef = useRef<() => void>(() => {});

  const commitTasks = useCallback((tasks: BackgroundTask[]) => {
    const next = trimTasks(tasks);
    tasksRef.current = next;
    setBackgroundTasks(next);
    void invoke('save_background_task_snapshot', { tasks: next }).catch((err) => {
      console.error('Failed to save background task snapshot:', err);
    });
  }, []);

  const patchTask = useCallback((id: string, patch: Partial<BackgroundTask>) => {
    commitTasks(tasksRef.current.map((task) => {
      if (task.id !== id) return task;
      if (task.status === 'cancelled' && patch.status && patch.status !== 'cancelled') {
        return task;
      }
      return { ...task, ...patch };
    }));
  }, [commitTasks]);

  useEffect(() => {
    void invoke<BackgroundTask[]>('get_background_tasks')
      .then((tasks) => {
        const safeTasks = Array.isArray(tasks) ? tasks : [];
        const restored = safeTasks.map((task) => (
          task.status === 'running' ? { ...task, status: 'queued' as const } : task
        ));
        commitTasks(restored);
        setTimeout(() => {
          processDownloadQueueRef.current();
          processCrawlQueueRef.current();
        }, 0);
      })
      .catch((err) => console.error('Failed to load background tasks:', err));
  }, [commitTasks]);

  const runDownloadTask = useCallback(async (task: BackgroundTask) => {
    const workshopId = task.targetIds[0];
    if (!workshopId) return;

    activeDownloadsRef.current += 1;
    patchTask(task.id, { status: 'running', progress: 0, startedAt: nowIso(), error: undefined });

    try {
      const data = await invoke<DatabasePayload>('download_addon', { workshopId });
      if (isDatabasePayload(data)) {
        updateLocalState(data);
      }
      const currentTask = tasksRef.current.find((t) => t.id === task.id);
      if (currentTask?.status === 'cancelled') {
        onDownloadCancelled(workshopId);
        return;
      }
      patchTask(task.id, { status: 'completed', progress: 100, finishedAt: nowIso() });
      onDownloadSuccess(workshopId);
    } catch (err) {
      const message = String(err);
      const currentTask = tasksRef.current.find((t) => t.id === task.id);
      if (currentTask?.status === 'cancelled') {
        onDownloadCancelled(workshopId);
      } else {
        patchTask(task.id, { status: 'failed', error: message, finishedAt: nowIso() });
        onTaskError(message, workshopId);
      }
    } finally {
      activeDownloadsRef.current = Math.max(0, activeDownloadsRef.current - 1);
      processDownloadQueueRef.current();
    }
  }, [onDownloadCancelled, onDownloadSuccess, onTaskError, patchTask, updateLocalState]);

  const processDownloadQueue = useCallback(() => {
    const concurrency = Math.max(1, Math.trunc(downloadConcurrency || 1));
    while (activeDownloadsRef.current < concurrency) {
      const next = tasksRef.current.find((task) => task.kind === 'download' && task.status === 'queued');
      if (!next) return;
      void runDownloadTask(next);
    }
  }, [downloadConcurrency, runDownloadTask]);

  useEffect(() => {
    processDownloadQueueRef.current = processDownloadQueue;
  }, [processDownloadQueue]);

  useEffect(() => {
    processDownloadQueueRef.current();
  }, [downloadConcurrency]);

  const enqueueDownloads = useCallback((
    items: (string | { workshopId: string; title?: string; imagePath?: string })[],
    source = 'user'
  ) => {
    const normalized = items.map((item) => {
      if (typeof item === 'string') {
        return { workshopId: item.trim() };
      }
      return {
        workshopId: item.workshopId.trim(),
        title: item.title,
        imagePath: item.imagePath,
      };
    }).filter((x) => !!x.workshopId);

    if (normalized.length === 0) return;

    const existing = new Set(
      tasksRef.current
        .filter((task) => task.kind === 'download' && ['queued', 'running'].includes(task.status))
        .map((task) => task.targetIds[0])
    );
    const createdAt = nowIso();
    const newTasks = normalized
      .filter((item) => !existing.has(item.workshopId))
      .map<BackgroundTask>((item) => {
        const id = item.workshopId;
        const ad = Object.values(addons).find((a) => a.workshopId === id) ||
                   Object.values(knownUninstalledAddons).find((a) => a.workshopId === id);
        const resolvedTitle = item.title || ad?.steamDetails?.title || ad?.addonInfo?.addontitle || ad?.vpkName;
        const taskTitle = resolvedTitle && resolvedTitle !== id ? `${resolvedTitle} (${id})` : id;
        return {
          id: taskId('download', id),
          kind: 'download',
          status: 'queued',
          source,
          targetIds: [id],
          progress: 0,
          createdAt,
          title: taskTitle,
          imagePath: item.imagePath || ad?.imagePath || undefined,
        };
      });

    if (newTasks.length === 0) return;
    commitTasks([...tasksRef.current, ...newTasks]);
    processDownloadQueueRef.current();
  }, [addons, knownUninstalledAddons, commitTasks]);

  const runCrawlTask = useCallback(async (task: BackgroundTask) => {
    const workshopId = task.targetIds[0];
    if (!workshopId) return;

    crawlRunningRef.current = true;
    patchTask(task.id, { status: 'running', progress: 0, startedAt: nowIso(), error: undefined });

    const waitMs = Math.max(0, WORKSHOP_CRAWL_COOLDOWN_MS - (Date.now() - lastCrawlAtRef.current));
    if (waitMs > 0) {
      await wait(waitMs);
    }

    try {
      lastCrawlAtRef.current = Date.now();
      const itemResult = await fetchWorkshopItem(workshopId);
      const data = await invoke<DatabasePayload>('record_workshop_items_seen', {
        items: [itemResult.item],
        source: task.source || 'background-refresh',
      });
      if (isDatabasePayload(data)) {
        updateLocalState(data);
      }
      patchTask(task.id, { status: 'completed', progress: 100, finishedAt: nowIso() });
    } catch (err) {
      const message = String(err);
      patchTask(task.id, { status: 'failed', error: message, finishedAt: nowIso() });
      console.error('Background workshop crawl failed:', message);
    } finally {
      crawlRunningRef.current = false;
      processCrawlQueueRef.current();
    }
  }, [patchTask, updateLocalState]);

  const processCrawlQueue = useCallback(() => {
    if (crawlRunningRef.current) return;
    const next = tasksRef.current.find((task) => task.kind === 'workshop-crawl' && task.status === 'queued');
    if (!next) return;
    void runCrawlTask(next);
  }, [runCrawlTask]);

  useEffect(() => {
    processCrawlQueueRef.current = processCrawlQueue;
  }, [processCrawlQueue]);

  useEffect(() => {
    if (!enabled) return;

    const knownWorkshopIds = new Set(
      [...Object.values(addons), ...Object.values(knownUninstalledAddons)]
        .map((addon) => addon.workshopId?.trim())
        .filter((id): id is string => !!id)
    );

    const filteredTasks = tasksRef.current.filter((task) => (
      task.kind !== 'workshop-crawl' ||
      task.status === 'running' ||
      knownWorkshopIds.has(task.targetIds[0]?.trim() || '')
    ));

    if (filteredTasks.length !== tasksRef.current.length) {
      commitTasks(filteredTasks);
    }
  }, [addons, commitTasks, enabled, knownUninstalledAddons]);

  const enqueueWorkshopCrawl = useCallback((workshopIds: string[], source = 'background-refresh') => {
    const cleanIds = [...new Set(workshopIds.map((id) => id.trim()).filter(Boolean))];
    if (cleanIds.length === 0) return;

    const existing = new Set(
      tasksRef.current
        .filter((task) => task.kind === 'workshop-crawl' && ['queued', 'running'].includes(task.status))
        .map((task) => task.targetIds[0])
    );
    const createdAt = nowIso();
    const newTasks = cleanIds
      .filter((id) => !existing.has(id))
      .map<BackgroundTask>((id) => {
        const ad = Object.values(addons).find((a) => a.workshopId === id) ||
                   Object.values(knownUninstalledAddons).find((a) => a.workshopId === id);
        const resolvedTitle = ad?.steamDetails?.title || ad?.addonInfo?.addontitle || ad?.vpkName;
        const taskTitle = resolvedTitle && resolvedTitle !== id ? `${resolvedTitle} (${id})` : id;
        return {
          id: taskId('workshop-crawl', id),
          kind: 'workshop-crawl',
          status: 'queued',
          source,
          targetIds: [id],
          progress: 0,
          createdAt,
          title: taskTitle,
          imagePath: ad?.imagePath || undefined,
        };
      });

    if (newTasks.length === 0) return;
    commitTasks([...tasksRef.current, ...newTasks]);
    processCrawlQueueRef.current();
  }, [addons, knownUninstalledAddons, commitTasks]);

  const recordSeenItems = useCallback(async (items: any[], source = 'workshop-browser') => {
    if (items.length === 0) return;
    try {
      const data = await invoke<DatabasePayload>('record_workshop_items_seen', { items, source });
      if (isDatabasePayload(data)) {
        updateLocalState(data);
      }
    } catch (err) {
      console.error('Failed to record seen workshop items:', err);
    }
  }, [updateLocalState]);

  const cancelTask = useCallback((id: string) => {
    const task = tasksRef.current.find((t) => t.id === id);
    if (!task) return;

    patchTask(id, { status: 'cancelled', finishedAt: nowIso() });

    if (task.kind === 'download') {
      const workshopId = task.targetIds[0];
      if (workshopId) {
        onDownloadCancelled(workshopId);
        if (task.status === 'running') {
          void invoke('cancel_download', { workshopId }).catch((err) => {
            console.error('Failed to cancel download:', err);
          });
        }
      }
    }

    if (task.status !== 'queued') return;

    if (task.kind === 'download') {
      setTimeout(() => processDownloadQueueRef.current(), 0);
    } else if (task.kind === 'workshop-crawl') {
      setTimeout(() => processCrawlQueueRef.current(), 0);
    }
  }, [onDownloadCancelled, patchTask]);

  const retryTask = useCallback((id: string) => {
    const task = tasksRef.current.find((t) => t.id === id);
    if (!task) return;

    patchTask(id, {
      status: 'queued',
      progress: 0,
      error: undefined,
      createdAt: nowIso(),
      startedAt: undefined,
      finishedAt: undefined
    });

    if (task.kind === 'download') {
      setTimeout(() => processDownloadQueueRef.current(), 0);
    } else if (task.kind === 'workshop-crawl') {
      setTimeout(() => processCrawlQueueRef.current(), 0);
    }
  }, [patchTask]);

  const clearFinishedTasks = useCallback(() => {
    const activeTasks = tasksRef.current.filter(
      (task) => task.status === 'queued' || task.status === 'running'
    );
    commitTasks(activeTasks);
  }, [commitTasks]);

  return {
    backgroundTasks,
    enqueueDownloads,
    enqueueWorkshopCrawl,
    recordSeenItems,
    cancelTask,
    retryTask,
    clearFinishedTasks,
  };
}
