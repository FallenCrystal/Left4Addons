import type {
  AddonBatchReport,
  AddonDirType,
  AddonOperationResult,
  BundleRecord,
  DashboardSnapshot,
  ProfileApplyReport,
  ReconcileAction,
  ReconcileBatchItem,
  ReconcileBatchReport,
  ReconcileItemResult,
  RenamePreviewItem,
  RenameRuleConfig,
  TrackedCollectionRecord,
} from '../ipc';
import { IpcError } from '../ipc';
import {
  previewFilenameByRules,
  resolveAddonMetadataTitle,
  sanitizeVpkFilename,
} from '../../utils/format';
import { PREVIEW_CATALOG } from './fixtures';
import { reconcilePreviewExclusions } from './filter';
import type { PreviewEmit } from './workshop';

export function resolvePreviewReconcileIssue(
  state: DashboardSnapshot,
  emit: PreviewEmit,
  issueId: string,
  action: ReconcileAction,
): DashboardSnapshot {
  const idx = state.openIssues.findIndex((i) => i.id === issueId);
  if (idx === -1) {
    throw new IpcError('conflict', 'Issue already resolved');
  }
  const issue = state.openIssues[idx];
  if (issue.addonId) {
    const addon = state.addons.find((a) => a.id === issue.addonId);
    if (addon) {
      if (action === 'adopt_workshop_update') {
        addon.updateAvailable = false;
        addon.localTimeUpdated = addon.remoteTimeUpdated ?? addon.localTimeUpdated;
        addon.contentHash = `updated_${addon.workshopId ?? addon.id}_${addon.localTimeUpdated ?? Date.now()}`;
        addon.hasDummyBypass = state.config.enableDummyBypass;
      } else if (action === 'import_to_loading_and_create_dummy') {
        addon.primaryDir = 'loading';
        addon.hasDummyBypass = state.config.enableDummyBypass;
      } else if (action === 'keep_workshop_delete_loading') {
        addon.primaryDir = 'workshop';
        addon.hasDummyBypass = false;
      } else if (action === 'enable_workshop_file') {
        addon.enabled = true;
      }
    }
  }
  state.openIssues.splice(idx, 1);
  reconcilePreviewExclusions(state);
  emit({ type: 'snapshot_updated', externalChange: false });
  return structuredClone(state);
}

export function resolvePreviewReconcileIssuesBatch(
  state: DashboardSnapshot,
  emit: PreviewEmit,
  items: ReconcileBatchItem[],
): ReconcileBatchReport {
  const results: ReconcileItemResult[] = [];
  for (const item of items) {
    try {
      resolvePreviewReconcileIssue(state, emit, item.issueId, item.action);
      results.push({ issueId: item.issueId, action: item.action, ok: true });
    } catch (err) {
      results.push({
        issueId: item.issueId,
        action: item.action,
        ok: false,
        error: {
          code:
            err instanceof IpcError
              ? (err.code as ReconcileItemResult['error'] extends { code: infer C } ? C : 'conflict')
              : 'conflict',
          message: err instanceof Error ? err.message : String(err),
        },
      });
    }
  }
  return { items: results, snapshot: structuredClone(state) };
}

export function movePreviewAddons(
  state: DashboardSnapshot,
  emit: PreviewEmit,
  addonIds: string[],
  targetDirType: AddonDirType,
): AddonBatchReport {
  const items: AddonOperationResult[] = [];
  for (const addonId of addonIds) {
    const addon = state.addons.find((a) => a.id === addonId);
    if (!addon || addon.installState !== 'installed') {
      items.push({
        addonId,
        status: 'failed',
        error: { code: 'not_found', message: `Addon not installed: ${addonId}` },
      });
      continue;
    }
    if (addon.primaryDir === targetDirType) {
      items.push({ addonId, status: 'unchanged' });
      continue;
    }
    addon.primaryDir = targetDirType;
    addon.hasDummyBypass =
      targetDirType === 'loading' &&
      Boolean(addon.workshopId) &&
      state.config.enableDummyBypass;
    if (targetDirType === 'workshop') {
      addon.enabled = true;
    }
    items.push({ addonId, status: 'applied' });
  }
  emit({ type: 'snapshot_updated', externalChange: false });
  return { items };
}

export function togglePreviewAddons(
  state: DashboardSnapshot,
  emit: PreviewEmit,
  addonIds: string[],
  enabled: boolean,
  allowWorkshopDir = false,
): AddonBatchReport {
  const items: AddonOperationResult[] = [];
  let failed = false;
  for (const addonId of addonIds) {
    if (failed) {
      items.push({ addonId, status: 'not_attempted' });
      continue;
    }
    const addon = state.addons.find((a) => a.id === addonId);
    if (!addon) {
      failed = true;
      items.push({
        addonId,
        status: 'failed',
        error: { code: 'not_found', message: `Not found: ${addonId}` },
      });
      continue;
    }
    if (addon.enabled === enabled) {
      items.push({ addonId, status: 'unchanged' });
      continue;
    }
    if (addon.primaryDir === 'workshop' && !allowWorkshopDir) {
      failed = true;
      items.push({
        addonId,
        status: 'failed',
        error: { code: 'conflict', message: `Addon ${addonId} is in workshop/` },
      });
      continue;
    }
    addon.enabled = enabled;
    items.push({ addonId, status: 'applied' });
  }
  emit({ type: 'snapshot_updated', externalChange: false });
  return { items };
}

export function buildPreviewRenames(
  state: DashboardSnapshot,
  rules: RenameRuleConfig,
  addonIds: string[] = [],
): RenamePreviewItem[] {
  const idSet = new Set(addonIds);
  const groupByAddon = new Map<string, string>();
  for (const group of state.userGroups) {
    for (const aid of group.addonIds) {
      if (!groupByAddon.has(aid)) groupByAddon.set(aid, group.name);
    }
  }
  return state.addons
    .filter(
      (a) =>
        a.installState === 'installed' &&
        a.currentFileName &&
        (idSet.size === 0 || idSet.has(a.id)),
    )
    .map((a) => ({
      addonId: a.id,
      currentFileName: a.currentFileName!,
      targetFileName: previewFilenameByRules(rules, {
        workshopId: a.workshopId,
        groupName: groupByAddon.get(a.id),
        title: resolveAddonMetadataTitle(
          a,
          state.config.addonNamePreference === 'addon_title'
            ? 'addon_title'
            : 'workshop_title',
        ),
      }),
      conflict: a.primaryDir === 'workshop' ? 'workshop/' : undefined,
    }));
}

export function applyPreviewRenames(
  state: DashboardSnapshot,
  emit: PreviewEmit,
  renames: [addonId: string, fileName: string][],
): AddonBatchReport {
  const items: AddonOperationResult[] = [];
  for (const [addonId, rawName] of renames) {
    const addon = state.addons.find((a) => a.id === addonId);
    if (!addon) {
      items.push({
        addonId,
        status: 'failed',
        error: { code: 'not_found', message: addonId },
      });
      continue;
    }
    if (addon.primaryDir === 'workshop') {
      items.push({
        addonId,
        status: 'failed',
        error: { code: 'conflict', message: 'workshop/' },
      });
      continue;
    }
    const clean = sanitizeVpkFilename(rawName, state.config.renameRules);
    if (addon.currentFileName === clean) {
      items.push({ addonId, status: 'unchanged' });
    } else {
      addon.currentFileName = clean;
      addon.currentPath = `${state.config.loadingDir}/${clean}${addon.enabled ? '' : '.disabled'}`;
      items.push({ addonId, status: 'applied' });
    }
  }
  emit({ type: 'snapshot_updated', externalChange: false });
  return { items };
}

export function deletePreviewAddons(
  state: DashboardSnapshot,
  emit: PreviewEmit,
  addonIds: string[],
  deleteDiskFile: boolean,
  purgeKnownRecord: boolean,
): AddonBatchReport {
  const items: AddonOperationResult[] = [];
  for (const addonId of addonIds) {
    const idx = state.addons.findIndex((a) => a.id === addonId);
    if (idx === -1) {
      items.push({ addonId, status: 'unchanged' });
      continue;
    }
    const addon = state.addons[idx];
    if (purgeKnownRecord && !deleteDiskFile && addon.installState === 'installed') {
      items.push({
        addonId,
        status: 'failed',
        error: { code: 'conflict', message: 'Still installed' },
      });
      continue;
    }
    if (purgeKnownRecord) {
      state.addons.splice(idx, 1);
      state.filterExclusions = state.filterExclusions.filter((e) => e.addonId !== addonId);
      for (const g of state.userGroups) {
        g.addonIds = g.addonIds.filter((id) => id !== addonId);
      }
      for (const b of state.bundles) {
        b.memberAddonIds = b.memberAddonIds.filter((id) => id !== addonId);
      }
      for (const p of state.profiles) {
        p.enabledAddonIds = p.enabledAddonIds.filter((id) => id !== addonId);
      }
      items.push({ addonId, status: 'applied' });
    } else if (deleteDiskFile) {
      addon.installState = 'uninstalled_known';
      addon.primaryDir = undefined;
      addon.currentPath = undefined;
      addon.contentHash = undefined;
      addon.enabled = false;
      addon.hasDummyBypass = false;
      items.push({ addonId, status: 'applied' });
    } else {
      items.push({ addonId, status: 'unchanged' });
    }
  }
  reconcilePreviewExclusions(state);
  emit({ type: 'snapshot_updated', externalChange: false });
  return { items };
}

export function safeDeletePreviewObsoleteDependency(
  state: DashboardSnapshot,
  emit: PreviewEmit,
  workshopId: string,
  deleteDiskFile: boolean,
  purgeKnownRecord: boolean,
): AddonBatchReport {
  const addon = state.addons.find((a) => a.workshopId === workshopId);
  if (!addon) {
    throw new IpcError('not_found', `Dependency not found: ${workshopId}`);
  }
  return deletePreviewAddons(state, emit, [addon.id], deleteDiskFile, purgeKnownRecord);
}

export function createPreviewManualBundle(
  state: DashboardSnapshot,
  name: string,
  memberAddonIds: string[],
): BundleRecord {
  const maps = state.addons
    .filter((a) => memberAddonIds.includes(a.id))
    .flatMap((a) => a.maps);
  const bundle: BundleRecord = {
    id: `bundle-manual-${Date.now()}`,
    name,
    kind: 'manual',
    locked: true,
    memberAddonIds,
    maps,
  };
  state.bundles.push(bundle);
  return structuredClone(bundle);
}

export function trackPreviewCollection(
  state: DashboardSnapshot,
  collectionId: string,
  autoSync: boolean,
): TrackedCollectionRecord {
  const catalog = PREVIEW_CATALOG.find((i) => i.workshopId === collectionId);
  const existing = state.collections.find((c) => c.collectionId === collectionId);
  const memberWorkshopIds = catalog?.collectionChildren.length
    ? catalog.collectionChildren
    : ['2814104912', '3011942811'];
  const record: TrackedCollectionRecord = {
    collectionId,
    title: catalog?.title ?? existing?.title ?? `Collection #${collectionId}`,
    description: catalog?.description ?? existing?.description,
    autoSync,
    memberWorkshopIds,
    removedUpstreamIds: (existing?.removedUpstreamIds ?? []).filter(
      (id) => !memberWorkshopIds.includes(id),
    ),
    lastSyncedAt: new Date().toISOString(),
  };
  const idx = state.collections.findIndex((c) => c.collectionId === collectionId);
  if (idx >= 0) state.collections[idx] = record;
  else state.collections.push(record);
  return structuredClone(record);
}

export function applyPreviewProfile(
  state: DashboardSnapshot,
  emit: PreviewEmit,
  profileId: string,
): ProfileApplyReport {
  const profile = state.profiles.find((p) => p.id === profileId);
  if (!profile) {
    throw new IpcError('not_found', `Profile not found: ${profileId}`);
  }
  const desired = new Set(profile.enabledAddonIds);
  const items: AddonOperationResult[] = [];
  const skippedWorkshopOnly: string[] = [];
  const missingAddons: string[] = [];
  let enabledCount = 0;
  let disabledCount = 0;

  for (const id of desired) {
    const a = state.addons.find((item) => item.id === id);
    if (!a || a.installState !== 'installed') {
      missingAddons.push(id);
    }
  }

  for (const addon of state.addons) {
    if (addon.installState !== 'installed') continue;
    if (addon.primaryDir !== 'loading') {
      if (desired.has(addon.id) !== addon.enabled) {
        skippedWorkshopOnly.push(addon.id);
      }
      continue;
    }
    const shouldEnable = desired.has(addon.id);
    if (addon.enabled !== shouldEnable) {
      addon.enabled = shouldEnable;
      items.push({ addonId: addon.id, status: 'applied' });
      if (shouldEnable) enabledCount += 1;
      else disabledCount += 1;
    }
  }

  emit({ type: 'snapshot_updated', externalChange: false });
  return {
    profileId,
    enabledCount,
    disabledCount,
    skippedWorkshopOnly,
    missingAddons,
    batch: { items },
  };
}
