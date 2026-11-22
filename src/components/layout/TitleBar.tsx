import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Minus, X } from 'lucide-react';
import { isTauriRuntime } from '../../api/ipc';
import { LoadingIndicator } from '../m3/primitives';

export interface TitleBarProps {
  activityLabel?: string | null;
  isPreview: boolean;
}

function BrandMark() {
  return (
    <svg width="18" height="18" viewBox="0 0 28 28" fill="none" aria-hidden="true">
      <rect x="2" y="2" width="10.5" height="10.5" rx="3" fill="currentColor" />
      <rect x="15.5" y="2" width="10.5" height="10.5" rx="5.25" fill="currentColor" opacity="0.48" />
      <rect x="2" y="15.5" width="10.5" height="10.5" rx="3" fill="currentColor" opacity="0.74" />
      <rect x="15.5" y="15.5" width="10.5" height="10.5" rx="3" fill="currentColor" />
    </svg>
  );
}

function MaximizeIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
      <rect
        x="1.75"
        y="1.75"
        width="8.5"
        height="8.5"
        rx="1.5"
        stroke="currentColor"
        strokeWidth="1.25"
      />
    </svg>
  );
}

function RestoreIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
      <path
        d="M3.75 3.75V2.5C3.75 1.95 4.2 1.5 4.75 1.5H9.5C10.05 1.5 10.5 1.95 10.5 2.5V7.25C10.5 7.8 10.05 8.25 9.5 8.25H8.25"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinecap="round"
      />
      <rect
        x="1.5"
        y="3.75"
        width="6.75"
        height="6.75"
        rx="1.25"
        stroke="currentColor"
        strokeWidth="1.2"
      />
    </svg>
  );
}

export function TitleBar({ activityLabel, isPreview }: TitleBarProps) {
  const { t } = useTranslation();
  const desktop = isTauriRuntime();
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!desktop) return;
    let active = true;
    let unlisten: (() => void) | undefined;

    const appWindow = getCurrentWindow();
    const syncState = async () => {
      try {
        const isMax = await appWindow.isMaximized();
        if (active) setMaximized(isMax);
      } catch {
        // Ignore in restricted environments/tests.
      }
    };

    void syncState();
    void appWindow
      .onResized(() => {
        void syncState();
      })
      .then((fn) => {
        if (!active) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // Ignore when event API is unavailable.
      });

    return () => {
      active = false;
      unlisten?.();
    };
  }, [desktop]);

  const handleMinimize = () => {
    if (!desktop) return;
    void getCurrentWindow().minimize().catch(() => {});
  };

  const handleToggleMaximize = () => {
    if (!desktop) return;
    const win = getCurrentWindow();
    void win
      .toggleMaximize()
      .then(() => win.isMaximized())
      .then((next) => setMaximized(next))
      .catch(() => {});
  };

  const handleClose = () => {
    if (!desktop) return;
    void getCurrentWindow().close().catch(() => {});
  };

  const handleDragMouseDown = (e: React.MouseEvent<HTMLDivElement>) => {
    if (!desktop || e.button !== 0 || e.detail > 1) return;
    if ((e.target as HTMLElement).closest('button, a, input')) return;
    void getCurrentWindow().startDragging().catch(() => {});
  };

  return (
    <header
      className="l4a-titlebar"
      data-maximized={maximized ? 'true' : 'false'}
      onMouseDown={handleDragMouseDown}
      onDoubleClick={(e) => {
        if ((e.target as HTMLElement).closest('button')) return;
        handleToggleMaximize();
      }}
    >
      <div className="l4a-titlebar-drag" data-tauri-drag-region>
        <div className="l4a-titlebar-brand" data-tauri-drag-region>
          <span className="l4a-titlebar-logo" data-tauri-drag-region>
            <BrandMark />
          </span>
          <span className="l4a-titlebar-name" data-tauri-drag-region>
            Left4Addons
          </span>
          {isPreview && <span className="l4a-preview-dot">{t('nav.preview')}</span>}
        </div>

        <div
          className="l4a-titlebar-activity"
          data-visible={activityLabel ? 'true' : 'false'}
          aria-live="polite"
          data-tauri-drag-region
        >
          {activityLabel && (
            <>
              <LoadingIndicator size="sm" label={activityLabel} />
              <span className="md-label-medium">{activityLabel}</span>
            </>
          )}
        </div>
      </div>

      {desktop && (
        <div className="l4a-window-controls" role="group" aria-label="Window controls">
          <button
            type="button"
            className="l4a-window-btn"
            aria-label={t('window.minimize')}
            data-tooltip={t('window.minimize')}
            onClick={handleMinimize}
          >
            <Minus size={14} strokeWidth={1.9} />
          </button>
          <button
            type="button"
            className="l4a-window-btn"
            aria-label={maximized ? t('window.restore') : t('window.maximize')}
            data-tooltip={maximized ? t('window.restore') : t('window.maximize')}
            onClick={handleToggleMaximize}
          >
            {maximized ? <RestoreIcon /> : <MaximizeIcon />}
          </button>
          <button
            type="button"
            className="l4a-window-btn l4a-window-btn-close"
            aria-label={t('window.close')}
            data-tooltip={t('window.close')}
            onClick={handleClose}
          >
            <X size={15} strokeWidth={1.9} />
          </button>
        </div>
      )}
    </header>
  );
}
