/* =============================================================================
 * File          : power.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Desktop power flow end-to-end tests (Playwright): restart / shutdown
 *   feedback after confirmation, with `POST /api/server/*` mocked so the
 *   suite never takes the host down.
 *
 * Dependencies:
 *   - none (test crate: Playwright)
 *
 * Usage:
 *   E2E: npx playwright test tests/e2e/desktop/power.spec.ts
 * =============================================================================
 */

/**
 * Desktop power flow — End-to-End Test Suite (ROADMAP 24.1)
 *
 * Before this pass the two branches after the confirmation were empty
 * (`if (response.success) { } else { }`), so a refused `systemctl` was
 * indistinguishable from a slow one and a successful restart said nothing at
 * all. The fix has to hold on three points, asserted here:
 *
 *   1. a refusal renders a `role="alert"` carrying the server's own reason
 *      and leaves the power menu usable (retry is possible);
 *   2. an accepted command renders a persistent `role="status"`
 *      ("Restarting…") before the session dies;
 *   3. the confirm button cannot be submitted twice.
 *
 * **SAFETY: every `POST /api/server/restart|shutdown` is intercepted with
 * `page.route`.** No request ever reaches the host command, so this suite is
 * safe to run on a real appliance — which is why the guard is a hard
 * assertion, not a comment: test 1 fails if a request escapes to the server.
 *
 * Run (all):    npx playwright test tests/e2e/desktop/power.spec.ts
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/desktop/power.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

interface Tokens {
  token: string;
  csrf: string;
}

async function apiLogin(ctx: APIRequestContext): Promise<Tokens> {
  const res = await ctx.post('/api/auth/login', {
    data: { username: ADMIN_USER, password: ADMIN_PASS },
  });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

/** Boots the desktop with a session seeded before the SPA loads. */
async function gotoDesktop(page: Page, tokens: Tokens) {
  await page.addInitScript((t: Tokens) => {
    try {
      localStorage.setItem('session-token', t.token);
      localStorage.setItem('csrf-token', t.csrf);
    } catch {
      /* storage unavailable — the desktop will surface it */
    }
  }, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  // The floating monitor panels overlay the desktop and would intercept the
  // topbar clicks (they are the Widgets suite's concern).
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

/** Opens the topbar power menu, picks Restart and confirms. */
async function confirmRestart(page: Page) {
  await page.locator('[data-testid="topbar-power"]').click();
  await expect(page.locator('.hv-power-dialog')).toBeVisible();
  await page.locator('.hv-power-option', { hasText: /restart|redémarr|neustart|reiniciar/i }).click();
  await expect(page.locator('.hv-power-footer')).toBeVisible({ timeout: 5000 });
  const confirm = page.locator(
    '.hv-power-footer .hv-power-btn:not(.hv-power-btn-secondary)',
  );
  await expect(confirm).toBeEnabled();
  await confirm.click();
  return confirm;
}

const notice = (page: Page) => page.locator('[data-testid="power-notice"]');

test('1 — a refused restart shows the server reason as an alert and the menu stays usable', async ({
  page,
  request,
}) => {
  const tokens = await apiLogin(request);

  // Hard safety net: only the mocked route may answer. If the component ever
  // stopped calling `/api/server/restart` the assertion below would pass
  // vacuously, so also require the mock to have been used exactly once.
  const reason = 'Server restart refused: Interactive authentication required';
  let mocked = 0;
  await page.route('**/api/server/restart', (route) => {
    mocked += 1;
    return route.fulfill({
      status: 500,
      contentType: 'application/json',
      body: JSON.stringify({ success: false, message: reason }),
    });
  });

  await gotoDesktop(page, tokens);
  await confirmRestart(page);

  const alert = notice(page);
  await expect(alert).toBeVisible();
  await expect(alert).toHaveAttribute('role', 'alert');
  await expect(alert).toContainText(reason);
  expect(mocked, 'the mocked route must absorb the power request').toBe(1);

  // Retry is possible: the topbar power menu still opens and its confirm
  // button is enabled again once the request has settled.
  await page.locator('[data-testid="topbar-power"]').click();
  await expect(page.locator('.hv-power-dialog')).toBeVisible();
  await page.locator('.hv-power-option', { hasText: /restart|redémarr|neustart|reiniciar/i }).click();
  await expect(
    page.locator('.hv-power-footer .hv-power-btn:not(.hv-power-btn-secondary)'),
  ).toBeEnabled();
  await page.locator('.hv-power-btn-secondary').click();
  await expect(page.locator('.hv-power-footer')).toHaveCount(0);
});

test('2 — an accepted restart is announced as a status, and cannot be submitted twice', async ({
  page,
  request,
}) => {
  const tokens = await apiLogin(request);

  let calls = 0;
  await page.route('**/api/server/restart', async (route) => {
    calls += 1;
    // Hold the answer so the in-flight state is observable: the confirm
    // button must be disabled for the duration.
    await new Promise((resolve) => setTimeout(resolve, 1200));
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: { message: 'Server restart initiated', status: 'restarting' },
      }),
    });
  });

  await gotoDesktop(page, tokens);
  const confirm = await confirmRestart(page);

  // In flight: disabled, so a second activation is a no-op.
  await expect(confirm).toBeDisabled();
  await confirm.click({ force: true }).catch(() => {});

  const status = notice(page);
  await expect(status).toBeVisible({ timeout: 10_000 });
  await expect(status).toHaveAttribute('role', 'status');
  await expect(status).toContainText(/restart|redémarr|neustart|reiniciar/i);

  expect(calls, 'one click, one request').toBe(1);
});

test('3 — the outcome band is dismissible with a 44px labelled control', async ({
  page,
  request,
}) => {
  const tokens = await apiLogin(request);
  await page.route('**/api/server/shutdown', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: true, data: { status: 'shutting_down' } }),
    }),
  );

  await gotoDesktop(page, tokens);
  await page.locator('[data-testid="topbar-power"]').click();
  await page.locator('.hv-power-option', { hasText: /shut\s*down|éteind|herunter|apagar/i }).click();
  await page.locator('.hv-power-footer .hv-power-btn:not(.hv-power-btn-secondary)').click();

  const band = notice(page);
  await expect(band).toBeVisible();
  await expect(band).toHaveAttribute('role', 'status');

  const dismiss = band.locator('.hv-power-banner__dismiss');
  // Accessible name from an i18n key, never `title` alone.
  const label = await dismiss.getAttribute('aria-label');
  expect(label && label.trim().length > 0, 'dismiss has an accessible name').toBeTruthy();

  // WCAG 2.5.5: the icon-only control is a 44px hit area.
  const box = await dismiss.boundingBox();
  expect(box!.width).toBeGreaterThanOrEqual(44);
  expect(box!.height).toBeGreaterThanOrEqual(44);

  // Visible focus ring, not `outline: none` with no replacement. Walk the tab
  // order instead of calling `focus()`: a programmatic focus after a mouse
  // click does not reliably match `:focus-visible` in either browser.
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  let reached = false;
  for (let i = 0; i < 40 && !reached; i++) {
    await page.keyboard.press('Tab');
    reached = await dismiss.evaluate((el) => el === document.activeElement);
  }
  expect(reached, 'the dismiss is reachable with Tab').toBeTruthy();
  const ring = await dismiss.evaluate((el) => getComputedStyle(el).boxShadow);
  expect(ring, 'keyboard focus is visible').not.toBe('none');

  await dismiss.click();
  await expect(notice(page)).toHaveCount(0);
});

test('4 — a dropped connection is never reported as a refusal', async ({ page, request }) => {
  const tokens = await apiLogin(request);

  // No HTTP answer at all: exactly what happens when the box goes down
  // before the response is flushed. The desktop must not claim the command
  // was refused — that is the bug this item is about, in reverse.
  await page.route('**/api/server/restart', (route) => route.abort('connectionreset'));

  await gotoDesktop(page, tokens);
  await confirmRestart(page);

  const band = notice(page);
  await expect(band).toBeVisible({ timeout: 10_000 });
  await expect(band).toHaveAttribute('role', 'status');
  await expect(band).not.toContainText(/refus|refused|abgelehnt|rechaz/i);
});

test('5 — the outcome band fits a phone viewport without horizontal overflow', async ({
  page,
  request,
}) => {
  const tokens = await apiLogin(request);
  const reason =
    'Server restart refused: Interactive authentication required (this message is deliberately long to exercise the wrapping)';
  await page.route('**/api/server/restart', (route) =>
    route.fulfill({
      status: 500,
      contentType: 'application/json',
      body: JSON.stringify({ success: false, message: reason }),
    }),
  );

  await page.setViewportSize({ width: 400, height: 800 });
  await gotoDesktop(page, tokens);
  await confirmRestart(page);

  const band = notice(page);
  await expect(band).toBeVisible();
  await expect(band).toContainText(reason);

  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  expect(overflow, 'a long refusal must wrap, not widen the page').toBeLessThanOrEqual(0);

  const box = await band.boundingBox();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(400);
});
