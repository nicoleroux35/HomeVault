/**
 * D3 — visual regression at three widths (360 / 768 / 1280).
 *
 * Every app window is screenshotted at a phone width, a tablet width and a
 * desktop width. A layout that only collapses at 360px (the W15 family) now
 * fails here instead of being found by eye. Baselines are committed; update
 * them intentionally with:
 *
 *   npx playwright test --config=playwright.visual.config.ts --update-snapshots
 *
 * and review the diff before committing.
 */
import { test, expect, request } from '@playwright/test';
import {
  ADMIN_PASS,
  ADMIN_USER,
  APP_IDS,
  apiLogin,
  closeLastWindow,
  gotoDesktop,
  openApp,
  type Tokens,
} from './designGates';

test.describe.configure({ mode: 'serial' });

let tokens: Tokens;

test.beforeAll(async () => {
  const ctx = await request.newContext({ baseURL: process.env.BASE_URL || 'http://localhost:8080' });
  tokens = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  await ctx.dispose();
  expect(tokens.token, 'the seeded admin must log in — is the server seeded?').toBeTruthy();
});

/** Freeze everything that animates or ticks so two runs are comparable. */
async function stabilise(page: import('@playwright/test').Page) {
  await page.addStyleTag({
    content: `
      *,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}
      [class*="-monitor"]{display:none!important;pointer-events:none!important}
    `,
  });
  await page.waitForTimeout(500);
}

/**
 * Some screens render live, unbounded data (the Journal's log rows grow as
 * the host logs). A strict pixel ratio would make those baselines flaky
 * without catching a layout regression, so they get a wider tolerance.
 */
const TOLERANCE: Partial<Record<(typeof APP_IDS)[number], number>> = {
  journal: 0.12,
};

test('desktop shell at this width', async ({ page }) => {
  await gotoDesktop(page, tokens);
  await stabilise(page);
  // The clock and the live resource gauges tick; mask them so the baseline
  // asserts layout, not the second the screenshot was taken.
  await expect(page).toHaveScreenshot('desktop.png', {
    mask: [page.locator('.hv-menubar'), page.locator('[class*="live"], [class*="gauge"]')],
    maxDiffPixelRatio: 0.01,
  });
});

for (const appId of APP_IDS) {
  test(`${appId} at this width`, async ({ page }) => {
    await gotoDesktop(page, tokens);
    await openApp(page, appId);
    await stabilise(page);
    try {
      await expect(page.locator('.hv-window').last()).toHaveScreenshot(`${appId}.png`, {
        maxDiffPixelRatio: TOLERANCE[appId] ?? 0.01,
      });
    } finally {
      await closeLastWindow(page).catch(() => {});
    }
  });
}
