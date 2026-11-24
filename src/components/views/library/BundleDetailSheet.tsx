import { useId } from 'react';
import { useTranslation } from 'react-i18next';
import {
  ArrowRightLeft,
  ArrowUpCircle,
  Copy,
  Download,
  FolderOpen,
  Layers,
  Map as MapIcon,
  Pencil,
  Trash2,
} from 'lucide-react';
import type {
  AddonDirType,
  AddonNamePreference,
  AddonSummary,
  BundleRecord,
} from '../../../api/ipc';
import type { LiveDownloadState } from '../../../state/useAppController';
import { formatBundleMemberLabel, formatBytes } from '../../../utils/format';
import { AddonCover } from '../../m3/AddonCover';
import { Button, IconButton, SideSheet, Switch, Tabs } from '../../m3/primitives';
import {
  isRemoteUnavailable,
  resolveLibraryMetadataTitle,
  summarizeBundle,
} from './helpers';

export interface BundleDetailSheetProps {
  bundle: BundleRecord | null;
  addonsById: Map<string, AddonSummary>;
  namePreference: AddonNamePreference;
  liveDownloads: Record<string, LiveDownloadState>;
  busy: boolean;
  bundleTab: 'members' | 'maps';
  onBundleTabChange: (tab: 'members' | 'maps') => void;
  onClose: () => void;
  onSelectAddon: (addon: AddonSummary) => void;
  onToggleSingle: (addon: AddonSummary, enabled: boolean) => void;
  onBatchToggle: (addonIds: string[], enabled: boolean) => void;
  onBatchRename: (addonIds: string[]) => void;
  onBatchMove: (addonIds: string[], targetDir: AddonDirType) => void;
  onBatchDelete: (addons: AddonSummary[]) => void;
  onDownloadItem: (workshopId: string, title?: string) => void;
  onOpenInFileManager: (path: string) => void;
  onCopy: (text: string) => void;
}

export function BundleDetailSheet({
  bundle,
  addonsById,
  namePreference,
  liveDownloads,
  busy,
  bundleTab,
  onBundleTabChange,
  onClose,
  onSelectAddon,
  onToggleSingle,
  onBatchToggle,
  onBatchRename,
  onBatchMove,
  onBatchDelete,
  onDownloadItem,
  onOpenInFileManager,
  onCopy,
}: BundleDetailSheetProps) {
  const { t } = useTranslation();
  const bundleTabsId = useId();

  const members = bundle
    ? bundle.memberAddonIds
        .map((id) => addonsById.get(id))
        .filter((a): a is AddonSummary => Boolean(a))
    : [];
  const summary = bundle ? summarizeBundle(members) : null;
  const loadingIds = summary ? summary.loading.map((m) => m.id) : [];
  const workshopIds = summary ? summary.workshopInstalled.map((m) => m.id) : [];
  const hasBundleMaps = Boolean(bundle && bundle.maps.length > 0);
  const activeTab: 'members' | 'maps' = hasBundleMaps ? bundleTab : 'members';

  return (
    /* Inline Campaign Bundle Inspector */
    <SideSheet
      open={Boolean(bundle)}
      title={bundle?.name ?? ''}
      onClose={onClose}
      headerTrailing={
        bundle && summary && loadingIds.length > 0 ? (
          <Switch
            checked={summary.allLoadingEnabled}
            disabled={busy}
            label={summary.allLoadingEnabled ? t('common.enabled') : t('common.disabled')}
            onChange={(next) => onBatchToggle(loadingIds, next)}
          />
        ) : undefined
      }
      footer={
        bundle && summary ? (
          <>
            {summary.firstPath && (
              <Button
                variant="tonal"
                size="sm"
                icon={<FolderOpen size={15} />}
                onClick={() => onOpenInFileManager(summary.firstPath!)}
              >
                {t('common.openInExplorer')}
              </Button>
            )}
            {loadingIds.length > 0 && (
              <Button
                variant="text"
                size="sm"
                disabled={busy}
                icon={<Pencil size={15} />}
                onClick={() => onBatchRename(loadingIds)}
              >
                {t('library.autoRename')}
              </Button>
            )}
            {workshopIds.length > 0 && (
              <Button
                variant="tonal"
                size="sm"
                disabled={busy}
                icon={<ArrowRightLeft size={15} />}
                onClick={() => onBatchMove(workshopIds, 'loading')}
              >
                {t('library.moveToLoading')}
              </Button>
            )}
            {members.length > 0 && (
              <Button
                variant="text"
                tone="error"
                size="sm"
                disabled={busy}
                icon={<Trash2 size={15} />}
                onClick={() => onBatchDelete(members)}
              >
                {t('common.delete')}
              </Button>
            )}
          </>
        ) : undefined
      }
    >
      {bundle && summary && (
        <>
          {hasBundleMaps && (
            <Tabs
              compact
              idPrefix={bundleTabsId}
              value={activeTab}
              onChange={onBundleTabChange}
              tabs={[
                {
                  value: 'members',
                  label: t('organize.members'),
                  count: members.length,
                },
                {
                  value: 'maps',
                  label: t('detail.maps'),
                  count: bundle.maps.length,
                },
              ]}
            />
          )}

          <div
            className="md-sheet-body"
            role={hasBundleMaps ? 'tabpanel' : undefined}
            id={hasBundleMaps ? `${bundleTabsId}-panel-${activeTab}` : undefined}
            aria-labelledby={hasBundleMaps ? `${bundleTabsId}-tab-${activeTab}` : undefined}
          >
            {activeTab === 'members' ? (
              <>
                <div className="md-chip-row">
                  <span className="md-pill" data-tone="secondary">
                    <Layers size={12} />
                    {t('library.bundleParts', { count: members.length })}
                  </span>
                  <span className="md-pill" data-tone="surface">
                    {formatBytes(summary.totalSize)}
                  </span>
                  {summary.loading.length > 0 && (
                    <span
                      className="md-pill"
                      data-tone={
                        summary.allLoadingEnabled
                          ? 'primary'
                          : summary.noneLoadingEnabled
                            ? 'surface'
                            : 'tertiary'
                      }
                    >
                      {summary.allLoadingEnabled
                        ? t('organize.statusEnabled')
                        : summary.noneLoadingEnabled
                          ? t('organize.statusDisabled')
                          : t('organize.statusPartial', {
                              enabled: summary.enabledLoading.length,
                              total: summary.loading.length,
                            })}
                    </span>
                  )}
                </div>

                <div className="l4a-list">
                  {members.map((member, idx) => {
                    const installed = member.installState === 'installed';
                    const remoteUnavailable = isRemoteUnavailable(member);
                    const dl = member.workshopId ? liveDownloads[member.workshopId] : undefined;
                    const memberLabel = formatBundleMemberLabel(
                      member,
                      members,
                      namePreference,
                      bundle.name,
                    );
                    return (
                      <div
                        key={member.id}
                        className="l4a-list-row l4a-list-row-dense"
                        data-muted={installed && !member.enabled ? 'true' : 'false'}
                      >
                        <div className="l4a-picker-thumb">
                          <AddonCover
                            compact
                            src={member.coverImage}
                            fallbackSrc={member.localCoverImage}
                            title={memberLabel}
                            category={member.categories[0]}
                          />
                        </div>
                        <button
                          type="button"
                          className="l4a-card-trigger l4a-list-main"
                          onClick={() => {
                            onClose();
                            onSelectAddon(member);
                          }}
                        >
                          <div className="l4a-row-wrap" style={{ gap: 6 }}>
                            <span className="md-pill md-mono" data-tone="secondary">
                              #{idx + 1}
                            </span>
                            <span className="md-title-small l4a-truncate">{memberLabel}</span>
                            {member.updateAvailable && (
                              <span className="md-pill" data-tone="tertiary">
                                <ArrowUpCircle size={11} />
                              </span>
                            )}
                            {member.installState === 'missing_file' && (
                              <span className="md-pill" data-tone="error">
                                {t('library.missingFile')}
                              </span>
                            )}
                            {member.installState === 'uninstalled_known' && (
                              <span className="md-pill" data-tone="surface">
                                {t('scope.uninstalled')}
                              </span>
                            )}
                          </div>
                          <div className="l4a-addon-meta">
                            {member.currentFileName ? (
                              <span className="md-mono l4a-truncate">{member.currentFileName}</span>
                            ) : (
                              <span className="l4a-truncate">
                                {member.author || t('common.unknown')}
                              </span>
                            )}
                            <span>·</span>
                            <span>{formatBytes(member.fileSize)}</span>
                            {member.maps.length > 0 && (
                              <>
                                <span>·</span>
                                <span
                                  style={{
                                    display: 'inline-flex',
                                    alignItems: 'center',
                                    gap: 3,
                                  }}
                                >
                                  <MapIcon size={11} />
                                  {member.maps.length}
                                </span>
                              </>
                            )}
                          </div>
                        </button>
                        <div className="l4a-list-trailing">
                          {installed ? (
                            <>
                              {member.currentPath && (
                                <IconButton
                                  size="sm"
                                  label={t('common.openInExplorer')}
                                  onClick={() => onOpenInFileManager(member.currentPath!)}
                                >
                                  <FolderOpen size={15} />
                                </IconButton>
                              )}
                              <Switch
                                compact
                                checked={member.enabled}
                                disabled={busy}
                                label={memberLabel}
                                onChange={(next) => onToggleSingle(member, next)}
                              />
                            </>
                          ) : member.workshopId && !remoteUnavailable ? (
                            <Button
                              variant="tonal"
                              size="sm"
                              disabled={Boolean(dl)}
                              icon={<Download size={13} />}
                              onClick={() =>
                                onDownloadItem(
                                  member.workshopId!,
                                  resolveLibraryMetadataTitle(member, namePreference),
                                )
                              }
                            >
                              {dl
                                ? dl.cancelling
                                  ? t('common.cancelling')
                                  : `${dl.percent}%`
                                : t('common.download')}
                            </Button>
                          ) : null}
                        </div>
                      </div>
                    );
                  })}
                </div>
              </>
            ) : (
              <div className="l4a-list">
                {bundle.maps.map((m) => (
                  <div
                    key={m.code}
                    className="l4a-list-row"
                    style={{ minHeight: 56, cursor: 'default' }}
                  >
                    {m.imagePath && (
                      <div className="l4a-list-thumb">
                        <AddonCover
                          compact
                          src={m.imagePath}
                          title={m.name}
                          category="Map"
                        />
                      </div>
                    )}
                    <div className="l4a-list-main">
                      <span className="md-title-small">{m.name}</span>
                      <span
                        className="md-body-small md-mono"
                        style={{ color: 'var(--md-sys-color-on-surface-variant)' }}
                      >
                        {m.code}
                      </span>
                    </div>
                    <Button
                      variant="tonal"
                      size="sm"
                      icon={<Copy size={13} />}
                      title={t('detail.copyMapCode')}
                      aria-label={`${t('detail.copyMapCode')} ${m.code}`}
                      onClick={() => onCopy(m.code)}
                    >
                      {t('common.copy')}
                    </Button>
                  </div>
                ))}
              </div>
            )}
          </div>
        </>
      )}
    </SideSheet>
  );
}
