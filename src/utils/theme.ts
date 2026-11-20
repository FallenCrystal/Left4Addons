export type ThemeMode = 'system' | 'light' | 'dark';
export type ThemeSeed = 'sage' | 'ocean' | 'terracotta' | 'violet';

export interface ThemePreference {
  mode: ThemeMode;
  seed: ThemeSeed;
}

const STORAGE_KEY = 'l4a.v2.theme';

export const THEME_SEEDS: { id: ThemeSeed; swatch: string }[] = [
  { id: 'sage', swatch: '#2D6A4F' },
  { id: 'ocean', swatch: '#006399' },
  { id: 'terracotta', swatch: '#9A4522' },
  { id: 'violet', swatch: '#6750A4' },
];

export function loadThemePreference(): ThemePreference {
  if (typeof window === 'undefined') {
    return { mode: 'system', seed: 'sage' };
  }
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { mode: 'system', seed: 'sage' };
    const parsed = JSON.parse(raw) as Partial<ThemePreference>;
    const mode: ThemeMode =
      parsed.mode === 'light' || parsed.mode === 'dark' || parsed.mode === 'system'
        ? parsed.mode
        : 'system';
    const seed: ThemeSeed =
      parsed.seed === 'sage' ||
      parsed.seed === 'ocean' ||
      parsed.seed === 'terracotta' ||
      parsed.seed === 'violet'
        ? parsed.seed
        : 'sage';
    return { mode, seed };
  } catch {
    return { mode: 'system', seed: 'sage' };
  }
}

export function resolveColorScheme(mode: ThemeMode): 'light' | 'dark' {
  if (mode === 'light' || mode === 'dark') return mode;
  if (typeof window !== 'undefined' && typeof window.matchMedia === 'function') {
    return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  }
  return 'light';
}

export function applyThemePreference(pref: ThemePreference): 'light' | 'dark' {
  const resolved = resolveColorScheme(pref.mode);
  if (typeof document !== 'undefined') {
    document.documentElement.dataset.theme = resolved;
    document.documentElement.dataset.seed = pref.seed;
    document.documentElement.style.colorScheme = resolved;
  }
  if (typeof window !== 'undefined') {
    try {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(pref));
    } catch {
      // Ignore storage quota errors in restricted webviews.
    }
  }
  return resolved;
}
