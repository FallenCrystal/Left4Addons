import type { AddonSummary, LoadoutProfile } from '../../../api/ipc';

export type OrganizeTab = 'bundles' | 'collections' | 'groups' | 'profiles';

export type InspectedTarget =
  | { kind: 'bundle'; id: string }
  | { kind: 'collection'; id: string }
  | { kind: 'group'; id: string }
  | { kind: 'profile'; id: string }
  | null;

export interface GroupMemberSummary {
  members: AddonSummary[];
  installed: AddonSummary[];
  loading: AddonSummary[];
  enabledLoading: AddonSummary[];
  totalSize: number;
  firstCover: string | undefined;
  firstLocalCover: string | undefined;
  firstPath: string | undefined;
  allEnabled: boolean;
  noneEnabled: boolean;
}

export interface ProfileDiff {
  toEnable: AddonSummary[];
  toDisable: AddonSummary[];
  unchangedEnabled: AddonSummary[];
  missingIds: string[];
}

export interface MemberRowBadge {
  text: string;
  tone: 'primary' | 'secondary' | 'tertiary' | 'error' | 'surface';
}

export function summarizeGroupMembers(
  memberIds: string[],
  addonsById: Map<string, AddonSummary>,
): GroupMemberSummary {
  const members: AddonSummary[] = [];
  for (const id of memberIds) {
    const a = addonsById.get(id);
    if (a) members.push(a);
  }
  const installed = members.filter((a) => a.installState === 'installed');
  const loading = installed.filter((a) => a.primaryDir === 'loading');
  const enabledLoading = loading.filter((a) => a.enabled);
  const totalSize = installed.reduce((sum, a) => sum + a.fileSize, 0);
  const firstCover =
    members.find((a) => a.maps.length > 0 && a.coverImage)?.coverImage ??
    members.find((a) => a.coverImage)?.coverImage;
  const firstLocalCover =
    members.find((a) => a.maps.length > 0 && a.localCoverImage)?.localCoverImage ??
    members.find((a) => a.localCoverImage)?.localCoverImage;
  const firstPath = installed.find((a) => a.currentPath)?.currentPath;

  const allEnabled = loading.length > 0 && enabledLoading.length === loading.length;
  const noneEnabled = enabledLoading.length === 0;
  return {
    members,
    installed,
    loading,
    enabledLoading,
    totalSize,
    firstCover,
    firstLocalCover,
    firstPath,
    allEnabled,
    noneEnabled,
  };
}

export function computeProfileDiff(
  profile: LoadoutProfile,
  addonsById: Map<string, AddonSummary>,
  loadingAddons: AddonSummary[],
): ProfileDiff {
  const desired = new Set(profile.enabledAddonIds);
  const toEnable: AddonSummary[] = [];
  const toDisable: AddonSummary[] = [];
  const unchangedEnabled: AddonSummary[] = [];
  const missingIds: string[] = [];

  for (const id of desired) {
    const found = addonsById.get(id);
    if (!found || found.installState !== 'installed') {
      missingIds.push(id);
    }
  }
  for (const addon of loadingAddons) {
    const want = desired.has(addon.id);
    if (want && !addon.enabled) toEnable.push(addon);
    else if (!want && addon.enabled) toDisable.push(addon);
    else if (want && addon.enabled) unchangedEnabled.push(addon);
  }
  return { toEnable, toDisable, unchangedEnabled, missingIds };
}
