(() => {
  const root = document.documentElement;
  const fallback = { version: 3, theme: 'cupertino', colorMode: 'system', scale: 1.05 };
  try {
    const parsed = JSON.parse(localStorage.getItem('camellia-nexus.appearance.v3') || 'null');
    const valid = parsed?.version === 3
      && ['cupertino', 'material', 'aurora'].includes(parsed.theme)
      && ['system', 'light', 'dark'].includes(parsed.colorMode)
      && [0.95, 1.05, 1.15, 1.3].includes(parsed.scale);
    const appearance = valid ? parsed : fallback;
    const mode = appearance.colorMode;
    const colorScheme = mode === 'system'
      ? matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
      : mode;
    root.dataset.uiTheme = appearance.theme;
    root.dataset.uiColorMode = mode;
    root.dataset.uiColorScheme = colorScheme;
    root.style.colorScheme = colorScheme;
  } catch {
    const colorScheme = matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
    root.dataset.uiTheme = fallback.theme;
    root.dataset.uiColorMode = fallback.colorMode;
    root.dataset.uiColorScheme = colorScheme;
    root.style.colorScheme = colorScheme;
  }
})();
