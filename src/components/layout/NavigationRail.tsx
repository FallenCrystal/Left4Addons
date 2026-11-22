import { useTranslation } from 'react-i18next';
import {
  Activity,
  GitCompareArrows,
  Layers,
  Moon,
  Package,
  RefreshCw,
  Settings,
  Store,
  Sun,
} from 'lucide-react';
import { IconButton } from '../m3/primitives';

export type NavDestination = 'library' | 'organize' | 'reconcile' | 'workshop' | 'settings';

export interface NavigationRailProps {
  active: NavDestination;
  onSelect: (dest: NavDestination) => void;
  onScan: () => void;
  scanning: boolean;
  openIssuesCount: number;
  activeTasksCount: number;
  onOpenTasks: () => void;
  resolvedTheme: 'light' | 'dark';
  onToggleTheme: () => void;
  language: 'zh' | 'en';
  onToggleLanguage: () => void;
  isPreview: boolean;
}

export function NavigationRail({
  active,
  onSelect,
  onScan,
  scanning,
  openIssuesCount,
  activeTasksCount,
  onOpenTasks,
  resolvedTheme,
  onToggleTheme,
  language,
  onToggleLanguage,
}: NavigationRailProps) {
  const { t } = useTranslation();

  const destinations: {
    id: NavDestination;
    label: string;
    icon: typeof Package;
    badge?: number;
  }[] = [
    { id: 'library', label: t('nav.library'), icon: Package },
    { id: 'organize', label: t('nav.organize'), icon: Layers },
    {
      id: 'reconcile',
      label: t('nav.reconcile'),
      icon: GitCompareArrows,
      badge: openIssuesCount > 0 ? openIssuesCount : undefined,
    },
    { id: 'workshop', label: t('nav.workshop'), icon: Store },
    { id: 'settings', label: t('nav.settings'), icon: Settings },
  ];

  return (
    <nav className="l4a-rail" aria-label={t('nav.primary')}>
      <button
        type="button"
        className="md-fab"
        aria-label={t('nav.scan')}
        title={t('nav.scan')}
        disabled={scanning}
        onClick={onScan}
      >
        <RefreshCw size={22} className={scanning ? 'md-spin' : undefined} />
      </button>

      <div className="l4a-rail-nav">
        {destinations.map((dest) => {
          const Icon = dest.icon;
          const isActive = active === dest.id;
          return (
            <button
              key={dest.id}
              type="button"
              className="l4a-rail-item"
              data-active={isActive ? 'true' : 'false'}
              aria-current={isActive ? 'page' : undefined}
              onClick={() => onSelect(dest.id)}
            >
              <span className="l4a-rail-pill">
                <Icon size={20} strokeWidth={isActive ? 2.2 : 1.75} />
                {dest.badge !== undefined && (
                  <span className="md-badge">{dest.badge > 99 ? '99+' : dest.badge}</span>
                )}
              </span>
              <span className="l4a-rail-label">{dest.label}</span>
            </button>
          );
        })}
      </div>

      <div className="l4a-rail-footer">
        <IconButton
          label={t('nav.tasks')}
          variant={activeTasksCount > 0 ? 'tonal' : 'standard'}
          onClick={onOpenTasks}
        >
          <Activity size={20} />
          {activeTasksCount > 0 && (
            <span className="md-badge" data-tone="primary">
              {activeTasksCount}
            </span>
          )}
        </IconButton>

        <IconButton label={t('settings.themeMode')} onClick={onToggleTheme}>
          {resolvedTheme === 'dark' ? <Sun size={19} /> : <Moon size={19} />}
        </IconButton>

        <IconButton label={t('settings.language')} onClick={onToggleLanguage}>
          <span className="md-label-medium">{language === 'zh' ? 'EN' : '中'}</span>
        </IconButton>
      </div>
    </nav>
  );
}
