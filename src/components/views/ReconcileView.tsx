import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AlertTriangle,
  ArrowLeftRight,
  CheckCircle2,
  Folder,
  RefreshCw,
  ShieldCheck,
  Store,
} from 'lucide-react';
import type {
  AddonSummary,
  AppConfig,
  ReconcileAction,
  ReconcileIssue,
  ReconcileIssueKind,
} from '../../api/ipc';
import { formatTimestamp, getFileBaseName } from '../../utils/format';
import { Button, Dialog, EmptyState, FilterChip } from '../m3/primitives';

export interface ReconcileViewProps {
  issues: ReconcileIssue[];
  addons: AddonSummary[];
  config: AppConfig;
  busy: boolean;
  scanning: boolean;
  onScan: () => void;
  onResolve: (issueId: string, action: ReconcileAction) => Promise<void>;
  onOpenBypassSettings: () => void;
}

const DESTRUCTIVE_ACTIONS = new Set<ReconcileAction>([
  'keep_workshop_delete_loading',
  'adopt_workshop_update',
  'keep_local_and_regenerate_dummy',
  'remove_orphan_dummy',
]);

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
  onOpenBypassSettings,
}: ReconcileViewProps) {
  const { t } = useTranslation();
  const [pendingConfirm, setPendingConfirm] = useState<{
    issue: ReconcileIssue;
    title: string;
    action: ReconcileAction;
  } | null>(null);

  const addonsById = useMemo(() => {
    const map = new Map<string, AddonSummary>();
    for (const a of addons) map.set(a.id, a);
    return map;
  }, [addons]);

  const handleActionClick = (issue: ReconcileIssue, title: string, action: ReconcileAction) => {
    if (DESTRUCTIVE_ACTIONS.has(action)) {
      setPendingConfirm({ issue, title, action });
      return;
    }
    void onResolve(issue.id, action);
  };

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
        <Button
          variant="tonal"
          disabled={scanning}
          icon={<RefreshCw size={15} className={scanning ? 'md-spin' : undefined} />}
          onClick={onScan}
        >
          {t('nav.scan')}
        </Button>
      </header>

      <div className="l4a-view-scroll">
        {issues.length === 0 ? (
          <EmptyState
            icon={<CheckCircle2 size={28} style={{ color: 'var(--md-sys-color-primary)' }} />}
            title={t('reconcile.clean')}
          />
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 12, maxWidth: 920 }}>
            {issues.map((issue) => {
              const addon = issue.addonId ? addonsById.get(issue.addonId) : undefined;
              const title = addon?.title ?? (issue.workshopId ? `#${issue.workshopId}` : issue.id);
              const tone = issueTone(issue.kind);

              return (
                <article key={issue.id} className="l4a-section-card">
                  <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                    <div className="l4a-row-wrap" style={{ minWidth: 0, flex: 1 }}>
                      <span className="md-pill" data-tone={tone}>
                        {t(`reconcile.kinds.${issue.kind}`)}
                      </span>
                      <span className="md-title-medium l4a-truncate">{title}</span>
                      {issue.workshopId && (
                        <span className="md-body-small md-mono" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                          #{issue.workshopId}
                        </span>
                      )}
                    </div>
                    <span className="md-body-small" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                      {formatTimestamp(issue.detectedAt)}
                    </span>
                  </div>

                  {(issue.loadingPath || issue.workshopPath) && (
                    <div className="l4a-path-compare">
                      {issue.loadingPath && (
                        <span className="l4a-path-chip">
                          <Folder size={13} style={{ color: 'var(--md-sys-color-primary)', flexShrink: 0 }} />
                          <span className="md-mono l4a-truncate">
                            {getFileBaseName(issue.loadingPath)}
                          </span>
                        </span>
                      )}
                      {issue.loadingPath && issue.workshopPath && (
                        <ArrowLeftRight
                          size={14}
                          style={{ color: 'var(--md-sys-color-on-surface-variant)', flexShrink: 0 }}
                        />
                      )}
                      {issue.workshopPath && (
                        <span className="l4a-path-chip">
                          <Store size={13} style={{ color: 'var(--md-sys-color-tertiary)', flexShrink: 0 }} />
                          <span className="md-mono l4a-truncate">
                            {getFileBaseName(issue.workshopPath)}
                          </span>
                        </span>
                      )}
                    </div>
                  )}

                  <div className="l4a-row-wrap">
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
                          onClick={() => handleActionClick(issue, title, act)}
                        >
                          {t(`reconcile.actions.${act}`)}
                        </Button>
                      );
                    })}
                  </div>
                </article>
              );
            })}
          </div>
        )}
      </div>

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
          </>
        )}
      </Dialog>
    </div>
  );
}
