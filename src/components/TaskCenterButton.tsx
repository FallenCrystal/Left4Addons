import React from 'react';
import { useTranslation } from 'react-i18next';
import { Download, RefreshCw, AlertTriangle, ClipboardCheck } from 'lucide-react';
import { BackgroundTask } from '../types/addon';

interface TaskCenterButtonProps {
  syncingSteam: boolean;
  backgroundTasks: BackgroundTask[];
  onClick: () => void;
}

export const TaskCenterButton: React.FC<TaskCenterButtonProps> = ({
  syncingSteam,
  backgroundTasks,
  onClick,
}) => {
  const { t } = useTranslation();

  // Determine states
  const isDownloading = backgroundTasks.some(
    (t) => t.kind === 'download' && (t.status === 'running' || t.status === 'queued')
  );

  const isSyncing = syncingSteam || backgroundTasks.some(
    (t) => t.kind === 'workshop-crawl' && (t.status === 'running' || t.status === 'queued')
  );

  const hasWarning = backgroundTasks.some((t) => t.status === 'failed');

  // Icon, color, animation priority: Download -> Sync -> Warning -> No tasks
  let icon = <ClipboardCheck size={20} />;
  let btnClass = 'btn-secondary';
  let animationClass = '';
  let tooltipKey = 'taskCenter.noTasks';
  let tooltipDefault = '暂无任务';

  if (isDownloading) {
    icon = <Download size={20} />;
    btnClass = 'btn-primary';
    animationClass = 'task-download-bounce';
    tooltipKey = 'taskCenter.downloading';
    tooltipDefault = '正在下载组件...';
  } else if (isSyncing) {
    icon = <RefreshCw size={20} />;
    btnClass = 'btn-primary';
    animationClass = 'animate-spin';
    tooltipKey = 'taskCenter.syncing';
    tooltipDefault = '正在同步创意工坊信息...';
  } else if (hasWarning) {
    icon = <AlertTriangle size={20} />;
    btnClass = 'btn-warning';
    animationClass = 'animate-pulse';
    tooltipKey = 'taskCenter.warning';
    tooltipDefault = '任务出现警告';
  }

  return (
    <button
      className={`btn ${btnClass} btn-icon-only task-center-btn`}
      style={{
        width: '42px',
        height: '42px',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        borderRadius: '12px',
        flexShrink: 0,
        position: 'relative',
      }}
      onClick={onClick}
      title={t(tooltipKey, tooltipDefault)}
    >
      <span className={animationClass} style={{ display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
        {icon}
      </span>
    </button>
  );
};
