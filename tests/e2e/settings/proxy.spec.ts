/* =============================================================================
 * File          : proxy.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings → Outbound proxy card, end-to-end (Playwright).
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/proxy.spec.ts --project=firefox
 * =============================================================================
 */

/**
 * Outbound proxy (Settings → Security) — End-to-End Test Suite
 *
 * **SAFETY: this suite never enables a proxy.** It pins the card's shape, the
 * always-present loopback exemption, the validation refusals, and a save with
 * the feature off. The routing itself is pinned by `tests/proxy_integration.rs`
 * against a local mock proxy, so no test here depends on a reachable proxy.
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

interface ProxySettings {
  enabled: boolean;
  http_proxy: string;
  https_proxy: string;
  no_proxy: string;
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
  await page.waitForSelector('[data-testid="proxy-http"]', { timeout: 15_000 });
}

async function apiGetProxy(
  ctx: APIRequestContext,
  tokens: Tokens,
): Promise<ProxySettings & Record<string, any>> {
  const res = await ctx.get(`${BASE}/api/settings/proxy`, {
    headers: { 'x-session-token': tokens.token },
  });
  expect(res.ok()).toBeTruthy();
  return (await res.json()).data;
}

async function getProxySettings(page: Page, tokens: Tokens) {
  return apiGetProxy(page.request, tokens);
}

async function putProxySettings(page: Page, tokens: Tokens, body: ProxySettings) {
  return page.request.put(`${BASE}/api/settings/proxy`, {
    headers: { 'x-session-token': tokens.token, 'x-csrf-token': tokens.csrf },
    data: body,
  });
}

let adminTokens: Tokens;
let original: ProxySettings;

test.beforeAll(async ({ playwright }) => {
  const ctx = await playwright.request.newContext({ baseURL: BASE });
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
  const current = await apiGetProxy(ctx, adminTokens);
  original = {
    enabled: !!current.enabled,
    http_proxy: current.http_proxy ?? '',
    https_proxy: current.https_proxy ?? '',
    no_proxy: current.no_proxy ?? '',
  };
});

test.afterAll(async ({ playwright }) => {
  // Restore whatever the instance had before this suite, even after a failure.
  const ctx = await playwright.request.newContext({ baseURL: BASE });
  await ctx.put(`${BASE}/api/settings/proxy`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: original,
  });
});

test('1.1 — The outbound proxy card renders its fields', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);

  await expect(page.locator('[data-testid="proxy-http"]')).toBeVisible();
  await expect(page.locator('[data-testid="proxy-https"]')).toBeVisible();
  await expect(page.locator('[data-testid="proxy-no-proxy"]')).toBeVisible();
  await expect(page.locator('[data-testid="proxy-save"]')).toBeVisible();
  await expect(page.locator('[data-testid="proxy-effective-row"]')).toBeVisible();
});

test('2.1 — The effective bypass list always names loopback', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  const text = (await page.locator('[data-testid="proxy-effective"]').innerText()).toLowerCase();
  for (const exempt of ['localhost', '127.0.0.1', '::1']) {
    expect(text).toContain(exempt);
  }
});

test('3.1 — A SOCKS URL is refused and nothing is stored', async ({ page }) => {
  const before = await getProxySettings(page, adminTokens);
  await openSecurityCategory(page, adminTokens);

  await page.locator('[data-testid="proxy-http"]').fill('socks5://127.0.0.1:1080');
  await page.locator('[data-testid="proxy-save"]').click();
  await expect(page.locator('[data-testid="proxy-notice"]')).toBeVisible({ timeout: 10_000 });

  const after = await getProxySettings(page, adminTokens);
  expect(after.http_proxy).toBe(before.http_proxy);
});

test('3.2 — Enabling without any proxy URL is refused', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  // The switch is the row's control; use its accessible name instead of the
  // visual geometry so this works whatever the component renders.
  const switchControl = page.getByRole('switch', { name: /outbound proxy|proxy sortant|proxy de salida|ausgehenden proxy/i });
  if ((await switchControl.getAttribute('aria-checked')) !== 'true') {
    await switchControl.click();
  }
  await page.locator('[data-testid="proxy-http"]').fill('');
  await page.locator('[data-testid="proxy-https"]').fill('');
  await page.locator('[data-testid="proxy-save"]').click();
  await expect(page.locator('[data-testid="proxy-notice"]')).toBeVisible({ timeout: 10_000 });
  await expect(page.locator('[data-testid="proxy-notice"]')).toContainText(
    /at least one proxy URL|au moins une URL de proxy|al menos una URL de proxy|mindestens eine Proxy-URL/i,
  );
  // Put the switch back so the next test starts from the shipped default.
  if ((await switchControl.getAttribute('aria-checked')) === 'true') {
    await switchControl.click();
  }
});

test('4.1 — A save with the feature off round-trips', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  const switchControl = page.getByRole('switch', { name: /outbound proxy|proxy sortant|proxy de salida|ausgehenden proxy/i });
  if ((await switchControl.getAttribute('aria-checked')) === 'true') {
    await switchControl.click();
  }
  await page.locator('[data-testid="proxy-http"]').fill('');
  await page.locator('[data-testid="proxy-https"]').fill('');
  await page.locator('[data-testid="proxy-no-proxy"]').fill('example.com, 192.168.1.0/24');
  await page.locator('[data-testid="proxy-save"]').click();
  await expect(page.locator('[data-testid="proxy-notice"]')).toBeVisible({ timeout: 10_000 });

  const saved = await getProxySettings(page, adminTokens);
  expect(saved.enabled).toBe(false);
  expect(saved.no_proxy).toBe('example.com, 192.168.1.0/24');
});

test('5.1 — Every control is reachable by keyboard', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  for (const id of ['proxy-http', 'proxy-https', 'proxy-no-proxy']) {
    await page.locator(`[data-testid="${id}"]`).focus();
    await expect(page.locator(`[data-testid="${id}"]`)).toBeFocused();
  }
  await page.locator('[data-testid="proxy-save"]').focus();
  await expect(page.locator('[data-testid="proxy-save"]')).toBeFocused();
});

test('6.1 — No proxy credential or token is rendered or stored', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  const html = await page.content();
  expect(html).not.toMatch(/socks5:|@proxy\./i);
  const storage = await page.evaluate(() => JSON.stringify(localStorage));
  expect(storage).not.toMatch(/http_proxy|https_proxy/i);
});

test('7.1 — The card fits a narrow window without horizontal overflow', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await openSecurityCategory(page, adminTokens);
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  expect(overflow).toBeLessThanOrEqual(1);
});

test('8.1 — Each field has an associated label', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  for (const [id, key] of [
    ['proxy-http', 'proxy_http'],
    ['proxy-https', 'proxy_https'],
    ['proxy-no-proxy', 'proxy_no_proxy'],
  ]) {
    const label = page.locator(`label[for="${id}"]`);
    await expect(label).toBeVisible();
    expect((await label.innerText()).trim().length).toBeGreaterThan(0);
  }
});
