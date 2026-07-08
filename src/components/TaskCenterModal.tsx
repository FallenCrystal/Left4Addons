import React from 'react';
import { useTranslation } from 'react-i18next';
import { X, RotateCw, Trash2, AlertTriangle, Loader2, RefreshCw, ClipboardCheck } from 'lucide-react';
import { BackgroundTask } from '../types/addon';
import { CacheImage } from './CacheImage';

interface TaskCenterModalProps {
  open: boolean;
  onCancel: () => void;
  backgroundTasks: BackgroundTask[];
  downloadProgress: Record<string, number>;
  syncingSteam: boolean;
  onSyncSteam: () => void;
  onCancelTask: (id: string) => void;
  onRetryTask: (id: string) => void;
  onClearFinishedTasks: () => void;
}

export const TaskCenterModal: React.FC<TaskCenterModalProps> = ({
  open,
  onCancel,
  backgroundTasks,
  downloadProgress,
  syncingSteam,
  onSyncSteam,
  onCancelTask,
  onRetryTask,
  onClearFinishedTasks,
}) => {
  const { t } = useTranslation();

  if (!open) return null;

  // Filter tasks into categories
  const activeTasks = backgroundTasks.filter(
    (t) => t.status === 'running' || t.status === 'queued'
  );

  const failedTasks = backgroundTasks.filter((t) => t.status === 'failed');

  const finishedTasks = backgroundTasks.filter(
    (t) => t.status === 'completed' || t.status === 'cancelled'
  );

  const hasFinishedTasks = finishedTasks.length > 0 || failedTasks.length > 0;

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div
        className="modal-content task-center-modal"
        onClick={(e) => e.stopPropagation()}
        style={{
          width: '600px',
          maxWidth: '90%',
          maxHeight: '85vh',
          display: 'flex',
          flexDirection: 'column',
          padding: '24px',
          borderRadius: '24px',
          backgroundColor: 'var(--md-sys-surface-container)',
          border: '1px solid var(--md-sys-color-outline-variant)',
          boxShadow: '0 12px 36px rgba(0, 0, 0, 0.5)',
          overflow: 'hidden',
        }}
      >
        {/* Header */}
        <div
          style={{
            display: 'flex',
            justifyContent: 'space-between',
            alignItems: 'center',
            marginBottom: '20px',
            flexShrink: 0,
          }}
        >
          <div>
            <h2 className="modal-title" style={{ margin: 0, fontSize: '20px', fontWeight: 600 }}>
              {t('taskCenter.title', '任务中心')}
            </h2>
            <p style={{ margin: '4px 0 0 0', fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
              {t('taskCenter.subtitle', '管理和查看组件的下载与同步任务')}
            </p>
          </div>
          <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
            {hasFinishedTasks && (
              <button
                className="btn btn-secondary"
                onClick={onClearFinishedTasks}
                title={t('taskCenter.clearTooltip', '清除所有非活动任务')}
                style={{
                  height: '36px',
                  borderRadius: '10px',
                  fontSize: '12px',
                  padding: '0 12px',
                  display: 'flex',
                  alignItems: 'center',
                  gap: '6px',
                }}
              >
                <Trash2 size={14} />
                <span>{t('taskCenter.clearFinished', '清空历史')}</span>
              </button>
            )}
            <button
              className="btn btn-primary"
              disabled={syncingSteam}
              onClick={onSyncSteam}
              style={{
                height: '36px',
                borderRadius: '10px',
                fontSize: '12px',
                padding: '0 12px',
                display: 'flex',
                alignItems: 'center',
                gap: '6px',
              }}
            >
              <RefreshCw size={14} className={syncingSteam ? 'animate-spin' : ''} />
              <span>{t('taskCenter.startSync', '同步创意工坊')}</span>
            </button>
            <button
              onClick={onCancel}
              style={{
                background: 'none',
                border: 'none',
                cursor: 'pointer',
                color: 'var(--md-sys-color-outline)',
                display: 'flex',
                padding: '4px',
              }}
            >
              <X size={20} />
            </button>
          </div>
        </div>

        {/* Scrollable list area */}
        <div
          style={{
            flex: 1,
            overflowY: 'auto',
            display: 'flex',
            flexDirection: 'column',
            gap: '16px',
            paddingRight: '4px',
          }}
        >
          {/* Steam Sync State (Separate active item if running) */}
          {syncingSteam && (
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: '12px',
                padding: '12px 16px',
                borderRadius: '16px',
                backgroundColor: 'var(--md-sys-color-primary-container)',
                border: '1px solid var(--md-sys-color-outline-variant)',
              }}
            >
              <Loader2 className="animate-spin" size={24} style={{ color: 'var(--md-sys-color-primary)' }} />
              <div style={{ flex: 1 }}>
                <div style={{ fontSize: '14px', fontWeight: 600, color: 'var(--md-sys-color-on-primary-container)' }}>
                  {t('taskCenter.syncing', '正在同步创意工坊信息...')}
                </div>
                <div style={{ fontSize: '12px', color: 'var(--md-sys-color-outline)', marginTop: '2px' }}>
                  {t('taskCenter.syncDesc', '正在获取和更新所有组件的最新 Steam 创意工坊元数据')}
                </div>
              </div>
            </div>
          )}

          {/* Active Tasks (Downloads, crawls) */}
          {activeTasks.length > 0 && (
            <div>
              <h3 style={{ fontSize: '13px', margin: '0 0 10px 0', color: 'var(--md-sys-color-primary)', fontWeight: 600 }}>
                {t('taskCenter.activeTasks', '正在运行的队列')} ({activeTasks.length})
              </h3>
              <div style={{ display: 'flex', flexDirection: 'column', gap: '10px' }}>
                {activeTasks.map((task) => {
                  const isDownload = task.kind === 'download';
                  const workshopId = task.targetIds[0];
                  // If download, read current progress percent
                  const percent = isDownload ? (downloadProgress[workshopId] ?? task.progress ?? 0) : 0;
                  const isRunning = task.status === 'running';

                  return (
                    <div
                      key={task.id}
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        gap: '12px',
                        padding: '12px',
                        borderRadius: '14px',
                        backgroundColor: 'var(--md-sys-surface-container-high)',
                        border: '1px solid var(--md-sys-color-outline-variant)',
                      }}
                    >
                      {/* Image Preview */}
                      {isDownload ? (
                        <CacheImage
                          srcPath={task.imagePath}
                          style={{
                            width: '44px',
                            height: '44px',
                            borderRadius: '8px',
                            objectFit: 'cover',
                            backgroundColor: 'var(--md-sys-surface-container-highest)',
                          }}
                        />
                      ) : (
                        <div
                          style={{
                            width: '44px',
                            height: '44px',
                            borderRadius: '8px',
                            display: 'flex',
                            alignItems: 'center',
                            justifyContent: 'center',
                            backgroundColor: 'var(--md-sys-surface-container-highest)',
                            color: 'var(--md-sys-color-primary)',
                          }}
                        >
                          <RefreshCw size={20} className={isRunning ? 'animate-spin' : ''} />
                        </div>
                      )}

                      {/* Content */}
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div
                          style={{
                            fontSize: '13.5px',
                            fontWeight: 600,
                            color: '#fff',
                            overflow: 'hidden',
                            textOverflow: 'ellipsis',
                            whiteSpace: 'nowrap',
                          }}
                        >
                          {task.title || `Workshop Item ${workshopId}`}
                        </div>
                        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginTop: '4px', fontSize: '11px', color: 'var(--md-sys-color-outline)' }}>
                          <span>
                            {isDownload
                              ? isRunning
                                ? `${t('taskCenter.downloading', '正在下载')}...`
                                : t('taskCenter.inQueue', '等待下载')
                              : isRunning
                                ? `${t('taskCenter.syncing', '正在同步')}...`
                                : t('taskCenter.inQueue', '等待同步')}
                          </span>
                          {isDownload && isRunning && <span>{percent}%</span>}
                        </div>

                        {/* Progress Bar */}
                        {isDownload && isRunning && (
                          <div
                            style={{
                              height: '4px',
                              width: '100%',
                              backgroundColor: 'var(--md-sys-surface-container-highest)',
                              borderRadius: '2px',
                              marginTop: '6px',
                              overflow: 'hidden',
                            }}
                          >
                            <div
                              style={{
                                height: '100%',
                                backgroundColor: 'var(--md-sys-color-primary)',
                                width: `${percent}%`,
                                transition: 'width 0.3s ease-out',
                              }}
                            />
                          </div>
                        )}
                      </div>

                      {/* Actions */}
                      <button
                        className="btn btn-icon-only"
                        onClick={() => onCancelTask(task.id)}
                        title={t('taskCenter.cancel', '取消任务')}
                        style={{
                          width: '32px',
                          height: '32px',
                          borderRadius: '8px',
                          display: 'flex',
                          alignItems: 'center',
                          justifyContent: 'center',
                          background: 'none',
                          border: 'none',
                          color: 'var(--md-sys-color-outline)',
                          cursor: 'pointer',
                        }}
                      >
                        <X size={16} />
                      </button>
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          {/* Failed / Warning Tasks */}
          {failedTasks.length > 0 && (
            <div>
              <h3 style={{ fontSize: '13px', margin: '0 0 10px 0', color: 'var(--md-sys-color-error)', fontWeight: 600 }}>
                {t('taskCenter.errors', '警告通知 / 失败的任务')} ({failedTasks.length})
              </h3>
              <div style={{ display: 'flex', flexDirection: 'column', gap: '10px' }}>
                {failedTasks.map((task) => {
                  const workshopId = task.targetIds[0];
                  return (
                    <div
                      key={task.id}
                      style={{
                        display: 'flex',
                        flexDirection: 'column',
                        padding: '12px',
                        borderRadius: '14px',
                        backgroundColor: 'rgba(255, 180, 171, 0.1)',
                        border: '1px solid rgba(255, 180, 171, 0.3)',
                      }}
                    >
                      <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
                        <AlertTriangle size={18} style={{ color: 'var(--md-sys-color-error)' }} />
                        <div style={{ flex: 1, minWidth: 0 }}>
                          <div
                            style={{
                              fontSize: '13.5px',
                              fontWeight: 600,
                              color: '#fff',
                              overflow: 'hidden',
                              textOverflow: 'ellipsis',
                              whiteSpace: 'nowrap',
                            }}
                          >
                            {task.title || `Workshop Item ${workshopId}`}
                          </div>
                          <div style={{ fontSize: '11px', color: 'var(--md-sys-color-error)', marginTop: '2px' }}>
                            {task.kind === 'download' ? t('taskCenter.downloadFailed', '下载失败') : t('taskCenter.syncFailed', '同步元数据失败')}
                          </div>
                        </div>

                        {/* Actions */}
                        <div style={{ display: 'flex', gap: '4px' }}>
                          <button
                            className="btn btn-secondary"
                            onClick={() => onRetryTask(task.id)}
                            title={t('taskCenter.retry', '重新尝试')}
                            style={{
                              width: '32px',
                              height: '32px',
                              borderRadius: '8px',
                              padding: 0,
                              display: 'flex',
                              alignItems: 'center',
                              justifyContent: 'center',
                            }}
                          >
                            <RotateCw size={14} />
                          </button>
                          <button
                            className="btn btn-secondary"
                            onClick={() => onCancelTask(task.id)}
                            title={t('taskCenter.clear', '移除通知')}
                            style={{
                              width: '32px',
                              height: '32px',
                              borderRadius: '8px',
                              padding: 0,
                              display: 'flex',
                              alignItems: 'center',
                              justifyContent: 'center',
                            }}
                          >
                            <X size={14} />
                          </button>
                        </div>
                      </div>

                      {/* Error details */}
                      {task.error && (
                        <div
                          style={{
                            marginTop: '8px',
                            padding: '8px 12px',
                            borderRadius: '8px',
                            backgroundColor: 'var(--md-sys-surface-container-highest)',
                            color: 'var(--md-sys-color-on-surface-variant)',
                            fontSize: '11.5px',
                            fontFamily: 'monospace',
                            whiteSpace: 'pre-wrap',
                            wordBreak: 'break-all',
                            border: '1px solid var(--md-sys-color-outline-variant)',
                          }}
                        >
                          {t('taskCenter.errorDetails', { err: task.error })}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          {/* Empty State */}
          {!syncingSteam && activeTasks.length === 0 && failedTasks.length === 0 && (
            <div
              style={{
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'center',
                justifyContent: 'center',
                flex: 1,
                padding: '48px 0',
                color: 'var(--md-sys-color-outline)',
              }}
            >
              <ClipboardCheck size={48} style={{ marginBottom: '16px', opacity: 0.5 }} />
              <p style={{ margin: 0, fontSize: '15px', fontWeight: 500 }}>
                {t('taskCenter.noTasks', '暂无任何任务')}
              </p>
              <p style={{ margin: '6px 0 0 0', fontSize: '12px', opacity: 0.8 }}>
                {t('taskCenter.noTasksDesc', '下载组件或同步创意工坊时将会在此处显示')}
              </p>
            </div>
          )}
        </div>

        {/* Footer */}
        <div
          style={{
            marginTop: '20px',
            display: 'flex',
            justifyContent: 'flex-end',
            flexShrink: 0,
          }}
        >
          <button className="btn btn-secondary" onClick={onCancel} style={{ borderRadius: '10px', height: '38px', padding: '0 20px' }}>
            {t('taskCenter.close', '关闭')}
          </button>
        </div>
      </div>
    </div>
  );
};
