import type { AddonBatchReport, AddonSummary, RenameRuleConfig } from '../api/ipc';

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
    const id = parsed.searchParams.get('id')?.trim();
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
  const lower = rawName.toLowerCase();
  const suffix = ['.vpk.disabled', '.disabled', '.vpk'].find((suffix) => lower.endsWith(suffix));
  const base = suffix ? rawName.slice(0, -suffix.length) : rawName;
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
