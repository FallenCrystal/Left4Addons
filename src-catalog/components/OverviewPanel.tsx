import { useState } from 'react';
import {
  Activity,
  DownloadCloud,
  Play,
  RefreshCw,
  RotateCcw,
  Save,
  Wifi,
} from 'lucide-react';
import {
  fetchUpstreamItems,
  formatIsoTime,
  formatUnixSeconds,
  parseWorkshopIdsInput,
  resetBackfillCursor,
  triggerIncrementalSync,
  updateSyncConfig,
  type AdminFetchUpstreamResponse,
  type CatalogOverviewResponse,
  type CatalogSyncConfigDto,
} from '../api';

export interface OverviewPanelProps {
  overview: CatalogOverviewResponse;
  onRefresh: () => Promise<void>;
  onInspectItem: (workshopId: string) => void;
  onOpenSteamAuth: () => void;
  onNotify: (msg: string, tone?: 'default' | 'error') => void;
}

export function OverviewPanel({
  overview,
  onRefresh,
  onInspectItem,
  onOpenSteamAuth,
  onNotify,
}: OverviewPanelProps) {
  const { stats, steam, sync } = overview;
  const [syncDraft, setSyncDraft] = useState<CatalogSyncConfigDto | null>(null);
  const [savingSync, setSavingSync] = useState(false);
  const [triggering, setTriggering] = useState(false);
  const [fetchInput, setFetchInput] = useState('');
  const [fetchingIds, setFetchingIds] = useState(false);
  const [lastFetchResult, setLastFetchResult] = useState<AdminFetchUpstreamResponse | null>(null);

  const effectiveSync = syncDraft ?? sync.config;
  const updateDraft = (updater: (curr: CatalogSyncConfigDto) => CatalogSyncConfigDto) => {
    setSyncDraft((prev) => updater(prev ?? sync.config));
  };

  const dirty = syncDraft !== null && JSON.stringify(syncDraft) !== JSON.stringify(sync.config);

  const backfillPercent =
    sync.remoteWorkshopTotal && sync.remoteWorkshopTotal > 0
      ? Math.min(100, Math.round((sync.backfillSyncedTotal / sync.remoteWorkshopTotal) * 100))
      : 0;

  const handleSaveConfig = async () => {
    setSavingSync(true);
    try {
      await updateSyncConfig(effectiveSync);
      setSyncDraft(null);
      await onRefresh();
      onNotify('同步策略已保存并写入 catalog.toml');
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setSavingSync(false);
    }
  };

  const handleTriggerIncremental = async () => {
    setTriggering(true);
    try {
      await triggerIncrementalSync();
      await onRefresh();
      onNotify('已触发增量同步任务');
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setTriggering(false);
    }
  };

  const handleResetCursor = async () => {
    try {
      await resetBackfillCursor();
      await onRefresh();
      onNotify('全量回填游标已重置为起始位置');
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    }
  };

  const handleFetchIds = async () => {
    const ids = parseWorkshopIdsInput(fetchInput);
    if (ids.length === 0) {
      onNotify('请输入至少一个有效的 Workshop ID 或工坊链接', 'error');
      return;
    }
    setFetchingIds(true);
    try {
      const res = await fetchUpstreamItems(ids);
      setLastFetchResult(res);
      await onRefresh();
      onNotify(
        `上游拉取完成：缓存 ${res.items.length} 项，下架 ${res.removedIds.length} 项，不可用 ${res.unavailableIds.length} 项`,
      );
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setFetchingIds(false);
    }
  };

  const connTone =
    steam.connectionState === 'connected'
      ? 'success'
      : steam.connectionState === 'error'
        ? 'danger'
        : 'warning';

  return (
    <>
      <div className="cat-page-header">
        <div>
          <h2 className="cat-page-title">运行总览与缓存同步引擎</h2>
          <p className="cat-page-desc">
            服务端通过纯协议层 SteamKit CM 直接拉取创意工坊元数据、依赖树与 VPK 下载直链，替代客户端侧受 VAC 限制的 Steamworks SDK。
          </p>
        </div>
        <button type="button" className="cat-btn" onClick={() => void onRefresh()}>
          <RefreshCw size={14} />
          <span>刷新状态</span>
        </button>
      </div>

      <section className="cat-kpi-grid" aria-label="缓存核心指标">
        <div className="cat-kpi">
          <span className="cat-kpi-label">总缓存条目</span>
          <span className="cat-kpi-value cat-mono">{stats.totalItems.toLocaleString()}</span>
        </div>
        <div className="cat-kpi">
          <span className="cat-kpi-label">正常可用 (Active)</span>
          <span className="cat-kpi-value cat-mono" style={{ color: 'var(--cat-accent)' }}>
            {stats.activeItems.toLocaleString()}
          </span>
        </div>
        <div className="cat-kpi">
          <span className="cat-kpi-label">下架墓碑 (Removed)</span>
          <span className="cat-kpi-value cat-mono" style={{ color: 'var(--cat-warning)' }}>
            {stats.removedItems.toLocaleString()}
          </span>
        </div>
        <div className="cat-kpi">
          <span className="cat-kpi-label">私密/受限</span>
          <span className="cat-kpi-value cat-mono">{stats.privateItems.toLocaleString()}</span>
        </div>
        <div className="cat-kpi">
          <span className="cat-kpi-label">已缓存合集</span>
          <span className="cat-kpi-value cat-mono">{stats.collectionsCount.toLocaleString()}</span>
        </div>
        <div className="cat-kpi">
          <span className="cat-kpi-label">管理员覆写锁定</span>
          <span className="cat-kpi-value cat-mono">{stats.overriddenItems.toLocaleString()}</span>
        </div>
      </section>

      <div className="cat-grid-2">
        <section className="cat-card">
          <div className="cat-card-header">
            <h3 className="cat-card-title">
              <Wifi size={16} />
              <span>SteamKit CM 上游连接</span>
            </h3>
            <div className="cat-row">
              <span className="cat-badge" data-tone={connTone}>
                {steam.connectionState}
              </span>
              <span className="cat-badge" data-tone={steam.mode === 'account' ? 'info' : undefined}>
                {steam.mode === 'account' ? `账号: ${steam.accountName ?? '已登录'}` : '匿名 CM 会话'}
              </span>
            </div>
          </div>

          <div className="cat-kv-list">
            <div className="cat-kv">
              <span className="cat-kv-k">SteamID64</span>
              <span className="cat-kv-v cat-mono">{steam.steamId ?? '—'}</span>
            </div>
            <div className="cat-kv">
              <span className="cat-kv-k">CM 节点区域 / Cell</span>
              <span className="cat-kv-v cat-mono">
                {steam.countryCode ?? '—'} / {steam.cellId ?? '—'}
              </span>
            </div>
            <div className="cat-kv">
              <span className="cat-kv-k">累计 CM 请求 / 失败</span>
              <span className="cat-kv-v cat-mono">
                {steam.totalRequests.toLocaleString()} / {steam.failedRequests.toLocaleString()}
              </span>
            </div>
            <div className="cat-kv">
              <span className="cat-kv-k">建立连接时间</span>
              <span className="cat-kv-v">{formatIsoTime(steam.connectedSince)}</span>
            </div>
          </div>

          {steam.lastError && (
            <div className="cat-banner" data-tone="error">
              <span>最近异常：{steam.lastError}</span>
            </div>
          )}

          <div className="cat-row">
            <button type="button" className="cat-btn" onClick={onOpenSteamAuth}>
              配置 Steam 账号 / 扫码登录
            </button>
          </div>
        </section>

        <section className="cat-card">
          <div className="cat-card-header">
            <h3 className="cat-card-title">
              <DownloadCloud size={16} />
              <span>按需预热与指定 ID 抓取</span>
            </h3>
          </div>
          <p style={{ margin: 0, fontSize: 12.5, color: 'var(--cat-text-muted)' }}>
            输入一个或多个 Workshop ID 或 Steam 创意工坊链接，立即向 Steam CM 请求最新元数据与 <code className="cat-mono">file_url</code> 直链（若为合集也会自动拉取子条目）。
          </p>
          <div className="cat-row">
            <input
              className="cat-input cat-mono"
              placeholder="例如 2814104912, https://steamcommunity.com/sharedfiles/filedetails/?id=..."
              value={fetchInput}
              onChange={(e) => setFetchInput(e.target.value)}
            />
            <button
              type="button"
              className="cat-btn cat-btn-primary"
              disabled={fetchingIds || !fetchInput.trim()}
              onClick={() => void handleFetchIds()}
            >
              <DownloadCloud size={14} />
              <span>{fetchingIds ? '拉取中…' : '立即拉取'}</span>
            </button>
          </div>

          {lastFetchResult && (
            <div className="cat-banner" style={{ flexDirection: 'column', gap: 6 }}>
              <div>
                已缓存 <strong>{lastFetchResult.items.length}</strong> 项
                {lastFetchResult.removedIds.length > 0 &&
                  ` · 下架 ${lastFetchResult.removedIds.join(', ')}`}
                {lastFetchResult.unavailableIds.length > 0 &&
                  ` · 不可用 ${lastFetchResult.unavailableIds.join(', ')}`}
              </div>
              {lastFetchResult.items.length > 0 && (
                <div className="cat-row">
                  {lastFetchResult.items.slice(0, 6).map((item) => (
                    <button
                      key={item.workshopId}
                      type="button"
                      className="cat-btn cat-btn-sm"
                      onClick={() => onInspectItem(item.workshopId)}
                    >
                      #{item.workshopId} {item.title}
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}
        </section>
      </div>

      <div className="cat-grid-2">
        <section className="cat-card">
          <div className="cat-card-header">
            <h3 className="cat-card-title">
              <Activity size={16} />
              <span>增量更新追踪与全量游标状态</span>
            </h3>
            <div className="cat-row">
              <button
                type="button"
                className="cat-btn cat-btn-primary cat-btn-sm"
                disabled={triggering || sync.incrementalRunning}
                onClick={() => void handleTriggerIncremental()}
              >
                <Play size={13} />
                <span>{sync.incrementalRunning ? '增量同步运行中…' : '立即执行增量同步'}</span>
              </button>
              <button
                type="button"
                className="cat-btn cat-btn-sm"
                onClick={() => void handleResetCursor()}
              >
                <RotateCcw size={13} />
                <span>重置全量游标</span>
              </button>
            </div>
          </div>

          <div className="cat-kv-list">
            <div className="cat-kv">
              <span className="cat-kv-k">上次增量同步</span>
              <span className="cat-kv-v">{formatIsoTime(sync.lastIncrementalAt)}</span>
            </div>
            <div className="cat-kv">
              <span className="cat-kv-k">上次增量写入条数</span>
              <span className="cat-kv-v cat-mono">{sync.lastIncrementalUpserted}</span>
            </div>
            <div className="cat-kv">
              <span className="cat-kv-k">最新更新水位线</span>
              <span className="cat-kv-v">{formatUnixSeconds(sync.highWaterTimeUpdated)}</span>
            </div>
            <div className="cat-kv">
              <span className="cat-kv-k">全量游标 (Cursor)</span>
              <span className="cat-kv-v cat-mono">{sync.backfillCursor ?? '*'}</span>
            </div>
          </div>

          <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
            <div className="cat-row" style={{ justifyContent: 'space-between', fontSize: 12 }}>
              <span style={{ color: 'var(--cat-text-muted)' }}>
                全量创意工坊回填进度 {sync.backfillRunning ? '(运行中)' : ''}
              </span>
              <span className="cat-mono">
                {sync.backfillSyncedTotal.toLocaleString()} /{' '}
                {sync.remoteWorkshopTotal ? sync.remoteWorkshopTotal.toLocaleString() : '未知'} (
                {backfillPercent}%)
              </span>
            </div>
            <div className="cat-progress-track">
              <div className="cat-progress-fill" style={{ width: `${backfillPercent}%` }} />
            </div>
          </div>

          {sync.lastSyncError && (
            <div className="cat-banner" data-tone="error">
              <span>同步告警：{sync.lastSyncError}</span>
            </div>
          )}
        </section>

        <section className="cat-card">
          <div className="cat-card-header">
            <h3 className="cat-card-title">缓存刷新与同步策略 (持久化至 catalog.toml)</h3>
            <button
              type="button"
              className="cat-btn cat-btn-primary cat-btn-sm"
              disabled={!dirty || savingSync}
              onClick={() => void handleSaveConfig()}
            >
              <Save size={13} />
              <span>{savingSync ? '保存中…' : '保存策略'}</span>
            </button>
          </div>

          <div>
            <div className="cat-toggle-row">
              <div>
                <div style={{ fontWeight: 560 }}>Read-Through 穿透缓存与过期刷新</div>
                <div style={{ fontSize: 12, color: 'var(--cat-text-muted)' }}>
                  客户端查询未命中或超过 TTL 时自动向 Steam CM 拉取最新 <code className="cat-mono">file_url</code>
                </div>
              </div>
              <input
                type="checkbox"
                checked={effectiveSync.enableReadThrough}
                onChange={(e) =>
                  updateDraft((d) => ({ ...d, enableReadThrough: e.target.checked }))
                }
              />
            </div>

            <div className="cat-toggle-row">
              <div>
                <div style={{ fontWeight: 560 }}>定时增量同步 (RankedByLastUpdatedDate)</div>
                <div style={{ fontSize: 12, color: 'var(--cat-text-muted)' }}>
                  主动追踪工坊组件更新并即时刷新失效的旧版下载直链
                </div>
              </div>
              <input
                type="checkbox"
                checked={effectiveSync.enableIncrementalSync}
                onChange={(e) =>
                  updateDraft((d) => ({ ...d, enableIncrementalSync: e.target.checked }))
                }
              />
            </div>

            <div className="cat-toggle-row">
              <div>
                <div style={{ fontWeight: 560 }}>后台全量索引回填 (Cursor Backfill)</div>
                <div style={{ fontSize: 12, color: 'var(--cat-text-muted)' }}>
                  使用 <code className="cat-mono">QueryFiles</code> 游标逐步归档全量求生之路 2 创意工坊条目
                </div>
              </div>
              <input
                type="checkbox"
                checked={effectiveSync.enableBackfill}
                onChange={(e) => updateDraft((d) => ({ ...d, enableBackfill: e.target.checked }))}
              />
            </div>
          </div>

          <div className="cat-row">
            <div className="cat-field">
              <label className="cat-label">单条缓存 TTL (秒)</label>
              <input
                type="number"
                min={30}
                className="cat-input cat-mono"
                value={effectiveSync.itemTtlSecs}
                onChange={(e) =>
                  updateDraft((d) => ({
                    ...d,
                    itemTtlSecs: Math.max(30, Number.parseInt(e.target.value || '300', 10) || 300),
                  }))
                }
              />
            </div>
            <div className="cat-field">
              <label className="cat-label">增量轮询间隔 (秒)</label>
              <input
                type="number"
                min={30}
                className="cat-input cat-mono"
                value={effectiveSync.incrementalIntervalSecs}
                onChange={(e) =>
                  updateDraft((d) => ({
                    ...d,
                    incrementalIntervalSecs: Math.max(
                      30,
                      Number.parseInt(e.target.value || '180', 10) || 180,
                    ),
                  }))
                }
              />
            </div>
            <div className="cat-field">
              <label className="cat-label">每轮增量页数</label>
              <input
                type="number"
                min={1}
                max={20}
                className="cat-input cat-mono"
                value={effectiveSync.incrementalPagesPerTick}
                onChange={(e) =>
                  updateDraft((d) => ({
                    ...d,
                    incrementalPagesPerTick: Math.min(
                      20,
                      Math.max(1, Number.parseInt(e.target.value || '2', 10) || 2),
                    ),
                  }))
                }
              />
            </div>
          </div>

          <div className="cat-row">
            <div className="cat-field">
              <label className="cat-label">全量回填间隔 (秒)</label>
              <input
                type="number"
                min={2}
                className="cat-input cat-mono"
                value={effectiveSync.backfillIntervalSecs}
                onChange={(e) =>
                  updateDraft((d) => ({
                    ...d,
                    backfillIntervalSecs: Math.max(
                      2,
                      Number.parseInt(e.target.value || '15', 10) || 15,
                    ),
                  }))
                }
              />
            </div>
            <div className="cat-field">
              <label className="cat-label">全量单批条数 (1-100)</label>
              <input
                type="number"
                min={1}
                max={100}
                className="cat-input cat-mono"
                value={effectiveSync.backfillBatchSize}
                onChange={(e) =>
                  updateDraft((d) => ({
                    ...d,
                    backfillBatchSize: Math.min(
                      100,
                      Math.max(1, Number.parseInt(e.target.value || '50', 10) || 50),
                    ),
                  }))
                }
              />
            </div>
          </div>
        </section>
      </div>
    </>
  );
}
