import { mount } from 'svelte';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import App from './App.svelte';
import { applyAppearancePreferences, loadAppearancePreferences } from './lib/theme';
import { isNativeHost } from './runtime';
import './lib/theme/tokens.css';
import './styles.css';

async function bootstrap() {
  if (import.meta.env.MODE === 'e2e') await import('@wdio/tauri-plugin');

  const previewMode = import.meta.env.DEV && (
    new URLSearchParams(location.search).has('__ui_preview') || !isNativeHost()
  );
  if (previewMode) {
    const { installMockBackend } = await import('./testing/mockBackend');
    installMockBackend();
  }

  const appearance = loadAppearancePreferences();
  applyAppearancePreferences(appearance);
  if (!previewMode) void getCurrentWebview().setZoom(appearance.scale).catch(() => {});

  mount(App, {
    target: document.getElementById('app')!,
  });
}

void bootstrap();
