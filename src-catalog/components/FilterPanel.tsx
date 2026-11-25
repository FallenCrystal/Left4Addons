import { useState } from 'react';
import {
  Activity,
  FlaskConical,
  Play,
  RefreshCw,
  RotateCcw,
  Save,
  ShieldCheck,
  SlidersHorizontal,
} from 'lucide-react';
import {
  detectFilterPreset,
  getFilterStatus,
  getPresetFilterParams,
  parseWorkshopIdsInput,
  restartFilterBackend,
  testFilterPredict,
  updateFilterConfig,
  type CatalogFilterBackendKind,
  type CatalogFilterConfigDto,
  type CatalogFilterStatus,
  type FilterGalleryAggregation,
  type FilterModelParams,
  type FilterPredictRequest,
  type FilterPresetId,
  type FilterProbabilities,
} from '../api';

export interface FilterPanelProps {
  filter: CatalogFilterStatus;
  onFilterUpdated: (next: CatalogFilterStatus) => void;
  onNotify: (msg: string, tone?: 'default' | 'error') => void;
}

const PRESET_OPTIONS: Array<{
  id: Exclude<FilterPresetId, 'custom'>;
  label: string;
}> = [
  { id: 'recommended', label: '推荐均衡' },
  { id: 'strict', label: '严格高召回' },
  { id: 'relaxed', label: '宽松防误伤' },
  { id: 'nsfw_only', label: '仅擦边 NSFW' },
  { id: 'gore_only', label: '仅重口血腥' },
  { id: 'low_cpu', label: '轻量低占用' },
];

function formatProb(val?: number): string {
  if (val === undefined || val === null || Number.isNaN(val)) {
    return '—';
  }
  return `${(val * 100).toFixed(2)}% (${val.toFixed(4)})`;
}

function probBarColor(val: number): string {
  if (val >= 0.72) return 'var(--cat-danger)';
  if (val >= 0.3) return 'var(--cat-warning)';
  return 'var(--cat-accent)';
}

export function FilterPanel({ filter, onFilterUpdated, onNotify }: FilterPanelProps) {
  const [draft, setDraft] = useState<CatalogFilterConfigDto | null>(null);
  const [saving, setSaving] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [restarting, setRestarting] = useState(false);

  // Test sandbox state
  const [testWorkshopInput, setTestWorkshopInput] = useState('');
  const [testTitle, setTestTitle] = useState('');
  const [testTagsInput, setTestTagsInput] = useState('');
  const [testDescription, setTestDescription] = useState('');
  const [testImageUrlsInput, setTestImageUrlsInput] = useState('');
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<FilterProbabilities | null>(null);

  const effective = draft ?? filter.config;
  const dirty = draft !== null && JSON.stringify(draft) !== JSON.stringify(filter.config);
  const activePreset = detectFilterPreset(effective.params);

  const updateDraft = (updater: (curr: CatalogFilterConfigDto) => CatalogFilterConfigDto) => {
    setDraft((prev) => updater(prev ?? filter.config));
  };

  const updateParams = (updater: (curr: FilterModelParams) => FilterModelParams) => {
    updateDraft((curr) => ({
      ...curr,
      params: updater(curr.params),
    }));
  };

  const handleRefreshStatus = async () => {
    setRefreshing(true);
    try {
      const next = await getFilterStatus();
      onFilterUpdated(next);
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setRefreshing(false);
    }
  };

  const handleRestart = async () => {
    setRestarting(true);
    try {
      const next = await restartFilterBackend();
      onFilterUpdated(next);
      onNotify(
        next.ready
          ? '内容过滤推理后端已重启并就绪'
          : '已执行重启/健康检查，请留意服务就绪状态',
      );
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setRestarting(false);
    }
  };

  const handleSaveConfig = async () => {
    setSaving(true);
    try {
      const payload: CatalogFilterConfigDto = {
        ...effective,
        deployDir: effective.deployDir?.trim() ? effective.deployDir.trim() : undefined,
        pythonBin: effective.pythonBin?.trim() ? effective.pythonBin.trim() : undefined,
        upstreamUrl: effective.upstreamUrl?.trim() ? effective.upstreamUrl.trim() : undefined,
      };
      const next = await updateFilterConfig(payload);
      setDraft(null);
      onFilterUpdated(next);
      onNotify('内容过滤配置已应用并持久化至 catalog.toml');
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setSaving(false);
    }
  };

  const handleRunTest = async () => {
    const parsedIds = parseWorkshopIdsInput(testWorkshopInput);
    const workshopId = parsedIds[0];
    const title = testTitle.trim();
    const description = testDescription.trim();
    const tags = testTagsInput
      .split(/[,，]/)
      .map((s) => s.trim())
      .filter(Boolean);
    const imageUrls = testImageUrlsInput
      .split(/[\r\n,]+/)
      .map((s) => s.trim())
      .filter(Boolean);

    if (!workshopId && !title && !description && tags.length === 0 && imageUrls.length === 0) {
      onNotify('请输入 Workshop ID 或填写自定义标题/简介/预览图链接后再试算', 'error');
      return;
    }

    const req: FilterPredictRequest = {};
    if (workshopId) req.workshopId = workshopId;
    if (title) req.title = title;
    if (description) req.description = description;
    if (tags.length > 0) req.tags = tags;
    if (imageUrls.length > 0) req.imageUrls = imageUrls;

    setTesting(true);
    try {
      const probs = await testFilterPredict(req);
      setTestResult(probs);
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      setTesting(false);
    }
  };

  const hitRate =
    filter.totalRequests > 0
      ? Math.round((filter.cacheHits / filter.totalRequests) * 100)
      : 0;

  const readyTone = !filter.config.enabled
    ? undefined
    : filter.ready
      ? 'success'
      : 'danger';

  return (
    <>
      <div className="cat-page-header">
        <div>
          <h2 className="cat-page-title">内容过滤模型推理服务</h2>
          <p className="cat-page-desc">
            托管本地 Left4Filters 多模态 ONNX 模型或转发上游 HTTP 推理节点，向客户端提供工坊条目敏感度概率评估。
          </p>
        </div>
        <div className="cat-row">
          <button
            type="button"
            className="cat-btn"
            disabled={refreshing}
            onClick={() => void handleRefreshStatus()}
          >
            <RefreshCw size={14} className={refreshing ? 'cat-spin' : undefined} />
            <span>刷新状态</span>
          </button>
          <button
            type="button"
            className="cat-btn"
            disabled={restarting}
            onClick={() => void handleRestart()}
          >
            <RotateCcw size={14} className={restarting ? 'cat-spin' : undefined} />
            <span>{restarting ? '执行中…' : '重启 / 健康检查'}</span>
          </button>
        </div>
      </div>

      <div className="cat-banner">
        <ShieldCheck size={18} style={{ flexShrink: 0, color: 'var(--cat-accent)', marginTop: 2 }} />
        <div style={{ fontSize: 12.5, color: 'var(--cat-text-muted)' }}>
          <strong style={{ color: 'var(--cat-text)' }}>安全与职责边界说明：</strong>
          公开接口 <code className="cat-mono">POST /api/v1/filter/predict</code>{' '}
          <strong>仅返回校准后的连续敏感度概率</strong>（<code className="cat-mono">prob</code> /{' '}
          <code className="cat-mono">goreProb</code> / <code className="cat-mono">nsfwProb</code>{' '}
          及单模态分项），<strong>绝不发起任何外部大语言模型 (LLM) 请求</strong>，也不替代客户端的阈值与二审策略；模型参数严格受服务端{' '}
          <code className="cat-mono">catalog.toml</code> 与本控制台管控，客户端请求无法覆盖模型参数。
        </div>
      </div>

      <section className="cat-card" aria-label="过滤服务运行健康度">
        <div className="cat-card-header">
          <h3 className="cat-card-title">
            <Activity size={16} />
            <span>推理后端实时状态</span>
          </h3>
          <div className="cat-row">
            <span
              className="cat-badge"
              data-tone={filter.config.enabled ? 'info' : undefined}
            >
              {filter.config.enabled ? '已启用公开接口' : '未启用'}
            </span>
            <span className="cat-badge" data-tone={readyTone}>
              {!filter.config.enabled ? '待机' : filter.ready ? '就绪 (Ready)' : '未就绪'}
            </span>
            <span className="cat-badge">
              {filter.config.backend === 'managed_python'
                ? 'ManagedPython 托管进程'
                : 'UpstreamHttp 远程推理'}
            </span>
          </div>
        </div>

        <div className="cat-kv-list">
          <div className="cat-kv">
            <span className="cat-kv-k">加载模型 / 版本</span>
            <span className="cat-kv-v cat-mono">
              {filter.modelName
                ? `${filter.modelName}${filter.modelVersion ? ` (${filter.modelVersion})` : ''}`
                : '—'}
            </span>
          </div>
          <div className="cat-kv">
            <span className="cat-kv-k">进程 PID / 活动端点</span>
            <span className="cat-kv-v cat-mono">
              {filter.runningPid ? `PID ${filter.runningPid} · ` : ''}
              {filter.activeEndpoint ?? '—'}
            </span>
          </div>
          <div className="cat-kv">
            <span className="cat-kv-k">累计推理请求 / 缓存命中</span>
            <span className="cat-kv-v cat-mono">
              {filter.totalRequests.toLocaleString()} / {filter.cacheHits.toLocaleString()} (
              {hitRate}%)
            </span>
          </div>
          <div className="cat-kv">
            <span className="cat-kv-k">异常失败计数</span>
            <span className="cat-kv-v cat-mono">{filter.failedRequests.toLocaleString()}</span>
          </div>
        </div>

        {filter.lastError && (
          <div className="cat-banner" data-tone="error">
            <span>最近错误：{filter.lastError}</span>
          </div>
        )}
      </section>

      <div className="cat-grid-2">
        <section className="cat-card" aria-label="过滤后端与模型参数配置">
          <div className="cat-card-header">
            <h3 className="cat-card-title">
              <SlidersHorizontal size={16} />
              <span>后端部署与模型校准参数 (持久化至 catalog.toml)</span>
            </h3>
            <div className="cat-row">
              {dirty && (
                <button
                  type="button"
                  className="cat-btn cat-btn-sm"
                  disabled={saving}
                  onClick={() => setDraft(null)}
                >
                  撤销修改
                </button>
              )}
              <button
                type="button"
                className="cat-btn cat-btn-primary cat-btn-sm"
                disabled={!dirty || saving}
                onClick={() => void handleSaveConfig()}
              >
                <Save size={13} />
                <span>{saving ? '保存中…' : '保存配置'}</span>
              </button>
            </div>
          </div>

          <div>
            <div className="cat-toggle-row">
              <div>
                <div style={{ fontWeight: 560 }}>启用内容过滤概率接口</div>
                <div style={{ fontSize: 12, color: 'var(--cat-text-muted)' }}>
                  开启后启动后端进程并响应 <code className="cat-mono">/api/v1/filter/predict</code>
                </div>
              </div>
              <input
                type="checkbox"
                aria-label="启用内容过滤概率接口"
                checked={effective.enabled}
                onChange={(e) =>
                  updateDraft((d) => ({ ...d, enabled: e.target.checked }))
                }
              />
            </div>

            <div className="cat-toggle-row">
              <div>
                <div style={{ fontWeight: 560 }}>启用文本特征推理 (enableText)</div>
                <div style={{ fontSize: 12, color: 'var(--cat-text-muted)' }}>
                  评估模组标题、简介与标签词法特征
                </div>
              </div>
              <input
                type="checkbox"
                aria-label="启用文本特征推理"
                checked={effective.params.enableText}
                onChange={(e) =>
                  updateParams((p) => ({ ...p, enableText: e.target.checked }))
                }
              />
            </div>

            <div className="cat-toggle-row">
              <div>
                <div style={{ fontWeight: 560 }}>启用预览图视觉推理 (enableImage)</div>
                <div style={{ fontSize: 12, color: 'var(--cat-text-muted)' }}>
                  评估封面与画廊预览图（仅拉取受信任 Steam CDN HTTPS 域名）
                </div>
              </div>
              <input
                type="checkbox"
                aria-label="启用预览图视觉推理"
                checked={effective.params.enableImage}
                onChange={(e) =>
                  updateParams((p) => ({ ...p, enableImage: e.target.checked }))
                }
              />
            </div>
          </div>

          <div className="cat-row">
            <div className="cat-field">
              <label className="cat-label" htmlFor="cat-filter-backend">
                推理后端模式
              </label>
              <select
                id="cat-filter-backend"
                className="cat-select"
                value={effective.backend}
                onChange={(e) =>
                  updateDraft((d) => ({
                    ...d,
                    backend: e.target.value as CatalogFilterBackendKind,
                  }))
                }
              >
                <option value="managed_python">ManagedPython (托管本地 Python 子进程)</option>
                <option value="upstream_http">UpstreamHttp (转发外部 HTTP 推理服务)</option>
              </select>
            </div>
            <div className="cat-field">
              <label className="cat-label" htmlFor="cat-filter-ttl">
                结果缓存 TTL (秒，0 禁用)
              </label>
              <input
                id="cat-filter-ttl"
                type="number"
                min={0}
                max={86400}
                className="cat-input cat-mono"
                value={effective.cacheTtlSecs}
                onChange={(e) =>
                  updateDraft((d) => ({
                    ...d,
                    cacheTtlSecs: Math.min(
                      86400,
                      Math.max(0, Number.parseInt(e.target.value || '0', 10) || 0),
                    ),
                  }))
                }
              />
            </div>
          </div>

          {effective.backend === 'managed_python' ? (
            <div className="cat-row">
              <div className="cat-field">
                <label className="cat-label" htmlFor="cat-filter-deploy-dir">
                  Left4Filters 部署路径 (deploy_dir)
                </label>
                <input
                  id="cat-filter-deploy-dir"
                  className="cat-input cat-mono"
                  placeholder="如 ./Left4Filters 或 /opt/Left4Filters/deploy"
                  value={effective.deployDir ?? ''}
                  onChange={(e) =>
                    updateDraft((d) => ({ ...d, deployDir: e.target.value }))
                  }
                />
              </div>
              <div className="cat-field">
                <label className="cat-label" htmlFor="cat-filter-python-bin">
                  Python 解释器路径 (可选)
                </label>
                <input
                  id="cat-filter-python-bin"
                  className="cat-input cat-mono"
                  placeholder="留空自动探测 .venv / python3"
                  value={effective.pythonBin ?? ''}
                  onChange={(e) =>
                    updateDraft((d) => ({ ...d, pythonBin: e.target.value }))
                  }
                />
              </div>
            </div>
          ) : (
            <div className="cat-row">
              <div className="cat-field">
                <label className="cat-label" htmlFor="cat-filter-upstream-url">
                  外部推理服务 URL (upstream_url)
                </label>
                <input
                  id="cat-filter-upstream-url"
                  className="cat-input cat-mono"
                  placeholder="http://127.0.0.1:8090"
                  value={effective.upstreamUrl ?? ''}
                  onChange={(e) =>
                    updateDraft((d) => ({ ...d, upstreamUrl: e.target.value }))
                  }
                />
              </div>
            </div>
          )}

          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            <div className="cat-row" style={{ justifyContent: 'space-between' }}>
              <span className="cat-label">快速参数预设</span>
              {activePreset === 'custom' && (
                <span className="cat-badge">当前为自定义参数</span>
              )}
            </div>
            <div className="cat-row">
              {PRESET_OPTIONS.map((preset) => (
                <button
                  key={preset.id}
                  type="button"
                  className={
                    activePreset === preset.id
                      ? 'cat-btn cat-btn-primary cat-btn-sm'
                      : 'cat-btn cat-btn-sm'
                  }
                  onClick={() => updateParams(() => getPresetFilterParams(preset.id))}
                >
                  {preset.label}
                </button>
              ))}
            </div>
          </div>

          <div className="cat-row">
            <div className="cat-field">
              <label className="cat-label" htmlFor="cat-filter-threads">
                推理线程数 (1-16)
              </label>
              <input
                id="cat-filter-threads"
                type="number"
                min={1}
                max={16}
                className="cat-input cat-mono"
                value={effective.params.threads}
                onChange={(e) =>
                  updateParams((p) => ({
                    ...p,
                    threads: Math.min(
                      16,
                      Math.max(1, Number.parseInt(e.target.value || '4', 10) || 4),
                    ),
                  }))
                }
              />
            </div>
            <div className="cat-field">
              <label className="cat-label" htmlFor="cat-filter-max-images">
                最大预览图张数 (1-8)
              </label>
              <input
                id="cat-filter-max-images"
                type="number"
                min={1}
                max={8}
                className="cat-input cat-mono"
                value={effective.params.maxImages}
                onChange={(e) =>
                  updateParams((p) => ({
                    ...p,
                    maxImages: Math.min(
                      8,
                      Math.max(1, Number.parseInt(e.target.value || '2', 10) || 2),
                    ),
                  }))
                }
              />
            </div>
            <div className="cat-field">
              <label className="cat-label" htmlFor="cat-filter-gallery-agg">
                画廊多图聚合策略
              </label>
              <select
                id="cat-filter-gallery-agg"
                className="cat-select"
                value={effective.params.galleryAggregation}
                onChange={(e) =>
                  updateParams((p) => ({
                    ...p,
                    galleryAggregation: e.target.value as FilterGalleryAggregation,
                  }))
                }
              >
                <option value="max">Max (取单图最高概率)</option>
                <option value="top2_mean">Top-2 Mean (最高两张均值)</option>
                <option value="mean">Mean (全部预览图均值)</option>
              </select>
            </div>
          </div>

          <div className="cat-row">
            <div className="cat-field">
              <label className="cat-label" htmlFor="cat-filter-gore-weight">
                血腥权重 goreWeight (0-3)
              </label>
              <input
                id="cat-filter-gore-weight"
                type="number"
                step="0.05"
                min={0}
                max={3}
                className="cat-input cat-mono"
                value={effective.params.goreWeight}
                onChange={(e) => {
                  const v = Number.parseFloat(e.target.value);
                  updateParams((p) => ({
                    ...p,
                    goreWeight: Number.isFinite(v) ? Math.min(3, Math.max(0, v)) : 1,
                  }));
                }}
              />
            </div>
            <div className="cat-field">
              <label className="cat-label" htmlFor="cat-filter-nsfw-weight">
                擦边权重 nsfwWeight (0-3)
              </label>
              <input
                id="cat-filter-nsfw-weight"
                type="number"
                step="0.05"
                min={0}
                max={3}
                className="cat-input cat-mono"
                value={effective.params.nsfwWeight}
                onChange={(e) => {
                  const v = Number.parseFloat(e.target.value);
                  updateParams((p) => ({
                    ...p,
                    nsfwWeight: Number.isFinite(v) ? Math.min(3, Math.max(0, v)) : 1,
                  }));
                }}
              />
            </div>
          </div>

          <div className="cat-row">
            <div className="cat-field">
              <label className="cat-label" htmlFor="cat-filter-temperature">
                校准温度 temperature (0.25-4)
              </label>
              <input
                id="cat-filter-temperature"
                type="number"
                step="0.05"
                min={0.25}
                max={4}
                className="cat-input cat-mono"
                value={effective.params.temperature}
                onChange={(e) => {
                  const v = Number.parseFloat(e.target.value);
                  updateParams((p) => ({
                    ...p,
                    temperature: Number.isFinite(v) ? Math.min(4, Math.max(0.25, v)) : 1,
                  }));
                }}
              />
            </div>
            <div className="cat-field">
              <label className="cat-label" htmlFor="cat-filter-bias">
                对数偏置 bias (-5 ~ +5)
              </label>
              <input
                id="cat-filter-bias"
                type="number"
                step="0.05"
                min={-5}
                max={5}
                className="cat-input cat-mono"
                value={effective.params.bias}
                onChange={(e) => {
                  const v = Number.parseFloat(e.target.value);
                  updateParams((p) => ({
                    ...p,
                    bias: Number.isFinite(v) ? Math.min(5, Math.max(-5, v)) : 0,
                  }));
                }}
              />
            </div>
          </div>
        </section>

        <section className="cat-card" aria-label="在线概率试算沙盒">
          <div className="cat-card-header">
            <h3 className="cat-card-title">
              <FlaskConical size={16} />
              <span>在线概率试算 (Test Sandbox)</span>
            </h3>
            <div className="cat-row">
              <button
                type="button"
                className="cat-btn cat-btn-sm"
                onClick={() => {
                  setTestWorkshopInput('');
                  setTestTitle('');
                  setTestTagsInput('');
                  setTestDescription('');
                  setTestImageUrlsInput('');
                  setTestResult(null);
                }}
              >
                清空
              </button>
              <button
                type="button"
                className="cat-btn cat-btn-primary cat-btn-sm"
                disabled={testing}
                onClick={() => void handleRunTest()}
              >
                <Play size={13} />
                <span>{testing ? '试算中…' : '执行概率试算'}</span>
              </button>
            </div>
          </div>

          <p style={{ margin: 0, fontSize: 12.5, color: 'var(--cat-text-muted)' }}>
            输入已缓存的公开 Workshop ID（自动补全标题、简介、标签与预览图）或手动输入文本/图片链接，检验当前生效参数的输出概率。
          </p>

          <div className="cat-row">
            <div className="cat-field">
              <label className="cat-label" htmlFor="cat-test-wid">
                Workshop ID 或工坊链接 (可选)
              </label>
              <input
                id="cat-test-wid"
                className="cat-input cat-mono"
                placeholder="如 2814104912"
                value={testWorkshopInput}
                onChange={(e) => setTestWorkshopInput(e.target.value)}
              />
            </div>
            <div className="cat-field">
              <label className="cat-label" htmlFor="cat-test-tags">
                自定义标签 (逗号分隔，可选)
              </label>
              <input
                id="cat-test-tags"
                className="cat-input"
                placeholder="如 Survivors, Anime"
                value={testTagsInput}
                onChange={(e) => setTestTagsInput(e.target.value)}
              />
            </div>
          </div>

          <div className="cat-field">
            <label className="cat-label" htmlFor="cat-test-title">
              自定义标题 (可选)
            </label>
            <input
              id="cat-test-title"
              className="cat-input"
              placeholder="输入待评估的模组标题…"
              value={testTitle}
              onChange={(e) => setTestTitle(e.target.value)}
            />
          </div>

          <div className="cat-field">
            <label className="cat-label" htmlFor="cat-test-desc">
              自定义简介 (可选)
            </label>
            <textarea
              id="cat-test-desc"
              className="cat-textarea"
              placeholder="输入待评估的模组简介文本…"
              value={testDescription}
              onChange={(e) => setTestDescription(e.target.value)}
            />
          </div>

          <div className="cat-field">
            <label className="cat-label" htmlFor="cat-test-img-urls">
              Steam CDN 预览图链接 (可选，仅支持 HTTPS 白名单域名)
            </label>
            <input
              id="cat-test-img-urls"
              className="cat-input cat-mono"
              placeholder="https://images.steamusercontent.com/ugc/..."
              value={testImageUrlsInput}
              onChange={(e) => setTestImageUrlsInput(e.target.value)}
            />
          </div>

          {testResult && (
            <div
              style={{
                display: 'flex',
                flexDirection: 'column',
                gap: 12,
                paddingTop: 10,
                borderTop: '1px solid var(--cat-border)',
              }}
            >
              <div style={{ fontWeight: 600, fontSize: 13 }}>试算输出概率明细</div>

              {(
                [
                  { label: '综合敏感概率 (prob)', value: testResult.prob },
                  { label: '擦边成人概率 (nsfwProb)', value: testResult.nsfwProb },
                  { label: '血腥重口概率 (goreProb)', value: testResult.goreProb },
                ] as const
              ).map((item) => (
                <div key={item.label} style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                  <div className="cat-row" style={{ justifyContent: 'space-between', fontSize: 12.5 }}>
                    <span>{item.label}</span>
                    <span className="cat-mono">{formatProb(item.value)}</span>
                  </div>
                  <div className="cat-progress-track">
                    <div
                      className="cat-progress-fill"
                      style={{
                        width: `${Math.min(100, Math.max(0, item.value * 100))}%`,
                        background: probBarColor(item.value),
                      }}
                    />
                  </div>
                </div>
              ))}

              <div className="cat-kv-list" style={{ paddingTop: 4 }}>
                <div className="cat-kv">
                  <span className="cat-kv-k">文本模态 (textProb)</span>
                  <span className="cat-kv-v cat-mono">{formatProb(testResult.textProb)}</span>
                </div>
                <div className="cat-kv">
                  <span className="cat-kv-k">标题分项 (titleProb)</span>
                  <span className="cat-kv-v cat-mono">{formatProb(testResult.titleProb)}</span>
                </div>
                <div className="cat-kv">
                  <span className="cat-kv-k">简介分项 (descriptionProb)</span>
                  <span className="cat-kv-v cat-mono">{formatProb(testResult.descriptionProb)}</span>
                </div>
                <div className="cat-kv">
                  <span className="cat-kv-k">图像模态 (imageProb)</span>
                  <span className="cat-kv-v cat-mono">{formatProb(testResult.imageProb)}</span>
                </div>
              </div>

              {testResult.perImageProbs && testResult.perImageProbs.length > 0 && (
                <div className="cat-kv">
                  <span className="cat-kv-k">逐张预览图概率 (perImageProbs)</span>
                  <span className="cat-kv-v cat-mono">
                    {testResult.perImageProbs.map((v, idx) => `#${idx + 1}: ${formatProb(v)}`).join(' · ')}
                  </span>
                </div>
              )}
            </div>
          )}
        </section>
      </div>
    </>
  );
}
