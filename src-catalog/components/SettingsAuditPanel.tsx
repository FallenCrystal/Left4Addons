import { useCallback, useEffect, useState, type FormEvent } from 'react';
import { FileText, KeyRound, RefreshCw, Server } from 'lucide-react';
import {
  changeAdminPassword,
  formatIsoTime,
  getAuditLogs,
  type AuditLogEntry,
  type CatalogOverviewResponse,
} from '../api';

export interface SettingsAuditPanelProps {
  overview: CatalogOverviewResponse;
  onInspectItem: (workshopId: string) => void;
  onNotify: (msg: string, tone?: 'default' | 'error') => void;
}

export function SettingsAuditPanel({
  overview,
  onInspectItem,
  onNotify,
}: SettingsAuditPanelProps) {
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [savingPw, setSavingPw] = useState(false);

  const [logs, setLogs] = useState<AuditLogEntry[]>([]);
  const [loadingLogs, setLoadingLogs] = useState(true);

  const loadLogs = useCallback(async () => {
    setLoadingLogs(true);
    try {
      const list = await getAuditLogs(200);
      setLogs(list);
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setLoadingLogs(false);
    }
  }, [onNotify]);

  useEffect(() => {
    let cancelled = false;
    void getAuditLogs(200)
      .then((list) => {
        if (!cancelled) setLogs(list);
      })
      .catch((err) => {
        if (!cancelled) onNotify(err instanceof Error ? err.message : String(err), 'error');
      })
      .finally(() => {
        if (!cancelled) setLoadingLogs(false);
      });
    return () => {
      cancelled = true;
    };
  }, [onNotify]);

  const handlePasswordSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (newPassword.trim().length < 8) {
      onNotify('新密码长度至少需要 8 个字符', 'error');
      return;
    }
    setSavingPw(true);
    try {
      await changeAdminPassword(currentPassword, newPassword);
      setCurrentPassword('');
      setNewPassword('');
      await loadLogs();
      onNotify('管理员密码已更新并同步写入 catalog.toml');
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setSavingPw(false);
    }
  };

  return (
    <>
      <div className="cat-page-header">
        <div>
          <h2 className="cat-page-title">系统配置与安全审计日志</h2>
          <p className="cat-page-desc">
            管理 <code className="cat-mono">catalog.toml</code> 持久化配置、修改控制台管理员密码并审阅关键运维事件。
          </p>
        </div>
        <button type="button" className="cat-btn" onClick={() => void loadLogs()}>
          <RefreshCw size={14} className={loadingLogs ? 'cat-spin' : undefined} />
          <span>刷新日志</span>
        </button>
      </div>

      <div className="cat-grid-2">
        <section className="cat-card">
          <h3 className="cat-card-title">
            <Server size={16} />
            <span>实例运行路径</span>
          </h3>
          <div className="cat-kv-list" style={{ gridTemplateColumns: '1fr' }}>
            <div className="cat-kv">
              <span className="cat-kv-k">监听地址 (server.bind)</span>
              <span className="cat-kv-v cat-mono">{overview.bindAddr}</span>
            </div>
            <div className="cat-kv">
              <span className="cat-kv-k">配置文件路径 (--config)</span>
              <span className="cat-kv-v cat-mono">{overview.configPath ?? '(内存测试模式)'}</span>
            </div>
            <div className="cat-kv">
              <span className="cat-kv-k">SQLite 数据库路径 (server.db_path)</span>
              <span className="cat-kv-v cat-mono">{overview.dbPath}</span>
            </div>
          </div>
        </section>

        <section className="cat-card">
          <h3 className="cat-card-title">
            <KeyRound size={16} />
            <span>修改控制台管理员密码</span>
          </h3>
          <form
            onSubmit={(e) => void handlePasswordSubmit(e)}
            style={{ display: 'flex', flexDirection: 'column', gap: 12 }}
          >
            <div className="cat-field">
              <label className="cat-label">当前密码</label>
              <input
                type="password"
                className="cat-input"
                autoComplete="current-password"
                value={currentPassword}
                onChange={(e) => setCurrentPassword(e.target.value)}
                required
              />
            </div>
            <div className="cat-field">
              <label className="cat-label">新密码 (至少 8 位，保存至 catalog.toml)</label>
              <input
                type="password"
                className="cat-input"
                autoComplete="new-password"
                minLength={8}
                value={newPassword}
                onChange={(e) => setNewPassword(e.target.value)}
                required
              />
            </div>
            <button
              type="submit"
              className="cat-btn cat-btn-primary"
              style={{ alignSelf: 'flex-start' }}
              disabled={savingPw || !currentPassword || newPassword.length < 8}
            >
              <KeyRound size={14} />
              <span>{savingPw ? '正在更新…' : '更新密码'}</span>
            </button>
          </form>
        </section>
      </div>

      <section className="cat-card">
        <div className="cat-card-header">
          <h3 className="cat-card-title">
            <FileText size={16} />
            <span>审计事件记录 ({logs.length})</span>
          </h3>
        </div>
        <div className="cat-table-wrap">
          <table className="cat-table">
            <thead>
              <tr>
                <th>时间</th>
                <th>操作人</th>
                <th>事件类型</th>
                <th>Workshop ID</th>
                <th>说明</th>
              </tr>
            </thead>
            <tbody>
              {logs.length === 0 ? (
                <tr>
                  <td
                    colSpan={5}
                    style={{ textAlign: 'center', padding: 28, color: 'var(--cat-text-muted)' }}
                  >
                    暂无审计事件
                  </td>
                </tr>
              ) : (
                logs.map((entry) => {
                  const wid = entry.workshopId;
                  return (
                    <tr key={entry.id}>
                      <td style={{ fontSize: 12, whiteSpace: 'nowrap' }}>
                        {formatIsoTime(entry.createdAt)}
                      </td>
                      <td className="cat-mono" style={{ fontSize: 12.5 }}>
                        {entry.actor}
                      </td>
                      <td>
                        <span className="cat-badge" data-tone="info">
                          {entry.action}
                        </span>
                      </td>
                      <td className="cat-mono">
                        {wid ? (
                          <button
                            type="button"
                            className="cat-btn cat-btn-sm cat-mono"
                            onClick={() => onInspectItem(wid)}
                          >
                            #{wid}
                          </button>
                        ) : (
                          '—'
                        )}
                      </td>
                      <td style={{ fontSize: 12.5 }}>{entry.reason}</td>
                    </tr>
                  );
                })
              )}
            </tbody>
          </table>
        </div>
      </section>
    </>
  );
}
