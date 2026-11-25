import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Copy, ExternalLink, EyeOff, Lock, Trash2, Unlock } from 'lucide-react';
import type { AddonNamePreference, AddonSummary, BundleRecord } from '../../../api/ipc';
import { useContentFilterContext } from '../../../state/ContentFilterContext';
import {
  buildFilteredBundleMaps,
  formatRedactedAddonLabel,
  resolveBundleCoverPresentation,
  resolveLibraryAddonCardFilter,
} from '../../../utils/contentFilter';
import { formatBundleMemberLabel, formatBytes } from '../../../utils/format';
import { AddonCover } from '../../m3/AddonCover';
import { Button, IconButton, SideSheet, Switch, Tabs } from '../../m3/primitives';
import type { ExternalAddonFilter } from '../LibraryView';
import { summarizeGroupMembers } from './helpers';
import { GroupStatusPill, OrganizeMemberRow } from './OrganizeShared';

export interface BundleInspectorSheetProps {
  inspectedBundle: BundleRecord | null;
  addonsById: Map<string, AddonSummary>;
  namePreference: AddonNamePreference;
  busy: boolean;
  onClose: () => void;
  onToggleBundleLock: (bundleId: string, locked: boolean) => void;
  onDeleteBundle: (bundleId: string) => void;
  onBulkToggle: (addonIds: string[], enabled: boolean) => void;
  onToggleSingle: (addon: AddonSummary, enabled: boolean) => void;
  onSelectAddon: (addon: AddonSummary) => void;
  onOpenInFileManager: (path: string) => void;
  onOpenFilterInLibrary: (filter: ExternalAddonFilter) => void;
  onCopy: (text: string) => void;
}

export function BundleInspectorSheet({
  inspectedBundle,
  addonsById,
  namePreference,
  busy,
  onClose,
  onToggleBundleLock,
  onDeleteBundle,
  onBulkToggle,
  onToggleSingle,
  onSelectAddon,
  onOpenInFileManager,
  onOpenFilterInLibrary,
  onCopy,
}: BundleInspectorSheetProps) {
  const { t } = useTranslation();
  const [bundleSheetTab, setBundleSheetTab] = useState<'members' | 'maps'>('members');
  const bundleTabsId = useId();
  const filterCtx = useContentFilterContext();

  const allMembers = inspectedBundle
    ? inspectedBundle.memberAddonIds
        .map((id) => addonsById.get(id))
        .filter((a): a is AddonSummary => Boolean(a))
    : [];
  const visibleMembers = allMembers.filter(
    (m) => !resolveLibraryAddonCardFilter(m, filterCtx).hiddenCard,
  );
  const hiddenMemberCount = allMembers.length - visibleMembers.length;
  const visibleMemberIds = visibleMembers.map((m) => m.id);

  const bundleSummary = inspectedBundle
    ? summarizeGroupMembers(visibleMemberIds, addonsById)
    : null;
  const coverPres = inspectedBundle
    ? resolveBundleCoverPresentation(visibleMembers, filterCtx, {
        bundleCoverImage: inspectedBundle.coverImage,
      })
    : null;
  const sheetTitle = inspectedBundle
    ? coverPres?.redactTitle
      ? formatRedactedAddonLabel(visibleMembers.find((m) => m.workshopId)?.workshopId, t)
      : inspectedBundle.name
    : '';

  const filteredMaps = inspectedBundle
    ? buildFilteredBundleMaps(inspectedBundle, visibleMembers, allMembers, filterCtx)
    : [];
  const bundleToggleableIds = bundleSummary ? bundleSummary.loading.map((a) => a.id) : [];
  const hasBundleMaps = filteredMaps.length > 0;
  const activeBundleTab: 'members' | 'maps' = hasBundleMaps ? bundleSheetTab : 'members';

  return (
    /* Inline Bundle Inspector */
    <SideSheet
      open={Boolean(inspectedBundle)}
      title={sheetTitle}
      onClose={onClose}
      headerTrailing={
        inspectedBundle && (
          <IconButton
            size="sm"
            variant={inspectedBundle.locked ? 'filled' : 'tonal'}
            label={inspectedBundle.locked ? t('organize.unlock') : t('organize.lock')}
            disabled={busy}
            onClick={() => onToggleBundleLock(inspectedBundle.id, !inspectedBundle.locked)}
          >
            {inspectedBundle.locked ? <Lock size={15} /> : <Unlock size={15} />}
          </IconButton>
        )
      }
      footer={
        inspectedBundle && (
          <>
            <Button
              variant="outlined"
              size="sm"
              icon={<ExternalLink size={14} />}
              onClick={() => {
                onOpenFilterInLibrary({
                  label: sheetTitle,
                  addonIds: visibleMemberIds,
                });
                onClose();
              }}
            >
              {t('organize.filterInLibrary')}
            </Button>
            {inspectedBundle.kind === 'manual' && (
              <Button
                variant="text"
                tone="error"
                size="sm"
                disabled={busy}
                icon={<Trash2 size={14} />}
                onClick={() => {
                  onDeleteBundle(inspectedBundle.id);
                  onClose();
                }}
              >
                {t('common.delete')}
              </Button>
            )}
          </>
        )
      }
    >
      {inspectedBundle && bundleSummary && (
        <>
          {hasBundleMaps && (
            <Tabs
              compact
              idPrefix={bundleTabsId}
              value={activeBundleTab}
              onChange={setBundleSheetTab}
              tabs={[
                {
                  value: 'members',
                  label: t('organize.members'),
                  count: bundleSummary.members.length,
                },
                {
                  value: 'maps',
                  label: t('detail.maps'),
                  count: filteredMaps.length,
                },
              ]}
            />
          )}
          <div
            className="md-sheet-body"
            role={hasBundleMaps ? 'tabpanel' : undefined}
            id={hasBundleMaps ? `${bundleTabsId}-panel-${activeBundleTab}` : undefined}
            aria-labelledby={
              hasBundleMaps ? `${bundleTabsId}-tab-${activeBundleTab}` : undefined
            }
          >
            {activeBundleTab === 'members' ? (
              <>
                <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                  <div className="md-chip-row">
                    <span className="md-pill" data-tone="secondary">
                      {t(`organize.bundleKind.${inspectedBundle.kind}`)}
                    </span>
                    <GroupStatusPill summary={bundleSummary} />
                    <span className="md-pill" data-tone="surface">
                      {formatBytes(bundleSummary.totalSize)}
                    </span>
                    {hiddenMemberCount > 0 && (
                      <span className="md-pill" data-tone="surface">
                        <EyeOff size={12} />
                        {t('filter.filteredCount', { count: hiddenMemberCount })}
                      </span>
                    )}
                  </div>
                  {bundleToggleableIds.length > 0 && (
                    <Switch
                      compact
                      checked={bundleSummary.allEnabled}
                      disabled={busy}
                      label={t('organize.toggleAll')}
                      onChange={(next) => onBulkToggle(bundleToggleableIds, next)}
                    />
                  )}
                </div>

                <div className="l4a-list">
                  {bundleSummary.members.length === 0 ? (
                    <div
                      style={{
                        padding: 20,
                        textAlign: 'center',
                        color: 'var(--md-sys-color-on-surface-variant)',
                      }}
                    >
                      {t('common.empty')}
                    </div>
                  ) : (
                    bundleSummary.members.map((addon, idx) => (
                      <OrganizeMemberRow
                        key={addon.id}
                        addon={addon}
                        namePreference={namePreference}
                        busy={busy}
                        displayLabel={formatBundleMemberLabel(
                          addon,
                          bundleSummary.members,
                          namePreference,
                          inspectedBundle.name,
                        )}
                        indexBadge={idx + 1}
                        showFileName
                        onSelectAddon={onSelectAddon}
                        onOpenInFileManager={onOpenInFileManager}
                        onToggleSingle={onToggleSingle}
                      />
                    ))
                  )}
                </div>
              </>
            ) : (
              <div className="l4a-list">
                {filteredMaps.map(({ map: m, coverMode, pendingEvaluation, redactText }) => {
                  const mapTitle = redactText ? m.code : m.name;
                  return (
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
                            title={mapTitle}
                            category="Map"
                            filterMode={coverMode}
                            filterLabel={
                              pendingEvaluation ? t('filter.evaluatingMedia') : undefined
                            }
                          />
                        </div>
                      )}
                      <div className="l4a-list-main">
                        <span className="md-title-small">{mapTitle}</span>
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
                  );
                })}
              </div>
            )}
          </div>
        </>
      )}
    </SideSheet>
  );
}
