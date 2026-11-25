import type {
  AddonBatchReport,
  AddonNamePreference,
  AddonSummary,
  RenameRuleConfig,
} from '../api/ipc';

const MAX_U64 = 18446744073709551615n;

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const exp = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** exp;
  return `${value >= 100 || exp === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[exp]}`;
}

export function formatSpeed(bytesPerSec: number): string {
  if (!Number.isFinite(bytesPerSec) || bytesPerSec <= 0) return '0 B/s';
  return `${formatBytes(bytesPerSec)}/s`;
}

export function formatCompactNumber(value?: number): string {
  if (value === undefined || value === null || !Number.isFinite(value)) return '—';
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return String(value);
}

export function formatTimestamp(value?: number | string | null): string {
  if (value === undefined || value === null || value === '') return '—';
  const date =
    typeof value === 'number'
      ? new Date(value > 1e12 ? value : value * 1000)
      : new Date(value);
  if (Number.isNaN(date.getTime())) return '—';
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, '0');
  const d = String(date.getDate()).padStart(2, '0');
  const hh = String(date.getHours()).padStart(2, '0');
  const mm = String(date.getMinutes()).padStart(2, '0');
  return `${y}-${m}-${d} ${hh}:${mm}`;
}

function normalizePositiveU64(digits: string): string | null {
  if (!/^\d+$/.test(digits)) return null;
  try {
    const val = BigInt(digits);
    if (val <= 0n || val > MAX_U64) return null;
    return val.toString();
  } catch {
    return null;
  }
}

export function extractWorkshopIdInput(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;

  const direct = normalizePositiveU64(trimmed);
  if (direct) return direct;

  const steamProtoMatch = trimmed.match(/^steam:\/\/url\/CommunityFilePage\/(\d+)\/?$/i);
  if (steamProtoMatch) {
    return normalizePositiveU64(steamProtoMatch[1]);
  }

  try {
    const parsed = new URL(trimmed);
    const id = Array.from(parsed.searchParams)
      .find(([key]) => key.toLowerCase() === 'id')?.[1].trim();
    if (id) {
      return normalizePositiveU64(id);
    }
  } catch {
    const match = trimmed.match(/[?&]id=(\d+)/i);
    if (match) {
      return normalizePositiveU64(match[1]);
    }
  }
  return null;
}

export function getFileBaseName(path?: string | null): string {
  if (!path) return '—';
  const normalized = path.replace(/\\/g, '/');
  const parts = normalized.split('/');
  return parts[parts.length - 1] || path;
}

export function stripVpkExtension(rawName: string): string {
  const trimmed = rawName.trim();
  const lower = trimmed.toLowerCase();
  const suffix = ['.vpk.disabled', '.disabled', '.vpk'].find((ext) => lower.endsWith(ext));
  return suffix ? trimmed.slice(0, -suffix.length) : trimmed;
}

const PLACEHOLDER_ADDON_TITLES = new Set([
  'addon',
  'addon_title',
  'addontitle',
  'my addon',
  'my add-on',
  'my first addon',
  'my first add-on',
  'untitled',
  'none',
  'n/a',
]);

function cleanMetadataTitle(value?: string | null): string | null {
  const trimmed = value?.trim();
  if (!trimmed) return null;
  if (PLACEHOLDER_ADDON_TITLES.has(trimmed.toLowerCase())) return null;
  if (/^workshop\s*#\d+$/i.test(trimmed)) return null;
  return trimmed;
}

export function stripLeadingBracketPrefixes(raw: string): string {
  return raw.replace(/^(?:\[[^\]]+\]\s*)+/, '').trim();
}

export function resolveAddonMetadataTitle(
  addon: AddonSummary,
  preference: 'workshop_title' | 'addon_title' = 'workshop_title',
): string {
  const wsTitle = cleanMetadataTitle(addon.workshopTitle);
  const adTitle =
    cleanMetadataTitle(addon.addonTitle) ?? cleanMetadataTitle(addon.addonInfo.addontitle);
  const rawTitle = cleanMetadataTitle(addon.title);
  const fileStem = addon.currentFileName ? stripVpkExtension(addon.currentFileName) : '';
  const hasCustomOverride = Boolean(
    rawTitle && rawTitle !== wsTitle && rawTitle !== adTitle && rawTitle !== fileStem,
  );

  const cleaned =
    preference === 'addon_title'
      ? (adTitle ?? (hasCustomOverride ? rawTitle : null) ?? wsTitle ?? rawTitle)
      : ((hasCustomOverride ? rawTitle : null) ?? wsTitle ?? adTitle ?? rawTitle);

  if (cleaned) return cleaned;
  return fileStem || addon.title;
}

export function formatAddonDisplayName(
  addon: AddonSummary,
  preference: AddonNamePreference = 'workshop_title',
): string {
  if (preference === 'file_name') {
    const fileStem = addon.currentFileName ? stripVpkExtension(addon.currentFileName) : '';
    if (fileStem) return fileStem;
    return resolveAddonMetadataTitle(addon, 'workshop_title');
  }
  return resolveAddonMetadataTitle(addon, preference);
}

const PART_OR_ASSET_MARKER_RE =
  /(?:part|pt|disc|dvd|vol|chapter|ch|ep|episode|pak|pack|assets?|resources?|models?|materials?|textures?|sounds?|media|分包|材质|模型|贴图|音效|主包|地图包|资源包|音频包)[\s._-]*\d*|第\s*[\d一二三四五六七八九十]+\s*[部章分卷包]|(?:[-_]+|\s+\()0*[1-9]\d?\)?$/i;

export function formatBundleMemberLabel(
  addon: AddonSummary,
  siblings: readonly AddonSummary[],
  preference: AddonNamePreference = 'workshop_title',
  bundleName?: string,
): string {
  const primary = formatAddonDisplayName(addon, preference);
  const key = primary.trim().toLowerCase();
  const collidingCount = siblings.reduce((acc, item) => {
    const label = formatAddonDisplayName(item, preference).trim().toLowerCase();
    return label === key ? acc + 1 : acc;
  }, 0);

  const fileStem = addon.currentFileName ? stripVpkExtension(addon.currentFileName) : '';
  const matchesGenericBundleName =
    siblings.length > 1 &&
    Boolean(bundleName && bundleName.trim().toLowerCase() === key) &&
    !PART_OR_ASSET_MARKER_RE.test(primary) &&
    PART_OR_ASSET_MARKER_RE.test(fileStem);

  if (collidingCount <= 1 && !matchesGenericBundleName) {
    return primary;
  }

  if (preference !== 'file_name') {
    const alternateTitle = (member: AddonSummary) =>
      preference === 'addon_title'
        ? cleanMetadataTitle(member.workshopTitle)
        : (cleanMetadataTitle(member.addonTitle) ??
          cleanMetadataTitle(member.addonInfo.addontitle));
    const altTitle = alternateTitle(addon);
    const altKey = altTitle?.toLowerCase();
    if (
      altTitle &&
      altKey !== key &&
      !siblings.some(
        (member) =>
          member.id !== addon.id &&
          (alternateTitle(member)?.toLowerCase() === altKey ||
            formatAddonDisplayName(member, preference).trim().toLowerCase() === altKey),
      )
    ) {
      return altTitle;
    }
  }
  if (fileStem && fileStem.toLowerCase() !== key) {
    return `${primary} · ${fileStem}`;
  }
  if (addon.workshopId) {
    return `${primary} · #${addon.workshopId}`;
  }
  return primary;
}

const WIN_INVALID_RE = /[\\/:*?"<>|]/g;
const WIN_RESERVED_RE = /^(?:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/i;

export function previewFilenameByRules(
  rules: RenameRuleConfig,
  sample: { workshopId?: string; groupName?: string; title: string },
): string {
  const parts: string[] = [];
  if (rules.enableWorkshopIdPrefix && sample.workshopId) {
    parts.push(`[${sample.workshopId}]`);
  }
  if (rules.enableGroupPrefix && sample.groupName?.trim()) {
    parts.push(`[${sample.groupName.trim()}]`);
  }
  parts.push(sample.title.trim());
  return sanitizeVpkFilename(parts.join(' '), rules);
}

export function sanitizeVpkFilename(rawName: string, rules: RenameRuleConfig): string {
  const base = stripVpkExtension(rawName);
  let out = base.replace(/\p{Cc}/gu, '');

  if (rules.cleanSpecialChars) {
    const rep =
      rules.invalidCharReplace === 'space'
        ? ' '
        : rules.invalidCharReplace === 'empty'
          ? ''
          : '_';
    out = out.replace(/[^0-9a-zA-Z_()[\] ]/gu, rep);
  } else {
    out = out.replace(WIN_INVALID_RE, '_');
  }

  if (rules.enableTrim) {
    out = out.trim();
  }
  if (rules.enableRemoveDoubleSpaces) {
    out = out.replace(/ {2,}/g, ' ');
  }
  if (rules.maxFilenameLength > 0) {
    const maxBase = Math.max(1, rules.maxFilenameLength - 4);
    const chars = Array.from(out);
    if (chars.length > maxBase) {
      out = chars.slice(0, maxBase).join('');
    }
  }
  if (rules.enableTrim) {
    out = out.trim();
  }

  const cleaned = out.replace(/^[. ]+|[. ]+$/g, '') || 'addon';
  const stem = WIN_RESERVED_RE.test(cleaned.split('.', 1)[0]) ? `_${cleaned}` : cleaned;
  return `${stem}.vpk`;
}

export interface BatchSummaryCounts {
  applied: number;
  unchanged: number;
  failed: number;
  notAttempted: number;
  hasIssue: boolean;
}

export function summarizeBatchReport(report: AddonBatchReport): BatchSummaryCounts {
  let applied = 0;
  let unchanged = 0;
  let failed = 0;
  let notAttempted = 0;

  for (const item of report.items) {
    if (item.status === 'applied') applied += 1;
    else if (item.status === 'unchanged') unchanged += 1;
    else if (item.status === 'failed') failed += 1;
    else notAttempted += 1;
  }

  return {
    applied,
    unchanged,
    failed,
    notAttempted,
    hasIssue: failed > 0 || Boolean(report.refreshError),
  };
}

export function buildWorkshopUrl(workshopId: string): string {
  return `https://steamcommunity.com/sharedfiles/filedetails/?id=${workshopId}`;
}

export function buildSteamProtocolUrl(workshopId: string): string {
  return `steam://url/CommunityFilePage/${workshopId}`;
}

export function matchesAddonSearch(addon: AddonSummary, needle: string): boolean {
  const q = needle.trim().toLowerCase();
  if (!q) return true;
  if (addon.title.toLowerCase().includes(q)) return true;
  if (addon.addonTitle?.toLowerCase().includes(q)) return true;
  if (addon.workshopTitle?.toLowerCase().includes(q)) return true;
  if (addon.author?.toLowerCase().includes(q)) return true;
  if (addon.workshopId?.toLowerCase().includes(q)) return true;
  if (addon.currentFileName?.toLowerCase().includes(q)) return true;
  if (addon.categories.some((c) => c.toLowerCase().includes(q))) return true;
  if (addon.maps.some((m) => m.code.toLowerCase().includes(q) || m.name.toLowerCase().includes(q))) {
    return true;
  }
  const desc = addon.addonInfo.addondescription ?? addon.addonInfo.description;
  if (desc?.toLowerCase().includes(q)) return true;
  return false;
}
