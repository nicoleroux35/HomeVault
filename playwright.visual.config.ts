import { defineConfig } from '@playwright/test';

/**
 * Visual-regression runner — one project per target width.
 *
 * Kept out of `playwright.config.ts` on purpose: the functional suite must not
 * multiply every spec by three viewports, and visual baselines are
 * browser-specific. This config runs the design visual spec in Chromium only.
 *
 * Usage:
 *   npx playwright test --config=playwright.visual.config.ts
 *   npx playwright test --config=playwright.visual.config.ts --update-snapshots
 */
export default defineConfig({
  testDir: './tests/e2e/design',
  testMatch: 'visual.spec.ts',
  timeout: 120_000,
  expect: {
    timeout: 15_000,
    toHaveScreenshot: {
      animations: 'disabled',
      caret: 'hide',
      // 1 % of pixels: absorbs live counters/uptime digits while still
      // catching a reflowed card, a clipped table or a missing control.
      maxDiffPixelRatio: 0.01,
    },
  },
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report-visual' }]],
  use: {
    baseURL: process.env.BASE_URL || 'http://localhost:8080',
    headless: true,
    screenshot: 'only-on-failure',
    trace: 'off',
  },
  projects: [
    { name: 'w360', use: { browserName: 'chromium', viewport: { width: 360, height: 780 } } },
    { name: 'w768', use: { browserName: 'chromium', viewport: { width: 768, height: 1024 } } },
    { name: 'w1280', use: { browserName: 'chromium', viewport: { width: 1280, height: 800 } } },
  ],
});
