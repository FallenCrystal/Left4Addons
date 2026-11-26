import { useId, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Activity,
  AlertTriangle,
  ArrowUpCircle,
  CloudOff,
  Download,
  RefreshCw,
  Trash2,
  X,
} from 'lucide-react';
import type {
  AddonNamePreference,
  AddonSummary,
  SyncReport,
  TaskRecord,
} from '../../api/ipc';
import { useContentFilterContext } from '../../state/ContentFilterContext';
import type { LiveDownloadState } from '../../state/useAppController';
import {
  formatFilteredAddonTitle,
  formatRedactedAddonLabel,
  isLinkedWorkshopTitleRedacted,
} from '../../utils/contentFilter';
import {
  formatAddonDisplayName,
  formatBytes,
  formatSpeed,
  formatTimestamp,
} from '../../utils/format';
import {
  Button,
  EmptyState,
  IconButton,
  LinearProgress,
  SideSheet,
  Tabs,
} from '../m3/primitives';

export interface TaskCenterSheetProps {
  open: boolean;
  tab: 'queue' | 'sync';
  onTabChange: (tab: 'queue' | 'sync') => void;
  onClose: () => void;
  addons: AddonSummary[];
  namePreference?: AddonNamePreference;
  tasks: TaskRecord[];
  liveDownloads: Record<string, LiveDownloadState>;
  syncReport: SyncReport | null;
  syncing: boolean;
  busy: boolean;
  onSync: () => void;
  onCancelDownload: (workshopId: string) => void;
  onDownloadItem: (workshopId: string, title?: string) => void;
  onUpdateItem: (workshopId: string, title?: string) => void;
  onSafeDeleteObsolete: (workshopId: string) => void;
}

export function TaskCenterSheet({
  open,
  tab,
  onTabChange,
  onClose,
  addons,
  namePreference = 'workshop_title',
  tasks,
  liveDownloads,
  syncReport,
  syncing,
  busy,
  onSync,
  onCancelDownload,
  onDownloadItem,
  onUpdateItem,
  onSafeDeleteObsolete,
}: TaskCenterSheetProps) {
  const { t } = useTranslation();
  const filterCtx = useContentFilterContext();
  const tabsId = useId();

  const activeList = Object.values(liveDownloads);

  const addonsById = useMemo(() => {
    const map = new Map<string, AddonSummary>();
    for (const a of addons) map.set(a.id, a);
    return map;
  }, [addons]);

  const addonsByWid = useMemo(() => {
    const map = new Map<string, AddonSummary>();
    for (const a of addons) {
      if (a.workshopId) map.set(a.workshopId, a);
    }
    return map;
  }, [addons]);

  const isWidTitleRedacted = (wid: string, rawTitle?: string | null, addonId?: string): boolean => {
    const found = (addonId ? addonsById.get(addonId) : undefined) ?? addonsByWid.get(wid);
    return isLinkedWorkshopTitleRedacted(wid, found, rawTitle ?? undefined, filterCtx);
  };

  const resolveSafeWidTitle = (
    wid: string,
    rawFallback?: string | null,
    addonId?: string,
  ): string | undefined => {
    const found = (addonId ? addonsById.get(addonId) : undefined) ?? addonsByWid.get(wid);
    if (isLinkedWorkshopTitleRedacted(wid, found, rawFallback ?? undefined, filterCtx)) {
      return formatRedactedAddonLabel(wid, t);
    }
    if (found) {
      return rawFallback?.trim() || formatAddonDisplayName(found, namePreference);
    }
    return rawFallback?.trim() || undefined;
  };

  const addonLabel = (wid: string): string | undefined => {
    const found = addonsByWid.get(wid);
    if (isLinkedWorkshopTitleRedacted(wid, found, undefined, filterCtx)) {
      return formatRedactedAddonLabel(wid, t);
    }
    if (found) {
      return formatFilteredAddonTitle(found, namePreference, filterCtx, t);
    }
    return undefined;
  };

  const safeTitleHint = (wid: string, rawHint?: string | null): string | undefined => {
    if (isWidTitleRedacted(wid, rawHint)) {
      return undefined;
    }
    return rawHint?.trim() || undefined;
  };

  const activeTasksByWid = useMemo(() => {
    const map = new Map<string, TaskRecord>();
    for (const task of tasks) {
      if (
        task.kind === 'download' &&
        (task.status === 'running' || task.status === 'queued') &&
        !map.has(task.targetId)
      ) {
        map.set(task.targetId, task);
      }
    }
    return map;
  }, [tasks]);

  const activeTaskIds = useMemo(
    () => new Set(activeList.map((dl) => dl.taskId)),
    [activeList],
  );
  const activeWids = useMemo(
    () => new Set(activeList.map((dl) => dl.workshopId)),
    [activeList],
  );

  const historyTasks = useMemo(
    () =>
      tasks.filter(
        (task) =>
          !activeTaskIds.has(task.id) &&
          !(
            task.kind === 'download' &&
            (task.status === 'running' || task.status === 'queued') &&
            activeWids.has(task.targetId)
          ),
      ),
    [tasks, activeTaskIds, activeWids],
  );

  const syncIssueCount = syncReport
    ? syncReport.updatesAvailable.length +
      syncReport.removedUpstream.length +
      syncReport.obsoleteDependencies.length +
      syncReport.dependencyDiffs.length +
      syncReport.collectionDiffs.length +
      syncReport.warnings.length
    : undefined;

  return (
    <SideSheet
      open={open}
      title={t('nav.tasks')}
      onClose={onClose}
      headerTrailing={
        <Button
          variant="tonal"
          size="sm"
          disabled={syncing}
          icon={<RefreshCw size={14} className={syncing ? 'md-spin' : undefined} />}
          onClick={onSync}
        >
          {t('nav.sync')}
        </Button>
      }
    >
      <Tabs
        compact
        idPrefix={tabsId}
        value={tab}
        onChange={onTabChange}
        tabs={[
          { value: 'queue', label: t('tasks.queue'), count: activeList.length + historyTasks.length },
          {
            value: 'sync',
            label: t('tasks.syncReport'),
            count: syncIssueCount,
          },
        ]}
      />

      <div
        className="md-sheet-body"
        role="tabpanel"
        id={`${tabsId}-panel-${tab}`}
        aria-labelledby={`${tabsId}-tab-${tab}`}
      >
        {tab === 'queue' ? (
          activeList.length === 0 && historyTasks.length === 0 ? (
            <EmptyState icon={<Activity size={24} />} title={t('tasks.emptyTasks')} />
          ) : (
            <>
              {activeList.length > 0 && (
                <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                  <span className="md-label-medium" style={{ color: 'var(--md-sys-color-primary)' }}>
                    {t('tasks.active')}
                  </span>
                  {activeList.map((dl) => {
                    const taskRecord = activeTasksByWid.get(dl.workshopId);
                    const resolvedTitle = resolveSafeWidTitle(
                      dl.workshopId,
                      dl.title?.trim() || taskRecord?.targetTitle?.trim(),
                    );
                    const isQueued =
                      !dl.hasProgress && !dl.cancelling && taskRecord?.status === 'queued';
                    const statusLabel = dl.cancelling
                      ? t('common.cancelling')
                      : isQueued
                        ? t('tasks.status.queued')
                        : `${dl.percent}%`;
                    const progressLabel = resolvedTitle ?? `#${dl.workshopId}`;

                    return (
                      <div key={dl.workshopId} className="l4a-section-card" style={{ padding: 14, gap: 8 }}>
                        <div
                          className="l4a-row-wrap"
                          style={{
                            justifyContent: 'space-between',
                            flexWrap: 'nowrap',
                            alignItems: 'flex-start',
                          }}
                        >
                          <div className="l4a-list-main" style={{ gap: 2 }}>
                            {resolvedTitle ? (
                              <>
                                <span className="md-title-small l4a-truncate">{resolvedTitle}</span>
                                <span
                                  className="md-body-small md-mono"
                                  style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                                >
                                  #{dl.workshopId}
                                </span>
                              </>
                            ) : (
                              <span className="md-title-small md-mono">#{dl.workshopId}</span>
                            )}
                          </div>
                          <div className="l4a-row-wrap" style={{ flexShrink: 0, flexWrap: 'nowrap' }}>
                            <span className="md-label-medium">{statusLabel}</span>
                            <IconButton
                              size="sm"
                              disabled={dl.cancelling}
                              label={t('common.cancel')}
                              onClick={() => onCancelDownload(dl.workshopId)}
                            >
                              <X size={15} />
                            </IconButton>
                          </div>
                        </div>
                        <LinearProgress
                          value={dl.percent}
                          indeterminate={dl.cancelling || isQueued || dl.percent === 0}
                          label={progressLabel}
                        />
                        <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                          <span className="md-body-small" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                            {dl.total
                              ? `${formatBytes(dl.downloaded)} / ${formatBytes(dl.total)}`
                              : formatBytes(dl.downloaded)}
                          </span>
                          <span className="md-body-small md-mono" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                            {dl.hasProgress && !isQueued ? formatSpeed(dl.speedBps) : ''}
                          </span>
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}

              {historyTasks.length > 0 && (
                <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                  <span className="md-label-medium" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                    {t('tasks.history')}
                  </span>
                  <div className="l4a-list">
                    {historyTasks.slice(0, 40).map((task) => {
                      const tone =
                        task.status === 'completed'
                          ? 'primary'
                          : task.status === 'failed'
                            ? 'error'
                            : task.status === 'running' || task.status === 'queued'
                              ? 'tertiary'
                              : 'surface';
                      const resolvedTitle =
                        task.kind === 'download'
                          ? resolveSafeWidTitle(task.targetId, task.targetTitle)
                          : undefined;
                      const detailText =
                        task.error || task.message || formatTimestamp(task.updatedAt);
                      return (
                        <div
                          key={task.id}
                          className="l4a-list-row"
                          style={{ minHeight: 56, cursor: 'default' }}
                        >
                          <div className="l4a-list-main">
                            {resolvedTitle && (
                              <span className="md-title-small l4a-truncate">{resolvedTitle}</span>
                            )}
                            <div className="l4a-row-wrap">
                              <span className={resolvedTitle ? 'md-body-small' : 'md-title-small'}>
                                {t(`tasks.kind.${task.kind}`)}
                              </span>
                              <span
                                className="md-body-small md-mono"
                                style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                              >
                                {task.kind === 'download' ? `#${task.targetId}` : task.targetId}
                              </span>
                            </div>
                            <span
                              className={task.error ? 'md-body-small' : 'md-body-small l4a-truncate'}
                              style={{
                                color: 'var(--md-sys-color-on-surface-variant)',
                                overflowWrap: task.error ? 'anywhere' : undefined,
                              }}
                            >
                              {detailText}
                            </span>
                          </div>
                          <span className="md-pill" data-tone={tone}>
                            {t(`tasks.status.${task.status}`)}
                          </span>
                        </div>
                      );
                    })}
                  </div>
                </div>
              )}
            </>
          )
        ) : !syncReport ? (
          <EmptyState
            icon={<RefreshCw size={24} />}
            title={t('tasks.emptySync')}
            action={
              <Button variant="filled" disabled={syncing} onClick={onSync}>
                {t('nav.sync')}
              </Button>
            }
          />
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
            <div className="md-chip-row">
              <span className="md-pill" data-tone="secondary">
                {t('tasks.checked', { count: syncReport.checkedItems })}
              </span>
            </div>

            {syncReport.updatesAvailable.length > 0 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                <div className="l4a-row-wrap">
                  <span className="md-label-medium">{t('tasks.updates')}</span>
                  <span className="md-pill" data-tone="tertiary">
                    {syncReport.updatesAvailable.length}
                  </span>
                </div>
                <div className="l4a-list">
                  {syncReport.updatesAvailable.map((item) => {
                    const current = addonsByWid.get(item.workshopId);
                    const stillNeedsUpdate = !current || current.updateAvailable;
                    const downloading = Boolean(liveDownloads[item.workshopId]);
                    const displayTitle =
                      resolveSafeWidTitle(item.workshopId, item.title, item.addonId) ??
                      `#${item.workshopId}`;
                    return (
                      <div key={item.addonId} className="l4a-list-row" style={{ cursor: 'default' }}>
                        <ArrowUpCircle size={18} style={{ color: 'var(--md-sys-color-tertiary)', flexShrink: 0 }} />
                        <div className="l4a-list-main">
                          <span className="md-title-small l4a-truncate">{displayTitle}</span>
                          <span className="md-body-small md-mono" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                            #{item.workshopId} · {formatTimestamp(item.remoteTimeUpdated)}
                          </span>
                        </div>
                        {stillNeedsUpdate && (
                          <IconButton
                            size="sm"
                            variant="tonal"
                            disabled={downloading}
                            label={t('common.update')}
                            onClick={() =>
                              onUpdateItem(
                                item.workshopId,
                                safeTitleHint(item.workshopId, item.title),
                              )
                            }
                          >
                            <ArrowUpCircle size={15} />
                          </IconButton>
                        )}
                      </div>
                    );
                  })}
                </div>
              </div>
            )}

            {syncReport.removedUpstream.length > 0 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                <div className="l4a-row-wrap">
                  <span className="md-label-medium">{t('tasks.removed')}</span>
                  <span className="md-pill" data-tone="error">
                    {syncReport.removedUpstream.length}
                  </span>
                </div>
                <div className="l4a-list">
                  {syncReport.removedUpstream.map((item) => {
                    const displayTitle =
                      resolveSafeWidTitle(item.workshopId, item.title, item.addonId) ??
                      `#${item.workshopId}`;
                    return (
                      <div key={item.addonId} className="l4a-list-row" style={{ cursor: 'default' }}>
                        <CloudOff size={18} style={{ color: 'var(--md-sys-color-error)', flexShrink: 0 }} />
                        <div className="l4a-list-main">
                          <span className="md-title-small l4a-truncate">{displayTitle}</span>
                          <span className="md-body-small md-mono" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                            #{item.workshopId}
                          </span>
                        </div>
                        {item.localFilePreserved && (
                          <span className="md-pill" data-tone="surface">
                            {t('tasks.localPreserved')}
                          </span>
                        )}
                      </div>
                    );
                  })}
                </div>
              </div>
            )}

            {syncReport.obsoleteDependencies.length > 0 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                <div className="l4a-row-wrap">
                  <span className="md-label-medium">{t('tasks.obsoleteDeps')}</span>
                  <span className="md-pill" data-tone="secondary">
                    {syncReport.obsoleteDependencies.length}
                  </span>
                </div>
                <div className="l4a-list">
                  {syncReport.obsoleteDependencies.map((dep) => {
                    const currentAddon = addonsByWid.get(dep.dependencyWorkshopId);
                    const currentlyInstalled = currentAddon?.installState === 'installed';
                    const depLabel =
                      resolveSafeWidTitle(dep.dependencyWorkshopId, dep.dependencyTitle) ??
                      `#${dep.dependencyWorkshopId}`;
                    const parentLabels = dep.remainingActiveParents
                      .map((wid) => addonLabel(wid) ?? `#${wid}`)
                      .join(', ');
                    return (
                      <div
                        key={dep.dependencyWorkshopId}
                        className="l4a-list-row"
                        style={{ cursor: 'default' }}
                      >
                        <div className="l4a-list-main">
                          <span className="md-title-small l4a-truncate">{depLabel}</span>
                          <span className="md-body-small" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                            {dep.canSafelyRemove
                              ? t('tasks.safeRemove')
                              : t('tasks.stillUsed', { parents: parentLabels })}
                          </span>
                        </div>
                        {dep.canSafelyRemove && currentlyInstalled && (
                          <IconButton
                            size="sm"
                            disabled={busy}
                            label={t('common.delete')}
                            onClick={() => onSafeDeleteObsolete(dep.dependencyWorkshopId)}
                          >
                            <Trash2 size={15} />
                          </IconButton>
                        )}
                      </div>
                    );
                  })}
                </div>
              </div>
            )}

            {syncReport.dependencyDiffs.length > 0 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                <div className="l4a-row-wrap">
                  <span className="md-label-medium">{t('tasks.depDiffs')}</span>
                  <span className="md-pill" data-tone="surface">
                    {syncReport.dependencyDiffs.length}
                  </span>
                </div>
                <div className="l4a-list">
                  {syncReport.dependencyDiffs.map((diff) => {
                    const parentLabel =
                      resolveSafeWidTitle(diff.parentWorkshopId, diff.parentTitle) ??
                      `#${diff.parentWorkshopId}`;
                    return (
                      <div
                        key={diff.parentWorkshopId}
                        className="l4a-list-row"
                        style={{ cursor: 'default' }}
                      >
                        <div className="l4a-list-main">
                          <span className="md-title-small l4a-truncate">{parentLabel}</span>
                          <div className="l4a-row-wrap">
                            {diff.addedDependencies.map((id) => {
                              const installed = addonsByWid.get(id)?.installState === 'installed';
                              return (
                                <Button
                                  key={id}
                                  variant={installed ? 'text' : 'tonal'}
                                  size="sm"
                                  disabled={installed || Boolean(liveDownloads[id])}
                                  icon={<Download size={12} />}
                                  onClick={() =>
                                    onDownloadItem(id, safeTitleHint(id, addonsByWid.get(id)?.title))
                                  }
                                >
                                  +{id}
                                </Button>
                              );
                            })}
                            {diff.removedDependencies.map((id) => (
                              <span key={id} className="md-pill md-mono" data-tone="surface">
                                -{id}
                              </span>
                            ))}
                          </div>
                        </div>
                      </div>
                    );
                  })}
                </div>
              </div>
            )}

            {syncReport.collectionDiffs.length > 0 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                <div className="l4a-row-wrap">
                  <span className="md-label-medium">{t('tasks.collectionDiffs')}</span>
                  <span className="md-pill" data-tone="surface">
                    {syncReport.collectionDiffs.length}
                  </span>
                </div>
                <div className="l4a-list">
                  {syncReport.collectionDiffs.map((diff) => (
                    <div
                      key={diff.collectionId}
                      className="l4a-list-row"
                      style={{ cursor: 'default' }}
                    >
                      <div className="l4a-list-main">
                        <span className="md-title-small l4a-truncate">{diff.title}</span>
                        <div className="l4a-row-wrap">
                          {diff.addedItems.map((id) => {
                            const installed = addonsByWid.get(id)?.installState === 'installed';
                            return (
                              <Button
                                key={`add-${id}`}
                                variant={installed ? 'text' : 'tonal'}
                                size="sm"
                                disabled={installed || Boolean(liveDownloads[id])}
                                icon={<Download size={12} />}
                                onClick={() =>
                                  onDownloadItem(id, safeTitleHint(id, addonsByWid.get(id)?.title))
                                }
                              >
                                +{addonLabel(id) ?? id}
                              </Button>
                            );
                          })}
                          {diff.removedItems.map((id) => (
                            <span key={`rem-${id}`} className="md-pill md-mono" data-tone="error">
                              -{addonLabel(id) ?? id}
                            </span>
                          ))}
                        </div>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {syncReport.warnings.length > 0 && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                <div className="l4a-row-wrap">
                  <span className="md-label-medium">{t('tasks.warnings')}</span>
                  <span className="md-pill" data-tone="error">
                    {syncReport.warnings.length}
                  </span>
                </div>
                {syncReport.warnings.map((w, idx) => (
                  <div
                    key={`${idx}-${w}`}
                    className="l4a-path-chip"
                    style={{
                      backgroundColor: 'var(--md-sys-color-error-container)',
                      color: 'var(--md-sys-color-on-error-container)',
                    }}
                  >
                    <AlertTriangle size={14} style={{ flexShrink: 0 }} />
                    <span>{w}</span>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
    </SideSheet>
  );
}
