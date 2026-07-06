import { Grid, CheckCircle, Lock, HardDrive } from 'lucide-react';
import { formatBytes } from '../utils/addonHelpers';
import { useTranslation } from 'react-i18next';

interface StatsBarProps {
  totalAddonsCount: number;
  activeCount: number;
  disabledCount: number;
  totalStorageSize: number;
}

export function StatsBar({
  totalAddonsCount,
  activeCount,
  disabledCount,
  totalStorageSize,
}: StatsBarProps) {
  const { t } = useTranslation();

  return (
    <div className="stats-card-container">
      <div className="stat-card">
        <div className="stat-icon"><Grid size={20} /></div>
        <div className="stat-info">
          <span className="stat-value">{totalAddonsCount}</span>
          <span className="stat-label">{t('statsBar.totalCount')}</span>
        </div>
      </div>
      <div className="stat-card">
        <div className="stat-icon" style={{ color: 'var(--md-sys-color-success)' }}><CheckCircle size={20} /></div>
        <div className="stat-info">
          <span className="stat-value">{activeCount}</span>
          <span className="stat-label">{t('statsBar.enabled')}</span>
        </div>
      </div>
      <div className="stat-card">
        <div className="stat-icon" style={{ color: 'var(--md-sys-color-error)' }}><Lock size={20} /></div>
        <div className="stat-info">
          <span className="stat-value">{disabledCount}</span>
          <span className="stat-label">{t('statsBar.disabled')}</span>
        </div>
      </div>
      <div className="stat-card">
        <div className="stat-icon"><HardDrive size={20} /></div>
        <div className="stat-info">
          <span className="stat-value">{formatBytes(totalStorageSize)}</span>
          <span className="stat-label">{t('statsBar.totalDisk')}</span>
        </div>
      </div>
    </div>
  );
}
