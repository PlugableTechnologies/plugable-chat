import { defineConfig, devices } from '@playwright/test';

// The Tauri backend is replaced by e2e/support/tauri-mock.ts, so these tests need only the Vite
// dev server and a browser: no Rust build, no Windows, no GPU. See docs/testing-ladder.md.
// PW_CHANNEL=chrome uses an installed Chrome instead of the downloaded Chromium (local runs).
export default defineConfig({
  testDir: './e2e',
  timeout: 30_000,
  expect: { timeout: 5_000 },
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: process.env.CI ? [['list'], ['junit', { outputFile: 'e2e-results/junit.xml' }]] : 'list',
  outputDir: 'e2e-results/artifacts',
  use: {
    baseURL: 'http://localhost:1420',
    viewport: { width: 1280, height: 800 },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'], channel: process.env.PW_CHANNEL || undefined } }],
  webServer: {
    // vite.config.ts pins port 1420 with strictPort, which is where the Tauri window loads from too.
    command: 'npx vite',
    url: 'http://localhost:1420',
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
});
