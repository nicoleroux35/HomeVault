/* =============================================================================
 * File          : power.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings → Power card end-to-end tests (Playwright): covers the
 *   screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Power card (Settings → System) — End-to-End Test Suite
 *
 * The card (`Settings.svelte`, system category) manages the CPU governor,
 * host power actions and per-disk spindown (docs/guides/power.md).
 *
 * **SAFETY: this suite never triggers a real host power action.** The
 * reboot/shutdown/suspend buttons are asserted for *presence* and the
 * cancel path of their confirm dialog only — a click on "confirm" would
 * take the test host down. Governor/spindown assertions are read-only
 * (status values, presence of controls); mutating the governor on the test
 * host is also skipped — validation is covered by
 * `tests/power_integration.rs`.
 *
 * Run (all):    npx playwright test tests/e2e/settings/power.spec.ts --project=firefox
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/settings/power.spec.ts
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

async function newCtx(playwright: any): Promise<APIRequestContext> {
  return playwright.request.newContext({ baseURL: BASE });
}

async function apiLogin(ctx: APIRequestContext, username: string, password: string) {
  const res = await ctx.post('/api/auth/login', { data: { username, password } });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

async function seedBrowser(page: Page, tokens: Tokens | null, lang?: string) {
  await page.addInitScript(
    ({ t, l }) => {
      try {
        if (t) {
          localStorage.setItem('session-token', t.token);
          localStorage.setItem('csrf-token', t.csrf);
        }
        if (l) localStorage.setItem('hv-language', l);
      } catch {
        /* storage unavailable — the test will surface it */
      }
    },
    { t: tokens, l: lang },
  );
}

/** Fresh installs show the once-per-version "What's New" modal — close it. */
async function dismissWhatsNew(page: Page) {
  const modal = page.locator('[data-testid="whats-new-modal"]');
  if (await modal.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await modal.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

async function openPowerCard(page: Page, tokens: Tokens, lang?: string) {
  await seedBrowser(page, tokens, lang);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await dismissWhatsNew(page);
  // Settings lives in the dock (not the launcher) — same entry as
  // settings.spec.ts, whose dock-icon selector is the stable one.
  const dockBtn = page.locator('.hv-dock-item:has(.hv-dock-icon-settings)').first();
  await dockBtn.scrollIntoViewIfNeeded().catch(() => {});
  try {
    await dockBtn.click({ timeout: 5_000 });
  } catch {
    await dockBtn.click({ force: true });
  }
  await page.waitForSelector('[data-testid="settings-page"]', { timeout: 15_000 });
  await page.locator('[data-testid="settings-nav-item"]').first().waitFor({ state: 'visible', timeout: 15_000 });
  // Switch to the System category (nav items share one testid; pick by label).
  const systemItem = page.locator('[data-testid="settings-nav-item"]', {
    hasText: /System|Système/,
  });
  await systemItem.first().click();
  await page.waitForSelector('[data-testid="power-actions"]', { timeout: 15_000 });
}

let adminTokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
});

test('1.1 — The Power card renders the three host actions', async ({ page }) => {
  await openPowerCard(page, adminTokens);
  for (const action of ['reboot', 'shutdown', 'suspend']) {
    await expect(page.locator(`[data-testid="power-${action}"]`)).toBeVisible();
  }
});

test('2.1 — Power actions ask for confirmation and cancel keeps the host alive', async ({ page }) => {
  await openPowerCard(page, adminTokens);
  await page.locator('[data-testid="power-shutdown"]').click();
  // The confirm dialog appears. NOTE: the Settings *window frame* is itself
  // a [role=dialog] — scope by the confirm-dialog message testid, not by
  // role alone (the first dialog in the DOM is the window, not the prompt).
  const dialog = page.locator('[data-testid="confirm-dialog-message"]').locator('xpath=ancestor-or-self::*[@role="dialog"][1]');
  await expect(dialog).toBeVisible({ timeout: 5000 });
  // Cancel — NEVER confirm: a real shutdown would kill the test host.
  await dialog
    .locator('button:has-text("Cancel"), button:has-text("Annuler"), button:has-text("Abbrechen"), button:has-text("Cancelar")')
    .first()
    .click();
  await expect(dialog).toBeHidden({ timeout: 5000 });
  // The host is still alive: the session still works.
  const me = await page.request.get(`${BASE}/api/auth/me`, {
    headers: { 'x-session-token': adminTokens.token },
  });
  expect(me.ok()).toBeTruthy();
});

test('3.1 — Governor select shows kernel governors when supported', async ({ page }) => {
  await openPowerCard(page, adminTokens);
  const select = page.locator('[data-testid="power-governor-select"]');
  const unsupported = page.locator('text=/not available|pas disponible|nicht verfügbar|no disponible/i');
  if (await select.isVisible()) {
    const options = await select.locator('option').count();
    expect(options).toBeGreaterThan(0);
  } else {
    // VMs without cpufreq render the unsupported note instead.
    await expect(unsupported.first()).toBeVisible();
  }
});

test('4.1 — Spindown section accepts a device row and lists it', async ({ page }) => {
  await openPowerCard(page, adminTokens);
  const addRow = page.locator('[data-testid="power-spindown-add"]');
  test.skip(!(await addRow.isVisible()), 'spindown unsupported on this host (no hdparm)');

  const device = 'e2edisk';
  await addRow.locator('input[aria-label]').first().fill(device);
  await addRow.locator('button:visible').last().click();
  await expect(page.locator(`[data-testid="power-spindown-remove-${device}"]`)).toBeVisible({ timeout: 10_000 });

  // Cleanup: remove the row through the API (idempotent).
  await page.request.put(`${BASE}/api/power/spindown`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: { device },
  });
});
