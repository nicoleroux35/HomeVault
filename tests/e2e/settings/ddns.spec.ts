/* =============================================================================
 * File          : ddns.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings → Dynamic DNS and → Reachability cards, end-to-end (Playwright).
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/ddns.spec.ts --project=firefox
 * =============================================================================
 */

/**
 * Dynamic DNS + Reachability (Settings → Security) — End-to-End Test Suite
 *
 * **SAFETY: this suite never contacts a provider and never enables an
 * outbound update.** It asserts the cards' shape, that the provider token is
 * never rendered, and that "Update now" stays disabled while the feature is
 * off. The update requests themselves are pinned in `src/ddns.rs`'s unit
 * tests, and the endpoint contract in `tests/ddns_integration.rs`.
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

interface Tokens {
  username: string;
  password: string;
  token: string;
  csrf: string;
}

async function apiLogin(ctx: APIRequestContext, username: string, password: string) {
  const res = await ctx.post('/api/auth/login', { data: { username, password } });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

async function seedBrowser(page: Page, tokens: Tokens) {
  await page.addInitScript(
    ({ t }) => {
      try {
        localStorage.setItem('session-token', t.token);
        localStorage.setItem('csrf-token', t.csrf);
      } catch {
        /* storage unavailable — the test will surface it */
      }
    },
    { t: tokens },
  );
}

async function dismissWhatsNew(page: Page) {
  const modal = page.locator('[data-testid="whats-new-modal"]');
  if (await modal.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await modal.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

async function openSecurityCategory(page: Page, tokens: Tokens) {
  await seedBrowser(page, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await dismissWhatsNew(page);

  const dockBtn = page.locator('.hv-dock-item:has(.hv-dock-icon-settings)').first();
  await dockBtn.scrollIntoViewIfNeeded().catch(() => {});
  try {
    await dockBtn.click({ timeout: 5_000 });
  } catch {
    await dockBtn.click({ force: true });
  }
  await page.waitForSelector('[data-testid="settings-page"]', { timeout: 15_000 });
  await page.locator('[data-testid="settings-nav-item"]').first().waitFor({ state: 'visible', timeout: 15_000 });

  const securityItem = page.locator('[data-testid="settings-nav-item"]', {
    hasText: /Security|Sécurité|Sicherheit|Seguridad/,
  });
  await securityItem.first().click();
  await page.waitForSelector('[data-testid="ddns-status"]', { timeout: 15_000 });
}

async function ddnsSettings(page: Page, tokens: Tokens) {
  const res = await page.request.get(`${BASE}/api/settings/ddns`, {
    headers: { 'x-session-token': tokens.token },
  });
  expect(res.ok()).toBeTruthy();
  return (await res.json()).data;
}

let adminTokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  const ctx = await playwright.request.newContext({ baseURL: BASE });
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
});

test('1.1 — The dynamic DNS card renders its seven providers', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);

  const provider = page.locator('[data-testid="ddns-provider"]');
  await expect(provider).toBeVisible();
  const values = await provider.locator('option').evaluateAll((els) =>
    els.map((e) => (e as HTMLOptionElement).value),
  );
  expect(values).toEqual([
    'duckdns',
    'dynv6',
    'cloudflare',
    'ovh',
    'namecheap',
    'desec',
    'custom',
  ]);

  await expect(page.locator('[data-testid="ddns-hostname"]')).toBeVisible();
  await expect(page.locator('[data-testid="ddns-save"]')).toBeVisible();
});

test('1.2 — Provider-specific fields appear only for their provider', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  const provider = page.locator('[data-testid="ddns-provider"]');

  await provider.selectOption('ovh');
  await expect(page.locator('[data-testid="ddns-username"]')).toBeVisible();
  await expect(page.locator('[data-testid="ddns-domain"]')).toHaveCount(0);

  await provider.selectOption('namecheap');
  await expect(page.locator('[data-testid="ddns-domain"]')).toBeVisible();
  await expect(page.locator('[data-testid="ddns-username"]')).toHaveCount(0);

  await provider.selectOption('desec');
  await expect(page.locator('[data-testid="ddns-username"]')).toHaveCount(0);
  await expect(page.locator('[data-testid="ddns-domain"]')).toHaveCount(0);
  await expect(page.locator('[data-testid="ddns-token"]')).toBeVisible();
});

test('2.1 — The status badge is one of the documented values', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  const text = (await page.locator('[data-testid="ddns-status"]').innerText()).trim().toLowerCase();
  expect(['disabled', 'unknown', 'up-to-date', 'error']).toContain(text);
});

test('2.2 — A provider token is never rendered in clear', async ({ page }) => {
  const settings = await ddnsSettings(page, adminTokens);
  await openSecurityCategory(page, adminTokens);
  const token = page.locator('[data-testid="ddns-token"]');
  if (await token.isVisible()) {
    // The API answers "set" or ""; the real secret never leaves the server.
    expect(['set', '']).toContain(await token.inputValue());
    expect(settings.token === 'set' || settings.token === '').toBeTruthy();
  }
});

test('3.1 — "Update now" is disabled while dynamic DNS is off', async ({ page }) => {
  const settings = await ddnsSettings(page, adminTokens);
  test.skip(settings.enabled, 'dynamic DNS is enabled on this instance — refusing to send an update');
  await openSecurityCategory(page, adminTokens);
  await expect(page.locator('[data-testid="ddns-update"]')).toBeDisabled();
});

test('4.1 — A bare URL as the host name is refused and nothing is stored', async ({ page }) => {
  const before = await ddnsSettings(page, adminTokens);
  await openSecurityCategory(page, adminTokens);

  await page.locator('[data-testid="ddns-hostname"]').fill('https://nas.example.com');
  await page.locator('[data-testid="ddns-save"]').click();
  await expect(page.locator('[data-testid="ddns-notice"]')).toBeVisible({ timeout: 10_000 });

  const after = await ddnsSettings(page, adminTokens);
  expect(after.hostname).toBe(before.hostname);
});

test('5.1 — The reachability report lists ordered checks, starting with HTTPS', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);

  const list = page.locator('[data-testid="reachability-list"]');
  await list.scrollIntoViewIfNeeded();
  await expect(list).toBeVisible();
  const ids = await page
    .locator('[data-testid="reachability-check"]')
    .evaluateAll((els) => els.map((e) => e.getAttribute('data-check-id')));
  expect(ids[0]).toBe('tls.enabled');
  expect(ids.length).toBeGreaterThanOrEqual(6);
});

test('5.2 — The counts add up to the number of checks', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  const checks = await page.locator('[data-testid="reachability-check"]').count();
  const counts = await page.locator('[data-testid="reachability-counts"]').innerText();
  const numbers = counts.match(/\d+/g)?.map(Number) ?? [];
  expect(numbers.reduce((a, b) => a + b, 0)).toBe(checks);
});
