/* =============================================================================
 * File          : autoblock.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings → Security → Failed sign-in blocking card, end-to-end
 *   (Playwright): the policy form, the block list, a pardon and a clear (both
 *   mocked so no firewall is touched), validation, access control and
 *   accessibility.
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/autoblock.spec.ts --project=firefox
 * =============================================================================
 */

/**
 * Login auto-block (Settings → Security) — End-to-End Test Suite
 *
 * **SAFETY: no `ufw` rule is ever created and no real address is blocked.**
 * With `apply_firewall` off (the default) a ban touches only the database, and
 * even that is avoided here: the pardon/clear tests seed a ban row and exercise
 * them against RFC 5737 (documentation) addresses that no test host uses, then
 * remove it. The save tests restore the shipped defaults in `afterAll`.
 *
 * The decisions (threshold, expiry, never-ban-ourselves) live in
 * `src/auto_block.rs`'s unit tests and the blocked-login flow in
 * `tests/auto_block_integration.rs`; this file owns the screen.
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

/** Opens Settings → Security and waits for the auto-block card. */
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
  await page
    .locator('[data-testid="settings-nav-item"]')
    .first()
    .waitFor({ state: 'visible', timeout: 15_000 });

  const securityItem = page.locator('[data-testid="settings-nav-item"]', {
    hasText: /Security|Sécurité|Sicherheit|Seguridad/,
  });
  await securityItem.first().click();
  await page.waitForSelector('[data-testid="autoblock-threshold"]', { timeout: 15_000 });
}

async function autoBlockState(ctx: APIRequestContext, tokens: Tokens) {
  const res = await ctx.get(`${BASE}/api/security/auto-block`, {
    headers: { 'x-session-token': tokens.token },
  });
  expect(res.ok()).toBeTruthy();
  return (await res.json()).data;
}

/** Restore the shipped defaults, so a run never leaves blocking switched on. */
async function restoreDefaults(ctx: APIRequestContext, tokens: Tokens) {
  await ctx
    .put(`${BASE}/api/security/auto-block`, {
      headers: { 'x-session-token': tokens.token, 'x-csrf-token': tokens.csrf },
      data: {
        enabled: false,
        threshold: 5,
        window_seconds: 300,
        ban_seconds: 1800,
        apply_firewall: false,
        dos_enabled: true,
        dos_requests_per_minute: 600,
      },
    })
    .catch(() => {});
  // Lift every seed row a previous run may have left behind.
  await ctx
    .post(`${BASE}/api/security/auto-block/clear`, {
      headers: { 'x-session-token': tokens.token, 'x-csrf-token': tokens.csrf },
    })
    .catch(() => {});
}

let adminTokens: Tokens;
let sharedCtx: APIRequestContext;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await playwright.request.newContext({ baseURL: BASE });
  const { token, csrf } = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
  await restoreDefaults(sharedCtx, adminTokens);
});

test.afterAll(async () => {
  await restoreDefaults(sharedCtx, adminTokens);
  await sharedCtx.dispose();
});

// ===========================================================================
// 1. Shape
// ===========================================================================

test('1.1 — the card renders the policy, the block list and the DoS row', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);

  await expect(page.locator('[data-testid="autoblock-threshold"]')).toBeVisible();
  await expect(page.locator('[data-testid="autoblock-window"]')).toBeVisible();
  await expect(page.locator('[data-testid="autoblock-ban"]')).toBeVisible();
  await expect(page.locator('[data-testid="autoblock-save"]')).toBeVisible();
  await expect(page.locator('[data-testid="autoblock-refresh"]')).toBeVisible();
  // The list container always exists; emptiness is its own state.
  const bans = page.locator('[data-testid="autoblock-bans"]');
  const empty = page.locator('[data-testid="autoblock-empty"]');
  expect((await bans.count()) + (await empty.count())).toBeGreaterThan(0);
});

test('1.2 — the shipped defaults are the ones the server reports', async ({ page }) => {
  const state = await autoBlockState(sharedCtx, adminTokens);
  test.skip(state.active > 0, 'this instance has live bans — refusing to assume defaults');
  await openSecurityCategory(page, adminTokens);

  await expect(page.locator('[data-testid="autoblock-threshold"]')).toHaveValue(
    String(state.settings.threshold),
  );
  await expect(page.locator('[data-testid="autoblock-window"]')).toHaveValue(
    String(state.settings.window_seconds),
  );
  await expect(page.locator('[data-testid="autoblock-ban"]')).toHaveValue(
    String(state.settings.ban_seconds),
  );
});

test('1.3 — the empty state says what has to happen for a block to appear', async ({ page }) => {
  const state = await autoBlockState(sharedCtx, adminTokens);
  test.skip(state.bans.length > 0, 'this instance already has blocked addresses');
  await openSecurityCategory(page, adminTokens);
  const empty = page.locator('[data-testid="autoblock-empty"]');
  await expect(empty).toBeVisible();
  await expect(empty).toContainText(/failed sign-ins|échecs|Fehlanmeldungen|inicios fallidos/i);
});

test('1.4 — the DoS rate field follows its switch', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  const rate = page.locator('[data-testid="autoblock-dos-rpm"]');
  const visibleBefore = await rate.count();

  const dosSwitch = page.locator('.autoblock-card .settings-toggle-label').nth(2);
  await dosSwitch.click();
  await expect(rate).toHaveCount(visibleBefore === 1 ? 0 : 1, { timeout: 5_000 });

  // Put the switch back so the next test starts from the served state.
  await dosSwitch.click();
});

// ===========================================================================
// 2. Save
// ===========================================================================

test('2.1 — a saved policy is what the next read returns', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);

  await page.locator('[data-testid="autoblock-threshold"]').fill('4');
  await page.locator('[data-testid="autoblock-window"]').fill('120');
  await page.locator('[data-testid="autoblock-ban"]').fill('900');
  await page.locator('[data-testid="autoblock-save"]').click();
  await expect(page.locator('[data-testid="autoblock-notice"]')).toBeVisible({ timeout: 10_000 });

  const state = await autoBlockState(sharedCtx, adminTokens);
  expect(state.settings.threshold).toBe(4);
  expect(state.settings.window_seconds).toBe(120);
  expect(state.settings.ban_seconds).toBe(900);
  // `enabled` was left as the server had it; saving numbers alone must not
  // switch the feature on behind the admin's back.
  expect(state.settings.enabled).toBe(false);
});

test('2.2 — an impossible policy is refused with the reason on screen', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);

  // Turn blocking on so the validators apply, then ask for a zero-length ban.
  await page.locator('.autoblock-card .settings-toggle-label').first().click();
  await page.locator('[data-testid="autoblock-ban"]').fill('0');
  await page.locator('[data-testid="autoblock-save"]').click();

  const notice = page.locator('[data-testid="autoblock-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'alert');
  await expect(notice).toContainText(/ban duration|durée du blocage|Sperrdauer|duración/i);

  // Nothing was stored: the ban duration is still the served one.
  const state = await autoBlockState(sharedCtx, adminTokens);
  expect(state.settings.ban_seconds).toBeGreaterThan(0);

  await restoreDefaults(sharedCtx, adminTokens);
});

test('2.3 — the feature can always be switched off', async ({ page }) => {
  // An admin must be able to reach "off" from any state, even one whose numbers
  // an enabled policy would reject (that is what makes "off" reachable at all).
  await sharedCtx
    .put(`${BASE}/api/security/auto-block`, {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
      data: { enabled: false, threshold: 0, window_seconds: 0, ban_seconds: 0 },
    })
    .catch(() => {});
  await openSecurityCategory(page, adminTokens);

  await page.locator('[data-testid="autoblock-threshold"]').fill('5');
  await page.locator('[data-testid="autoblock-window"]').fill('300');
  await page.locator('[data-testid="autoblock-ban"]').fill('1800');
  await page.locator('[data-testid="autoblock-save"]').click();
  await expect(page.locator('[data-testid="autoblock-notice"]')).toBeVisible({ timeout: 10_000 });

  const state = await autoBlockState(sharedCtx, adminTokens);
  expect(state.settings.enabled).toBe(false);
  expect(state.settings.threshold).toBe(5);
});

// ===========================================================================
// 3. The block list: pardon and clear (mocked — no firewall is touched)
// ===========================================================================

test('3.1 — a blocked address shows its remaining time and can be pardoned', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);

  const ip = '203.0.113.42';
  await page.route('**/api/security/auto-block/pardon', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: {
          state: {
            settings: { enabled: false, threshold: 5, window_seconds: 300, ban_seconds: 1800 },
            bans: [],
            active: 0,
            firewall_rules: 0,
            firewall_available: false,
          },
        },
      }),
    }),
  );
  // Seed the visible row through the read endpoint, not the database: the test
  // asserts the card, not the ban machinery.
  await page.route('**/api/security/auto-block', (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: {
          settings: {
            enabled: true,
            threshold: 5,
            window_seconds: 300,
            ban_seconds: 1800,
            apply_firewall: false,
            dos_enabled: true,
            dos_requests_per_minute: 600,
          },
          bans: [
            {
              ip,
              reason: '5 failed sign-ins in 5 minutes',
              failures: 5,
              created_at: '2026-10-31T12:00:00+00:00',
              expires_at: '2099-01-01T00:00:00+00:00',
              firewall_rule: true,
              active: true,
              remaining_minutes: 12,
            },
          ],
          active: 1,
          firewall_rules: 1,
          firewall_available: true,
          limits: {
            min_threshold: 1,
            max_threshold: 100,
            max_window_seconds: 86400,
            max_ban_seconds: 31536000,
            max_dos_requests_per_minute: 1000000,
          },
        },
      }),
    });
  });

  await page.locator('[data-testid="autoblock-refresh"]').click();
  const row = page.locator(`[data-testid="autoblock-ban-row"][data-ip="${ip}"]`);
  await expect(row).toBeVisible({ timeout: 10_000 });
  // The row says how long is left, why, and that a firewall rule is installed.
  await expect(row).toContainText(/12|minute/i);
  await expect(row).toContainText(/failed sign-ins/i);
  await expect(row).toContainText(/firewall/i);

  await row.locator('[data-testid="autoblock-pardon"]').click();
  const notice = page.locator('[data-testid="autoblock-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'status');
  await expect(notice).toContainText(ip);
  // The list is emptied by the mocked state, not by a reload.
  await expect(page.locator('[data-testid="autoblock-ban-row"]')).toHaveCount(0);

  await page.unroute('**/api/security/auto-block');
  await page.unroute('**/api/security/auto-block/pardon');
});

test('3.2 — "lift every block" is offered only when something is blocked', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  const state = await autoBlockState(sharedCtx, adminTokens);
  const clear = page.locator('[data-testid="autoblock-clear"]');
  if (state.bans.length === 0) {
    await expect(clear).toHaveCount(0);
  } else {
    await expect(clear).toBeVisible();
  }
});

test('3.3 — a failed pardon is shown as an alert, not as success', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  const state = await autoBlockState(sharedCtx, adminTokens);
  test.skip(state.bans.length === 0, 'no blocked address to pardon');

  await page.route('**/api/security/auto-block/pardon', (route) =>
    route.fulfill({
      status: 400,
      contentType: 'application/json',
      body: JSON.stringify({
        success: false,
        message: "the ban was lifted but its firewall rule could not be removed: ufw not available",
      }),
    }),
  );

  await page.locator('[data-testid="autoblock-pardon"]').first().click();
  const notice = page.locator('[data-testid="autoblock-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'alert');
  await expect(notice).toContainText(/firewall rule could not be removed|could not lift/i);

  await page.unroute('**/api/security/auto-block/pardon');
});

// ===========================================================================
// 4. Access control and accessibility
// ===========================================================================

test('4.1 — a non-admin session cannot read, change or pardon blocks', async ({ page }) => {
  const name = `blockmember${Date.now().toString(36).slice(-4)}`;
  const created = await page.request.post(`${BASE}/api/users`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: { username: name, password: 'MemberSecret123', fullname: 'Block Member', email: '', role_id: 1 },
  });
  expect(created.ok()).toBeTruthy();
  const member = await apiLogin(sharedCtx, name, 'MemberSecret123');
  expect(member.token).toBeTruthy();

  const read = await page.request.get(`${BASE}/api/security/auto-block`, {
    headers: { 'x-session-token': member.token },
  });
  expect(read.status()).toBe(403);
  const write = await page.request.put(`${BASE}/api/security/auto-block`, {
    headers: { 'x-session-token': member.token, 'x-csrf-token': member.csrf },
    data: { enabled: true, threshold: 1 },
  });
  expect(write.status()).toBe(403);
  const pardon = await page.request.post(`${BASE}/api/security/auto-block/pardon`, {
    headers: { 'x-session-token': member.token, 'x-csrf-token': member.csrf },
    data: { ip: '203.0.113.42' },
  });
  expect(pardon.status()).toBe(403);

  await page.request
    .delete(`${BASE}/api/users/${encodeURIComponent(name)}`, {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    })
    .catch(() => {});
});

test('4.2 — every field has a label and every switch an accessible name', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);

  for (const id of ['autoblock-threshold', 'autoblock-window', 'autoblock-ban']) {
    const labelled = await page.locator(`label[for="${id}"]`).count();
    expect(labelled, `${id} must have exactly one <label for>`).toBe(1);
  }

  const switches = page.locator('.autoblock-card input[type="checkbox"]');
  const count = await switches.count();
  expect(count).toBeGreaterThanOrEqual(3);
  for (let i = 0; i < count; i++) {
    const labelledBy = await switches.nth(i).getAttribute('aria-labelledby');
    if (labelledBy) {
      expect(await page.locator(`#${labelledBy}`).count()).toBe(1);
    } else {
      expect((await switches.nth(i).getAttribute('aria-label')) || '').not.toBe('');
    }
  }
});

test('4.3 — no duplicate element ids on the screen', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  const duplicates = await page.evaluate(() => {
    const seen = new Map<string, number>();
    document.querySelectorAll('[id]').forEach((el) => {
      seen.set(el.id, (seen.get(el.id) ?? 0) + 1);
    });
    return [...seen.entries()].filter(([, n]) => n > 1).map(([id]) => id);
  });
  expect(duplicates).toEqual([]);
});

test('4.4 — the card does not overflow its panel horizontally', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  const overflow = await page.evaluate(() => {
    const panel = document.querySelector('[data-testid="settings-page"]');
    if (!panel) return -1;
    const box = panel.getBoundingClientRect();
    let worst = 0;
    panel.querySelectorAll('[data-testid^="autoblock-"]').forEach((el) => {
      const r = el.getBoundingClientRect();
      worst = Math.max(worst, r.right - box.right);
    });
    return Math.round(worst);
  });
  expect(overflow).toBeLessThanOrEqual(0);
});
