export {
  APPEARANCE_STORAGE_KEY,
  COLOR_MODES,
  DEFAULT_APPEARANCE_PREFERENCES,
  THEME_IDS,
  UI_SCALES,
  applyAppearancePreferences,
  isAppearancePreferences,
  isColorMode,
  isThemeId,
  isUiScale,
  loadAppearancePreferences,
  normalizeAppearancePreferences,
  resetAppearancePreferences,
  resolveColorScheme,
  saveAppearancePreferences,
  systemPrefersDark,
  watchSystemColorScheme,
} from './preferences';

export type {
  AppearancePreferences,
  AppearanceStorage,
  AppliedAppearance,
  ColorMode,
  EffectiveColorScheme,
  ThemeId,
  UiScale,
} from './preferences';
