/* =============================================================================
 * File          : reverse-proxy.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Generic reverse proxy settings-card end-to-end tests (Playwright, phase
 *   C2): the listener state, the empty list, validation, create/edit/toggle/
 *   delete through the UI, a public-upstream refusal, keyboard operation,
 *   responsive widths and the security checks.
 *
 *   Unlike the DNS/SNMP cards, a rule only touches SQLite and the loopback
 *   dial is never made unless someone requests the rule, so this suite runs
 *   against the real backend. It cleans up every rule it creates (and any
 *   leftover `e2e-proxy-` rule) in `afterAll`, so a failed run does not poison
 *   the next one. The listener's own port is whatever the instance was
 *   started with; the card reports either state truthfully, so no test
 *   assumes it is up.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/reverse-proxy.spec.ts
 * ============================================================================= */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const HOST_PREFIX = 'e2e-proxy-';

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

function authHeaders(tokens: Tokens) {
  return { 'x-session-token': tokens.token, 'x-csrf-token': tokens.csrf };
}

async function seedSession(page: Page, tokens: Tokens) {
  await page.addInitScript((t) => {
    try {
      localStorage.setItem('session-token', t.token);
      localStorage.setItem('csrf-token', t.csrf);
    } catch {
      /* storage unavailable — the test will surface it */
    }
  }, tokens);
}

async function gotoDesktop(page: Page) {
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
  const whatsNew = page.locator('[data-testid="whats-new-modal"]');
  if (await whatsNew.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await whatsNew.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

/** Settings is reached through the ⌘K overlay (it has no launcher tile). */
async function openSettings(page: Page, category: RegExp) {
  await gotoDesktop(page);
  await page.keyboard.press('Control+k');
  const dialog = page.getByRole('dialog').filter({ has: page.locator('input') }).first();
  await dialog.waitFor({ state: 'visible', timeout: 10_000 });
  await dialog.locator('input').first().fill('Open Settings');
  const openEntry = dialog.getByRole('option', { name: /^Open Settings/i }).first();
  await openEntry.waitFor({ state: 'visible', timeout: 10_000 });
  await openEntry.click();
  const rail = page.locator('nav[aria-label] [data-testid="settings-nav-item"]');
  await rail.first().waitFor({ state: 'visible', timeout: 20_000 });
  await rail.filter({ hasText: category }).first().click();
}

function card(page: Page) {
  return page.locator('[data-testid="reverse-proxy-card"]');
}

async function openAdd(page: Page) {
  await card(page).locator('[data-testid="reverse-proxy-add"]').click();
  await expect(page.locator('[data-testid="reverse-proxy-host"]')).toBeVisible();
}

async function fillRule(page: Page, host: string, upstream: string, path = '') {
  await page.locator('[data-testid="reverse-proxy-host"]').fill(host);
  await page.locator('[data-testid="reverse-proxy-path"]').fill(path);
  await page.locator('[data-testid="reverse-proxy-upstream"]').fill(upstream);
}

let tokens: Tokens;
let created: string[] = [];

test.describe.configure({ mode: 'serial' });

test.beforeAll(async ({ request }) => {
  tokens = await apiLogin(request);
  expect(tokens.token).toBeTruthy();

  // A previous failed run must not leave a rule behind that makes the
  // duplicate/host assertions ambiguous.
  const list = await request.get(`${BASE}/api/proxy/rules`, {
    headers: { 'x-session-token': tokens.token },
  });
  const body = await list.json().catch(() => ({}));
  for (const rule of body?.data?.rules ?? []) {
    if (String(rule.host).startsWith(HOST_PREFIX)) {
      await request.delete(`${BASE}/api/proxy/rules/${rule.id}`, { headers: authHeaders(tokens) });
    }
  }
});

test.afterAll(async ({ request }) => {
  for (const host of created) {
    const list = await request.get(`${BASE}/api/proxy/rules`, {
      headers: { 'x-session-token': tokens.token },
    });
    const body = await list.json().catch(() => ({}));
    for (const rule of body?.data?.rules ?? []) {
      if (rule.host === host) {
        await request.delete(`${BASE}/api/proxy/rules/${rule.id}`, {
          headers: authHeaders(tokens),
        });
      }
    }
  }
});

test('1.1 — the card loads and reports whether the listener is up', async ({ page }) => {
  await seedSession(page, tokens);
  await openSettings(page, /security/i);
  await expect(card(page)).toBeVisible({ timeout: 20_000 });
  const status = page.locator('[data-testid="reverse-proxy-listener-status"]');
  await expect(status).toBeVisible();
  // Either state is truthful; the card must always say one of them.
  await expect(status).toContainText(/Listening on port|Not listening on port/i);
});

test('2.1 — the empty state says what to do next', async ({ page }) => {
  await seedSession(page, tokens);
  await openSettings(page, /security/i);
  await expect(card(page)).toBeVisible({ timeout: 20_000 });
  await expect(page.locator('[data-testid="reverse-proxy-empty"]')).toContainText(
    /Add one to reach a service/i
  );
  await expect(page.locator('[data-testid="reverse-proxy-add"]')).toBeVisible();
});

test('3.1 — an invalid host shows the server\'s own sentence, not a generic failure', async ({
  page,
}) => {
  await seedSession(page, tokens);
  await openSettings(page, /security/i);
  await expect(card(page)).toBeVisible({ timeout: 20_000 });
  await openAdd(page);
  await fillRule(page, 'e2e proxy', '127.0.0.1:9000');
  await page.locator('[data-testid="reverse-proxy-save"]').click();

  const error = page.locator('[data-testid="reverse-proxy-form-error"]');
  await expect(error).toBeVisible({ timeout: 10_000 });
  await expect(error).toContainText(/host name is not a valid DNS name/i);
  // The modal stays open so the value can be corrected.
  await expect(page.locator('[data-testid="reverse-proxy-host"]')).toBeVisible();
});

test('4.1 — adding a rule lists it with its upstream', async ({ page }) => {
  const host = `${HOST_PREFIX}${Date.now().toString(36)}.lan`;
  created.push(host);
  await seedSession(page, tokens);
  await openSettings(page, /security/i);
  await expect(card(page)).toBeVisible({ timeout: 20_000 });
  await openAdd(page);
  await fillRule(page, host, '127.0.0.1:9000');
  await page.locator('[data-testid="reverse-proxy-save"]').click();

  const row = card(page)
    .locator('[data-testid="reverse-proxy-rule"]')
    .filter({ hasText: host });
  await expect(row).toBeVisible({ timeout: 10_000 });
  await expect(row.locator('[data-testid="reverse-proxy-rule-upstream"]')).toContainText(
    'http://127.0.0.1:9000'
  );
  // The gate is on by default, and the row says so.
  await expect(row).toContainText(/Sign-in required/i);
});

test('5.1 — a rule can be disabled and edited without being recreated', async ({ page }) => {
  const host = `${HOST_PREFIX}edit-${Date.now().toString(36)}.lan`;
  created.push(host);
  await seedSession(page, tokens);
  await openSettings(page, /security/i);
  await expect(card(page)).toBeVisible({ timeout: 20_000 });

  await openAdd(page);
  await fillRule(page, host, '127.0.0.1:9001', '/api');
  await page.locator('[data-testid="reverse-proxy-save"]').click();
  const row = card(page)
    .locator('[data-testid="reverse-proxy-rule"]')
    .filter({ hasText: host });
  await expect(row).toBeVisible({ timeout: 10_000 });
  await expect(row).toContainText(`${host}/api`);

  await row.locator('[data-testid="reverse-proxy-toggle"]').click();
  await expect(row).toContainText(/Disabled/i, { timeout: 10_000 });

  await row.locator('[data-testid="reverse-proxy-edit"]').click();
  await expect(page.locator('[data-testid="reverse-proxy-host"]')).toHaveValue(host);
  await expect(page.locator('[data-testid="reverse-proxy-path"]')).toHaveValue('/api');
  await page.locator('[data-testid="reverse-proxy-upstream"]').fill('127.0.0.1:9002');
  await page.locator('[data-testid="reverse-proxy-save"]').click();
  await expect(row.locator('[data-testid="reverse-proxy-rule-upstream"]')).toContainText(':9002', {
    timeout: 10_000,
  });
});

test('6.1 — deleting a rule asks, then removes it', async ({ page }) => {
  const host = `${HOST_PREFIX}del-${Date.now().toString(36)}.lan`;
  await seedSession(page, tokens);
  await openSettings(page, /security/i);
  await expect(card(page)).toBeVisible({ timeout: 20_000 });
  await openAdd(page);
  await fillRule(page, host, '127.0.0.1:9003');
  await page.locator('[data-testid="reverse-proxy-save"]').click();
  await expect(
    card(page).locator('[data-testid="reverse-proxy-rule"]').filter({ hasText: host })
  ).toBeVisible({ timeout: 10_000 });

  const row = card(page)
    .locator('[data-testid="reverse-proxy-rule"]')
    .filter({ hasText: host });
  await row.locator('[data-testid="reverse-proxy-delete"]').click();
  // The confirmation names the rule, so a misclick is visible before it acts.
  await expect(page.getByRole('dialog', { name: /delete the rule/i })).toContainText(host);
  await page.locator('[data-testid="reverse-proxy-delete-confirm"]').click();
  await expect(row).toHaveCount(0, { timeout: 10_000 });
});

test('7.1 — a public upstream is refused by the server, through the UI', async ({ page }) => {
  await seedSession(page, tokens);
  await openSettings(page, /security/i);
  await expect(card(page)).toBeVisible({ timeout: 20_000 });
  await openAdd(page);
  await fillRule(page, `${HOST_PREFIX}public.lan`, '8.8.8.8:53');
  await page.locator('[data-testid="reverse-proxy-save"]').click();
  const error = page.locator('[data-testid="reverse-proxy-form-error"]');
  await expect(error).toBeVisible({ timeout: 10_000 });
  await expect(error).toContainText(/not a private or loopback address/i);
});

test('8.1 — the editor is reachable and dismissible by keyboard', async ({ page }) => {
  await seedSession(page, tokens);
  await openSettings(page, /security/i);
  await expect(card(page)).toBeVisible({ timeout: 20_000 });

  const add = page.locator('[data-testid="reverse-proxy-add"]');
  await add.focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('[data-testid="reverse-proxy-host"]')).toBeVisible();

  // Tab reaches the first field; Escape closes the dialog (bits-ui Dialog).
  await page.locator('[data-testid="reverse-proxy-host"]').focus();
  await page.keyboard.press('Escape');
  await expect(page.locator('[data-testid="reverse-proxy-host"]')).toHaveCount(0, {
    timeout: 10_000,
  });
  // Focus returns to the control that opened it.
  await expect(add).toBeFocused();
});

test('9.1 — no horizontal overflow at 360, 768 and 1920 px', async ({ page }) => {
  await seedSession(page, tokens);
  await openSettings(page, /security/i);
  await expect(card(page)).toBeVisible({ timeout: 20_000 });

  for (const width of [360, 768, 1920]) {
    await page.setViewportSize({ width, height: 900 });
    await page.waitForTimeout(150);
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth
    );
    expect(overflow, `${width}px`).toBeLessThanOrEqual(1);
  }
});

test('10.1 — nothing sensitive is put in the URL or in localStorage', async ({ page }) => {
  const host = `${HOST_PREFIX}sec-${Date.now().toString(36)}.lan`;
  created.push(host);
  await seedSession(page, tokens);
  await openSettings(page, /security/i);
  await expect(card(page)).toBeVisible({ timeout: 20_000 });
  await openAdd(page);
  await fillRule(page, host, '127.0.0.1:9004');
  await page.locator('[data-testid="reverse-proxy-save"]').click();
  await expect(
    card(page).locator('[data-testid="reverse-proxy-rule"]').filter({ hasText: host })
  ).toBeVisible({ timeout: 10_000 });

  expect(page.url()).not.toContain(host);
  expect(page.url()).not.toContain('127.0.0.1:9004');
  const stored = await page.evaluate(() => JSON.stringify({ ...localStorage }));
  expect(stored).not.toContain(host);
  expect(stored).not.toContain('127.0.0.1:9004');
});

test('11.1 — the page raises no uncaught error while the card is used', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await seedSession(page, tokens);
  await openSettings(page, /security/i);
  await expect(card(page)).toBeVisible({ timeout: 20_000 });
  await openAdd(page);
  await page.keyboard.press('Escape');
  expect(errors).toEqual([]);
});
