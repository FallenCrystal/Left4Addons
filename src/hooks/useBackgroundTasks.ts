import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Addon, BackgroundTask, DatabasePayload } from '../types/addon';
import { fetchWorkshopPageDetails, persistWorkshopPageDetails } from '../services/workshopClient';

const DOWNLOAD_CONCURRENCY = 3;
const WORKSHOP_CRAWL_COOLDOWN_MS = 5000;
const WORKSHOP_CACHE_TTL_MS = 24 * 60 * 60 * 1000;

interface UseBackgroundTasksArgs {
  enabled: boolean;
  addons: Record<string, Addon>;
  knownUninstalledAddons: Record<string, Addon>;
  updateLocalState: (data: DatabasePayload) => void;
  onDownloadSuccess: (workshopId: string) => void;
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
  addons,
  knownUninstalledAddons,
  updateLocalState,
  onDownloadSuccess,
  onTaskError,
}: UseBackgroundTasksArgs) {
  const [backgroundTasks, setBackgroundTasks] = useState<BackgroundTask[]>([]);
  const tasksRef = useRef<BackgroundTask[]>([]);
  const activeDownloadsRef = useRef(0);
  const crawlRunningRef = useRef(false);
  const lastCrawlAtRef = useRef(0);
  const startupQueuedRef = useRef(false);
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
    commitTasks(tasksRef.current.map((task) => (
      task.id === id ? { ...task, ...patch } : task
    )));
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
      patchTask(task.id, { status: 'completed', progress: 100, finishedAt: nowIso() });
      onDownloadSuccess(workshopId);
    } catch (err) {
      const message = String(err);
      patchTask(task.id, { status: 'failed', error: message, finishedAt: nowIso() });
      onTaskError(message, workshopId);
    } finally {
      activeDownloadsRef.current = Math.max(0, activeDownloadsRef.current - 1);
      processDownloadQueueRef.current();
    }
  }, [onDownloadSuccess, onTaskError, patchTask, updateLocalState]);

  const processDownloadQueue = useCallback(() => {
    while (activeDownloadsRef.current < DOWNLOAD_CONCURRENCY) {
      const next = tasksRef.current.find((task) => task.kind === 'download' && task.status === 'queued');
      if (!next) return;
      void runDownloadTask(next);
    }
  }, [runDownloadTask]);

  useEffect(() => {
    processDownloadQueueRef.current = processDownloadQueue;
  }, [processDownloadQueue]);

  const enqueueDownloads = useCallback((workshopIds: string[], source = 'user') => {
    const cleanIds = [...new Set(workshopIds.map((id) => id.trim()).filter(Boolean))];
    if (cleanIds.length === 0) return;

    const existing = new Set(
      tasksRef.current
        .filter((task) => task.kind === 'download' && ['queued', 'running'].includes(task.status))
        .map((task) => task.targetIds[0])
    );
    const createdAt = nowIso();
    const newTasks = cleanIds
      .filter((id) => !existing.has(id))
      .map<BackgroundTask>((id) => ({
        id: taskId('download', id),
        kind: 'download',
        status: 'queued',
        source,
        targetIds: [id],
        progress: 0,
        createdAt,
      }));

    if (newTasks.length === 0) return;
    commitTasks([...tasksRef.current, ...newTasks]);
    processDownloadQueueRef.current();
  }, [commitTasks]);

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
      const details = await fetchWorkshopPageDetails(workshopId, task.source || 'background-refresh');
      const data = await persistWorkshopPageDetails(workshopId, details, task.source || 'background-refresh') as DatabasePayload;
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
      .map<BackgroundTask>((id) => ({
        id: taskId('workshop-crawl', id),
        kind: 'workshop-crawl',
        status: 'queued',
        source,
        targetIds: [id],
        progress: 0,
        createdAt,
      }));

    if (newTasks.length === 0) return;
    commitTasks([...tasksRef.current, ...newTasks]);
    processCrawlQueueRef.current();
  }, [commitTasks]);

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

  useEffect(() => {
    if (!enabled || startupQueuedRef.current) return;
    startupQueuedRef.current = true;

    void (async () => {
      try {
        const cache = await invoke<Record<string, any>>('get_workshop_cache');
        const candidateIds = new Set<string>();

        [...Object.values(addons), ...Object.values(knownUninstalledAddons)].forEach((addon) => {
          if (addon.workshopId) candidateIds.add(addon.workshopId);
        });

        const staleIds = [...candidateIds].filter((id) => {
          const lastFetched = cache?.[id]?.lastPageFetchedAt;
          if (!lastFetched) return true;
          const timestamp = Date.parse(lastFetched);
          return Number.isNaN(timestamp) || Date.now() - timestamp > WORKSHOP_CACHE_TTL_MS;
        });

        enqueueWorkshopCrawl(staleIds, 'startup-auto');
      } catch (err) {
        console.error('Failed to enqueue startup workshop crawl:', err);
      }
    })();
  }, [addons, enabled, enqueueWorkshopCrawl, knownUninstalledAddons]);

  // Future task-manager UI should consume these values/actions instead of invoking
  // download/crawl commands directly.
  return {
    backgroundTasks,
    enqueueDownloads,
    enqueueWorkshopCrawl,
    recordSeenItems,
  };
}
