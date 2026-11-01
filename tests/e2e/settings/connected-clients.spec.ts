/* =============================================================================
 * File          : connected-clients.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings → Security → Connected clients card, end-to-end (Playwright):
 *   the per-source provenance, the list, the two-step disconnect (mocked, so
 *   nothing is signalled), the refusals, access control and accessibility.
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/connected-clients.spec.ts --project=firefox
 * =============================================================================
 */

/**
 * Connected clients (Settings → Security) — End-to-End Test Suite
 *
 * **SAFETY: this suite never disconnects a real client.** The list and the kick
 * are both served through `page.route`, so the assertions are about the screen
 * — the real collectors are covered by `tests/connected_clients_integration.rs`
 * (endpoint contract) and `src/connected_clients.rs` (parsers on real
 * `smbstatus -j` / `showmount -a` / `ss -tnp` samples).
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
  await page
    .locator('[data-testid="settings-nav-item"]')
    .first()
    .waitFor({ state: 'visible', timeout: 15_000 });

  const securityItem = page.locator('[data-testid="settings-nav-item"]', {
    hasText: /Security|Sécurité|Sicherheit|Seguridad/,
  });
  await securityItem.first().click();
  await page.waitForSelector('[data-testid="clients-sources"]', { timeout: 15_000 });
}

/** A listing with one kickable SMB client and one NFS row that cannot be cut. */
const seeding = {
  success: true,
  data: {
    clients: [
      {
        id: 'smb:4242:192.168.1.20',
        protocol: 'smb',
        username: 'alice',
        client_ip: '192.168.1.20',
        client_host: 'laptop',
        target: 'photos',
        opened_files: 2,
        since: null,
        kickable: true,
        detail: 'smbd pid 4242',
      },
      {
        id: 'nfs:/srv/backups:192.168.1.30',
        protocol: 'nfs',
        username: '-',
        client_ip: '192.168.1.30',
        client_host: null,
        target: '/srv/backups',
        opened_files: 0,
        since: null,
        kickable: false,
        detail: 'kernel export',
      },
    ],
    sources: [
      { source: 'smb', available: true, error: null, count: 1 },
      { source: 'nfs', available: true, error: null, count: 1 },
      { source: 'sessions', available: false, error: 'ss: command not found', count: 0 },
    ],
    kickable: 1,
    checked_at: '2026-10-31T12:00:00+00:00',
  },
};

let adminTokens: Tokens;
let sharedCtx: APIRequestContext;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await playwright.request.newContext({ baseURL: BASE });
  const { token, csrf } = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
});

test.afterAll(async () => {
  await sharedCtx.dispose();
});

// ===========================================================================
// 1. Shape and provenance
// ===========================================================================

test('1.1 — the card renders and shows one status per source', async ({ page }) => {
  await openSecurityCategory(page, adminTokens);
  await expect(page.locator('[data-testid="clients-sources"]')).toBeVisible();
  const sources = page.locator('[data-testid="clients-source"]');
  await expect(sources).toHaveCount(3);
  for (let i = 0; i < 3; i++) {
    const name = await sources.nth(i).getAttribute('data-source');
    expect(['smb', 'nfs', 'sessions']).toContain(name);
  }
});

test('1.2 — a source that is not installed says so instead of showing nothing', async ({
  page,
}) => {
  const real = await sharedCtx.get(`${BASE}/api/security/connected-clients`, {
    headers: { 'x-session-token': adminTokens.token },
  });
  const state = (await real.json()).data;
  const missing = state.sources.find((s: any) => !s.available);
  test.skip(!missing, 'every collector answered on this host');
  await openSecurityCategory(page, adminTokens);
  const row = page.locator(`[data-testid="clients-source"][data-source="${missing.source}"]`);
  await expect(row).toContainText(/not available|non disponible|nicht verfügbar|no disponible/i);
  // …and the reason comes from the server, not from a generic sentence.
  if (missing.error) {
    await expect(row).toContainText(missing.error);
  }
});

// ===========================================================================
// 2. The list (mocked: no real client is touched)
// ===========================================================================

test('2.1 — the list shows who is connected, what they hold, and what cannot be cut', async ({
  page,
}) => {
  await page.route('**/api/security/connected-clients', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(seeding),
    }),
  );
  await openSecurityCategory(page, adminTokens);

  const smbRow = page.locator('[data-testid="clients-row"][data-client-id="smb:4242:192.168.1.20"]');
  await expect(smbRow).toBeVisible({ timeout: 10_000 });
  await expect(smbRow).toContainText('192.168.1.20');
  await expect(smbRow).toContainText('alice');
  await expect(smbRow).toContainText('photos');
  await expect(smbRow).toContainText('2'); // two open files
  await expect(smbRow.locator('[data-testid="clients-kick"]')).toBeVisible();

  // An NFS row offers no button: the export is kernel-held, and the row says so
  // rather than hiding the client entirely.
  const nfsRow = page.locator(
    '[data-testid="clients-row"][data-client-id="nfs:/srv/backups:192.168.1.30"]',
  );
  await expect(nfsRow).toBeVisible();
  await expect(nfsRow.locator('[data-testid="clients-kick"]')).toHaveCount(0);
  await expect(nfsRow.locator('[data-testid="clients-not-kickable"]')).toBeVisible();

  await page.unroute('**/api/security/connected-clients');
});

test('2.2 — disconnecting takes a second, explicit click', async ({ page }) => {
  let kicks = 0;
  await page.route('**/api/security/connected-clients', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(seeding),
    }),
  );
  await page.route('**/api/security/connected-clients/kick', (route) => {
    kicks += 1;
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: { disconnected: true, detail: 'Closed share \'photos\' for 192.168.1.20' },
      }),
    });
  });

  await openSecurityCategory(page, adminTokens);
  const kick = page
    .locator('[data-testid="clients-row"][data-client-id="smb:4242:192.168.1.20"] [data-testid="clients-kick"]')
    .first();
  await kick.click();
  // The first click only asks; nothing is sent.
  await expect(kick).toContainText(/now\?|maintenant|jetzt|ahora/i);
  expect(kicks).toBe(0);

  await kick.click();
  await expect.poll(() => kicks, { timeout: 10_000 }).toBe(1);

  const notice = page.locator('[data-testid="clients-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'status');
  await expect(notice).toContainText(/192\.168\.1\.20|photos/i);

  await page.unroute('**/api/security/connected-clients');
  await page.unroute('**/api/security/connected-clients/kick');
});

test('2.3 — a refused disconnect shows the server reason as an alert', async ({ page }) => {
  await page.route('**/api/security/connected-clients', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(seeding) }),
  );
  await page.route('**/api/security/connected-clients/kick', (route) =>
    route.fulfill({
      status: 400,
      contentType: 'application/json',
      body: JSON.stringify({
        success: false,
        message:
          "No client 'smb:4242:192.168.1.20' is connected right now — refresh the list and try again",
      }),
    }),
  );

  await openSecurityCategory(page, adminTokens);
  const kick = page.locator('[data-testid="clients-kick"]').first();
  await kick.click();
  await kick.click();

  const notice = page.locator('[data-testid="clients-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'alert');
  await expect(notice).toContainText(/is connected right now|refresh/i);

  await page.unroute('**/api/security/connected-clients');
  await page.unroute('**/api/security/connected-clients/kick');
});

// ===========================================================================
// 3. Access control and accessibility
// ===========================================================================

test('3.1 — a non-admin session cannot read the list or disconnect anyone', async ({ page }) => {
  const name = `clientsmember${Date.now().toString(36).slice(-4)}`;
  const created = await page.request.post(`${BASE}/api/users`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: { username: name, password: 'MemberSecret123', fullname: 'Clients Member', email: '', role_id: 1 },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const member = await apiLogin(sharedCtx, name, 'MemberSecret123');
  expect(member.token).toBeTruthy();

  const list = await page.request.get(`${BASE}/api/security/connected-clients`, {
    headers: { 'x-session-token': member.token },
  });
  expect(list.status()).toBe(403);
  const kick = await page.request.post(`${BASE}/api/security/connected-clients/kick`, {
    headers: { 'x-session-token': member.token, 'x-csrf-token': member.csrf },
    data: { id: 'smb:1:192.168.1.20' },
  });
  expect(kick.status()).toBe(403);

  await page.request
    .delete(`${BASE}/api/users/${encodeURIComponent(name)}`, {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    })
    .catch(() => {});
});

test('3.2 — the disconnect button has an accessible name naming the client', async ({ page }) => {
  await page.route('**/api/security/connected-clients', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(seeding) }),
  );
  await openSecurityCategory(page, adminTokens);

  const kick = page.locator('[data-testid="clients-kick"]').first();
  const name = await kick.getAttribute('aria-label');
  expect(name, 'an icon-bearing action needs a name').toBeTruthy();
  expect(name).toContain('192.168.1.20');

  await page.unroute('**/api/security/connected-clients');
});

test('3.3 — no duplicate element ids on the screen', async ({ page }) => {
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

test('3.4 — the card does not overflow its panel horizontally', async ({ page }) => {
  await page.route('**/api/security/connected-clients', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(seeding) }),
  );
  await openSecurityCategory(page, adminTokens);
  const overflow = await page.evaluate(() => {
    const panel = document.querySelector('[data-testid="settings-page"]');
    if (!panel) return -1;
    const box = panel.getBoundingClientRect();
    let worst = 0;
    panel.querySelectorAll('[data-testid^="clients-"]').forEach((el) => {
      const r = el.getBoundingClientRect();
      worst = Math.max(worst, r.right - box.right);
    });
    return Math.round(worst);
  });
  expect(overflow).toBeLessThanOrEqual(0);
  await page.unroute('**/api/security/connected-clients');
});
