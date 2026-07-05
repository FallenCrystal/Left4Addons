import React, { useState } from 'react';
import { AlertTriangle, ExternalLink, CheckCircle2, AlertCircle } from 'lucide-react';
import { Addon } from '../types/addon';
import { invoke } from '@tauri-apps/api/core';

interface WorkshopActionWarningModalProps {
  open: boolean;
  actionName: string;
  addons: Addon[];
  onCancel: () => void;
  onConfirm: (skipWarning: boolean) => void;
}

export const WorkshopActionWarningModal: React.FC<WorkshopActionWarningModalProps> = ({
  open,
  actionName,
  addons,
  onCancel,
  onConfirm,
}) => {
  const [skipWarning, setSkipWarning] = useState(false);

  if (!open) return null;

  const handleOpenWorkshop = async () => {
    const workshopId = addons.find(ad => ad.workshopId)?.workshopId;
    if (workshopId) {
      await invoke('open_url', { url: `steam://url/CommunityFilePage/${workshopId}` });
    }
    onConfirm(skipWarning);
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-content" style={{ width: '500px' }} onClick={(e) => e.stopPropagation()}>
        <h2 className="modal-title" style={{ display: 'flex', alignItems: 'center', gap: '8px', color: '#8be19a' }}>
          <AlertTriangle size={24} />
          <span>创意工坊附件已移动</span>
        </h2>
        
        <div className="warning-box" style={{ marginTop: '16px', marginBottom: '20px', backgroundColor: 'rgba(139, 225, 154, 0.1)', borderColor: 'rgba(139, 225, 154, 0.3)', color: '#8be19a' }}>
          <div>
            <div className="warning-title" style={{ color: '#8be19a', display: 'flex', alignItems: 'center', gap: '6px' }}>
              <CheckCircle2 size={18} />
              <span>自动转移成功</span>
            </div>
            <div style={{ color: '#e2e2e9', marginTop: '8px' }}>
              为了防止游戏重新下载覆盖，执行 <strong>{actionName}</strong> 操作前，已自动将相关的创意工坊附件移动至<strong>手动安装目录 (Addons)</strong>。
              <br /><br />
              <strong style={{ color: '#ffb4ab', display: 'inline-flex', alignItems: 'center', gap: '4px' }}>
                <AlertCircle size={16} />
                <span>请注意：</span>
              </strong><br />
              您必须前往 Steam 创意工坊<strong>取消订阅</strong>此组件，否则游戏下次启动时仍会重复下载该附件并导致冲突！
            </div>
          </div>
        </div>

        <div style={{ marginBottom: '20px', display: 'flex', alignItems: 'center', gap: '8px' }}>
          <label className="switch" title="本次运行期间不再提醒" style={{ transform: 'scale(0.8)', transformOrigin: 'left center' }}>
            <input 
              type="checkbox" 
              checked={skipWarning} 
              onChange={(e) => setSkipWarning(e.target.checked)}
            />
            <span className="slider"></span>
          </label>
          <span style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>当前会话不再提醒此警告，并静默执行移动</span>
        </div>

        <div className="modal-actions">
          <button 
            type="button" 
            className="btn btn-secondary" 
            onClick={() => onConfirm(skipWarning)}
          >
            我知道了
          </button>
          
          <button 
            type="button" 
            className="btn btn-primary"
            onClick={handleOpenWorkshop}
            disabled={!addons.some(ad => ad.workshopId)}
          >
            <ExternalLink size={14} />
            <span>前往创意工坊</span>
          </button>
        </div>
      </div>
    </div>
  );
};
