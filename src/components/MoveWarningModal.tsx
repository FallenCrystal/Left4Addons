import React from 'react';
import { AlertTriangle, ExternalLink } from 'lucide-react';

interface MoveWarningModalProps {
  open: boolean;
  vpkName: string;
  currentDirType: string;
  workshopId: string;
  onCancel: () => void;
  onConfirm: (vpkName: string, currentDirType: string, unsubscribe: boolean) => void;
}

export const MoveWarningModal: React.FC<MoveWarningModalProps> = ({
  open,
  vpkName,
  currentDirType,
  workshopId,
  onCancel,
  onConfirm,
}) => {
  if (!open) return null;

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-content" style={{ width: '500px' }} onClick={(e) => e.stopPropagation()}>
        <h2 className="modal-title" style={{ display: 'flex', alignItems: 'center', gap: '8px', color: '#ffb300' }}>
          <AlertTriangle size={24} />
          <span>移动创意工坊附件提示</span>
        </h2>
        
        <div className="warning-box" style={{ marginTop: '16px', marginBottom: '20px' }}>
          <div>
            <div className="warning-title">⚠️ 非常重要：</div>
            <div style={{ color: '#e2e2e9' }}>
              您正在将组件从<strong> 创意工坊目录 </strong>移动到<strong> 手动安装目录 (Addons) </strong>。
              <br /><br />
              移动后，<strong>请务必在 Steam 客户端或网页中“取消订阅”该组件！</strong>
              <br /><br />
              如果不取消订阅，每次您启动游戏时，Steam 客户端都可能会<strong>重新下载</strong>该组件，导致加载目录和创意工坊目录下同时存在两个相同的文件，引发资源冲突或重复加载。
            </div>
          </div>
        </div>

        <div className="modal-actions">
          <button 
            type="button" 
            className="btn btn-secondary" 
            onClick={onCancel}
          >
            取消
          </button>
          
          {workshopId && (
            <button
              type="button"
              className="btn btn-tertiary"
              onClick={() => onConfirm(vpkName, currentDirType, true)}
              style={{ display: 'inline-flex', alignItems: 'center', gap: '4px' }}
            >
              <ExternalLink size={14} />
              <span>取消订阅并移动</span>
            </button>
          )}

          <button 
            type="button" 
            className="btn btn-primary"
            onClick={() => onConfirm(vpkName, currentDirType, false)}
          >
            直接移动
          </button>
        </div>
      </div>
    </div>
  );
};
