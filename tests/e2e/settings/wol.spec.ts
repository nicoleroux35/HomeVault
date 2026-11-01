/* =============================================================================
 * File          : wol.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings → System → Wake-on-LAN card, end-to-end (Playwright): the saved
 *   host list, the interface evidence, add/edit/delete, validation, the wake
 *   button (mocked — no packet leaves the machine) and accessibility.
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/wol.spec.ts --project=firefox
 * =============================================================================
 */

/**
 * Wake-on-LAN (Settings → System) — End-to-End Test Suite
 *
 * **SAFETY: this suite never sends a magic packet.** `POST /api/power/wol/wake`
 * is intercepted with `page.route` (success in 5.1, a transport failure in 5.2),
 * and the saved test host uses an RFC 5737 documentation MAC that no real NIC
 * carries. `afterAll` deletes every host the run created, so a follow-up run —
 * or a real deployment — cannot inherit a test row.
 *
 * The bytes themselves are pinned by `tests/wol_integration.rs` (a real
 * localhost UDP socket) and by the unit tests in `src/wol.rs`; this file owns
 * the screen.
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

interface WolHost {
  id: number;
  label: string;
  mac: string;
  broadcast: string | null;
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

/** Opens Settings → System and waits for the Wake-on-LAN card. */
async function openSystemCategory(page: Page, tokens: Tokens) {
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

  const systemItem = page.locator('[data-testid="settings-nav-item"]', {
    hasText: /System|Système|System|Sistema/i,
  });
  await systemItem.first().click();
  await page.waitForSelector('[data-testid="wol-label"]', { timeout: 15_000 });
}

async function wolHosts(ctx: APIRequestContext, tokens: Tokens): Promise<WolHost[]> {
  const res = await ctx.get(`${BASE}/api/power/wol/hosts`, {
    headers: { 'x-session-token': tokens.token },
  });
  expect(res.ok()).toBeTruthy();
  return (await res.json()).data.hosts as WolHost[];
}

/** Delete every host whose label starts with the E2E prefix. */
async function sweep(ctx: APIRequestContext, tokens: Tokens) {
  const hosts = await wolHosts(ctx, tokens).catch(() => [] as WolHost[]);
  for (const host of hosts) {
    if (host.label.startsWith('e2e-wol')) {
      await ctx
        .delete(`${BASE}/api/power/wol/hosts/${host.id}`, {
          headers: { 'x-session-token': tokens.token, 'x-csrf-token': tokens.csrf },
        })
        .catch(() => {});
    }
  }
}

let adminTokens: Tokens;
let sharedCtx: APIRequestContext;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await playwright.request.newContext({ baseURL: BASE });
  const { token, csrf } = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
  // A previous aborted run must not leave rows behind for this one to trip on.
  await sweep(sharedCtx, adminTokens);
});

test.afterAll(async () => {
  await sweep(sharedCtx, adminTokens);
  await sharedCtx.dispose();
});

// ===========================================================================
// 1. Shape
// ===========================================================================

test('1.1 — the card renders the form, the list and the interface evidence', async ({ page }) => {
  await openSystemCategory(page, adminTokens);

  await expect(page.locator('[data-testid="wol-label"]')).toBeVisible();
  await expect(page.locator('[data-testid="wol-mac"]')).toBeVisible();
  await expect(page.locator('[data-testid="wol-broadcast"]')).toBeVisible();
  await expect(page.locator('[data-testid="wol-save"]')).toBeVisible();
  await expect(page.locator('[data-testid="wol-host-list"]')).toBeVisible();
  await expect(page.locator('[data-testid="wol-interfaces-row"]')).toBeVisible();
});

test('1.2 — the interface list names a broadcast destination, or explains why not', async ({
  page,
}) => {
  await openSystemCategory(page, adminTokens);
  const rows = page.locator('[data-testid="wol-interface"]');
  const count = await rows.count();
  if (count === 0) {
    // A host with no IPv4 interface must say so rather than show an empty row.
    await expect(page.locator('[data-testid="wol-interface-list"]')).toContainText(
      /no interface|aucune interface|keine Schnittstelle|ninguna interfaz/i,
    );
  } else {
    // Every entry is `name address/prefix → broadcast`.
    await expect(rows.first()).toContainText('→');
  }
});

test('1.3 — the empty state says what to do next', async ({ page }) => {
  await openSystemCategory(page, adminTokens);
  const hosts = await wolHosts(sharedCtx, adminTokens);
  if (hosts.length === 0) {
    await expect(page.locator('[data-testid="wol-empty"]')).toBeVisible();
    await expect(page.locator('[data-testid="wol-empty"]')).toContainText(/add|ajoutez|hinzu|añada/i);
  }
});

// ===========================================================================
// 2. Add / edit / delete
// ===========================================================================

test('2.1 — a host is added, shown, edited and removed', async ({ page }) => {
  await openSystemCategory(page, adminTokens);
  const label = `e2e-wol-${Date.now().toString(36).slice(-5)}`;

  await page.locator('[data-testid="wol-label"]').fill(label);
  await page.locator('[data-testid="wol-mac"]').fill('02:00:5e:10:00:01');
  await page.locator('[data-testid="wol-save"]').click();
  await expect(page.locator('[data-testid="wol-notice"]')).toBeVisible({ timeout: 10_000 });

  // Stored canonically, and visible in the list.
  const hosts = await wolHosts(sharedCtx, adminTokens);
  const created = hosts.find((h) => h.label === label);
  expect(created, 'the host must be stored').toBeTruthy();
  expect(created!.mac).toBe('02:00:5E:10:00:01');
  const row = page.locator(`[data-testid="wol-host"][data-host-id="${created!.id}"]`);
  await expect(row).toBeVisible();
  await expect(row).toContainText('02:00:5E:10:00:01');

  // Edit: the form is prefilled, saving updates the row.
  await row.locator('[data-testid="wol-edit"]').click();
  await expect(page.locator('[data-testid="wol-label"]')).toHaveValue(label);
  const renamed = `${label}-b`;
  await page.locator('[data-testid="wol-label"]').fill(renamed);
  await page.locator('[data-testid="wol-save"]').click();
  await expect
    .poll(async () => (await wolHosts(sharedCtx, adminTokens)).some((h) => h.label === renamed), {
      timeout: 10_000,
    })
    .toBe(true);

  // Delete: the row goes, and so does the API row.
  const updated = (await wolHosts(sharedCtx, adminTokens)).find((h) => h.label === renamed)!;
  await page
    .locator(`[data-testid="wol-host"][data-host-id="${updated.id}"] [data-testid="wol-delete"]`)
    .click();
  await expect
    .poll(async () => (await wolHosts(sharedCtx, adminTokens)).some((h) => h.id === updated.id), {
      timeout: 10_000,
    })
    .toBe(false);
});

// ===========================================================================
// 3. Validation
// ===========================================================================

test('3.1 — a MAC that is not one is refused with the reason on screen', async ({ page }) => {
  await openSystemCategory(page, adminTokens);

  await page.locator('[data-testid="wol-label"]').fill(`e2e-wol-bad-${Date.now().toString(36).slice(-4)}`);
  await page.locator('[data-testid="wol-mac"]').fill('not-a-mac');
  await page.locator('[data-testid="wol-save"]').click();

  const notice = page.locator('[data-testid="wol-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'alert');
  await expect(notice).toContainText(/MAC|hex|adresse/i);

  // Nothing was stored.
  const hosts = await wolHosts(sharedCtx, adminTokens);
  expect(hosts.some((h) => h.label.includes('e2e-wol-bad'))).toBe(false);
});

test('3.2 — a broadcast address given as a name is refused', async ({ page }) => {
  await openSystemCategory(page, adminTokens);

  await page
    .locator('[data-testid="wol-label"]')
    .fill(`e2e-wol-name-${Date.now().toString(36).slice(-4)}`);
  await page.locator('[data-testid="wol-mac"]').fill('02:00:5e:10:00:02');
  await page.locator('[data-testid="wol-broadcast"]').fill('nas.local');
  await page.locator('[data-testid="wol-save"]').click();

  const notice = page.locator('[data-testid="wol-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'alert');
  await expect(notice).toContainText(/IP address|adresse IP|IP-Adresse|dirección IP/i);
});

test('3.3 — a duplicate name is refused rather than silently creating a second row', async ({
  page,
}) => {
  const label = `e2e-wol-dup-${Date.now().toString(36).slice(-4)}`;
  const created = await sharedCtx.post(`${BASE}/api/power/wol/hosts`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: { label, mac: '02:00:5e:10:00:03' },
  });
  expect(created.ok()).toBeTruthy();

  await openSystemCategory(page, adminTokens);
  await page.locator('[data-testid="wol-label"]').fill(label);
  await page.locator('[data-testid="wol-mac"]').fill('02:00:5e:10:00:04');
  await page.locator('[data-testid="wol-save"]').click();

  const notice = page.locator('[data-testid="wol-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toContainText(/already exists|existe déjà|existiert bereits|ya existe/i);
});

// ===========================================================================
// 4. The wake button (mocked: no packet leaves this machine)
// ===========================================================================

test('4.1 — a wake reports "sent, check the machine" and never "woken"', async ({ page }) => {
  const label = `e2e-wol-wake-${Date.now().toString(36).slice(-4)}`;
  const created = await sharedCtx.post(`${BASE}/api/power/wol/hosts`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: { label, mac: '02:00:5e:10:00:05' },
  });
  expect(created.ok()).toBeTruthy();

  await openSystemCategory(page, adminTokens);

  await page.route('**/api/power/wol/wake', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: {
          sent: true,
          report: { mac: '02:00:5E:10:00:05', destination: '192.168.1.255', ports: [9, 7] },
          message:
            'Magic packet sent to 192.168.1.255 on eth0 (02:00:5E:10:00:05). This only confirms it left: switch the machine on and check that it answers.',
        },
      }),
    }),
  );

  const row = page.locator('[data-testid="wol-host"]', { hasText: label }).first();
  await expect(row).toBeVisible({ timeout: 10_000 });
  await row.locator('[data-testid="wol-wake"]').click();

  const notice = page.locator('[data-testid="wol-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'status');
  await expect(notice).toContainText(/only confirms it left|confirme seulement|bestätigt nur|solo confirma/i);
  await expect(notice).not.toContainText(/woken|réveillé|aufgeweckt|despertad/i);

  await page.unroute('**/api/power/wol/wake');
  // `afterAll`'s sweep removes the `e2e-wol*` row this test created.
});

test('4.2 — a refused wake shows the reason as an alert', async ({ page }) => {
  await openSystemCategory(page, adminTokens);
  const hosts = await wolHosts(sharedCtx, adminTokens);
  test.skip(hosts.length === 0, 'no saved host to click');

  await page.route('**/api/power/wol/wake', (route) =>
    route.fulfill({
      status: 400,
      contentType: 'application/json',
      body: JSON.stringify({
        success: false,
        message: 'The magic packet could not be sent: could not open a local socket: permission denied',
      }),
    }),
  );

  await page.locator('[data-testid="wol-wake"]').first().click();
  const notice = page.locator('[data-testid="wol-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'alert');
  await expect(notice).toContainText(/could not be sent|n'a pas pu être envoyé|konnte nicht gesendet|no se pudo enviar/i);

  await page.unroute('**/api/power/wol/wake');
});

// ===========================================================================
// 5. Access control and accessibility
// ===========================================================================

test('5.1 — a non-admin session cannot read or wake a host', async ({ page }) => {
  const name = `wolmember${Date.now().toString(36).slice(-4)}`;
  const created = await page.request.post(`${BASE}/api/users`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: { username: name, password: 'MemberSecret123', fullname: 'WoL Member', email: '', role_id: 1 },
  });
  expect(created.ok()).toBeTruthy();

  const member = await apiLogin(sharedCtx, name, 'MemberSecret123');
  expect(member.token).toBeTruthy();

  const list = await page.request.get(`${BASE}/api/power/wol/hosts`, {
    headers: { 'x-session-token': member.token },
  });
  expect(list.status()).toBe(403);

  const wake = await page.request.post(`${BASE}/api/power/wol/wake`, {
    headers: { 'x-session-token': member.token, 'x-csrf-token': member.csrf },
    data: { mac: '02:00:5e:10:00:06' },
  });
  expect(wake.status()).toBe(403);

  await page.request
    .delete(`${BASE}/api/users/${encodeURIComponent(name)}`, {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    })
    .catch(() => {});
});

test('5.2 — every field has a label and the destructive buttons are named', async ({ page }) => {
  await openSystemCategory(page, adminTokens);

  for (const id of ['wol-label', 'wol-mac', 'wol-broadcast']) {
    const labelled = await page.locator(`label[for="${id}"]`).count();
    expect(labelled, `${id} must have a <label for>`).toBe(1);
  }

  // Icon-only controls carry an accessible name (never just a `title`).
  const deletes = page.locator('[data-testid="wol-delete"]');
  const count = await deletes.count();
  for (let i = 0; i < count; i++) {
    const name = await deletes.nth(i).getAttribute('aria-label');
    expect(name, 'a delete button needs an accessible name').toBeTruthy();
  }
});

test('5.3 — no duplicate element ids on the screen', async ({ page }) => {
  await openSystemCategory(page, adminTokens);
  const duplicates = await page.evaluate(() => {
    const seen = new Map<string, number>();
    document.querySelectorAll('[id]').forEach((el) => {
      const id = el.id;
      seen.set(id, (seen.get(id) ?? 0) + 1);
    });
    return [...seen.entries()].filter(([, n]) => n > 1).map(([id]) => id);
  });
  expect(duplicates).toEqual([]);
});

test('5.4 — the card does not overflow its panel horizontally', async ({ page }) => {
  await openSystemCategory(page, adminTokens);
  const overflow = await page.evaluate(() => {
    const panel = document.querySelector('[data-testid="settings-page"]');
    if (!panel) return -1;
    const box = panel.getBoundingClientRect();
    let worst = 0;
    panel.querySelectorAll('[data-testid^="wol-"]').forEach((el) => {
      const r = el.getBoundingClientRect();
      worst = Math.max(worst, r.right - box.right);
    });
    return Math.round(worst);
  });
  expect(overflow).toBeLessThanOrEqual(0);
});
