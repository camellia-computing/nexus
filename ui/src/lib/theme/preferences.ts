export const THEME_IDS = ['cupertino', 'material', 'aurora'] as const;
export const COLOR_MODES = ['system', 'light', 'dark'] as const;
export const UI_SCALES = [0.95, 1.05, 1.15, 1.3] as const;

export type ThemeId = (typeof THEME_IDS)[number];
export type ColorMode = (typeof COLOR_MODES)[number];
export type UiScale = (typeof UI_SCALES)[number];
export type EffectiveColorScheme = Exclude<ColorMode, 'system'>;

export interface AppearancePreferences {
  version: 3;
  theme: ThemeId;
  colorMode: ColorMode;
  scale: UiScale;
}

export interface AppliedAppearance {
  theme: ThemeId;
  colorMode: ColorMode;
  colorScheme: EffectiveColorScheme;
  scale: UiScale;
}

export interface AppearanceStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

export const APPEARANCE_STORAGE_KEY = 'camellia-nexus.appearance.v3';

export const DEFAULT_APPEARANCE_PREFERENCES: Readonly<AppearancePreferences> = Object.freeze({
  version: 3,
  theme: 'cupertino',
  colorMode: 'system',
  scale: 1.05,
});

export function isThemeId(value: unknown): value is ThemeId {
  return typeof value === 'string' && THEME_IDS.some((theme) => theme === value);
}

export function isColorMode(value: unknown): value is ColorMode {
  return typeof value === 'string' && COLOR_MODES.some((mode) => mode === value);
}

export function isUiScale(value: unknown): value is UiScale {
  return typeof value === 'number' && UI_SCALES.some((scale) => scale === value);
}

export function isAppearancePreferences(value: unknown): value is AppearancePreferences {
  if (!isRecord(value)) return false;
  return (
    value.version === 3 &&
    isThemeId(value.theme) &&
    isColorMode(value.colorMode) &&
    isUiScale(value.scale)
  );
}

export function normalizeAppearancePreferences(value: unknown): AppearancePreferences {
  if (!isAppearancePreferences(value)) return defaultAppearancePreferences();
  return {
    version: 3,
    theme: value.theme,
    colorMode: value.colorMode,
    scale: value.scale,
  };
}

export function loadAppearancePreferences(
  storage: AppearanceStorage | null = resolveLocalStorage(),
): AppearancePreferences {
  if (!storage) return defaultAppearancePreferences();
  try {
    const serialized = storage.getItem(APPEARANCE_STORAGE_KEY);
    if (serialized === null) return defaultAppearancePreferences();
    return normalizeAppearancePreferences(JSON.parse(serialized) as unknown);
  } catch {
    return defaultAppearancePreferences();
  }
}

export function saveAppearancePreferences(
  preferences: AppearancePreferences,
  storage: AppearanceStorage | null = resolveLocalStorage(),
): boolean {
  if (!storage || !isAppearancePreferences(preferences)) return false;
  try {
    storage.setItem(APPEARANCE_STORAGE_KEY, JSON.stringify(normalizeAppearancePreferences(preferences)));
    return true;
  } catch {
    return false;
  }
}

export function resetAppearancePreferences(
  storage: AppearanceStorage | null = resolveLocalStorage(),
): AppearancePreferences {
  if (storage) {
    try {
      storage.removeItem(APPEARANCE_STORAGE_KEY);
    } catch {
      // The in-memory default still applies when persistent storage is unavailable.
    }
  }
  return defaultAppearancePreferences();
}

export function resolveColorScheme(
  mode: ColorMode,
  systemPrefersDark: boolean,
): EffectiveColorScheme {
  return mode === 'system' ? (systemPrefersDark ? 'dark' : 'light') : mode;
}

export function systemPrefersDark(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-color-scheme: dark)').matches
  );
}

export function applyAppearancePreferences(
  preferences: AppearancePreferences,
  prefersDark = systemPrefersDark(),
  root: HTMLElement | null = resolveDocumentRoot(),
): AppliedAppearance {
  const normalized = normalizeAppearancePreferences(preferences);
  const colorScheme = resolveColorScheme(normalized.colorMode, prefersDark);

  if (root) {
    root.dataset.uiTheme = normalized.theme;
    root.dataset.uiColorMode = normalized.colorMode;
    root.dataset.uiColorScheme = colorScheme;
    root.style.colorScheme = colorScheme;
  }

  return {
    theme: normalized.theme,
    colorMode: normalized.colorMode,
    colorScheme,
    scale: normalized.scale,
  };
}

export function watchSystemColorScheme(onChange: (prefersDark: boolean) => void): () => void {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') {
    return () => undefined;
  }
  const query = window.matchMedia('(prefers-color-scheme: dark)');
  const listener = (event: MediaQueryListEvent): void => onChange(event.matches);
  query.addEventListener('change', listener);
  return () => query.removeEventListener('change', listener);
}

function defaultAppearancePreferences(): AppearancePreferences {
  return { ...DEFAULT_APPEARANCE_PREFERENCES };
}

function isRecord(value: unknown): value is Record<PropertyKey, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function resolveLocalStorage(): AppearanceStorage | null {
  if (typeof window === 'undefined') return null;
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function resolveDocumentRoot(): HTMLElement | null {
  return typeof document === 'undefined' ? null : document.documentElement;
}
