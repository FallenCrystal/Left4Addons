import React from 'react';
import { X, ExternalLink, FileText, Move, FolderPlus } from 'lucide-react';
import { Addon, Group } from '../types/addon';
import { formatBytes, getAddonCategories, getAddonUrl, getAddonAuthor } from '../utils/addonHelpers';
import { CacheImage } from './CacheImage';

interface DetailModalProps {
  open: boolean;
  addon: Addon | null;
  groups: Group[];
  onCancel: () => void;
  onToggle: (vpkName: string, isEnabled: boolean) => void;
  onMove: (addon: Addon) => void;
  onOpenLink: (url: string) => void;
}

export const DetailModal: React.FC<DetailModalProps> = ({
  open,
  addon,
  groups,
  onCancel,
  onToggle,
  onMove,
  onOpenLink,
}) => {
  if (!open || !addon) return null;

  const title = addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName;
  const author = getAddonAuthor(addon);
  const desc = addon.steamDetails?.description || addon.addonInfo?.addondescription || addon.addonInfo?.addontagline || 'No description provided.';
  const categories = getAddonCategories(addon);
  const itemGroup = groups.find(g => g.addons.includes(addon.vpkName));
  const addonUrl = getAddonUrl(addon);

  const handleOpenLinkClick = (e: React.MouseEvent, url: string) => {
    e.preventDefault();
    onOpenLink(url);
  };

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-content modal-content-wide" onClick={(e) => e.stopPropagation()}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '16px' }}>
          <h2 className="modal-title" style={{ margin: 0 }}>附加组件详情</h2>
          <button 
            className="btn btn-secondary btn-icon-only" 
            onClick={onCancel}
          >
            <X size={16} />
          </button>
        </div>

        <div className="detail-modal-layout">
          <div className="detail-left">
            <div className="detail-image-box">
              {addon.imagePath ? (
                <CacheImage 
                  srcPath={addon.imagePath} 
                  alt={title} 
                  className="detail-image" 
                  fallback={
                    <div className="addon-placeholder-icon" style={{ height: '100%', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                      <svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="text-secondary"><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M10 9H8"/><path d="M16 13H8"/><path d="M16 17H8"/></svg>
                    </div>
                  }
                />
              ) : (
                <FileText size={64} className="text-secondary" />
              )}
            </div>

            <div className="detail-meta-list">
              <div className="detail-meta-item">
                <span className="detail-meta-label">文件名称</span>
                <span className="detail-meta-value">{addon.vpkName}</span>
              </div>
              <div className="detail-meta-item">
                <span className="detail-meta-label">组件大小</span>
                <span className="detail-meta-value">{formatBytes(addon.fileSize)}</span>
              </div>
              {addon.filesCount > 0 && (
                <div className="detail-meta-item">
                  <span className="detail-meta-label">包内文件</span>
                  <span className="detail-meta-value">{addon.filesCount} 个文件</span>
                </div>
              )}
              <div className="detail-meta-item">
                <span className="detail-meta-label">所在目录</span>
                <span className="detail-meta-value">
                  {addon.dirType === 'loading' ? '手动安装 (Addons)' : '创意工坊 (Workshop)'}
                </span>
              </div>
              <div className="detail-meta-item">
                <span className="detail-meta-label">当前状态</span>
                <span className="detail-meta-value" style={{ color: addon.isEnabled ? 'var(--md-sys-color-success)' : 'var(--md-sys-color-error)' }}>
                  {addon.isEnabled ? '已启用 (加载中)' : '已禁用'}
                </span>
              </div>
              {addon.workshopId && (
                <div className="detail-meta-item">
                  <span className="detail-meta-label">创意工坊 ID</span>
                  <span className="detail-meta-value">{addon.workshopId}</span>
                </div>
              )}
              {addonUrl && (
                <div className="detail-meta-item">
                  <span className="detail-meta-label">相关链接</span>
                  <span className="detail-meta-value">
                    <a 
                      href="#" 
                      onClick={(e) => handleOpenLinkClick(e, addonUrl)}
                      style={{ color: 'var(--md-sys-color-primary)', textDecoration: 'underline', wordBreak: 'break-all', display: 'inline-flex', alignItems: 'center', gap: '4px' }}
                    >
                      <span>点击访问</span>
                      <ExternalLink size={10} />
                    </a>
                  </span>
                </div>
              )}
            </div>
          </div>

          <div className="detail-right">
            <h3 style={{ margin: 0, fontSize: '20px', color: '#fff' }}>{title}</h3>
            
            <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap', alignItems: 'center' }}>
              <span style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
                作者: <strong style={{ color: '#fff' }}>{author}</strong>
              </span>
              {addon.addonInfo?.addonauthorSteamID && (
                <a 
                  href="#"
                  onClick={(e) => handleOpenLinkClick(e, `https://steamcommunity.com/profiles/${addon.addonInfo?.addonauthorSteamID}`)}
                  className="btn btn-text"
                  style={{ padding: '2px 4px', fontSize: '11px', display: 'inline-flex', alignItems: 'center', gap: '4px' }}
                >
                  <span>查看 Steam 档案</span>
                  <ExternalLink size={10} />
                </a>
              )}
            </div>

            {addon.addonInfo?.addonversion && (
              <div style={{ fontSize: '12px', color: 'var(--md-sys-color-outline)' }}>
                版本: {addon.addonInfo.addonversion}
              </div>
            )}

            {itemGroup && (
              <div className="group-tag" style={{ fontSize: '13px' }}>
                <FolderPlus size={14} />
                <span>所属分组: <strong>{itemGroup.name}</strong></span>
              </div>
            )}

            <div style={{ display: 'flex', gap: '6px', flexWrap: 'wrap' }}>
              {categories.map(c => (
                <span key={c} className="tag-chip" style={{ fontSize: '11px', padding: '4px 10px' }}>{c}</span>
              ))}
            </div>

            <div style={{ fontWeight: '600', fontSize: '14px', color: '#fff', marginTop: '8px' }}>组件描述:</div>
            <div className="description-block">{desc}</div>
          </div>
        </div>

        <div className="modal-actions" style={{ 
          display: 'flex', 
          justifyContent: 'space-between', 
          alignItems: 'center', 
          padding: '16px 0 0 0', 
          borderTop: '1px solid var(--md-sys-color-outline-variant)',
          marginTop: '24px'
        }}>
          <div style={{ display: 'flex', gap: '12px' }}>
            {addon.workshopId && (
              <>
                <a 
                  href="#"
                  onClick={(e) => handleOpenLinkClick(e, `steam://url/CommunityFilePage/${addon.workshopId}`)}
                  className="btn btn-secondary"
                  style={{ display: 'inline-flex', gap: '6px' }}
                >
                  <ExternalLink size={14} />
                  <span>在 Steam 打开</span>
                </a>
                <a 
                  href="#"
                  onClick={(e) => handleOpenLinkClick(e, `https://steamcommunity.com/sharedfiles/filedetails/?id=${addon.workshopId}`)}
                  className="btn btn-text"
                  style={{ display: 'inline-flex', gap: '6px' }}
                >
                  <span>网页查看 (官方)</span>
                </a>
                <a 
                  href="#"
                  onClick={(e) => handleOpenLinkClick(e, `https://steamcommunity.net/sharedfiles/filedetails/?id=${addon.workshopId}`)}
                  className="btn btn-text"
                  style={{ display: 'inline-flex', gap: '6px' }}
                >
                  <span>国内网页镜像</span>
                </a>
              </>
            )}
            {!addon.workshopId && addonUrl && (
              <a 
                href="#"
                onClick={(e) => handleOpenLinkClick(e, addonUrl)}
                className="btn btn-secondary"
                style={{ display: 'inline-flex', gap: '6px' }}
              >
                <ExternalLink size={14} />
                <span>访问来源网页</span>
              </a>
            )}
          </div>
          
          <div style={{ display: 'flex', gap: '12px' }}>
            <button 
              className={`btn ${addon.isEnabled ? 'btn-secondary' : 'btn-primary'}`}
              onClick={() => onToggle(addon.vpkName, addon.isEnabled)}
            >
              {addon.isEnabled ? '禁用组件' : '启用组件'}
            </button>

            {addon.dirType === 'workshop' && (
              <button 
                className="btn btn-primary"
                onClick={() => onMove(addon)}
              >
                <Move size={14} />
                <span>移动到手动安装</span>
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
