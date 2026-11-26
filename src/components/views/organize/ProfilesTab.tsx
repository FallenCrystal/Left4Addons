import { useTranslation } from 'react-i18next';
import { Pencil, Play, SlidersHorizontal, Trash2 } from 'lucide-react';
import type { AddonNamePreference, AddonSummary, LoadoutProfile } from '../../../api/ipc';
import { useContentFilterContext } from '../../../state/ContentFilterContext';
import {
  formatRedactedAddonLabel,
  resolveLibraryAddonCardFilter,
} from '../../../utils/contentFilter';
import { formatAddonDisplayName } from '../../../utils/format';
import { Button, EmptyState, IconButton } from '../../m3/primitives';
import { computeProfileDiff } from './helpers';

export interface ProfilesTabProps {
  profiles: LoadoutProfile[];
  filteredProfiles: LoadoutProfile[];
  addonsById: Map<string, AddonSummary>;
  loadingAddons: AddonSummary[];
  namePreference: AddonNamePreference;
  busy: boolean;
  onInspectProfile: (profileId: string) => void;
  onEditProfile: (profile: LoadoutProfile) => void;
  onDeleteProfile: (profileId: string) => void;
  onApplyProfile: (profileId: string) => void;
}

export function ProfilesTab({
  profiles,
  filteredProfiles,
  addonsById,
  loadingAddons,
  namePreference,
  busy,
  onInspectProfile,
  onEditProfile,
  onDeleteProfile,
  onApplyProfile,
}: ProfilesTabProps) {
  const { t } = useTranslation();
  const filterCtx = useContentFilterContext();

  if (filteredProfiles.length === 0) {
    return (
      <EmptyState
        icon={<SlidersHorizontal size={26} />}
        title={profiles.length === 0 ? t('common.empty') : t('common.noResults')}
      />
    );
  }

  return (
    <div className="l4a-card-grid-wide">
      {filteredProfiles.map((profile) => {
        const rawDiff = computeProfileDiff(profile, addonsById, loadingAddons);
        const noChange = rawDiff.toEnable.length === 0 && rawDiff.toDisable.length === 0;
        const previewMembers = profile.enabledAddonIds
          .map((id) => addonsById.get(id))
          .filter((a): a is AddonSummary => Boolean(a));

        return (
          <article
            key={profile.id}
            className="md-card"
            data-clickable="true"
            style={{ padding: 16, gap: 10 }}
          >
            <button
              type="button"
              className="l4a-card-trigger"
              onClick={() => onInspectProfile(profile.id)}
            >
              <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                <span className="md-title-medium l4a-truncate" style={{ flex: 1 }}>
                  {profile.name}
                </span>
                <span className="md-pill" data-tone="primary">
                  {t('common.items', { count: profile.enabledAddonIds.length })}
                </span>
              </div>

              {profile.description && (
                <div
                  className="md-body-small"
                  style={{
                    color: 'var(--md-sys-color-on-surface-variant)',
                    marginTop: 4,
                  }}
                >
                  {profile.description}
                </div>
              )}

              <div className="md-chip-row" style={{ marginTop: 8 }}>
                {noChange ? (
                  <span className="md-pill" data-tone="surface">
                    {t('organize.profileDiffNoChange')}
                  </span>
                ) : (
                  <>
                    {rawDiff.toEnable.length > 0 && (
                      <span className="md-pill" data-tone="primary">
                        {t('organize.profileDiffEnable', { count: rawDiff.toEnable.length })}
                      </span>
                    )}
                    {rawDiff.toDisable.length > 0 && (
                      <span className="md-pill" data-tone="tertiary">
                        {t('organize.profileDiffDisable', { count: rawDiff.toDisable.length })}
                      </span>
                    )}
                  </>
                )}
                {rawDiff.missingIds.length > 0 && (
                  <span className="md-pill" data-tone="error">
                    {t('organize.profileMissing', { count: rawDiff.missingIds.length })}
                  </span>
                )}
              </div>

              {previewMembers.length > 0 && (
                <div className="l4a-member-preview">
                  {previewMembers.slice(0, 3).map((m) => {
                    const cardFilter = resolveLibraryAddonCardFilter(m, filterCtx);
                    const shouldRedact =
                      cardFilter.hiddenCard ||
                      cardFilter.redactTitle ||
                      cardFilter.blurWholeCard;
                    return (
                      <span key={m.id} className="l4a-member-chip l4a-truncate">
                        {shouldRedact
                          ? formatRedactedAddonLabel(m.workshopId, t)
                          : formatAddonDisplayName(m, namePreference)}
                      </span>
                    );
                  })}
                  {previewMembers.length > 3 && (
                    <span className="l4a-member-chip">
                      {t('organize.moreMembers', { count: previewMembers.length - 3 })}
                    </span>
                  )}
                </div>
              )}
            </button>

            <div className="l4a-addon-card-footer">
              <div className="l4a-row-wrap" style={{ gap: 4 }}>
                <IconButton
                  size="sm"
                  label={t('common.edit')}
                  onClick={() => onEditProfile(profile)}
                >
                  <Pencil size={15} />
                </IconButton>
                <IconButton
                  size="sm"
                  label={t('common.delete')}
                  disabled={busy}
                  onClick={() => onDeleteProfile(profile.id)}
                >
                  <Trash2 size={15} />
                </IconButton>
              </div>
              <Button
                variant={noChange ? 'tonal' : 'filled'}
                size="sm"
                disabled={busy}
                icon={<Play size={14} />}
                onClick={() => onApplyProfile(profile.id)}
              >
                {t('common.apply')}
              </Button>
            </div>
          </article>
        );
      })}
    </div>
  );
}
