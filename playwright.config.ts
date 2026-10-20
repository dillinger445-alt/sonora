import { defineConfig, devices } from '@playwright/test';
import { BRIDGE_PORT } from './e2e/paths.ts';

/**
 * End-to-end tests: the built interface (`npm run build` → dist/) served by
 * the Rust dev bridge, which runs the real analysis core on a generated
 * audio corpus. `e2e/serve.mjs` builds the bridge, generates the corpus once
 * and starts every run from an empty library.
 */
export default defineConfig({
  testDir: 'e2e',
  timeout: 90_000,
  expect: { timeout: 20_000 },
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [['github'], ['html', { open: 'never' }]] : [['list']],
  use: {
    baseURL: `http://127.0.0.1:${BRIDGE_PORT}`,
    locale: 'en-US',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    { name: 'chromium', testMatch: /sonora\.spec\.ts/, use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } },
    // The macOS app shows the interface in WebKit (Safari's engine), which
    // lays some things out differently from Chromium. Checked there after the
    // main suite, in the state it leaves.
    { name: 'webkit', testMatch: /webkit\.spec\.ts/, dependencies: ['chromium'], use: { ...devices['Desktop Safari'], viewport: { width: 1440, height: 900 } } },
  ],
  webServer: {
    command: `node e2e/serve.mjs ${BRIDGE_PORT}`,
    url: `http://127.0.0.1:${BRIDGE_PORT}/api/health`,
    reuseExistingServer: false,
    // The first run compiles the Rust core in release mode.
    timeout: 15 * 60_000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
});
