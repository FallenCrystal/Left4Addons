import { useCallback, useEffect, useState } from 'react';
import {
  Database,
  Gauge,
  LogOut,
  Moon,
  Radio,
  Server,
  Shield,
  Sun,
} from 'lucide-react';
import {
  ApiError,
  getAdminSession,
  getOverview,
  logoutAdmin,
  setSessionToken,
  type CatalogOverviewResponse,
  type CatalogSteamStatus,
} from './api';
import { ItemInspectorDrawer } from './components/ItemInspectorDrawer';
import { LoginScreen } from './components/LoginScreen';
import { OverviewPanel } from './components/OverviewPanel';
import { SettingsAuditPanel } from './components/SettingsAuditPanel';
import { SteamAuthPanel } from './components/SteamAuthPanel';
import { WorkshopCachePanel } from './components/WorkshopCachePanel';

type NavTab = 'overview' | 'cache' | 'steam' | 'settings';

export function App() {
  const [theme, setTheme] = useState<'dark' | 'light'>(() => {
    const saved = window.localStorage.getItem('l4a_catalog_theme');
    return saved === 'light' ? 'light' : 'dark';
  });
  const [checkingSession, setCheckingSession] = useState(true);
  const [username, setUsername] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<NavTab>('overview');
  const [overview, setOverview] = useState<CatalogOverviewResponse | null>(null);
  const [inspectId, setInspectId] = useState<string | null>(null);
  const [toast, setToast] = useState<{ msg: string; tone: 'default' | 'error' } | null>(null);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    window.localStorage.setItem('l4a_catalog_theme', theme);
  }, [theme]);

  const notify = useCallback((msg: string, tone: 'default' | 'error' = 'default') => {
    setToast({ msg, tone });
  }, []);

  useEffect(() => {
    if (!toast) return;
    const t = window.setTimeout(() => setToast(null), 3800);
    return () => window.clearTimeout(t);
  }, [toast]);

  const refreshOverview = useCallback(async () => {
    try {
      const data = await getOverview();
      setOverview(data);
    } catch (err) {
      if (err instanceof ApiError && (err.status === 401 || err.status === 403)) {
        setUsername(null);
        setSessionToken(null);
        return;
      }
      notify(err instanceof Error ? err.message : String(err), 'error');
    }
  }, [notify]);

  useEffect(() => {
    void getAdminSession()
      .then(async (session) => {
        if (session.authenticated) {
          setUsername(session.username ?? 'admin');
          await refreshOverview();
        }
      })
      .catch(() => {
        setUsername(null);
      })
      .finally(() => {
        setCheckingSession(false);
      });
  }, [refreshOverview]);

  useEffect(() => {
    if (!username) return;
    const timer = window.setInterval(() => {
      void refreshOverview();
    }, 8000);
    return () => window.clearInterval(timer);
  }, [refreshOverview, username]);

  const handleLogout = async () => {
    try {
      await logoutAdmin();
    } catch {
      // clear client state regardless
    }
    setUsername(null);
    setOverview(null);
  };

  const handleSteamUpdated = useCallback((nextSteam: CatalogSteamStatus) => {
    setOverview((prev) => (prev ? { ...prev, steam: nextSteam } : prev));
  }, []);

  if (checkingSession) {
    return (
      <div className="cat-login-shell">
        <div style={{ color: 'var(--cat-text-muted)' }}>正在初始化 Catalog 控制台…</div>
      </div>
    );
  }

  if (!username) {
    return (
      <LoginScreen
        theme={theme}
        onToggleTheme={() => setTheme((t) => (t === 'dark' ? 'light' : 'dark'))}
        onLoggedIn={(session) => {
          setUsername(session.username);
          void refreshOverview();
        }}
      />
    );
  }

  return (
    <div className="cat-app">
      <aside className="cat-sidebar">
        <div className="cat-brand">
          <div className="cat-brand-mark">
            <Server size={18} />
          </div>
          <div>
            <h1 className="cat-brand-title">L4A Catalog</h1>
            <p className="cat-brand-sub">SteamKit CM Backend</p>
          </div>
        </div>

        <nav className="cat-nav" aria-label="控制台主导航">
          <button
            type="button"
            className="cat-nav-btn"
            data-active={activeTab === 'overview' ? 'true' : 'false'}
            onClick={() => setActiveTab('overview')}
          >
            <Gauge size={16} />
            <span>总览与同步引擎</span>
          </button>
          <button
            type="button"
            className="cat-nav-btn"
            data-active={activeTab === 'cache' ? 'true' : 'false'}
            onClick={() => setActiveTab('cache')}
          >
            <Database size={16} />
            <span>工坊缓存库</span>
          </button>
          <button
            type="button"
            className="cat-nav-btn"
            data-active={activeTab === 'steam' ? 'true' : 'false'}
            onClick={() => setActiveTab('steam')}
          >
            <Radio size={16} />
            <span>SteamKit 账号会话</span>
          </button>
          <button
            type="button"
            className="cat-nav-btn"
            data-active={activeTab === 'settings' ? 'true' : 'false'}
            onClick={() => setActiveTab('settings')}
          >
            <Shield size={16} />
            <span>配置与审计日志</span>
          </button>
        </nav>

        <div className="cat-sidebar-footer">
          {overview && (
            <div className="cat-steam-mini">
              <div className="cat-row" style={{ justifyContent: 'space-between' }}>
                <span style={{ fontWeight: 600 }}>Steam CM</span>
                <span
                  className="cat-badge"
                  data-tone={
                    overview.steam.connectionState === 'connected' ? 'success' : 'warning'
                  }
                >
                  {overview.steam.connectionState}
                </span>
              </div>
              <span className="cat-mono" style={{ color: 'var(--cat-text-muted)', fontSize: 11.5 }}>
                {overview.steam.mode === 'account'
                  ? overview.steam.accountName ?? 'account'
                  : 'anonymous'}
              </span>
            </div>
          )}

          <div className="cat-row" style={{ justifyContent: 'space-between' }}>
            <button
              type="button"
              className="cat-btn cat-btn-sm"
              onClick={() => setTheme((t) => (t === 'dark' ? 'light' : 'dark'))}
              title="切换主题"
            >
              {theme === 'dark' ? <Sun size={14} /> : <Moon size={14} />}
              <span>{theme === 'dark' ? '浅色' : '深色'}</span>
            </button>
            <button
              type="button"
              className="cat-btn cat-btn-sm"
              onClick={() => void handleLogout()}
            >
              <LogOut size={14} />
              <span>退出 ({username})</span>
            </button>
          </div>
        </div>
      </aside>

      <main className="cat-main">
        {!overview ? (
          <div style={{ color: 'var(--cat-text-muted)', padding: 24 }}>正在加载服务状态…</div>
        ) : (
          <>
            {activeTab === 'overview' && (
              <OverviewPanel
                overview={overview}
                onRefresh={refreshOverview}
                onInspectItem={(wid) => setInspectId(wid)}
                onOpenSteamAuth={() => setActiveTab('steam')}
                onNotify={notify}
              />
            )}
            {activeTab === 'cache' && (
              <WorkshopCachePanel
                onInspectItem={(wid) => setInspectId(wid)}
                onNotify={notify}
              />
            )}
            {activeTab === 'steam' && (
              <SteamAuthPanel
                steam={overview.steam}
                onSteamUpdated={handleSteamUpdated}
                onNotify={notify}
              />
            )}
            {activeTab === 'settings' && (
              <SettingsAuditPanel
                overview={overview}
                onInspectItem={(wid) => setInspectId(wid)}
                onNotify={notify}
              />
            )}
          </>
        )}
      </main>

      <ItemInspectorDrawer
        workshopId={inspectId}
        onClose={() => setInspectId(null)}
        onSelectWorkshopId={(nextId) => setInspectId(nextId)}
        onUpdated={() => void refreshOverview()}
        onNotify={notify}
      />

      {toast && (
        <div className="cat-toast" data-tone={toast.tone} role="status">
          {toast.msg}
        </div>
      )}
    </div>
  );
}
