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
import type { AddonSummary, SyncReport, TaskRecord } from '../../api/ipc';
import type { LiveDownloadState } from '../../state/useAppController';
import { formatBytes, formatSpeed, formatTimestamp } from '../../utils/format';
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
  tasks: TaskRecord[];
  liveDownloads: Record<string, LiveDownloadState>;
  syncReport: SyncReport | null;
  syncing: boolean;
  busy: boolean;
  onSync: () => void;
  onCancelDownload: (workshopId: string) => void;
  onDownloadItem: (workshopId: string) => void;
  onUpdateItem: (workshopId: string) => void;
  onSafeDeleteObsolete: (workshopId: string) => void;
}

export function TaskCenterSheet({
  open,
  tab,
  onTabChange,
  onClose,
  addons,
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
  const tabsId = useId();

  const activeList = Object.values(liveDownloads);

  const addonsByWid = useMemo(() => {
    const map = new Map<string, AddonSummary>();
    for (const a of addons) {
      if (a.workshopId) map.set(a.workshopId, a);
    }
    return map;
  }, [addons]);

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
          !(task.kind === 'download' && task.status === 'running' && activeWids.has(task.targetId)),
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
                  {activeList.map((dl) => (
                    <div key={dl.workshopId} className="l4a-section-card" style={{ padding: 14, gap: 8 }}>
                      <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                        <span className="md-title-small md-mono">#{dl.workshopId}</span>
                        <div className="l4a-row-wrap">
                          <span className="md-label-medium">
                            {dl.cancelling ? t('common.cancelling') : `${dl.percent}%`}
                          </span>
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
                        indeterminate={dl.cancelling || dl.percent === 0}
                        label={`#${dl.workshopId}`}
                      />
                      <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                        <span className="md-body-small" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                          {dl.total
                            ? `${formatBytes(dl.downloaded)} / ${formatBytes(dl.total)}`
                            : formatBytes(dl.downloaded)}
                        </span>
                        <span className="md-body-small md-mono" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                          {formatSpeed(dl.speedBps)}
                        </span>
                      </div>
                    </div>
                  ))}
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
                      return (
                        <div
                          key={task.id}
                          className="l4a-list-row"
                          style={{ minHeight: 56, cursor: 'default' }}
                        >
                          <div className="l4a-list-main">
                            <div className="l4a-row-wrap">
                              <span className="md-title-small">
                                {t(`tasks.kind.${task.kind}`)}
                              </span>
                              <span className="md-body-small md-mono" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                                {task.targetId}
                              </span>
                            </div>
                            <span className="md-body-small l4a-truncate" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                              {task.error || task.message || formatTimestamp(task.updatedAt)}
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
                    return (
                      <div key={item.addonId} className="l4a-list-row" style={{ cursor: 'default' }}>
                        <ArrowUpCircle size={18} style={{ color: 'var(--md-sys-color-tertiary)', flexShrink: 0 }} />
                        <div className="l4a-list-main">
                          <span className="md-title-small l4a-truncate">{item.title}</span>
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
                            onClick={() => onUpdateItem(item.workshopId)}
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
                  {syncReport.removedUpstream.map((item) => (
                    <div key={item.addonId} className="l4a-list-row" style={{ cursor: 'default' }}>
                      <CloudOff size={18} style={{ color: 'var(--md-sys-color-error)', flexShrink: 0 }} />
                      <div className="l4a-list-main">
                        <span className="md-title-small l4a-truncate">{item.title}</span>
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
                  ))}
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
                    const parentLabels = dep.remainingActiveParents
                      .map((wid) => addonsByWid.get(wid)?.title ?? `#${wid}`)
                      .join(', ');
                    return (
                      <div
                        key={dep.dependencyWorkshopId}
                        className="l4a-list-row"
                        style={{ cursor: 'default' }}
                      >
                        <div className="l4a-list-main">
                          <span className="md-title-small l4a-truncate">
                            {dep.dependencyTitle || `#${dep.dependencyWorkshopId}`}
                          </span>
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
                  {syncReport.dependencyDiffs.map((diff) => (
                    <div
                      key={diff.parentWorkshopId}
                      className="l4a-list-row"
                      style={{ cursor: 'default' }}
                    >
                      <div className="l4a-list-main">
                        <span className="md-title-small l4a-truncate">{diff.parentTitle}</span>
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
                                onClick={() => onDownloadItem(id)}
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
                  ))}
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
                                onClick={() => onDownloadItem(id)}
                              >
                                +{addonsByWid.get(id)?.title ?? id}
                              </Button>
                            );
                          })}
                          {diff.removedItems.map((id) => (
                            <span key={`rem-${id}`} className="md-pill md-mono" data-tone="error">
                              -{addonsByWid.get(id)?.title ?? id}
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
