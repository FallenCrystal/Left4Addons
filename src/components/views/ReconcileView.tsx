import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AlertTriangle,
  ArrowLeftRight,
  CheckCircle2,
  Folder,
  FolderOpen,
  HelpCircle,
  Info,
  RefreshCw,
  ShieldCheck,
  Sparkles,
  Store,
  X,
} from 'lucide-react';
import type {
  AddonSummary,
  AppConfig,
  ReconcileAction,
  ReconcileBatchItem,
  ReconcileBatchReport,
  ReconcileIssue,
  ReconcileIssueKind,
} from '../../api/ipc';
import { formatTimestamp, getFileBaseName } from '../../utils/format';
import { Button, Checkbox, Dialog, EmptyState, FilterChip, IconButton } from '../m3/primitives';

export interface ReconcileViewProps {
  issues: ReconcileIssue[];
  addons: AddonSummary[];
  config: AppConfig;
  busy: boolean;
  scanning: boolean;
  onScan: () => void;
  onResolve: (issueId: string, action: ReconcileAction) => Promise<void>;
  onResolveBatch: (items: ReconcileBatchItem[]) => Promise<ReconcileBatchReport | null>;
  onOpenInFileManager: (path: string) => void;
  onOpenBypassSettings: () => void;
}

const DESTRUCTIVE_ACTIONS = new Set<ReconcileAction>([
  'keep_workshop_delete_loading',
  'adopt_workshop_update',
  'keep_local_and_regenerate_dummy',
  'remove_orphan_dummy',
]);

const ALL_ISSUE_KINDS: ReconcileIssueKind[] = [
  'dummy_overwritten_by_game_update',
  'new_workshop_auto_download',
  'duplicate_identical_vpk',
  'orphan_dummy',
  'stale_dummy_metadata',
  'disabled_workshop_conflict',
];

const ALL_ACTIONS: ReconcileAction[] = [
  'adopt_workshop_update',
  'keep_local_and_regenerate_dummy',
  'keep_workshop_delete_loading',
  'import_to_loading_and_create_dummy',
  'leave_in_workshop',
  'remove_orphan_dummy',
  'refresh_dummy',
  'enable_workshop_file',
  'ignore',
];

function issueTone(kind: ReconcileIssueKind): 'error' | 'tertiary' | 'secondary' {
  if (kind === 'dummy_overwritten_by_game_update' || kind === 'disabled_workshop_conflict') {
    return 'error';
  }
  if (kind === 'new_workshop_auto_download' || kind === 'duplicate_identical_vpk') {
    return 'tertiary';
  }
  return 'secondary';
}

export function ReconcileView({
  issues,
  addons,
  config,
  busy,
  scanning,
  onScan,
  onResolve,
  onResolveBatch,
  onOpenInFileManager,
  onOpenBypassSettings,
}: ReconcileViewProps) {
  const { t } = useTranslation();
  const [kindFilter, setKindFilter] = useState<ReconcileIssueKind | 'all'>('all');
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [expandedHelpIds, setExpandedHelpIds] = useState<Set<string>>(new Set());
  const [helpOpen, setHelpOpen] = useState(false);

  const [pendingConfirm, setPendingConfirm] = useState<{
    issue: ReconcileIssue;
    title: string;
    action: ReconcileAction;
  } | null>(null);

  const [pendingBatch, setPendingBatch] = useState<{
    items: ReconcileBatchItem[];
  } | null>(null);

  const addonsById = useMemo(() => {
    const map = new Map<string, AddonSummary>();
    for (const a of addons) map.set(a.id, a);
    return map;
  }, [addons]);

  const kindCounts = useMemo(() => {
    const map = new Map<ReconcileIssueKind, number>();
    for (const issue of issues) {
      map.set(issue.kind, (map.get(issue.kind) ?? 0) + 1);
    }
    return Array.from(map.entries());
  }, [issues]);

  const activeKindFilter =
    kindFilter === 'all' || kindCounts.some(([k]) => k === kindFilter) ? kindFilter : 'all';

  const visibleIssues = useMemo(
    () =>
      activeKindFilter === 'all'
        ? issues
        : issues.filter((issue) => issue.kind === activeKindFilter),
    [issues, activeKindFilter],
  );

  const existingIds = useMemo(() => new Set(issues.map((i) => i.id)), [issues]);
  const visibleSelectedIssues = useMemo(
    () => visibleIssues.filter((i) => selectedIds.has(i.id) && existingIds.has(i.id)),
    [visibleIssues, selectedIds, existingIds],
  );

  const commonActions = useMemo(() => {
    if (visibleSelectedIssues.length === 0) return [] as ReconcileAction[];
    const first = visibleSelectedIssues[0].availableActions;
    return first.filter((act) =>
      visibleSelectedIssues.every((issue) => issue.availableActions.includes(act)),
    );
  }, [visibleSelectedIssues]);

  const toggleSelect = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set<string>();
      for (const existing of prev) {
        if (existingIds.has(existing)) next.add(existing);
      }
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const toggleAllVisible = () => {
    if (visibleSelectedIssues.length === visibleIssues.length && visibleIssues.length > 0) {
      setSelectedIds(new Set());
    } else {
      setSelectedIds(new Set(visibleIssues.map((i) => i.id)));
    }
  };

  const toggleRowHelp = (id: string) => {
    setExpandedHelpIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const handleActionClick = (issue: ReconcileIssue, title: string, action: ReconcileAction) => {
    if (DESTRUCTIVE_ACTIONS.has(action)) {
      setPendingConfirm({ issue, title, action });
      return;
    }
    void onResolve(issue.id, action);
  };

  const queueBatch = (items: ReconcileBatchItem[]) => {
    if (items.length === 0) return;
    setPendingBatch({ items });
  };

  const batchSummaryGroups = useMemo(() => {
    if (!pendingBatch) return [];
    const counts = new Map<ReconcileAction, number>();
    for (const item of pendingBatch.items) {
      counts.set(item.action, (counts.get(item.action) ?? 0) + 1);
    }
    return Array.from(counts.entries());
  }, [pendingBatch]);

  const batchHasDestructive = Boolean(
    pendingBatch?.items.some((item) => DESTRUCTIVE_ACTIONS.has(item.action)),
  );

  return (
    <div className="l4a-view">
      <header className="l4a-view-header">
        <h1 className="md-headline-small">{t('reconcile.title')}</h1>
        {issues.length > 0 && (
          <span className="md-pill" data-tone="error">
            {issues.length}
          </span>
        )}
        <div className="l4a-spacer" />
        <FilterChip
          label={`${t('library.dummyBypass')} · ${t(`settings.policy.${config.gameUpdateDummyPolicy}`)}`}
          selected={config.enableDummyBypass}
          icon={<ShieldCheck size={14} />}
          onClick={onOpenBypassSettings}
        />
        {visibleIssues.length > 1 && (
          <Button
            variant="filled"
            size="sm"
            disabled={busy}
            icon={<Sparkles size={14} />}
            onClick={() =>
              queueBatch(
                visibleIssues.map((issue) => ({
                  issueId: issue.id,
                  action: issue.recommendedAction,
                })),
              )
            }
          >
            {t('reconcile.batchApplyRecommended', { count: visibleIssues.length })}
          </Button>
        )}
        <IconButton
          variant="outlined"
          size="sm"
          label={t('reconcile.helpTitle')}
          title={t('reconcile.helpTitle')}
          onClick={() => setHelpOpen(true)}
        >
          <HelpCircle size={17} />
        </IconButton>
        <Button
          variant="tonal"
          size="sm"
          disabled={scanning}
          icon={<RefreshCw size={14} className={scanning ? 'md-spin' : undefined} />}
          onClick={onScan}
        >
          {t('nav.scan')}
        </Button>
      </header>

      {kindCounts.length > 1 && (
        <div className="l4a-view-toolbar">
          <div className="md-chip-row">
            <FilterChip
              label={t('common.all')}
              count={issues.length}
              selected={activeKindFilter === 'all'}
              onClick={() => setKindFilter('all')}
            />
            {kindCounts.map(([kind, count]) => (
              <FilterChip
                key={kind}
                label={t(`reconcile.kinds.${kind}`)}
                count={count}
                selected={activeKindFilter === kind}
                onClick={() => setKindFilter((curr) => (curr === kind ? 'all' : kind))}
              />
            ))}
          </div>
        </div>
      )}

      {visibleSelectedIssues.length > 0 && (
        <div className="l4a-batch-bar">
          <span className="md-label-large">
            {t('common.selected', { count: visibleSelectedIssues.length })}
          </span>
          <Button variant="text" size="sm" onClick={toggleAllVisible}>
            {t('reconcile.selectAll')}
          </Button>
          <div className="l4a-spacer" />
          <Button
            variant="filled"
            size="sm"
            disabled={busy}
            icon={<Sparkles size={14} />}
            onClick={() =>
              queueBatch(
                visibleSelectedIssues.map((issue) => ({
                  issueId: issue.id,
                  action: issue.recommendedAction,
                })),
              )
            }
          >
            {t('reconcile.batchApplySelected', { count: visibleSelectedIssues.length })}
          </Button>
          {commonActions.map((act) => (
            <Button
              key={act}
              variant={act === 'ignore' ? 'text' : 'outlined'}
              tone={act === 'keep_workshop_delete_loading' ? 'error' : 'default'}
              size="sm"
              disabled={busy}
              title={t(`reconcile.actionHelp.${act}`)}
              onClick={() =>
                queueBatch(
                  visibleSelectedIssues.map((issue) => ({
                    issueId: issue.id,
                    action: act,
                  })),
                )
              }
            >
              {t(`reconcile.actions.${act}`)}
            </Button>
          ))}
          <IconButton
            size="sm"
            label={t('common.clear')}
            onClick={() => setSelectedIds(new Set())}
          >
            <X size={16} />
          </IconButton>
        </div>
      )}

      <div className="l4a-view-scroll">
        {issues.length === 0 ? (
          <EmptyState
            icon={<CheckCircle2 size={28} style={{ color: 'var(--md-sys-color-primary)' }} />}
            title={t('reconcile.clean')}
          />
        ) : (
          <div className="l4a-reconcile-list">
            <div className="l4a-reconcile-list-head">
              <Checkbox
                checked={
                  visibleIssues.length > 0 &&
                  visibleSelectedIssues.length === visibleIssues.length
                }
                label={t('reconcile.selectAll')}
                onChange={toggleAllVisible}
              />
              <span
                className="md-label-medium"
                style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
              >
                {t('common.items', { count: visibleIssues.length })}
              </span>
            </div>

            {visibleIssues.map((issue) => {
              const addon = issue.addonId ? addonsById.get(issue.addonId) : undefined;
              const title = addon?.title ?? (issue.workshopId ? `#${issue.workshopId}` : issue.id);
              const tone = issueTone(issue.kind);
              const selected = selectedIds.has(issue.id);
              const helpExpanded = expandedHelpIds.has(issue.id);

              return (
                <article
                  key={issue.id}
                  className="l4a-reconcile-row"
                  data-selected={selected ? 'true' : 'false'}
                >
                  <div className="l4a-reconcile-row-main">
                    <Checkbox
                      checked={selected}
                      label={title}
                      onChange={() => toggleSelect(issue.id)}
                    />
                    <div className="l4a-reconcile-row-content">
                      <div className="l4a-reconcile-row-top">
                        <span className="md-pill" data-tone={tone}>
                          {t(`reconcile.kinds.${issue.kind}`)}
                        </span>
                        <span className="md-title-small l4a-truncate">{title}</span>
                        {issue.workshopId && (
                          <span
                            className="md-body-small md-mono"
                            style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                          >
                            #{issue.workshopId}
                          </span>
                        )}
                        <span
                          className="md-body-small l4a-reconcile-time"
                          style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                        >
                          {formatTimestamp(issue.detectedAt)}
                        </span>
                      </div>

                      <div className="l4a-reconcile-row-bottom">
                        {(issue.loadingPath || issue.workshopPath) && (
                          <div className="l4a-path-compare">
                            {issue.loadingPath && (
                              <button
                                type="button"
                                className="l4a-path-chip l4a-path-chip-btn"
                                data-tooltip={t('common.openInExplorer')}
                                onClick={() => onOpenInFileManager(issue.loadingPath!)}
                              >
                                <Folder
                                  size={12}
                                  style={{ color: 'var(--md-sys-color-primary)', flexShrink: 0 }}
                                />
                                <span className="md-mono l4a-truncate">
                                  {getFileBaseName(issue.loadingPath)}
                                </span>
                                <FolderOpen size={11} style={{ opacity: 0.65, flexShrink: 0 }} />
                              </button>
                            )}
                            {issue.loadingPath && issue.workshopPath && (
                              <ArrowLeftRight
                                size={12}
                                style={{
                                  color: 'var(--md-sys-color-on-surface-variant)',
                                  flexShrink: 0,
                                }}
                              />
                            )}
                            {issue.workshopPath && (
                              <button
                                type="button"
                                className="l4a-path-chip l4a-path-chip-btn"
                                data-tooltip={t('common.openInExplorer')}
                                onClick={() => onOpenInFileManager(issue.workshopPath!)}
                              >
                                <Store
                                  size={12}
                                  style={{ color: 'var(--md-sys-color-tertiary)', flexShrink: 0 }}
                                />
                                <span className="md-mono l4a-truncate">
                                  {getFileBaseName(issue.workshopPath)}
                                </span>
                                <FolderOpen size={11} style={{ opacity: 0.65, flexShrink: 0 }} />
                              </button>
                            )}
                          </div>
                        )}

                        <div className="l4a-reconcile-actions">
                          {issue.availableActions.map((act) => {
                            const isRecommended = act === issue.recommendedAction;
                            const isIgnore = act === 'ignore';
                            const isDeleteLocal = act === 'keep_workshop_delete_loading';
                            return (
                              <Button
                                key={act}
                                size="sm"
                                variant={isRecommended ? 'filled' : isIgnore ? 'text' : 'outlined'}
                                tone={isDeleteLocal ? 'error' : 'default'}
                                disabled={busy}
                                title={t(`reconcile.actionHelp.${act}`)}
                                onClick={() => handleActionClick(issue, title, act)}
                              >
                                {t(`reconcile.actions.${act}`)}
                              </Button>
                            );
                          })}
                          <IconButton
                            size="sm"
                            active={helpExpanded}
                            label={t('reconcile.explainIssue')}
                            onClick={() => toggleRowHelp(issue.id)}
                          >
                            <HelpCircle size={15} />
                          </IconButton>
                        </div>
                      </div>
                    </div>
                  </div>

                  {helpExpanded && (
                    <div className="l4a-reconcile-explain">
                      <div className="l4a-reconcile-explain-cause">
                        <Info size={14} style={{ flexShrink: 0, marginTop: 2 }} />
                        <span className="md-body-small">{t(`reconcile.kindHelp.${issue.kind}`)}</span>
                      </div>
                      <ul className="l4a-reconcile-explain-list">
                        {issue.availableActions.map((act) => (
                          <li key={act} className="md-body-small">
                            <strong>{t(`reconcile.actions.${act}`)}</strong>
                            {act === issue.recommendedAction && (
                              <span className="md-pill" data-tone="primary" style={{ marginLeft: 6, height: 18 }}>
                                {t('reconcile.recommended')}
                              </span>
                            )}
                            <span>：{t(`reconcile.actionHelp.${act}`)}</span>
                          </li>
                        ))}
                      </ul>
                    </div>
                  )}
                </article>
              );
            })}
          </div>
        )}
      </div>

      {/* Single Destructive Confirm Dialog */}
      <Dialog
        open={Boolean(pendingConfirm)}
        title={t('reconcile.confirmTitle')}
        icon={
          <AlertTriangle
            size={20}
            style={{
              color:
                pendingConfirm?.action === 'keep_workshop_delete_loading'
                  ? 'var(--md-sys-color-error)'
                  : 'var(--md-sys-color-primary)',
            }}
          />
        }
        onClose={() => setPendingConfirm(null)}
        actions={
          pendingConfirm && (
            <>
              <Button variant="text" onClick={() => setPendingConfirm(null)}>
                {t('common.cancel')}
              </Button>
              <Button
                variant="filled"
                tone={
                  pendingConfirm.action === 'keep_workshop_delete_loading' ||
                  pendingConfirm.action === 'remove_orphan_dummy'
                    ? 'error'
                    : 'default'
                }
                disabled={busy}
                onClick={async () => {
                  const target = pendingConfirm;
                  setPendingConfirm(null);
                  await onResolve(target.issue.id, target.action);
                }}
              >
                {t(`reconcile.actions.${pendingConfirm.action}`)}
              </Button>
            </>
          )
        }
      >
        {pendingConfirm && (
          <>
            <div className="l4a-row-wrap">
              <span className="md-pill" data-tone={issueTone(pendingConfirm.issue.kind)}>
                {t(`reconcile.kinds.${pendingConfirm.issue.kind}`)}
              </span>
              <span className="md-title-medium">{pendingConfirm.title}</span>
            </div>
            {(pendingConfirm.issue.loadingPath || pendingConfirm.issue.workshopPath) && (
              <div className="l4a-path-compare">
                {pendingConfirm.issue.loadingPath && (
                  <span className="l4a-path-chip">
                    <Folder size={13} style={{ color: 'var(--md-sys-color-primary)' }} />
                    <span className="md-mono">{getFileBaseName(pendingConfirm.issue.loadingPath)}</span>
                  </span>
                )}
                {pendingConfirm.issue.workshopPath && (
                  <span className="l4a-path-chip">
                    <Store size={13} style={{ color: 'var(--md-sys-color-tertiary)' }} />
                    <span className="md-mono">{getFileBaseName(pendingConfirm.issue.workshopPath)}</span>
                  </span>
                )}
              </div>
            )}
            <div
              className="l4a-callout"
              data-tone={
                pendingConfirm.action === 'keep_workshop_delete_loading' ? 'error' : 'info'
              }
            >
              <Info size={15} style={{ flexShrink: 0, marginTop: 2 }} />
              <div className="md-body-small">
                <strong>{t('reconcile.consequence')}：</strong>
                {t(`reconcile.actionHelp.${pendingConfirm.action}`)}
              </div>
            </div>
          </>
        )}
      </Dialog>

      {/* Batch Confirm Dialog */}
      <Dialog
        open={Boolean(pendingBatch)}
        wide
        title={t('reconcile.batchConfirmTitle')}
        icon={
          <AlertTriangle
            size={20}
            style={{
              color: batchHasDestructive
                ? 'var(--md-sys-color-error)'
                : 'var(--md-sys-color-primary)',
            }}
          />
        }
        onClose={() => setPendingBatch(null)}
        actions={
          pendingBatch && (
            <>
              <Button variant="text" onClick={() => setPendingBatch(null)}>
                {t('common.cancel')}
              </Button>
              <Button
                variant="filled"
                tone={batchHasDestructive ? 'error' : 'default'}
                disabled={busy}
                onClick={async () => {
                  const target = pendingBatch;
                  setPendingBatch(null);
                  const report = await onResolveBatch(target.items);
                  if (report) {
                    setSelectedIds(new Set());
                  }
                }}
              >
                {t('common.confirm')} ({pendingBatch.items.length})
              </Button>
            </>
          )
        }
      >
        {pendingBatch && (
          <>
            <div className="md-body-medium">
              {t('reconcile.batchConfirmSummary', { count: pendingBatch.items.length })}
            </div>
            <div className="l4a-diff-table">
              {batchSummaryGroups.map(([action, count]) => (
                <div key={action} className="l4a-diff-row">
                  <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                    <span className="md-title-small">{t(`reconcile.actions.${action}`)}</span>
                    <span
                      className="md-pill"
                      data-tone={action === 'keep_workshop_delete_loading' ? 'error' : 'primary'}
                    >
                      {t('common.items', { count })}
                    </span>
                  </div>
                  <span
                    className="md-body-small"
                    style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                  >
                    {t(`reconcile.actionHelp.${action}`)}
                  </span>
                </div>
              ))}
            </div>
          </>
        )}
      </Dialog>

      {/* Reconcile Guide / Help Dialog */}
      <Dialog
        open={helpOpen}
        wide
        title={t('reconcile.helpTitle')}
        icon={<HelpCircle size={20} style={{ color: 'var(--md-sys-color-primary)' }} />}
        onClose={() => setHelpOpen(false)}
        actions={
          <Button variant="filled" onClick={() => setHelpOpen(false)}>
            {t('common.close')}
          </Button>
        }
      >
        <p className="md-body-small" style={{ margin: 0, color: 'var(--md-sys-color-on-surface-variant)' }}>
          {t('reconcile.helpIntro')}
        </p>

        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <span className="md-title-small">{t('reconcile.helpIssuesHeading')}</span>
          <div className="l4a-help-grid">
            {ALL_ISSUE_KINDS.map((kind) => (
              <div key={kind} className="l4a-help-item">
                <span className="md-pill" data-tone={issueTone(kind)} style={{ alignSelf: 'flex-start' }}>
                  {t(`reconcile.kinds.${kind}`)}
                </span>
                <span className="md-body-small" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                  {t(`reconcile.kindHelp.${kind}`)}
                </span>
              </div>
            ))}
          </div>
        </div>

        <div className="l4a-divider" />

        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <span className="md-title-small">{t('reconcile.helpActionsHeading')}</span>
          <div className="l4a-help-actions-list">
            {ALL_ACTIONS.map((act) => (
              <div key={act} className="l4a-help-action-row">
                <span
                  className="md-label-medium"
                  style={{
                    color:
                      act === 'keep_workshop_delete_loading'
                        ? 'var(--md-sys-color-error)'
                        : 'var(--md-sys-color-on-surface)',
                  }}
                >
                  {t(`reconcile.actions.${act}`)}
                </span>
                <span className="md-body-small" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                  {t(`reconcile.actionHelp.${act}`)}
                </span>
              </div>
            ))}
          </div>
        </div>
      </Dialog>
    </div>
  );
}
