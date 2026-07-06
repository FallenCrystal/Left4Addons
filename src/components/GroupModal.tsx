import React, { useState, useEffect } from 'react';
import { Addon } from '../types/addon';

interface GroupModalProps {
  open: boolean;
  addons: Record<string, Addon>;
  onCancel: () => void;
  onConfirm: (name: string, selectedAddons: string[]) => void;
  isSubmitting?: boolean;
}

export const GroupModal: React.FC<GroupModalProps> = ({
  open,
  addons,
  onCancel,
  onConfirm,
  isSubmitting = false,
}) => {
  const [name, setName] = useState('');
  const [selectedAddons, setSelectedAddons] = useState<string[]>([]);

  useEffect(() => {
    if (open) {
      setName('');
      setSelectedAddons([]);
    }
  }, [open]);

  if (!open) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!name || selectedAddons.length === 0 || isSubmitting) return;
    onConfirm(name, selectedAddons);
  };

  const handleCheckboxChange = (vpkName: string, checked: boolean) => {
    if (checked) {
      setSelectedAddons(prev => [...prev, vpkName]);
    } else {
      setSelectedAddons(prev => prev.filter(item => item !== vpkName));
    }
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <form 
        className="modal-content" 
        onSubmit={handleSubmit} 
        style={{ width: '560px' }} 
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="modal-title">创建新附件分组</h2>
        <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '16px' }}>
          将多个 VPK 文件打包到同一个群组（例如一张地图的 Part 1、2、3），从而一键批量启用、禁用或移动。
        </p>

        <div className="form-group">
          <label className="form-label">分组名称:</label>
          <input 
            type="text" 
            className="form-input" 
            placeholder="例如：Early Days 战役地图包"
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            disabled={isSubmitting}
          />
        </div>

        <div className="form-group">
          <label className="form-label">选择要加入群组的组件 (可多选):</label>
          <div style={{ 
            maxHeight: '200px', 
            overflowY: 'auto', 
            backgroundColor: 'var(--md-sys-surface-container)',
            border: '1px solid var(--md-sys-color-outline-variant)',
            borderRadius: '12px',
            padding: '8px'
          }}>
            {Object.values(addons).map(addon => {
              const title = addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName;
              const isChecked = selectedAddons.includes(addon.vpkName);
              return (
                <label 
                  key={addon.vpkName} 
                  style={{ 
                    display: 'flex', 
                    alignItems: 'center', 
                    gap: '8px', 
                    padding: '8px',
                    cursor: isSubmitting ? 'not-allowed' : 'pointer',
                    borderRadius: '8px',
                    backgroundColor: isChecked ? 'rgba(179, 197, 255, 0.08)' : 'transparent'
                  }}
                >
                  <input 
                    type="checkbox" 
                    checked={isChecked}
                    onChange={(e) => handleCheckboxChange(addon.vpkName, e.target.checked)}
                    disabled={isSubmitting}
                  />
                  <span style={{ fontSize: '13px', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                    {title}
                  </span>
                </label>
              );
            })}
          </div>
        </div>

        <div className="modal-actions">
          <button 
            type="button" 
            className="btn btn-secondary" 
            onClick={onCancel}
            disabled={isSubmitting}
          >
            取消
          </button>
          <button type="submit" className="btn btn-primary" disabled={isSubmitting || !name || selectedAddons.length === 0}>
            {isSubmitting ? '正在创建...' : '创建分组'}
          </button>
        </div>
      </form>
    </div>
  );
};
