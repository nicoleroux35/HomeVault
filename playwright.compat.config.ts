import { defineConfig, devices } from '@playwright/test';

/**
 * Browser-compatibility runner.
 *
 * The functional suite runs Firefox + Chromium on a desktop viewport. This
 * one answers the two gaps the audit called out: **WebKit/Safari** (a real
 * engine, not an emulated Chromium) and **real mobile viewports**.
 *
 * It is deliberately a smoke suite (login, desktop, one app, keyboard,
 * overflow) rather than a second copy of every spec: a compatibility failure
 * is usually a mount/render/overflow problem, not a business-logic one.
 *
 * The mobile projects use Playwright's device profiles (emulated viewport,
 * touch, user agent). A physical-device pass remains a manual lab step; see
 * `docs/guides/hardware-testing.md`.
 *
 * Usage:
 *   npx playwright test --config=playwright.compat.config.ts
 *   npx playwright install webkit      # once
 */
export default defineConfig({
  testDir: './tests/e2e/compat',
  testMatch: '**/*.spec.ts',
  timeout: 90_000,
  expect: { timeout: 15_000 },
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report-compat' }]],
  use: {
    baseURL: process.env.BASE_URL || 'http://localhost:8080',
    headless: true,
    screenshot: 'only-on-failure',
    trace: process.env.CI ? 'off' : 'retain-on-failure',
  },
  projects: [
    {
      name: 'webkit-desktop',
      use: { browserName: 'webkit', viewport: { width: 1280, height: 720 } },
    },
    {
      name: 'webkit-mobile',
      use: { browserName: 'webkit', ...devices['iPhone 13'] },
    },
    {
      name: 'chromium-mobile',
      use: { browserName: 'chromium', ...devices['Pixel 7'] },
    },
  ],
});
