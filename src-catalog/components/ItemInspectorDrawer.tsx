import { useEffect, useState, type FormEvent } from 'react';
import { Copy, ExternalLink, Lock, RefreshCw, Unlock, X } from 'lucide-react';
import {
  clearItemOverride,
  formatBytes,
  formatIsoTime,
  formatUnixSeconds,
  getAdminItemDetail,
  overrideCatalogItem,
  parseWorkshopIdsInput,
  refreshAdminItem,
  type AdminItemDetailResponse,
  type WorkshopItemStatus,
} from '../api';

export interface ItemInspectorDrawerProps {
  workshopId: string | null;
  onClose: () => void;
  onSelectWorkshopId: (nextId: string) => void;
  onUpdated: () => void;
  onNotify: (msg: string, tone?: 'default' | 'error') => void;
}

export function ItemInspectorDrawer({
  workshopId,
  onClose,
  onSelectWorkshopId,
  onUpdated,
  onNotify,
}: ItemInspectorDrawerProps) {
  const [detail, setDetail] = useState<AdminItemDetailResponse | null>(null);
  const [busy, setBusy] = useState(false);

  const [ovStatus, setOvStatus] = useState<WorkshopItemStatus>('active');
  const [ovTitle, setOvTitle] = useState('');
  const [ovFileUrl, setOvFileUrl] = useState('');
  const [ovPreviewUrl, setOvPreviewUrl] = useState('');
  const [ovDepsText, setOvDepsText] = useState('');
  const [ovReason, setOvReason] = useState('');

  useEffect(() => {
    if (!workshopId) return;
    let cancelled = false;
    void getAdminItemDetail(workshopId)
      .then((res) => {
        if (cancelled) return;
        setDetail(res);
        setOvStatus(res.item.status);
        setOvTitle(res.item.title);
        setOvFileUrl(res.item.fileUrl ?? '');
        setOvPreviewUrl(res.item.previewUrl ?? '');
        setOvDepsText(res.item.dependencies.map((d) => d.workshopId).join(', '));
        setOvReason('');
      })
      .catch((err) => {
        if (!cancelled) {
          onNotify(err instanceof Error ? err.message : String(err), 'error');
          onClose();
        }
      });

    return () => {
      cancelled = true;
    };
  }, [onClose, onNotify, workshopId]);

  if (!workshopId) return null;
  const activeDetail = detail?.item.workshopId === workshopId ? detail : null;
  const loading = activeDetail === null;

  const handleForceRefresh = async () => {
    setBusy(true);
    try {
      const next = await refreshAdminItem(workshopId);
      setDetail(next);
      onUpdated();
      onNotify(`已从 Steam CM 刷新 #${workshopId}`);
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setBusy(false);
    }
  };

  const handleApplyOverride = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    try {
      const depIds = parseWorkshopIdsInput(ovDepsText);
      await overrideCatalogItem(workshopId, {
        status: ovStatus,
        title: ovTitle.trim() || undefined,
        fileUrl: ovFileUrl.trim() || undefined,
        previewUrl: ovPreviewUrl.trim() || undefined,
        dependencies: depIds.map((id) => ({ workshopId: id, required: true })),
        reason: ovReason.trim() || '控制台管理员手动覆写',
      });
      const refreshed = await getAdminItemDetail(workshopId);
      setDetail(refreshed);
      onUpdated();
      onNotify(`已应用并锁定 #${workshopId} 的管理员覆写`);
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setBusy(false);
    }
  };

  const handleClearOverride = async () => {
    setBusy(true);
    try {
      const next = await clearItemOverride(workshopId);
      setDetail(next);
      onUpdated();
      onNotify(`已解除 #${workshopId} 的覆写锁定并恢复自动同步`);
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setBusy(false);
    }
  };

  const copyText = async (text: string, label: string) => {
    try {
      await navigator.clipboard.writeText(text);
      onNotify(`${label}已复制`);
    } catch {
      onNotify('复制失败', 'error');
    }
  };

  const item = activeDetail?.item;
  const galleryUrls = [
    ...(item?.previewUrl ? [item.previewUrl] : []),
    ...((item?.previews ?? []).map((p) => p.url).filter((u): u is string => Boolean(u))),
  ];
  const uniqueGallery = Array.from(new Set(galleryUrls));

  return (
    <div className="cat-drawer-backdrop" onClick={onClose}>
      <aside
        className="cat-drawer"
        aria-label="工坊条目详情检视器"
        onClick={(e) => e.stopPropagation()}
      >
        <header className="cat-drawer-header">
          <div>
            <div className="cat-row" style={{ gap: 6, marginBottom: 4 }}>
              <span className="cat-mono" style={{ color: 'var(--cat-text-muted)' }}>
                #{workshopId}
              </span>
              {item && (
                <span
                  className="cat-badge"
                  data-tone={
                    item.status === 'active'
                      ? 'success'
                      : item.status === 'removed'
                        ? 'danger'
                        : 'warning'
                  }
                >
                  {item.status}
                </span>
              )}
              {activeDetail?.isAdminOverride && (
                <span className="cat-badge" data-tone="warning">
                  管理员覆写锁定
                </span>
              )}
            </div>
            <h3 style={{ margin: 0, fontSize: 17 }}>
              {loading ? '正在读取详情…' : (item?.title ?? `Workshop #${workshopId}`)}
            </h3>
          </div>
          <div className="cat-row">
            <a
              className="cat-btn cat-btn-sm"
              href={`https://steamcommunity.com/sharedfiles/filedetails/?id=${encodeURIComponent(workshopId)}`}
              target="_blank"
              rel="noreferrer"
            >
              <ExternalLink size={13} />
              <span>Steam 页面</span>
            </a>
            <button type="button" className="cat-btn cat-btn-sm" onClick={onClose}>
              <X size={15} />
            </button>
          </div>
        </header>

        <div className="cat-drawer-body">
          {item && activeDetail && (
            <>
              <div className="cat-row">
                <button
                  type="button"
                  className="cat-btn cat-btn-sm"
                  disabled={busy || activeDetail.isAdminOverride}
                  title={
                    activeDetail.isAdminOverride
                      ? '请先解除管理员覆写锁定后再强制重刷'
                      : '立即向 Steam CM 请求最新元数据与直链'
                  }
                  onClick={() => void handleForceRefresh()}
                >
                  <RefreshCw size={13} className={busy ? 'cat-spin' : undefined} />
                  <span>从 Steam CM 强制重刷</span>
                </button>
                {activeDetail.isAdminOverride && (
                  <button
                    type="button"
                    className="cat-btn cat-btn-sm cat-btn-danger"
                    disabled={busy}
                    onClick={() => void handleClearOverride()}
                  >
                    <Unlock size={13} />
                    <span>解除覆写锁定</span>
                  </button>
                )}
              </div>

              {uniqueGallery.length > 0 && (
                <div className="cat-gallery-strip">
                  {uniqueGallery.slice(0, 8).map((url) => (
                    <img
                      key={url}
                      src={url}
                      alt={item.title}
                      className="cat-gallery-thumb"
                      loading="lazy"
                    />
                  ))}
                </div>
              )}

              <div className="cat-kv-list">
                <div className="cat-kv">
                  <span className="cat-kv-k">创作者</span>
                  <span className="cat-kv-v">
                    {item.creatorName ?? '—'}{' '}
                    {item.creatorSteamId ? `(${item.creatorSteamId})` : ''}
                  </span>
                </div>
                <div className="cat-kv">
                  <span className="cat-kv-k">文件体积 / 订阅数</span>
                  <span className="cat-kv-v cat-mono">
                    {formatBytes(item.fileSize)} · {(item.subscriptions ?? 0).toLocaleString()} 订阅
                  </span>
                </div>
                <div className="cat-kv">
                  <span className="cat-kv-k">远端更新时间</span>
                  <span className="cat-kv-v">{formatUnixSeconds(item.timeUpdated)}</span>
                </div>
                <div className="cat-kv">
                  <span className="cat-kv-k">最近上游核验</span>
                  <span className="cat-kv-v">{formatIsoTime(activeDetail.lastVerifiedAt)}</span>
                </div>
                {activeDetail.removedAt && (
                  <div className="cat-kv">
                    <span className="cat-kv-k">下架墓碑记录于</span>
                    <span className="cat-kv-v" style={{ color: 'var(--cat-danger)' }}>
                      {formatIsoTime(activeDetail.removedAt)}
                    </span>
                  </div>
                )}
              </div>

              <div className="cat-kv">
                <span className="cat-kv-k">VPK 下载直链 (file_url)</span>
                {item.fileUrl ? (
                  <div className="cat-row" style={{ marginTop: 4 }}>
                    <code
                      className="cat-mono"
                      style={{
                        flex: 1,
                        fontSize: 11.5,
                        background: 'var(--cat-surface-1)',
                        padding: '6px 9px',
                        borderRadius: 7,
                        wordBreak: 'break-all',
                      }}
                    >
                      {item.fileUrl}
                    </code>
                    <button
                      type="button"
                      className="cat-btn cat-btn-sm"
                      onClick={() => void copyText(item.fileUrl!, '下载直链')}
                    >
                      <Copy size={12} />
                      <span>复制</span>
                    </button>
                  </div>
                ) : (
                  <span className="cat-kv-v" style={{ color: 'var(--cat-text-dim)' }}>
                    当前无可用直链（合集条目或已下架清理）
                  </span>
                )}
              </div>

              {item.tags.length > 0 && (
                <div className="cat-row" style={{ gap: 6 }}>
                  {item.tags.map((t) => (
                    <span key={t} className="cat-badge">
                      {t}
                    </span>
                  ))}
                </div>
              )}

              {item.dependencies.length > 0 && (
                <div>
                  <div className="cat-label" style={{ marginBottom: 6 }}>
                    前置依赖 ({item.dependencies.length})
                  </div>
                  <div className="cat-row" style={{ gap: 6 }}>
                    {item.dependencies.map((dep) => (
                      <button
                        key={dep.workshopId}
                        type="button"
                        className="cat-btn cat-btn-sm cat-mono"
                        onClick={() => onSelectWorkshopId(dep.workshopId)}
                      >
                        #{dep.workshopId} {dep.title ?? ''}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              {item.collectionChildren.length > 0 && (
                <div>
                  <div className="cat-label" style={{ marginBottom: 6 }}>
                    合集子条目 ({item.collectionChildren.length})
                  </div>
                  <div className="cat-row" style={{ gap: 6 }}>
                    {item.collectionChildren.map((cid) => (
                      <button
                        key={cid}
                        type="button"
                        className="cat-btn cat-btn-sm cat-mono"
                        onClick={() => onSelectWorkshopId(cid)}
                      >
                        #{cid}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              <form
                onSubmit={(e) => void handleApplyOverride(e)}
                className="cat-card"
                style={{ padding: 14 }}
              >
                <h4 className="cat-card-title" style={{ fontSize: 13.5 }}>
                  <Lock size={14} />
                  <span>管理员手动覆写与状态锁定</span>
                </h4>
                <div className="cat-row">
                  <div className="cat-field">
                    <label className="cat-label">状态</label>
                    <select
                      className="cat-select"
                      value={ovStatus}
                      onChange={(e) => setOvStatus(e.target.value as WorkshopItemStatus)}
                    >
                      <option value="active">active</option>
                      <option value="removed">removed</option>
                      <option value="private_or_unavailable">private_or_unavailable</option>
                    </select>
                  </div>
                  <div className="cat-field" style={{ flex: 2 }}>
                    <label className="cat-label">标题覆写</label>
                    <input
                      className="cat-input"
                      value={ovTitle}
                      onChange={(e) => setOvTitle(e.target.value)}
                    />
                  </div>
                </div>
                <div className="cat-field">
                  <label className="cat-label">备用直链覆写 (file_url)</label>
                  <input
                    className="cat-input cat-mono"
                    value={ovFileUrl}
                    onChange={(e) => setOvFileUrl(e.target.value)}
                    placeholder="https://..."
                  />
                </div>
                <div className="cat-field">
                  <label className="cat-label">前置依赖 ID 列表 (逗号分隔)</label>
                  <input
                    className="cat-input cat-mono"
                    value={ovDepsText}
                    onChange={(e) => setOvDepsText(e.target.value)}
                    placeholder="2814105100, ..."
                  />
                </div>
                <div className="cat-field">
                  <label className="cat-label">覆写原因 (写入审计日志)</label>
                  <input
                    className="cat-input"
                    value={ovReason}
                    onChange={(e) => setOvReason(e.target.value)}
                    placeholder="如：上游误删，锁定历史元数据与依赖边"
                  />
                </div>
                <button
                  type="submit"
                  className="cat-btn cat-btn-primary cat-btn-sm"
                  style={{ alignSelf: 'flex-start' }}
                  disabled={busy}
                >
                  <Lock size={13} />
                  <span>保存并锁定覆写</span>
                </button>
              </form>

              <div>
                <div className="cat-label" style={{ marginBottom: 8 }}>
                  历史快照修订时间线 ({activeDetail.revisions.length})
                </div>
                <div className="cat-table-wrap">
                  <table className="cat-table">
                    <thead>
                      <tr>
                        <th>记录时间</th>
                        <th>状态</th>
                        <th>远端版本</th>
                        <th>依赖数</th>
                        <th>来源</th>
                      </tr>
                    </thead>
                    <tbody>
                      {activeDetail.revisions.map((rev) => (
                        <tr key={rev.id}>
                          <td style={{ fontSize: 12 }}>{formatIsoTime(rev.recordedAt)}</td>
                          <td>
                            <span className="cat-badge">{rev.status}</span>
                          </td>
                          <td className="cat-mono" style={{ fontSize: 12 }}>
                            {rev.timeUpdated ?? '—'}
                          </td>
                          <td className="cat-mono">{rev.dependencies.length}</td>
                          <td className="cat-mono" style={{ fontSize: 12 }}>
                            {rev.source}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            </>
          )}
        </div>
      </aside>
    </div>
  );
}
