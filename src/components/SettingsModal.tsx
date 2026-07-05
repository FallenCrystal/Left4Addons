import React, { useState, useEffect } from 'react';

interface SettingsModalProps {
  open: boolean;
  initialLoadingDir: string;
  onCancel: () => void;
  onConfirm: (loadingDir: string) => void;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({
  open,
  initialLoadingDir,
  onCancel,
  onConfirm,
}) => {
  const [loadingDir, setLoadingDir] = useState('');

  useEffect(() => {
    if (open) {
      setLoadingDir(initialLoadingDir);
    }
  }, [open, initialLoadingDir]);

  if (!open) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!loadingDir.trim()) return;
    onConfirm(loadingDir.trim());
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <form 
        className="modal-content" 
        onSubmit={handleSubmit} 
        style={{ width: '560px' }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="modal-title">设置附加组件加载路径</h2>
        <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '16px' }}>
          请配置求生之路2的游戏附加组件目录（即 `addons` 文件夹）。程序将自动访问该文件夹及其下的 `workshop` 创意工坊文件夹。
        </p>

        <div className="form-group">
          <label className="form-label">附加组件目录 (Addons 路径):</label>
          <input 
            type="text" 
            className="form-input" 
            value={loadingDir}
            onChange={(e) => setLoadingDir(e.target.value)}
            placeholder="例如: C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2\left4dead2\addons"
            required
          />
          <span style={{ fontSize: '11px', color: 'var(--md-sys-color-outline)' }}>
            请选择游戏目录下的 `left4dead2/addons` 文件夹。
          </span>
        </div>

        <div className="modal-actions">
          <button 
            type="button" 
            className="btn btn-secondary" 
            onClick={onCancel}
          >
            取消
          </button>
          <button type="submit" className="btn btn-primary" disabled={!loadingDir.trim()}>
            保存并重新扫描
          </button>
        </div>
      </form>
    </div>
  );
};
