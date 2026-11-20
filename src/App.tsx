import { useCallback, useEffect, useState } from 'react';
import {
  getDashboardSnapshot,
  isTauriRuntime,
  resolveReconcileIssue,
  scanWorkspace,
  subscribeCoreEvents,
  syncWorkshop,
  type DashboardSnapshot,
  type ReconcileAction,
  type SyncReport,
} from './api/ipc';

export function App() {
  const [snapshot, setSnapshot] = useState<DashboardSnapshot | null>(null);
  const [syncReport, setSyncReport] = useState<SyncReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    if (!isTauriRuntime()) return;
    try {
      const next = await getDashboardSnapshot();
      setSnapshot(next);
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let active = true;
    let unlisten: (() => void) | undefined;

    void getDashboardSnapshot()
      .then((next) => {
        if (!active) return;
        setSnapshot(next);
        setError(null);
      })
      .catch((err) => {
        if (!active) return;
        setError(String(err));
      });

    void subscribeCoreEvents((ev) => {
      if (!active) return;
      if (ev.type === 'snapshot_updated') {
        void refresh();
      } else if (ev.type === 'watch_error') {
        setError(ev.message);
      }
    }).then((fn) => {
      if (!active) {
        fn();
        return;
      }
      unlisten = fn;
    });

    return () => {
      active = false;
      unlisten?.();
    };
  }, [refresh]);

  async function handleScan() {
    setBusy(true);
    try {
      setSnapshot(await scanWorkspace());
      setError(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleSync() {
    setBusy(true);
    try {
      setSyncReport(await syncWorkshop());
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleResolve(issueId: string, action: ReconcileAction) {
    setBusy(true);
    try {
      setSnapshot(await resolveReconcileIssue(issueId, action));
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="shell">
      <header className="panel header">
        <div>
          <h1>Left4Addons v2 Core Workbench</h1>
          <p className="muted">
            多 Crate Rust 底座 + SQLite 引擎 + Dummy Bypass v2 对账状态机 · 前端交互壳占位
          </p>
        </div>
        <div className="actions">
          <button disabled={busy || !isTauriRuntime()} onClick={() => void handleScan()}>
            扫描与对账
          </button>
          <button disabled={busy || !isTauriRuntime()} onClick={() => void handleSync()}>
            工坊生命周期同步
          </button>
        </div>
      </header>

      {!isTauriRuntime() && (
        <section className="panel notice">
          当前运行于纯浏览器预览模式。桌面核心引擎命令需通过 <code>npm run tauri:dev</code> 启动 Tauri
          窗口 (独立后端控制台可运行 <code>npm run server:run</code> 后访问 <code>/admin</code>)。
        </section>
      )}

      {error && <section className="panel error">{error}</section>}

      {snapshot && (
        <>
          <section className="grid">
            <div className="panel stat">
              <span className="muted">已知 Addons</span>
              <strong>{snapshot.addons.length}</strong>
            </div>
            <div className="panel stat">
              <span className="muted">对账待处理告警</span>
              <strong>{snapshot.openIssues.length}</strong>
            </div>
            <div className="panel stat">
              <span className="muted">Bundle / 预设 / 自定义组</span>
              <strong>
                {snapshot.bundles.length} / {snapshot.profiles.length} / {snapshot.userGroups.length}
              </strong>
            </div>
            <div className="panel stat">
              <span className="muted">追踪合集</span>
              <strong>{snapshot.collections.length}</strong>
            </div>
          </section>

          <section className="panel">
            <h2>Dummy Bypass v2 与目录对账告警</h2>
            {snapshot.openIssues.length === 0 ? (
              <p className="muted">暂无目录漂移或游戏覆写告警。</p>
            ) : (
              <ul className="list">
                {snapshot.openIssues.map((issue) => (
                  <li key={issue.id} className="list-item">
                    <div>
                      <strong>{issue.kind}</strong>
                      <p className="muted">{issue.summary}</p>
                    </div>
                    <div className="actions">
                      {issue.availableActions.map((act) => (
                        <button
                          key={act}
                          disabled={busy}
                          onClick={() => void handleResolve(issue.id, act)}
                        >
                          {act}
                        </button>
                      ))}
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </section>
        </>
      )}

      {syncReport && (
        <section className="panel">
          <h2>最近生命周期同步报告</h2>
          <pre>{JSON.stringify(syncReport, null, 2)}</pre>
        </section>
      )}
    </main>
  );
}

export default App;
