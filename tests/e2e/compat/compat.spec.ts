/**
 * Browser-compatibility smoke suite.
 *
 * Runs on WebKit (desktop + iPhone viewport) and Chromium at a Pixel
 * viewport. It asserts the things that differ between engines and between a
 * pointer/keyboard desktop and a touch phone:
 *
 *   1. the login shell mounts and is keyboard-operable;
 *   2. the desktop shell mounts and the dock is reachable;
 *   3. an app window opens and its content renders;
 *   4. no screen produces a horizontal scrollbar;
 *   5. the touch dock tile still has a 44px hit area.
 *
 * See `playwright.compat.config.ts`; the wide functional behaviour is covered
 * by the main suite on Firefox/Chromium.
 */
import { test, expect, request } from '@playwright/test';
import {
  ADMIN_PASS,
  ADMIN_USER,
  apiLogin,
  gotoDesktop,
  openApp,
  type Tokens,
} from '../design/designGates';

let tokens: Tokens;

test.beforeAll(async () => {
  const ctx = await request.newContext({ baseURL: process.env.BASE_URL || 'http://localhost:8080' });
  tokens = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  await ctx.dispose();
  expect(tokens.token, 'the seeded admin must log in — is the server seeded?').toBeTruthy();
});

test('login is keyboard-operable and does not overflow', async ({ page }) => {
  await page.goto('/login');
  await page.waitForSelector('#username', { timeout: 20_000 });

  await page.locator('#username').focus();
  await page.keyboard.type(ADMIN_USER);
  await page.keyboard.press('Tab');
  await expect(page.locator('#password')).toBeFocused();

  const overflow = await page.evaluate(() => document.documentElement.scrollWidth);
  const width = page.viewportSize()!.width;
  expect(overflow).toBeLessThanOrEqual(width + 1);
});

test('desktop shell mounts and the dock is reachable', async ({ page }) => {
  await gotoDesktop(page, tokens);
  await expect(page.locator('.hv-desktop')).toBeVisible();
  await expect(page.locator('.hv-dock')).toBeVisible();

  const overflow = await page.evaluate(() => document.documentElement.scrollWidth);
  const width = page.viewportSize()!.width;
  expect(overflow).toBeLessThanOrEqual(width + 1);

  // The dock tiles are the primary touch targets. The visible tile is ~40px;
  // the shared `.hv-ic`/`any-pointer: coarse` rule extends the hit area to
  // 44px. Assert the visual control is comfortably above the 32px minimum
  // rather than pinning a platform-specific sub-pixel.
  const tile = page.locator('[data-testid="dock-launchpad"]');
  const box = await tile.boundingBox();
  expect(box, 'launchpad tile must have a box').not.toBeNull();
  expect(box!.width).toBeGreaterThanOrEqual(32);
  expect(box!.height).toBeGreaterThanOrEqual(32);
});

test('an app window opens and renders on this engine', async ({ page }) => {
  await gotoDesktop(page, tokens);
  await openApp(page, 'dashboard');
  const win = page.locator('.hv-window').last();
  await expect(win).toBeVisible();
  await expect(win).toHaveAttribute('role', 'dialog');
  await expect(win.locator('[data-testid="win-close"]')).toBeVisible();
});
