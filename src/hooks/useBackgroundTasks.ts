import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Addon, BackgroundTask, DatabasePayload, Settings } from '../types/addon';
import { fetchWorkshopDependencySnapshot, fetchWorkshopItem } from '../services/workshopClient';

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
const isNetworkError = (msg: string) => msg.includes('error decoding body') || 
                                        msg.includes('Download request failed') || 
                                        msg.includes('Download chunk failed') || 
                                        msg.includes('error sending request');

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
  const runningDownloadTaskIdsRef = useRef<Set<string>>(new Set());
  const crawlRunningRef = useRef(false);
  const dependencyRunningRef = useRef(false);
  const lastCrawlAtRef = useRef(0);
  const processDownloadQueueRef = useRef<() => void>(() => {});
  const processCrawlQueueRef = useRef<() => void>(() => {});
  const processDependencyQueueRef = useRef<() => void>(() => {});
  const enqueueDependencyCheckRef = useRef<(workshopIds: string[], source?: string) => string | null>(() => null);

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
    let active = true;

    void invoke<BackgroundTask[]>('get_background_tasks')
      .then((tasks) => {
        if (!active) return;

        const safeTasks = Array.isArray(tasks) ? tasks : [];
        const restored = safeTasks.map((task) => (
          task.status === 'running' ? { ...task, status: 'queued' as const } : task
        ));
        commitTasks(restored);
        setTimeout(() => {
          if (!active) return;
          processDownloadQueueRef.current();
          processCrawlQueueRef.current();
          processDependencyQueueRef.current();
        }, 0);
      })
      .catch((err) => console.error('Failed to load background tasks:', err));

    return () => {
      active = false;
    };
  }, [commitTasks]);

  const runDownloadTask = useCallback(async (task: BackgroundTask) => {
    const workshopId = task.targetIds[0];
    if (!workshopId || runningDownloadTaskIdsRef.current.has(task.id)) return;

    runningDownloadTaskIdsRef.current.add(task.id);
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
        let maxRetries = 3;
        try {
          const settings = await invoke<Settings>('get_settings');
          maxRetries = settings.maxDownloadRetries ?? 3;
        } catch (e) {
          console.warn('Failed to fetch settings for retry count', e);
        }
        const currentRetryCount = currentTask?.retryCount || 0;
        
        if (isNetworkError(message) && currentRetryCount < maxRetries) {
          patchTask(task.id, { 
            status: 'queued', 
            retryCount: currentRetryCount + 1,
            error: undefined,
            progress: 0,
            startedAt: undefined,
            finishedAt: undefined,
          });
        } else {
          patchTask(task.id, { status: 'failed', error: message, finishedAt: nowIso() });
          onTaskError(message, workshopId);
        }
      }
    } finally {
      runningDownloadTaskIdsRef.current.delete(task.id);
      activeDownloadsRef.current = Math.max(0, activeDownloadsRef.current - 1);
      processDownloadQueueRef.current();
    }
  }, [onDownloadCancelled, onDownloadSuccess, onTaskError, patchTask, updateLocalState]);

  const processDownloadQueue = useCallback(() => {
    const concurrency = Math.max(1, Math.trunc(downloadConcurrency || 1));
    while (activeDownloadsRef.current < concurrency) {
      const next = tasksRef.current.find((task) => (
        task.kind === 'download' &&
        task.status === 'queued' &&
        !runningDownloadTaskIdsRef.current.has(task.id)
      ));
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
    enqueueDependencyCheckRef.current(newTasks.map((task) => task.targetIds[0]), 'download-dependency-check');
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

  const runDependencyCheckTask = useCallback(async (task: BackgroundTask) => {
    const rootIds = task.dependencyCheck?.rootIds?.length
      ? task.dependencyCheck.rootIds
      : task.targetIds;
    const seedIds = task.dependencyCheck?.seedIds?.length
      ? task.dependencyCheck.seedIds
      : rootIds;
    if (seedIds.length === 0) return;

    dependencyRunningRef.current = true;
    const pending = [...new Set(seedIds.map((id) => id.trim()).filter(Boolean))];
    const visited = new Set<string>();
    const failedNodes: { workshopId: string; error: string }[] = [];
    const discoveredDependencies = new Map<string, {
      workshopId: string;
      title?: string;
      previewUrl?: string;
      creatorName?: string;
    }>();
    let completedCount = 0;
    let latestData: DatabasePayload | null = null;
    patchTask(task.id, {
      status: 'running',
      progress: 0,
      startedAt: nowIso(),
      finishedAt: undefined,
      error: undefined,
      dependencyCheck: {
        rootIds,
        seedIds,
        discoveredCount: pending.length,
        completedCount: 0,
        failedNodes: [],
        discoveredDependencies: [],
      },
    });

    try {
      while (pending.length > 0) {
        const currentTask = tasksRef.current.find((candidate) => candidate.id === task.id);
        if (currentTask?.status === 'cancelled') return;

        const workshopId = pending.shift()!;
        if (visited.has(workshopId)) continue;
        visited.add(workshopId);

        try {
          const details = await fetchWorkshopDependencySnapshot(workshopId);
          const data = await invoke<DatabasePayload>('persist_workshop_page_details', {
            workshopId,
            details,
            source: 'dependency-check',
          });
          if (isDatabasePayload(data)) {
            latestData = data;
          }

          const currentItem = discoveredDependencies.get(workshopId);
          discoveredDependencies.set(workshopId, {
            workshopId,
            title: details.title?.trim() || currentItem?.title,
            previewUrl: details.previewUrl?.trim() || currentItem?.previewUrl,
            creatorName: details.creatorName?.trim() || currentItem?.creatorName,
          });

          for (const dependency of details.requiredItems || []) {
            const dependencyId = dependency.workshopId?.trim();
            if (dependencyId) {
              const existing = discoveredDependencies.get(dependencyId);
              discoveredDependencies.set(dependencyId, {
                workshopId: dependencyId,
                title: existing?.title || dependency.title?.trim() || undefined,
                previewUrl: existing?.previewUrl,
                creatorName: existing?.creatorName,
              });
            }
            if (dependencyId && !visited.has(dependencyId) && !pending.includes(dependencyId)) {
              pending.push(dependencyId);
            }
          }
        } catch (err) {
          failedNodes.push({ workshopId, error: String(err) });
        }

        completedCount += 1;
        const discoveredCount = visited.size + pending.length;
        const currentProgress = tasksRef.current.find((candidate) => candidate.id === task.id)?.progress || 0;
        patchTask(task.id, {
          progress: Math.max(currentProgress, Math.min(99, Math.ceil((completedCount / Math.max(discoveredCount, 1)) * 100))),
          dependencyCheck: {
            rootIds,
            seedIds,
            discoveredCount,
            completedCount,
            failedNodes: [...failedNodes],
            discoveredDependencies: [...discoveredDependencies.values()],
          },
        });
      }

      if (latestData) {
        updateLocalState(latestData);
      }
      const currentTask = tasksRef.current.find((candidate) => candidate.id === task.id);
      if (currentTask?.status !== 'cancelled') {
        patchTask(task.id, {
          status: 'completed',
          progress: 100,
          finishedAt: nowIso(),
          error: failedNodes.length > 0 ? `${failedNodes.length} dependency node(s) could not be resolved` : undefined,
          dependencyCheck: {
            rootIds,
            seedIds,
            discoveredCount: visited.size,
            completedCount,
            failedNodes,
            discoveredDependencies: [...discoveredDependencies.values()],
          },
        });
      }
    } finally {
      dependencyRunningRef.current = false;
      processDependencyQueueRef.current();
    }
  }, [patchTask, updateLocalState]);

  const processDependencyQueue = useCallback(() => {
    if (dependencyRunningRef.current) return;
    const next = tasksRef.current.find((task) => task.kind === 'dependency-check' && task.status === 'queued');
    if (next) {
      void runDependencyCheckTask(next);
    }
  }, [runDependencyCheckTask]);

  useEffect(() => {
    processDependencyQueueRef.current = processDependencyQueue;
  }, [processDependencyQueue]);

  const enqueueDependencyCheck = useCallback((workshopIds: string[], source = 'manual-dependency-check'): string | null => {
    const rootIds = [...new Set(workshopIds.map((id) => id.trim()).filter(Boolean))];
    if (rootIds.length === 0) return null;

    const activeRootIds = new Set(
      tasksRef.current
        .filter((task) => task.kind === 'dependency-check' && ['queued', 'running'].includes(task.status))
        .flatMap((task) => task.dependencyCheck?.rootIds || task.targetIds),
    );
    const newRootIds = rootIds.filter((id) => !activeRootIds.has(id));
    if (newRootIds.length === 0) return null;

    const id = taskId('dependency-check', newRootIds[0]);
    const task: BackgroundTask = {
      id,
      kind: 'dependency-check',
      status: 'queued',
      source,
      targetIds: newRootIds,
      progress: 0,
      createdAt: nowIso(),
      title: newRootIds.length === 1
        ? `Dependency check (${newRootIds[0]})`
        : `Dependency check (${newRootIds.length} items)`,
      dependencyCheck: {
        rootIds: newRootIds,
        seedIds: newRootIds,
        discoveredCount: newRootIds.length,
        completedCount: 0,
        failedNodes: [],
        discoveredDependencies: [],
      },
    };
    commitTasks([...tasksRef.current, task]);
    processDependencyQueueRef.current();
    return id;
  }, [commitTasks]);

  useEffect(() => {
    enqueueDependencyCheckRef.current = enqueueDependencyCheck;
  }, [enqueueDependencyCheck]);

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

  const upsertWarningTask = useCallback((warning: {
    id: string;
    source?: string;
    targetIds?: string[];
    title?: string;
    error?: string;
    imagePath?: string;
  }) => {
    const existing = tasksRef.current.find((task) => task.id === warning.id);
    const nextTask: BackgroundTask = {
      id: warning.id,
      kind: 'warning',
      status: 'failed',
      source: warning.source,
      targetIds: warning.targetIds || [],
      progress: 100,
      createdAt: existing?.createdAt || nowIso(),
      finishedAt: nowIso(),
      title: warning.title,
      error: warning.error,
      imagePath: warning.imagePath,
    };

    if (existing) {
      commitTasks(tasksRef.current.map((task) => task.id === warning.id ? nextTask : task));
      return;
    }

    commitTasks([...tasksRef.current, nextTask]);
  }, [commitTasks]);

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
    } else if (task.kind === 'dependency-check') {
      setTimeout(() => processDependencyQueueRef.current(), 0);
    }
  }, [onDownloadCancelled, patchTask]);

  const retryTask = useCallback((id: string) => {
    const task = tasksRef.current.find((t) => t.id === id);
    if (!task) return;

    const retryIds = task.kind === 'dependency-check'
      ? task.dependencyCheck?.failedNodes.map((node) => node.workshopId).filter(Boolean)
      : undefined;
    patchTask(id, {
      status: 'queued',
      progress: 0,
      error: undefined,
      createdAt: nowIso(),
      startedAt: undefined,
      finishedAt: undefined,
      dependencyCheck: task.kind === 'dependency-check'
        ? {
          rootIds: task.dependencyCheck?.rootIds || task.targetIds,
          seedIds: retryIds?.length ? retryIds : task.dependencyCheck?.rootIds || task.targetIds,
          discoveredCount: retryIds?.length || task.dependencyCheck?.rootIds?.length || task.targetIds.length,
          completedCount: 0,
          failedNodes: [],
          discoveredDependencies: [],
        }
        : task.dependencyCheck,
    });

    if (task.kind === 'download') {
      setTimeout(() => processDownloadQueueRef.current(), 0);
    } else if (task.kind === 'workshop-crawl') {
      setTimeout(() => processCrawlQueueRef.current(), 0);
    } else if (task.kind === 'dependency-check') {
      setTimeout(() => processDependencyQueueRef.current(), 0);
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
    enqueueDependencyCheck,
    recordSeenItems,
    upsertWarningTask,
    cancelTask,
    retryTask,
    clearFinishedTasks,
  };
}
