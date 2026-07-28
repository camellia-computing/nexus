import { defineConfig } from '@playwright/test';

const previewPort = 14321;
const previewOrigin = `http://[::1]:${previewPort}`;

export default defineConfig({
  testDir: './tests/e2e',
  outputDir: './test-results',
  fullyParallel: true,
  forbidOnly: true,
  retries: 0,
  workers: 3,
  reporter: 'line',
  use: {
    baseURL: previewOrigin,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  webServer: {
    command: `node node_modules/vite/bin/vite.js --host ::1 --port ${previewPort} --strictPort`,
    url: `${previewOrigin}/?__ui_preview`,
    reuseExistingServer: !process.env.CI,
    gracefulShutdown: { signal: 'SIGTERM', timeout: 1_000 },
    timeout: 120_000,
  },
});
