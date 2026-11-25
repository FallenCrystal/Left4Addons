import type {
  AddonSummary,
  CoreEvent,
  DashboardSnapshot,
  SearchQuery,
  SearchResponse,
  SyncReport,
  TaskRecord,
  WorkshopItemRecord,
} from '../ipc';
import { IpcError } from '../ipc';
import { extractWorkshopIdInput, previewFilenameByRules } from '../../utils/format';
import { PREVIEW_CATALOG } from './fixtures';

export type PreviewEmit = (event: CoreEvent) => void;
export type PreviewDownloadToken = { cancelled: boolean };

export function syncPreviewWorkshop(state: DashboardSnapshot, emit: PreviewEmit): SyncReport {
  const report: SyncReport = {
    checkedItems: state.addons.filter((a) => a.workshopId).length,
    updatesAvailable: state.addons
      .filter((a) => a.updateAvailable && a.workshopId)
      .map((a) => ({
        addonId: a.id,
        workshopId: a.workshopId!,
        title: a.title,
        localTimeUpdated: a.localTimeUpdated,
        remoteTimeUpdated: a.remoteTimeUpdated ?? Math.floor(Date.now() / 1000),
        source: 'catalog_server',
      })),
    removedUpstream: state.addons
      .filter((a) => a.remoteStatus === 'removed' && a.workshopId)
      .map((a) => ({
        addonId: a.id,
        workshopId: a.workshopId!,
        title: a.title,
        status: a.remoteStatus,
        localFilePreserved: a.installState === 'installed',
        detectedAt: new Date().toISOString(),
      })),
    dependencyDiffs: [
      {
        parentWorkshopId: '2814104912',
        parentTitle: 'Chernobyl: Chapter One',
        addedDependencies: ['2814105100'],
        removedDependencies: [],
      },
    ],
    obsoleteDependencies: [],
    collectionDiffs: state.collections
      .filter((c) => c.autoSync)
      .map((c) => ({
        collectionId: c.collectionId,
        title: c.title,
        addedItems: ['3350912881'],
        removedItems: c.removedUpstreamIds,
      })),
    warnings: [],
  };
  state.tasks.unshift({
    id: `sync-${Date.now()}`,
    kind: 'sync',
    status: 'completed',
    targetId: 'workshop-sync',
    progressPercent: 100,
    message: `${report.checkedItems} items`,
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  });
  emit({ type: 'snapshot_updated', externalChange: false });
  return report;
}

export function searchPreviewWorkshop(query: SearchQuery): SearchResponse {
  const rawQuery = (query.query ?? '').trim();
  const q = rawQuery.toLowerCase();
  const tag = query.tag?.trim().toLowerCase();
  const target = (query.searchTarget ?? 'all').trim().toLowerCase();
  const numericOnly = q.replace(/^#/, '');
  const isWorkshopLink =
    /^https?:\/\/(www\.)?steamcommunity\.com\/(sharedfiles|workshop)\/filedetails\/?([?#]|$)/i.test(
      rawQuery,
    ) || /^steam:\/\/url\/CommunityFilePage\/\d+\/?$/i.test(rawQuery);
  const urlId = isWorkshopLink ? extractWorkshopIdInput(rawQuery) : null;
  const isTitleTarget = target === 'title' || target === '1';
  const isDescTarget = target === 'description' || target === 'desc' || target === '2';
  const isBareLongId = /^\d{6,}$/.test(numericOnly) && !isTitleTarget && !isDescTarget;
  const effectiveFileType = query.fileType ?? (urlId ? undefined : 'item');

  let items = PREVIEW_CATALOG.filter((item) => {
    const allowUntypedExact = !query.fileType && isBareLongId && item.workshopId === numericOnly;
    if (effectiveFileType && !allowUntypedExact && item.fileType !== effectiveFileType) {
      return false;
    }
    if (q) {
      if (urlId) {
        if (item.workshopId !== urlId) return false;
      } else {
        const idMatch = item.workshopId === numericOnly || item.workshopId.includes(q);
        const hit = isTitleTarget
          ? item.title.toLowerCase().includes(q)
          : isDescTarget
            ? (item.description ?? '').toLowerCase().includes(q)
            : item.title.toLowerCase().includes(q) ||
              idMatch ||
              (item.creatorName ?? '').toLowerCase().includes(q) ||
              (item.description ?? '').toLowerCase().includes(q);
        if (!hit) return false;
      }
    }
    if (tag && !item.tags.some((t) => t.toLowerCase() === tag)) {
      return false;
    }
    if (query.status && item.status !== query.status) {
      return false;
    }
    return true;
  });

  const sort = (query.sort ?? '').trim().toLowerCase();
  if (sort === 'updated_asc') {
    items = [...items].sort((a, b) => (a.timeUpdated ?? 0) - (b.timeUpdated ?? 0));
  } else if (sort === 'updated' || sort === 'lastupdated') {
    items = [...items].sort((a, b) => (b.timeUpdated ?? 0) - (a.timeUpdated ?? 0));
  } else if (sort === 'title') {
    items = [...items].sort((a, b) => a.title.localeCompare(b.title));
  } else if (sort === 'newest' || sort === 'mostrecent') {
    items = [...items].sort((a, b) => (b.timeCreated ?? 0) - (a.timeCreated ?? 0));
  } else {
    items = [...items].sort(
      (a, b) =>
        (b.subscriptions ?? 0) - (a.subscriptions ?? 0) ||
        (b.timeUpdated ?? 0) - (a.timeUpdated ?? 0),
    );
  }

  const page = Math.max(1, Math.floor(query.page ?? 1));
  const pageSize = Math.min(100, Math.max(1, Math.floor(query.pageSize ?? 24)));
  const start = (page - 1) * pageSize;

  return {
    items: structuredClone(items.slice(start, start + pageSize)),
    total: items.length,
    page,
    pageSize,
    source: 'catalog_server',
  };
}

export function fetchPreviewWorkshopItem(workshopId: string): WorkshopItemRecord {
  const item = PREVIEW_CATALOG.find((i) => i.workshopId === workshopId);
  if (!item) {
    throw new IpcError('not_found', `Workshop item not found: ${workshopId}`);
  }
  return structuredClone(item);
}

export function getPreviewSubscribedWorkshopIds(state: DashboardSnapshot): string[] {
  if (state.config.disableSteamworksSdk) {
    throw new IpcError('unsupported', 'Steamworks SDK disabled');
  }
  return ['2814104912', '2814105100', '3350912881'];
}

export async function runPreviewTransfer(
  state: DashboardSnapshot,
  activeDownloads: Map<string, PreviewDownloadToken>,
  emit: PreviewEmit,
  workshopId: string,
  mode: 'install' | 'update',
  titleHint?: string,
): Promise<AddonSummary> {
  if (!/^\d+$/.test(workshopId)) {
    throw new IpcError('invalid_input', `Invalid Workshop ID: ${workshopId}`);
  }
  if (activeDownloads.has(workshopId)) {
    throw new IpcError('conflict', `Already downloading ${workshopId}`);
  }

  const existingIdx = state.addons.findIndex((a) => a.workshopId === workshopId);
  const existing = existingIdx >= 0 ? state.addons[existingIdx] : undefined;

  if (mode === 'install' && existing?.installState === 'installed') {
    throw new IpcError('conflict', `Addon already installed: ${workshopId}`);
  }
  if (mode === 'update' && (!existing || existing.installState !== 'installed')) {
    throw new IpcError('not_found', `Addon not installed: ${workshopId}`);
  }

  const token: PreviewDownloadToken = { cancelled: false };
  activeDownloads.set(workshopId, token);

  const catalogItem = PREVIEW_CATALOG.find((i) => i.workshopId === workshopId);
  const title =
    existing?.title.trim() ||
    catalogItem?.title.trim() ||
    titleHint?.trim() ||
    `Workshop Addon #${workshopId}`;
  const taskId = `dl-${workshopId}-${Date.now()}`;
  const total = catalogItem?.fileSize ?? 32_000_000;

  const taskRecord: TaskRecord = {
    id: taskId,
    kind: 'download',
    status: 'running',
    targetId: workshopId,
    targetTitle: title,
    progressPercent: 0,
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  };
  state.tasks.unshift(taskRecord);
  emit({ type: 'snapshot_updated', externalChange: false });

  try {
    let prevBytes = 0;
    for (const percent of [25, 60, 90, 100]) {
      await new Promise((r) => setTimeout(r, 100));
      if (token.cancelled) {
        taskRecord.status = 'cancelled';
        taskRecord.updatedAt = new Date().toISOString();
        emit({ type: 'snapshot_updated', externalChange: false });
        throw new IpcError('cancelled', `Cancelled ${workshopId}`);
      }
      const downloaded = Math.round((total * percent) / 100);
      const speedBps = Math.max(0, Math.round((downloaded - prevBytes) / 0.1));
      prevBytes = downloaded;
      taskRecord.progressPercent = percent;
      emit({
        type: 'download_progress',
        taskId,
        workshopId,
        title,
        downloaded,
        total,
        percent,
        speedBps,
      });
    }

    const nowTs = catalogItem?.timeUpdated ?? Math.floor(Date.now() / 1000);
    if (mode === 'update' && existing) {
      existing.localTimeUpdated = nowTs;
      existing.remoteTimeUpdated = nowTs;
      existing.updateAvailable = false;
      existing.fileSize = total;
      taskRecord.status = 'completed';
      taskRecord.progressPercent = 100;
      taskRecord.updatedAt = new Date().toISOString();
      emit({ type: 'snapshot_updated', externalChange: false });
      return structuredClone(existing);
    }

    const id = `ws:${workshopId}`;
    const fileName = previewFilenameByRules(state.config.renameRules, {
      workshopId,
      title,
    });
    const nextAddon: AddonSummary = {
      id,
      workshopId,
      title,
      author: catalogItem?.creatorName ?? 'Workshop',
      version: '1.0',
      installState: 'installed',
      primaryDir: 'loading',
      enabled: true,
      currentFileName: fileName,
      currentPath: `${state.config.loadingDir}/${fileName}`,
      contentHash: `preview_${workshopId}`,
      fileSize: total,
      filesCount: 42,
      categories:
        catalogItem?.tags.map((t) =>
          t === 'Campaigns'
            ? 'Campaign'
            : t === 'Maps'
              ? 'Map'
              : t === 'Weapons'
                ? 'Weapon'
                : t === 'Scripts'
                  ? 'Script'
                  : t === 'Sounds'
                    ? 'Audio'
                    : t,
        ) ?? ['Other'],
      maps: [],
      hasDummyBypass: state.config.enableDummyBypass,
      remoteStatus: 'active',
      localTimeUpdated: nowTs,
      remoteTimeUpdated: nowTs,
      updateAvailable: false,
      addonInfo: { addontitle: title },
    };

    if (existingIdx >= 0) {
      state.addons[existingIdx] = {
        ...state.addons[existingIdx],
        ...nextAddon,
        maps: state.addons[existingIdx].maps,
      };
    } else {
      state.addons.unshift(nextAddon);
    }
    taskRecord.status = 'completed';
    taskRecord.progressPercent = 100;
    taskRecord.updatedAt = new Date().toISOString();
    emit({ type: 'snapshot_updated', externalChange: false });
    return structuredClone(existingIdx >= 0 ? state.addons[existingIdx] : nextAddon);
  } finally {
    activeDownloads.delete(workshopId);
  }
}
