import { useState, type FormEvent } from 'react';
import { Lock, Moon, Server, Sun } from 'lucide-react';
import { loginAdmin, type AdminLoginResponse } from '../api';

export interface LoginScreenProps {
  theme: 'dark' | 'light';
  onToggleTheme: () => void;
  onLoggedIn: (session: AdminLoginResponse) => void;
}

export function LoginScreen({ theme, onToggleTheme, onLoggedIn }: LoginScreenProps) {
  const [username, setUsername] = useState('admin');
  const [password, setPassword] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!username.trim() || !password) return;
    setSubmitting(true);
    setError(null);
    try {
      const res = await loginAdmin(username.trim(), password);
      onLoggedIn(res);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="cat-login-shell">
      <form className="cat-login-card" onSubmit={(e) => void handleSubmit(e)}>
        <div className="cat-row" style={{ justifyContent: 'space-between' }}>
          <div className="cat-brand">
            <div className="cat-brand-mark">
              <Server size={18} />
            </div>
            <div>
              <h1 className="cat-brand-title">Left4Addons Catalog</h1>
              <p className="cat-brand-sub">SteamKit CM 缓存控制台</p>
            </div>
          </div>
          <button
            type="button"
            className="cat-btn cat-btn-sm"
            onClick={onToggleTheme}
            title="切换深浅色主题"
          >
            {theme === 'dark' ? <Sun size={14} /> : <Moon size={14} />}
          </button>
        </div>

        {error && (
          <div className="cat-banner" data-tone="error" role="alert">
            <span>{error}</span>
          </div>
        )}

        <div className="cat-field">
          <label className="cat-label" htmlFor="cat-login-user">
            管理员账号
          </label>
          <input
            id="cat-login-user"
            className="cat-input"
            autoComplete="username"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            required
          />
        </div>

        <div className="cat-field">
          <label className="cat-label" htmlFor="cat-login-pass">
            管理员密码
          </label>
          <input
            id="cat-login-pass"
            type="password"
            className="cat-input"
            autoComplete="current-password"
            placeholder="请查看 catalog.toml 中的 [admin].password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            required
          />
        </div>

        <button
          type="submit"
          className="cat-btn cat-btn-primary"
          disabled={submitting || !username.trim() || !password}
        >
          <Lock size={15} />
          <span>{submitting ? '正在验证会话…' : '登录控制台'}</span>
        </button>

        <p style={{ margin: 0, fontSize: 12, color: 'var(--cat-text-dim)' }}>
          首次启动会自动生成 <code className="cat-mono">catalog.toml</code> 配置文件并写入随机管理员密码，登录后可在系统设置中随时修改。
        </p>
      </form>
    </div>
  );
}
