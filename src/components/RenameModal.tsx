import React, { useState, useEffect } from 'react';
import { Addon, Group } from '../types/addon';
import { getSuggestedVpkName } from '../utils/addonHelpers';

interface RenameModalProps {
  open: boolean;
  currentName: string;
  title: string;
  suggestedName: string;
  addon: Addon | undefined;
  itemGroup: Group | undefined;
  addons: Record<string, Addon>;
  onCancel: () => void;
  onConfirm: (currentName: string, newVpkName: string) => void;
}

export const RenameModal: React.FC<RenameModalProps> = ({
  open,
  currentName,
  title,
  suggestedName,
  addon,
  itemGroup,
  addons,
  onCancel,
  onConfirm,
}) => {
  const [vpkName, setVpkName] = useState('');

  useEffect(() => {
    if (open) {
      setVpkName(suggestedName);
    }
  }, [open, suggestedName]);

  if (!open) return null;

  const handleApplyTitle = () => {
    if (addon) {
      const name = getSuggestedVpkName(addon, itemGroup?.name, addons);
      setVpkName(name);
    }
  };


  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    onConfirm(currentName, vpkName);
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <form className="modal-content" onSubmit={handleSubmit} onClick={(e) => e.stopPropagation()}>
        <h2 className="modal-title">重命名附加组件文件</h2>
        <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '16px' }}>
          修改底层的 VPK 文件名称。建议使用有意义的标题，防止由于一堆数字无法辨认。
        </p>
        
        {title && (
          <div style={{ 
            backgroundColor: 'var(--md-sys-surface-container)',
            border: '1px solid var(--md-sys-color-outline-variant)',
            padding: '12px 16px',
            borderRadius: '12px',
            marginBottom: '16px',
            fontSize: '12px'
          }}>
            <div style={{ fontWeight: '700', color: 'var(--md-sys-color-primary)' }}>创意工坊标题:</div>
            <div style={{ color: '#fff', marginTop: '4px' }}>{title}</div>
            <button 
              type="button" 
              className="btn btn-text" 
              style={{ padding: '4px 0', fontSize: '11px', marginTop: '4px' }}
              onClick={handleApplyTitle}
            >
              应用创意工坊标题作为文件名
            </button>
          </div>
        )}

        <div className="form-group">
          <label className="form-label">当前文件名:</label>
          <div style={{ fontStyle: 'italic', fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
            {currentName}
          </div>
        </div>

        <div className="form-group">
          <label className="form-label">新文件名 (.vpk):</label>
          <input 
            type="text" 
            className="form-input" 
            value={vpkName}
            onChange={(e) => setVpkName(e.target.value)}
            required
          />
        </div>

        <div className="modal-actions">
          <button 
            type="button" 
            className="btn btn-secondary" 
            onClick={onCancel}
          >
            取消
          </button>
          <button type="submit" className="btn btn-primary">
            重命名
          </button>
        </div>
      </form>
    </div>
  );
};
