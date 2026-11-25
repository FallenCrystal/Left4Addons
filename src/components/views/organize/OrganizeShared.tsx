import { useTranslation } from 'react-i18next';
import { AlertTriangle, Eye, FolderOpen, Map as MapIcon, X } from 'lucide-react';
import type { AddonNamePreference, AddonSummary } from '../../../api/ipc';
import { useContentFilterContext } from '../../../state/ContentFilterContext';
import {
  addonFilterKey,
  formatDiagnosticVerdictBadge,
  resolveCardFilterState,
} from '../../../utils/contentFilter';
import { formatAddonDisplayName, formatBytes } from '../../../utils/format';
import { AddonCover } from '../../m3/AddonCover';
import { IconButton, Switch } from '../../m3/primitives';
import type { GroupMemberSummary, MemberRowBadge } from './helpers';

export function GroupStatusPill({ summary }: { summary: GroupMemberSummary }) {
  const { t } = useTranslation();

  if (summary.loading.length === 0) {
    return (
      <span className="md-pill" data-tone="surface">
        {t('organize.statusEmpty')}
      </span>
    );
  }
  if (summary.allEnabled) {
    return (
      <span className="md-pill" data-tone="primary">
        {t('organize.statusEnabled')}
      </span>
    );
  }
  if (summary.noneEnabled) {
    return (
      <span className="md-pill" data-tone="surface">
        {t('organize.statusDisabled')}
      </span>
    );
  }
  return (
    <span className="md-pill" data-tone="tertiary">
      {t('organize.statusPartial', {
        enabled: summary.enabledLoading.length,
        total: summary.loading.length,
      })}
    </span>
  );
}

export interface OrganizeMemberRowProps {
  addon: AddonSummary;
  namePreference: AddonNamePreference;
  busy: boolean;
  displayLabel?: string;
  indexBadge?: number;
  showFileName?: boolean;
  badge?: MemberRowBadge;
  onSelectAddon: (addon: AddonSummary) => void;
  onOpenInFileManager: (path: string) => void;
  onToggleSingle: (addon: AddonSummary, enabled: boolean) => void;
  onRemove?: () => void;
}

export function OrganizeMemberRow({
  addon,
  namePreference,
  busy,
  displayLabel,
  indexBadge,
  showFileName,
  badge,
  onSelectAddon,
  onOpenInFileManager,
  onToggleSingle,
  onRemove,
}: OrganizeMemberRowProps) {
  const { t } = useTranslation();
  const { filterConfig, verdicts, exclusionsByAddonId, revealedCardIds, revealCard } =
    useContentFilterContext();

  const cardKey = addonFilterKey(addon.id);
  const verdict = verdicts[cardKey];
  const exclusion = exclusionsByAddonId.get(addon.id);
  const cardFilter = resolveCardFilterState({
    config: filterConfig,
    scope: 'library',
    verdict,
    exclusion,
    revealed: revealedCardIds.has(cardKey),
  });

  if (cardFilter.hiddenCard) {
    return null;
  }

  const installed = addon.installState === 'installed';
  const rawLabel = displayLabel ?? formatAddonDisplayName(addon, namePreference);
  const resolvedLabel = cardFilter.redactTitle
    ? addon.workshopId
      ? t('filter.redactedTitleWithId', { id: addon.workshopId })
      : t('filter.redactedTitle')
    : rawLabel;

  return (
    <div
      className="l4a-list-row l4a-list-row-dense"
      data-muted={installed && !addon.enabled ? 'true' : 'false'}
      data-blur-card={cardFilter.blurWholeCard ? 'true' : 'false'}
    >
      {cardFilter.blurWholeCard && (
        <button
          type="button"
          className="l4a-blur-card-overlay"
          onClick={(e) => {
            e.stopPropagation();
            revealCard(cardKey);
          }}
        >
          <span className="l4a-blur-card-chip">
            <Eye size={13} />
            <span>{t('filter.blurCardOverlay')}</span>
          </span>
        </button>
      )}
      <div className="l4a-picker-thumb">
        <AddonCover
          compact
          src={addon.coverImage}
          fallbackSrc={addon.localCoverImage}
          title={resolvedLabel}
          category={addon.categories[0]}
          filterMode={cardFilter.coverMode}
        />
      </div>
      <button
        type="button"
        className="l4a-card-trigger l4a-list-main"
        onClick={() => onSelectAddon(addon)}
      >
        <div className="l4a-row-wrap" style={{ gap: 6 }}>
          {indexBadge !== undefined && (
            <span className="md-pill md-mono" data-tone="secondary">
              #{indexBadge}
            </span>
          )}
          <span className="md-title-small l4a-truncate">{resolvedLabel}</span>
          {cardFilter.exclusionActive &&
            cardFilter.exclusionDisplayLevel &&
            cardFilter.exclusionDisplayLevel !== 'normal' && (
              <span className="md-pill" data-tone="secondary">
                {t(`filter.levels.${cardFilter.exclusionDisplayLevel}`)}
              </span>
            )}
          {cardFilter.exclusionInvalidated && (
            <span className="md-pill" data-tone="error">
              <AlertTriangle size={11} />
              {t('filter.exclusionInvalidPill')}
            </span>
          )}
          {cardFilter.showDiagnosticBadge && verdict && (
            <span className="md-pill" data-tone="tertiary">
              {formatDiagnosticVerdictBadge(verdict, cardFilter, {
                imageTag: t('filter.modImageShort'),
                textTag: t('filter.modTextShort'),
              })}
            </span>
          )}
          {badge && (
            <span className="md-pill" data-tone={badge.tone}>
              {badge.text}
            </span>
          )}
        </div>
        <div className="l4a-addon-meta">
          {cardFilter.redactMeta ? (
            <span className="l4a-truncate">{t('filter.redactedAuthor')}</span>
          ) : showFileName && addon.currentFileName ? (
            <span className="md-mono l4a-truncate">{addon.currentFileName}</span>
          ) : (
            <span className="l4a-truncate">{addon.author || t('common.unknown')}</span>
          )}
          <span>·</span>
          <span>{formatBytes(addon.fileSize)}</span>
          {addon.maps.length > 0 && (
            <>
              <span>·</span>
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 3 }}>
                <MapIcon size={11} />
                {addon.maps.length}
              </span>
            </>
          )}
          {addon.primaryDir && (
            <>
              <span>·</span>
              <span>
                {addon.primaryDir === 'loading'
                  ? t('library.loadingDir')
                  : t('library.workshopDir')}
              </span>
            </>
          )}
        </div>
      </button>
      <div className="l4a-list-trailing">
        {installed && addon.currentPath && (
          <IconButton
            size="sm"
            label={t('common.openInExplorer')}
            onClick={() => onOpenInFileManager(addon.currentPath!)}
          >
            <FolderOpen size={15} />
          </IconButton>
        )}
        {installed && (
          <Switch
            compact
            checked={addon.enabled}
            disabled={busy}
            label={resolvedLabel}
            onChange={(next) => onToggleSingle(addon, next)}
          />
        )}
        {onRemove && (
          <IconButton
            size="sm"
            label={t('organize.removeMember')}
            disabled={busy}
            onClick={onRemove}
          >
            <X size={15} />
          </IconButton>
        )}
      </div>
    </div>
  );
}
