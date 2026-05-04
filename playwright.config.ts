import { defineConfig, devices } from '@playwright/test';

// Operon uses port 8123 for e2e (uncommon, avoids clashes with Archon and
// other apps that default to 8080). Override via OPERON_E2E_BASE_URL.
const BASE_URL = process.env.OPERON_E2E_BASE_URL ?? 'http://localhost:8123';

export default defineConfig({
  testDir: './e2e/specs',
  testMatch: '**/*.spec.ts',
  outputDir: 'test-results',
  // 120s per test: the first request to `dx serve` triggers a lazy wasm
  // build (wasm-bindgen-cli + esbuild + compile) that can take ~60s on a
  // cold target/. Subsequent runs are fast.
  timeout: 120_000,
  expect: { timeout: 10_000 },
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: [
    ['list'],
    ['html', { open: 'never' }],
  ],
  use: {
    baseURL: BASE_URL,
    headless: !process.env.OPERON_E2E_HEADED,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
    // Add firefox / webkit projects here once the smoke is green.
  ],
  // Spawn `dx serve --platform web` only when the user has not pointed
  // OPERON_E2E_BASE_URL at an already-running instance. Locally we reuse
  // an existing dev server; CI always spawns a fresh one.
  webServer: process.env.OPERON_E2E_BASE_URL
    ? undefined
    : {
        command: 'dx serve --platform web --port 8123',
        url: BASE_URL,
        reuseExistingServer: !process.env.CI,
        timeout: 120_000,
        stdout: 'pipe',
        stderr: 'pipe',
      },
});
