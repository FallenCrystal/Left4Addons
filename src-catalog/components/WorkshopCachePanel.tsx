import { useCallback, useEffect, useState, type FormEvent, type MouseEvent } from 'react';
import { ChevronLeft, ChevronRight, Link2, RefreshCw, Search } from 'lucide-react';
import {
  formatBytes,
  formatUnixSeconds,
  refreshAdminItem,
  searchCatalogItems,
  type SearchResponse,
  type WorkshopItemStatus,
} from '../api';

export interface WorkshopCachePanelProps {
  onInspectItem: (workshopId: string) => void;
  onNotify: (msg: string, tone?: 'default' | 'error') => void;
}

export function WorkshopCachePanel({ onInspectItem, onNotify }: WorkshopCachePanelProps) {
  const [q, setQ] = useState('');
  const [tag, setTag] = useState('');
  const [status, setStatus] = useState<WorkshopItemStatus | ''>('');
  const [sort, setSort] = useState('updated');
  const [page, setPage] = useState(1);
  const [loading, setLoading] = useState(true);
  const [refreshingId, setRefreshingId] = useState<string | null>(null);
  const [data, setData] = useState<SearchResponse | null>(null);

  const pageSize = 25;

  const loadPage = useCallback(
    async (targetPage: number) => {
      setLoading(true);
      try {
        const res = await searchCatalogItems({
          q,
          tag,
          status,
          sort,
          page: targetPage,
          pageSize,
        });
        setData(res);
      } catch (err) {
        onNotify(err instanceof Error ? err.message : String(err), 'error');
      } finally {
        setLoading(false);
      }
    },
    [onNotify, q, sort, status, tag],
  );

  useEffect(() => {
    let cancelled = false;
    void searchCatalogItems({
      q,
      tag,
      status,
      sort,
      page,
      pageSize,
    })
      .then((res) => {
        if (!cancelled) setData(res);
      })
      .catch((err) => {
        if (!cancelled) onNotify(err instanceof Error ? err.message : String(err), 'error');
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [onNotify, page, q, sort, status, tag]);

  const handleSearchSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (page === 1) {
      void loadPage(1);
    } else {
      setPage(1);
    }
  };

  const handleQuickRefresh = async (e: MouseEvent, workshopId: string) => {
    e.stopPropagation();
    setRefreshingId(workshopId);
    try {
      await refreshAdminItem(workshopId);
      await loadPage(page);
      onNotify(`已从 Steam CM 强制刷新 #${workshopId}`);
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setRefreshingId(null);
    }
  };

  const totalPages = data ? Math.max(1, Math.ceil(data.total / pageSize)) : 1;

  return (
    <>
      <div className="cat-page-header">
        <div>
          <h2 className="cat-page-title">工坊缓存库与墓碑检视</h2>
          <p className="cat-page-desc">
            浏览本地 SQLite 已归档的模组与合集、检查 <code className="cat-mono">file_url</code> 直链新鲜度、强制重刷或执行管理员覆写。
          </p>
        </div>
        <button type="button" className="cat-btn" onClick={() => void loadPage(page)}>
          <RefreshCw size={14} className={loading ? 'cat-spin' : undefined} />
          <span>刷新列表</span>
        </button>
      </div>

      <form className="cat-card" onSubmit={handleSearchSubmit}>
        <div className="cat-row">
          <div className="cat-field" style={{ flex: 2 }}>
            <label className="cat-label">关键词 / Workshop ID</label>
            <input
              className="cat-input"
              placeholder="搜索标题、简介或 Workshop ID…"
              value={q}
              onChange={(e) => setQ(e.target.value)}
            />
          </div>
          <div className="cat-field">
            <label className="cat-label">标签 (Tag)</label>
            <input
              className="cat-input"
              placeholder="如 Campaigns, Weapons"
              value={tag}
              onChange={(e) => setTag(e.target.value)}
            />
          </div>
          <div className="cat-field">
            <label className="cat-label">状态筛选</label>
            <select
              className="cat-select"
              value={status}
              onChange={(e) => {
                setStatus(e.target.value as WorkshopItemStatus | '');
                setPage(1);
              }}
            >
              <option value="">全部状态</option>
              <option value="active">Active (正常)</option>
              <option value="removed">Removed (下架墓碑)</option>
              <option value="private_or_unavailable">Private (私密/不可用)</option>
            </select>
          </div>
          <div className="cat-field">
            <label className="cat-label">排序方式</label>
            <select
              className="cat-select"
              value={sort}
              onChange={(e) => {
                setSort(e.target.value);
                setPage(1);
              }}
            >
              <option value="updated">最近更新</option>
              <option value="subscriptions">最高订阅</option>
              <option value="newest">最新发布</option>
              <option value="title">标题排序</option>
              <option value="relevance">搜索相关度</option>
            </select>
          </div>
          <button
            type="submit"
            className="cat-btn cat-btn-primary"
            style={{ alignSelf: 'flex-end' }}
          >
            <Search size={14} />
            <span>查询</span>
          </button>
        </div>
      </form>

      <div className="cat-table-wrap">
        <table className="cat-table">
          <thead>
            <tr>
              <th>Workshop ID</th>
              <th>类型</th>
              <th>标题 / 作者</th>
              <th>状态 / 来源</th>
              <th>直链 / 体积</th>
              <th>依赖 / 子项</th>
              <th>最后更新</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {!data || data.items.length === 0 ? (
              <tr>
                <td colSpan={8} style={{ textAlign: 'center', padding: 32, color: 'var(--cat-text-muted)' }}>
                  {loading ? '正在检索缓存数据库…' : '当前筛选条件下暂无缓存记录'}
                </td>
              </tr>
            ) : (
              data.items.map((item) => {
                const statusTone =
                  item.status === 'active'
                    ? 'success'
                    : item.status === 'removed'
                      ? 'danger'
                      : 'warning';
                return (
                  <tr
                    key={item.workshopId}
                    className="cat-clickable-row"
                    onClick={() => onInspectItem(item.workshopId)}
                  >
                    <td className="cat-mono">#{item.workshopId}</td>
                    <td>
                      <span
                        className="cat-badge"
                        data-tone={item.fileType === 'collection' ? 'info' : undefined}
                      >
                        {item.fileType === 'collection' ? '合集' : '组件'}
                      </span>
                    </td>
                    <td>
                      <div style={{ fontWeight: 580 }}>{item.title}</div>
                      <div style={{ fontSize: 12, color: 'var(--cat-text-muted)' }}>
                        {item.creatorName ?? item.creatorSteamId ?? '—'}
                      </div>
                    </td>
                    <td>
                      <div className="cat-row" style={{ gap: 5 }}>
                        <span className="cat-badge" data-tone={statusTone}>
                          {item.status}
                        </span>
                        {item.source === 'admin_override' && (
                          <span className="cat-badge" data-tone="warning">
                            覆写锁定
                          </span>
                        )}
                      </div>
                    </td>
                    <td>
                      {item.fileUrl ? (
                        <span className="cat-badge" data-tone="success">
                          <Link2 size={11} />
                          <span>{formatBytes(item.fileSize)}</span>
                        </span>
                      ) : (
                        <span style={{ color: 'var(--cat-text-dim)', fontSize: 12 }}>无直链</span>
                      )}
                    </td>
                    <td className="cat-mono" style={{ fontSize: 12.5 }}>
                      {item.fileType === 'collection'
                        ? `${item.collectionChildren.length} 子项`
                        : `${item.dependencies.length} 依赖`}
                    </td>
                    <td style={{ fontSize: 12, color: 'var(--cat-text-muted)' }}>
                      {formatUnixSeconds(item.timeUpdated)}
                    </td>
                    <td>
                      <button
                        type="button"
                        className="cat-btn cat-btn-sm"
                        disabled={refreshingId === item.workshopId}
                        title="强制从 Steam CM 重新拉取"
                        onClick={(e) => void handleQuickRefresh(e, item.workshopId)}
                      >
                        <RefreshCw
                          size={12}
                          className={refreshingId === item.workshopId ? 'cat-spin' : undefined}
                        />
                        <span>重刷</span>
                      </button>
                    </td>
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>

      {data && (
        <div className="cat-row" style={{ justifyContent: 'space-between' }}>
          <span style={{ fontSize: 12.5, color: 'var(--cat-text-muted)' }}>
            共 <strong className="cat-mono">{data.total.toLocaleString()}</strong> 条缓存记录 · 第{' '}
            <span className="cat-mono">
              {data.page} / {totalPages}
            </span>{' '}
            页
          </span>
          <div className="cat-row">
            <button
              type="button"
              className="cat-btn cat-btn-sm"
              disabled={page <= 1 || loading}
              onClick={() => setPage((p) => Math.max(1, p - 1))}
            >
              <ChevronLeft size={14} />
              <span>上一页</span>
            </button>
            <button
              type="button"
              className="cat-btn cat-btn-sm"
              disabled={page >= totalPages || loading}
              onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
            >
              <span>下一页</span>
              <ChevronRight size={14} />
            </button>
          </div>
        </div>
      )}
    </>
  );
}
