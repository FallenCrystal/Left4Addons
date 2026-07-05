import React, { useState, useEffect } from 'react';

interface EditGroupModalProps {
  open: boolean;
  groupId: string;
  currentName: string;
  onCancel: () => void;
  onConfirm: (groupId: string, newName: string) => void;
}

export const EditGroupModal: React.FC<EditGroupModalProps> = ({
  open,
  groupId,
  currentName,
  onCancel,
  onConfirm,
}) => {
  const [name, setName] = useState('');

  useEffect(() => {
    if (open) {
      setName(currentName);
    }
  }, [open, currentName]);

  if (!open) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;
    onConfirm(groupId, name.trim());
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <form className="modal-content" onSubmit={handleSubmit} onClick={(e) => e.stopPropagation()}>
        <h2 className="modal-title">重命名分组</h2>
        
        <div className="form-group">
          <label className="form-label">分组新名称:</label>
          <input 
            type="text" 
            className="form-input" 
            value={name}
            onChange={(e) => setName(e.target.value)}
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
          <button type="submit" className="btn btn-primary" disabled={!name.trim()}>
            保存
          </button>
        </div>
      </form>
    </div>
  );
};
