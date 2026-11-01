/* =============================================================================
 * File          : acme.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings → Certificate automation card, end-to-end (Playwright).
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/acme.spec.ts --project=firefox
 * =============================================================================
 */

/**
 * Certificate automation (Settings → Security) — End-to-End Test Suite
 *
 * **SAFETY: this suite never starts an issuance and never changes the saved
 * settings.** It asserts what the card renders, that the status is one of the
 * documented values, and that a refused save leaves the stored configuration
 * untouched. Issuance, the challenge registry and the retry policy are covered
 * by `tests/acme_integration.rs` and the unit tests in `src/acme/`.
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

async function openAcmeCard(page: Page, tokens: Tokens) {
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

  // The card lives in the Security category, below the HTTPS/TLS card.
  const securityItem = page.locator('[data-testid="settings-nav-item"]', {
    hasText: /Security|Sécurité|Sicherheit|Seguridad/,
  });
  await securityItem.first().click();
  await page.waitForSelector('[data-testid="acme-status"]', { timeout: 15_000 });
}

async function currentSettings(page: Page, tokens: Tokens) {
  const res = await page.request.get(`${BASE}/api/settings/acme`, {
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

test('1.1 — The certificate automation card renders its controls', async ({ page }) => {
  await openAcmeCard(page, adminTokens);

  await expect(page.locator('[data-testid="acme-domain"]')).toBeVisible();
  await expect(page.locator('[data-testid="acme-contact"]')).toBeVisible();
  await expect(page.locator('[data-testid="acme-challenge"]')).toBeVisible();
  await expect(page.locator('[data-testid="acme-save"]')).toBeVisible();
  await expect(page.locator('[data-testid="acme-test"]')).toBeVisible();
  await expect(page.locator('[data-testid="acme-refresh"]')).toBeVisible();
});

test('2.1 — The status badge is one of the documented values', async ({ page }) => {
  await openAcmeCard(page, adminTokens);
  const text = (await page.locator('[data-testid="acme-status"]').innerText()).trim().toLowerCase();
  expect(['disabled', 'idle', 'pending', 'valid', 'expiring', 'error']).toContain(text);
});

test('2.2 — Automation off and never issued leaves the status at idle or disabled', async ({ page }) => {
  const settings = await currentSettings(page, adminTokens);
  await openAcmeCard(page, adminTokens);
  const text = (await page.locator('[data-testid="acme-status"]').innerText()).trim().toLowerCase();
  if (!settings.enabled) {
    // A disabled configuration can only read `disabled`.
    expect(text).toBe('disabled');
  } else {
    expect(['idle', 'pending', 'valid', 'expiring', 'error']).toContain(text);
  }
});

test('3.1 — A copy-pasted URL as the public name is refused and nothing is stored', async ({ page }) => {
  const before = await currentSettings(page, adminTokens);
  await openAcmeCard(page, adminTokens);

  await page.locator('[data-testid="acme-domain"]').fill('https://nas.example.com/admin');
  await page.locator('[data-testid="acme-save"]').click();

  // The refusal is shown on the card, and the stored domain is unchanged.
  await expect(page.locator('[data-testid="acme-notice"]')).toBeVisible({ timeout: 10_000 });
  const after = await currentSettings(page, adminTokens);
  expect(after.domain).toBe(before.domain);
});

test('4.1 — The DNS fields appear only for the DNS-01 challenge', async ({ page }) => {
  await openAcmeCard(page, adminTokens);

  await page.locator('[data-testid="acme-challenge"]').selectOption('dns-01');
  await expect(page.locator('[data-testid="acme-provider"]')).toBeVisible();

  await page.locator('[data-testid="acme-challenge"]').selectOption('http-01');
  await expect(page.locator('[data-testid="acme-provider"]')).toBeHidden();
});
