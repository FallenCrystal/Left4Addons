import React from 'react';
import { ExternalLink } from 'lucide-react';

interface LinkConfirmModalProps {
  open: boolean;
  url: string;
  onCancel: () => void;
  onConfirm: (url: string) => void;
}

export const LinkConfirmModal: React.FC<LinkConfirmModalProps> = ({
  open,
  url,
  onCancel,
  onConfirm,
}) => {
  if (!open) return null;

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-content" style={{ width: '450px' }} onClick={(e) => e.stopPropagation()}>
        <h2 className="modal-title" style={{ display: 'flex', alignItems: 'center', gap: '8px', color: 'var(--md-sys-color-primary)' }}>
          <ExternalLink size={24} />
          <span>即将访问外部网站</span>
        </h2>
        
        <div style={{ marginTop: '16px', marginBottom: '20px', color: '#e2e2e9', fontSize: '14px', lineHeight: '1.6' }}>
          您点击的链接指向第三方外部网页：
          <div style={{ 
            background: 'var(--md-sys-surface-container-low)', 
            border: '1px solid var(--md-sys-color-outline-variant)', 
            borderRadius: '8px', 
            padding: '12px', 
            marginTop: '8px', 
            wordBreak: 'break-all', 
            color: 'var(--md-sys-color-primary)',
            fontFamily: 'Consolas, Monaco, "Lucida Console", Courier, monospace',
            fontSize: '14px',
            fontWeight: '600',
            lineHeight: '1.4'
          }}>
            {url}
          </div>
          <br />
          请确保您信任该网站，以防止钓鱼和恶意软件威胁。是否继续前往？
        </div>

        <div className="modal-actions">
          <button 
            type="button" 
            className="btn btn-secondary" 
            onClick={onCancel}
          >
            取消
          </button>
          
          <button 
            type="button" 
            className="btn btn-primary"
            onClick={() => onConfirm(url)}
          >
            继续访问
          </button>
        </div>
      </div>
    </div>
  );
};
