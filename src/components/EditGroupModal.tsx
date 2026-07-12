import React, { useState, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { Addon } from '../types/addon';
import { getAddonCategories } from '../utils/addonHelpers';
import { Sparkles, AlertTriangle } from 'lucide-react';

interface EditGroupModalProps {
  open: boolean;
  groupId: string;
  currentName: string;
  currentTags?: string[];
  currentCollectionId?: string;
  addonsInGroup: Addon[];
  onCancel: () => void;
  onConfirm: (groupId: string, newName: string, tags: string[], collectionId?: string) => void;
  isSubmitting?: boolean;
}

export const EditGroupModal: React.FC<EditGroupModalProps> = ({
  open,
  groupId,
  currentName,
  currentTags = [],
  currentCollectionId = '',
  addonsInGroup,
  onCancel,
  onConfirm,
  isSubmitting = false,
}) => {
  const { t } = useTranslation();
  const [name, setName] = useState('');
  const [tagsInput, setTagsInput] = useState('');
  const [collectionId, setCollectionId] = useState('');
  const [showConfirmSave, setShowConfirmSave] = useState(false);

  useEffect(() => {
    if (open) {
      setName(currentName);
      setTagsInput(currentTags.join(', '));
      setCollectionId(currentCollectionId);
      setShowConfirmSave(false);
    }
  }, [open, groupId, currentName, currentTags, currentCollectionId]);

  if (!open) return null;

  const handleAutoAllocateTags = () => {
    const categories = new Set<string>();
    addonsInGroup.forEach(addon => {
      getAddonCategories(addon).forEach(cat => categories.add(cat));
    });
    setTagsInput(Array.from(categories).join(', '));
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim() || isSubmitting) return;

    const targetCollectionId = collectionId.trim() || undefined;

    // If collection ID was added/changed, we prompt
    if (targetCollectionId !== (currentCollectionId || '') && targetCollectionId) {
      setShowConfirmSave(true);
      return;
    }

    const tags = tagsInput
      .split(',')
      .map(s => s.trim())
      .filter(Boolean);

    onConfirm(groupId, name.trim(), tags, targetCollectionId);
  };

  const isCollectionChanged = collectionId.trim() !== (currentCollectionId || '');

  if (showConfirmSave) {
    return (
      <div className="modal-overlay" onClick={onCancel} style={{ zIndex: 1100 }}>
        <div className="modal-content" style={{ maxWidth: '440px', borderRadius: '28px', padding: '24px' }} onClick={(e) => e.stopPropagation()}>
          <h2 className="modal-title" style={{ display: 'flex', alignItems: 'center', gap: '8px', color: '#ffb300' }}>
            <AlertTriangle size={24} />
            <span>确认修改</span>
          </h2>
          <div style={{ whiteSpace: 'pre-wrap', marginTop: '16px', marginBottom: '24px', color: '#e2e2e9', fontSize: '14px', lineHeight: '1.6' }}>
            设置创意工坊合集后，保存时将自动从 Steam 创意工坊拉取该合集内的所有组件，并丢弃当前分组的所有其它自定义分类组件。确认继续吗？
          </div>
          <div className="modal-actions" style={{ marginTop: 0 }}>
            <button 
              type="button" 
              className="btn btn-secondary" 
              onClick={() => setShowConfirmSave(false)}
              style={{ borderRadius: '12px' }}
            >
              取消
            </button>
            <button 
              type="button" 
              className="btn btn-primary"
              onClick={() => {
                const tags = tagsInput
                  .split(',')
                  .map(s => s.trim())
                  .filter(Boolean);
                const targetCollectionId = collectionId.trim() || undefined;
                onConfirm(groupId, name.trim(), tags, targetCollectionId);
                setShowConfirmSave(false);
              }}
              style={{ borderRadius: '12px' }}
            >
              确认
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <form className="modal-content" onSubmit={handleSubmit} onClick={(e) => e.stopPropagation()} style={{ maxWidth: '480px', borderRadius: '28px', padding: '24px' }}>
        <h2 className="modal-title">编辑分组</h2>
        
        {/* Group Name */}
        <div className="form-group" style={{ marginBottom: '16px' }}>
          <label className="form-label">分组名称</label>
          <input 
            type="text" 
            className="form-input" 
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            disabled={isSubmitting}
            style={{ borderRadius: '12px' }}
          />
        </div>

        {/* Group Tags */}
        <div className="form-group" style={{ marginBottom: '16px' }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '8px' }}>
            <label className="form-label" style={{ margin: 0 }}>标签 (逗号分隔)</label>
            <button
              type="button"
              onClick={handleAutoAllocateTags}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: '4px',
                background: 'none',
                border: 'none',
                color: 'var(--md-sys-color-primary)',
                fontSize: '12px',
                fontWeight: 500,
                cursor: 'pointer'
              }}
              title="根据当前分组内的组件，自动生成分类标签"
            >
              <Sparkles size={12} />
              自动分配标签
            </button>
          </div>
          <input 
            type="text" 
            className="form-input" 
            value={tagsInput}
            onChange={(e) => setTagsInput(e.target.value)}
            placeholder="例如: 战役, 武器模型, 皮肤"
            disabled={isSubmitting}
            style={{ borderRadius: '12px' }}
          />
        </div>

        {/* Steam Workshop Collection ID */}
        <div className="form-group" style={{ marginBottom: '16px' }}>
          <label className="form-label">Steam 创意工坊合集 ID (可选)</label>
          <input 
            type="text" 
            className="form-input" 
            value={collectionId}
            onChange={(e) => setCollectionId(e.target.value)}
            placeholder="例如: 3560901999"
            disabled={isSubmitting}
            style={{ borderRadius: '12px' }}
          />
        </div>

        {/* Warning Alert */}
        {isCollectionChanged && collectionId.trim() && (
          <div style={{
            display: 'flex',
            gap: '12px',
            backgroundColor: 'var(--md-sys-color-error-container)',
            color: 'var(--md-sys-color-on-error-container)',
            padding: '12px 16px',
            borderRadius: '16px',
            fontSize: '12px',
            lineHeight: '18px',
            marginBottom: '20px'
          }}>
            <AlertTriangle size={18} style={{ flexShrink: 0, marginTop: '2px', color: 'var(--md-sys-color-error)' }} />
            <div>
              <strong>安全警告：</strong>设置合集 ID 后，保存将自动覆盖该分组的文件列表，<strong>所有不属于该合集的自定义归类组件将被移出当前分组</strong>。
            </div>
          </div>
        )}

        <div className="modal-actions" style={{ marginTop: '24px' }}>
          <button 
            type="button" 
            className="btn btn-secondary" 
            onClick={onCancel}
            disabled={isSubmitting}
            style={{ borderRadius: '100px' }}
          >
            {t('common.cancel')}
          </button>
          <button type="submit" className="btn btn-primary" disabled={isSubmitting || !name.trim()} style={{ borderRadius: '100px' }}>
            {isSubmitting ? t('common.saving') : t('common.save')}
          </button>
        </div>
      </form>
    </div>
  );
};
