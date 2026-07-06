import React, { useState, useEffect } from 'react';
import { FolderOpen, Info, RefreshCw } from 'lucide-react';
import { Settings } from '../types/addon';

interface SettingsViewProps {
  settings: Settings;
  isSubmitting: boolean;
  onConfirm: (loadingDir: string) => Promise<void>;
}

export const SettingsView: React.FC<SettingsViewProps> = ({
  settings,
  isSubmitting,
  onConfirm,
}) => {
  const [activeTab, setActiveTab] = useState<'path' | 'about'>('path');
  const [loadingDir, setLoadingDir] = useState('');

  useEffect(() => {
    setLoadingDir(settings.loadingDir || '');
  }, [settings.loadingDir]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!loadingDir.trim() || isSubmitting) return;
    await onConfirm(loadingDir.trim());
  };

  return (
    <div className="settings-view">
      {/* Settings Navigation Sidebar */}
      <div className="settings-nav">
        <button
          className={`settings-nav-item ${activeTab === 'path' ? 'active' : ''}`}
          onClick={() => setActiveTab('path')}
          type="button"
        >
          <FolderOpen size={18} />
          <span>路径设置</span>
        </button>
        <button
          className={`settings-nav-item ${activeTab === 'about' ? 'active' : ''}`}
          onClick={() => setActiveTab('about')}
          type="button"
        >
          <Info size={18} />
          <span>关于软件</span>
        </button>
      </div>

      {/* Settings Content Area */}
      <div className="settings-content">
        {activeTab === 'path' && (
          <div>
            <h2 className="settings-title">游戏与目录配置</h2>
            <form onSubmit={handleSubmit} className="settings-section">
              <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '20px', lineHeight: '1.6' }}>
                请配置求生之路2的游戏附加组件目录（即 `addons` 文件夹）。程序将自动访问该文件夹及其下的 `workshop` 创意工坊文件夹，并解析和管理所有的 VPK 附件文件。
              </p>

              <div className="form-group" style={{ marginBottom: '24px' }}>
                <label className="form-label" style={{ fontWeight: '600', marginBottom: '8px', display: 'block' }}>
                  附加组件目录 (Addons 路径):
                </label>
                <input
                  type="text"
                  className="form-input"
                  value={loadingDir}
                  onChange={(e) => setLoadingDir(e.target.value)}
                  placeholder="例如: C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2\left4dead2\addons"
                  style={{ width: '100%' }}
                  required
                  disabled={isSubmitting}
                />
                <span style={{ fontSize: '11px', color: 'var(--md-sys-color-outline)', display: 'block', marginTop: '6px' }}>
                  请选择游戏目录下的 `left4dead2/addons` 文件夹。保存后程序将自动开始扫描该目录。
                </span>
              </div>

              <div style={{ display: 'flex', justifyContent: 'flex-start', marginTop: '32px' }}>
                <button
                  type="submit"
                  className="btn btn-primary"
                  disabled={isSubmitting || !loadingDir.trim()}
                  style={{ minWidth: '160px', height: '42px' }}
                >
                  {isSubmitting ? (
                    <>
                      <RefreshCw className="animate-spin" size={16} />
                      <span>正在保存并扫描...</span>
                    </>
                  ) : (
                    <span>保存并重新扫描</span>
                  )}
                </button>
              </div>
            </form>
          </div>
        )}

        {activeTab === 'about' && (
          <div>
            <h2 className="settings-title">关于 Left 4 Addons</h2>
            <div className="settings-section" style={{ lineHeight: '1.8' }}>
              <h3 style={{ margin: '0 0 12px 0', fontSize: '18px', color: 'var(--md-sys-color-primary)' }}>
                Left 4 Addons v1.0.0
              </h3>
              <p style={{ fontSize: '13px', color: 'var(--md-sys-color-on-surface)', marginBottom: '16px' }}>
                一个专为《求生之路2》（Left 4 Dead 2）设计的附加组件（Addons）管理器。采用 Tauri + React + Rust 驱动，旨在为玩家提供极速、优雅的 VPK 文件管理体验。
              </p>
              
              <h4 style={{ margin: '20px 0 8px 0', fontSize: '14px', fontWeight: '600' }}>主要功能</h4>
              <ul style={{ paddingLeft: '20px', margin: '0', fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
                <li><b>一键启用/禁用</b>：快速重命名 `.vpk` 文件以在游戏中生效或失效。</li>
                <li><b>创意工坊同步</b>：自动拉取并缓存创意工坊组件的封面图、标题和作者详情。</li>
                <li><b>分组管理</b>：将多 Part 地图或关联组件组合，实现一键批量操作。</li>
                <li><b>自动识别</b>：内置战役/地图包识别算法，自动检测并对关联附件进行重组。</li>
                <li><b>物理隔离</b>：一键将工坊文件转移到本地加载文件夹，防止游戏联机订阅冲突。</li>
              </ul>

              <h4 style={{ margin: '20px 0 8px 0', fontSize: '14px', fontWeight: '600' }}>开源许可</h4>
              <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', margin: '0' }}>
                本项目遵循 MIT 协议开源。
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
