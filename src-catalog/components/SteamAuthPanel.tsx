import { useEffect, useState, type FormEvent } from 'react';
import { KeyRound, QrCode, RefreshCw, ShieldCheck, UserCheck, UserX } from 'lucide-react';
import {
  getSteamStatus,
  reconnectSteam,
  setSteamRefreshToken,
  startCredentialsAuth,
  startQrAuth,
  submitSteamGuardCode,
  switchSteamAnonymous,
  type CatalogSteamStatus,
} from '../api';

export interface SteamAuthPanelProps {
  steam: CatalogSteamStatus;
  onSteamUpdated: (next: CatalogSteamStatus) => void;
  onNotify: (msg: string, tone?: 'default' | 'error') => void;
}

type AuthTab = 'qr' | 'credentials' | 'token';

export function SteamAuthPanel({ steam, onSteamUpdated, onNotify }: SteamAuthPanelProps) {
  const [tab, setTab] = useState<AuthTab>('qr');
  const [accountName, setAccountName] = useState('');
  const [password, setPassword] = useState('');
  const [guardCode, setGuardCode] = useState('');
  const [tokenAccount, setTokenAccount] = useState('');
  const [refreshToken, setRefreshToken] = useState('');
  const [busy, setBusy] = useState(false);

  const flow = steam.activeAuthFlow;
  const flowActive =
    flow &&
    (flow.state === 'waiting_for_scan' ||
      flow.state === 'waiting_for_code' ||
      flow.state === 'authenticating');

  useEffect(() => {
    if (!flowActive) return;
    const timer = window.setInterval(() => {
      void getSteamStatus()
        .then((next) => {
          onSteamUpdated(next);
          if (
            next.activeAuthFlow?.state === 'succeeded' &&
            flow?.state !== 'succeeded'
          ) {
            onNotify(`Steam 账号 ${next.accountName ?? ''} 已成功登录并保存 Refresh Token`);
          }
        })
        .catch(() => {
          // ignore transient poll failure
        });
    }, 2000);
    return () => window.clearInterval(timer);
  }, [flow?.state, flowActive, onNotify, onSteamUpdated]);

  const handleStartQr = async () => {
    setBusy(true);
    try {
      await startQrAuth();
      const next = await getSteamStatus();
      onSteamUpdated(next);
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setBusy(false);
    }
  };

  const handleStartCredentials = async (e: FormEvent) => {
    e.preventDefault();
    if (!accountName.trim() || !password) return;
    setBusy(true);
    try {
      await startCredentialsAuth(accountName.trim(), password);
      setPassword('');
      const next = await getSteamStatus();
      onSteamUpdated(next);
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setBusy(false);
    }
  };

  const handleSubmitGuard = async (e: FormEvent) => {
    e.preventDefault();
    if (!flow || !guardCode.trim()) return;
    setBusy(true);
    try {
      await submitSteamGuardCode(flow.flowId, guardCode.trim());
      setGuardCode('');
      const next = await getSteamStatus();
      onSteamUpdated(next);
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setBusy(false);
    }
  };

  const handleSaveToken = async (e: FormEvent) => {
    e.preventDefault();
    if (!tokenAccount.trim() || !refreshToken.trim()) return;
    setBusy(true);
    try {
      const next = await setSteamRefreshToken(tokenAccount.trim(), refreshToken.trim());
      setRefreshToken('');
      onSteamUpdated(next);
      onNotify('Refresh Token 已验证并写入 catalog.toml');
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setBusy(false);
    }
  };

  const handleAnonymous = async () => {
    setBusy(true);
    try {
      const next = await switchSteamAnonymous();
      onSteamUpdated(next);
      onNotify('已切换回匿名 SteamKit CM 会话');
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setBusy(false);
    }
  };

  const handleReconnect = async () => {
    setBusy(true);
    try {
      const next = await reconnectSteam();
      onSteamUpdated(next);
      onNotify('已重新建立 Steam CM 会话');
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="cat-page-header">
        <div>
          <h2 className="cat-page-title">SteamKit CM 会话与账号登录</h2>
          <p className="cat-page-desc">
            匿名 CM 会话默认即可获取模组详情、前置依赖、合集与 <code className="cat-mono">file_url</code> 直链；登录普通 Steam 账号后可额外解析创作者昵称并提升受限工坊条目访问配额。
          </p>
        </div>
        <div className="cat-row">
          <button
            type="button"
            className="cat-btn"
            disabled={busy}
            onClick={() => void handleReconnect()}
          >
            <RefreshCw size={14} />
            <span>重连 CM</span>
          </button>
          {steam.mode === 'account' && (
            <button
              type="button"
              className="cat-btn cat-btn-danger"
              disabled={busy}
              onClick={() => void handleAnonymous()}
            >
              <UserX size={14} />
              <span>退出账号并切回匿名</span>
            </button>
          )}
        </div>
      </div>

      <section className="cat-card">
        <div className="cat-card-header">
          <h3 className="cat-card-title">
            <UserCheck size={16} />
            <span>当前连接身份</span>
          </h3>
          <span
            className="cat-badge"
            data-tone={steam.connectionState === 'connected' ? 'success' : 'warning'}
          >
            {steam.connectionState}
          </span>
        </div>
        <div className="cat-kv-list">
          <div className="cat-kv">
            <span className="cat-kv-k">身份模式</span>
            <span className="cat-kv-v">
              {steam.mode === 'account' ? '已登录 Steam 账号 (持久化 Refresh Token)' : '匿名客户端 (Anonymous CM)'}
            </span>
          </div>
          <div className="cat-kv">
            <span className="cat-kv-k">账号名</span>
            <span className="cat-kv-v cat-mono">{steam.accountName ?? 'anonymous'}</span>
          </div>
          <div className="cat-kv">
            <span className="cat-kv-k">SteamID64</span>
            <span className="cat-kv-v cat-mono">{steam.steamId ?? '—'}</span>
          </div>
          <div className="cat-kv">
            <span className="cat-kv-k">协议后端</span>
            <span className="cat-kv-v cat-mono">{steam.providerName}</span>
          </div>
        </div>
      </section>

      <section className="cat-card">
        <div className="cat-card-header">
          <div className="cat-tabs" role="tablist">
            <button
              type="button"
              role="tab"
              className="cat-tab"
              data-active={tab === 'qr' ? 'true' : 'false'}
              onClick={() => setTab('qr')}
            >
              手机令牌扫码登录
            </button>
            <button
              type="button"
              role="tab"
              className="cat-tab"
              data-active={tab === 'credentials' ? 'true' : 'false'}
              onClick={() => setTab('credentials')}
            >
              账号密码 + Steam Guard
            </button>
            <button
              type="button"
              role="tab"
              className="cat-tab"
              data-active={tab === 'token' ? 'true' : 'false'}
              onClick={() => setTab('token')}
            >
              导入 Refresh Token
            </button>
          </div>
        </div>

        {flow?.error && (
          <div className="cat-banner" data-tone="error">
            <span>认证失败：{flow.error}</span>
          </div>
        )}
        {flow?.state === 'succeeded' && (
          <div className="cat-banner" data-tone="success">
            <span>
              已成功登录账号 <strong className="cat-mono">{flow.accountName}</strong>，Refresh Token 已自动写入 <code className="cat-mono">catalog.toml</code>。
            </span>
          </div>
        )}

        {tab === 'qr' && (
          <div className="cat-row" style={{ alignItems: 'flex-start', gap: 24 }}>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 12, maxWidth: 420 }}>
              <p style={{ margin: 0, color: 'var(--cat-text-muted)', fontSize: 13 }}>
                使用手机 Steam App 扫描二维码即可直接签发持久化的 CM Refresh Token，无需在服务器输入账号密码。
              </p>
              <button
                type="button"
                className="cat-btn cat-btn-primary"
                style={{ alignSelf: 'flex-start' }}
                disabled={busy}
                onClick={() => void handleStartQr()}
              >
                <QrCode size={15} />
                <span>{flow?.kind === 'qr' && flowActive ? '刷新登录二维码' : '生成 Steam 登录二维码'}</span>
              </button>
              {flow?.kind === 'qr' && flow.challengeUrl && (
                <div className="cat-kv">
                  <span className="cat-kv-k">挑战链接 (状态: {flow.state})</span>
                  <span className="cat-kv-v cat-mono" style={{ fontSize: 11.5 }}>
                    {flow.challengeUrl}
                  </span>
                </div>
              )}
            </div>

            {flow?.kind === 'qr' && flow.qrSvg && (
              <div
                className="cat-qr-box"
                aria-label="Steam 登录二维码"
                dangerouslySetInnerHTML={{ __html: flow.qrSvg }}
              />
            )}
          </div>
        )}

        {tab === 'credentials' && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 16, maxWidth: 480 }}>
            <form
              onSubmit={(e) => void handleStartCredentials(e)}
              style={{ display: 'flex', flexDirection: 'column', gap: 12 }}
            >
              <div className="cat-field">
                <label className="cat-label">Steam 登录用户名</label>
                <input
                  className="cat-input cat-mono"
                  value={accountName}
                  onChange={(e) => setAccountName(e.target.value)}
                  placeholder="Steam 账号名 (非昵称)"
                  required
                />
              </div>
              <div className="cat-field">
                <label className="cat-label">Steam 密码</label>
                <input
                  type="password"
                  className="cat-input"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  placeholder="仅用于向 Steam CM 换取 Refresh Token，落盘不保存明文密码"
                  required
                />
              </div>
              <button
                type="submit"
                className="cat-btn cat-btn-primary"
                style={{ alignSelf: 'flex-start' }}
                disabled={busy || !accountName.trim() || !password}
              >
                <ShieldCheck size={15} />
                <span>发起账号认证</span>
              </button>
            </form>

            {flow?.kind === 'credentials' && flow.state === 'authenticating' && (
              <div className="cat-banner">正在验证 Steam 登录。如手机 Steam App 收到确认请求，请在手机上批准。</div>
            )}
            {flow?.kind === 'credentials' && flow.state === 'waiting_for_code' && (
                <form
                  onSubmit={(e) => void handleSubmitGuard(e)}
                  className="cat-banner"
                  style={{ flexDirection: 'column', alignItems: 'stretch' }}
                >
                  <div>
                    <strong>等待 Steam Guard 验证：</strong>{' '}
                    {flow.codePrompt ?? '请在手机 Steam App 点击确认，或在下方输入邮箱/令牌 5 位验证码'}
                  </div>
                  <div className="cat-row">
                    <input
                      className="cat-input cat-mono"
                      placeholder="输入 5 位 Steam Guard 验证码"
                      value={guardCode}
                      onChange={(e) => setGuardCode(e.target.value)}
                    />
                    <button
                      type="submit"
                      className="cat-btn cat-btn-primary"
                      disabled={busy || !guardCode.trim()}
                    >
                      提交验证码
                    </button>
                  </div>
                </form>
              )}
          </div>
        )}

        {tab === 'token' && (
          <form
            onSubmit={(e) => void handleSaveToken(e)}
            style={{ display: 'flex', flexDirection: 'column', gap: 12, maxWidth: 520 }}
          >
            <div className="cat-field">
              <label className="cat-label">Steam 登录账号名</label>
              <input
                className="cat-input cat-mono"
                value={tokenAccount}
                onChange={(e) => setTokenAccount(e.target.value)}
                placeholder="例如 my_steam_account"
                required
              />
            </div>
            <div className="cat-field">
              <label className="cat-label">Steam CM Refresh Token (JWT)</label>
              <textarea
                className="cat-textarea cat-mono"
                value={refreshToken}
                onChange={(e) => setRefreshToken(e.target.value)}
                placeholder="eyAidHlwIjogIkpXVCIsICJhbGciOiAiRWREU0EiIH0..."
                required
              />
            </div>
            <button
              type="submit"
              className="cat-btn cat-btn-primary"
              style={{ alignSelf: 'flex-start' }}
              disabled={busy || !tokenAccount.trim() || !refreshToken.trim()}
            >
              <KeyRound size={15} />
              <span>验证并保存 Token</span>
            </button>
          </form>
        )}
      </section>
    </>
  );
}
