/* =============================================================================
 * File          : vpn.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Vpn screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * VPN app — End-to-End Test Suite
 *
 * The VPN app is not a route: it is a window opened from the desktop's
 * Applications launcher (id `vpn`, "VPN" tile). It manages a single
 * always-on WireGuard server + provisioned peers, and OpenVPN client
 * connections, through `src/vpn.rs`.
 *
 * **Safety model (different from Network's — see that file's header):**
 * unlike flipping a real network interface, provisioning the app's
 * WireGuard server and creating/revoking a peer is the *intended*,
 * additive, reversible, primary use of this page (creating a brand new
 * resource, never touching existing connectivity) — `GET .../wireguard/
 * server` already provisions the server lazily on first call by design
 * (see `handle_wg_server_status`'s own doc comment), so merely opening
 * this page's WireGuard tab does that regardless of what this suite does.
 * Section 13 below therefore creates one real, uniquely-named throwaway
 * peer and always revokes it in a `finally`-equivalent — mirroring
 * Docker's throwaway-container pattern — skipping cleanly if WireGuard
 * isn't available or the host's sudoers isn't configured for it (real
 * `sudo wg`/`wg-quick` calls fail cleanly otherwise, they're never forced).
 * OpenVPN start is never attempted for real (would need a genuine
 * `.ovpn` targeting a reachable server); a throwaway OpenVPN connection
 * *record* (create → verify listed → delete) is exercised the same
 * cautious way as WireGuard, skipping cleanly if `/etc/openvpn` isn't
 * writable by this process.
 *
 * Run (all, safe subset): npx playwright test tests/e2e/vpn/vpn.spec.ts --project=firefox
 * Run (single):            npx playwright test tests/e2e/vpn/vpn.spec.ts -g "4.1"
 * Run (category):          npx playwright test tests/e2e/vpn/vpn.spec.ts -g "Security"
 * Custom server:            BASE_URL=http://localhost:3000 npx playwright test tests/e2e/vpn/vpn.spec.ts
 *
 * Requires a live server whose setup is complete and that is seeded with an
 * admin account (defaults: e2eadmin / E2eSecret123, override with
 * E2E_ADMIN_USER / E2E_ADMIN_PASS).
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

async function apiLogin(
  ctx: APIRequestContext,
  username: string,
  password: string,
): Promise<{ token: string; csrf: string }> {
  const res = await ctx.post('/api/auth/login', { data: { username, password } });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

/**
 * A non-admin session that can actually use the API.
 *
 * `POST /api/users` creates the account with an admin-chosen password, so
 * `password_change_guard` answers `403 password_change_required` to every
 * call until the owner picks their own. Log in, change it, log in again —
 * otherwise these authorization tests would assert a 403 that comes from the
 * forced-change gate instead of the admin gate they are about.
 */
async function apiLoginAsViewer(
  ctx: APIRequestContext,
  username: string,
  initialPassword: string,
  newPassword = 'Viewer67890!',
): Promise<{ token: string; csrf: string }> {
  const first = await apiLogin(ctx, username, initialPassword);
  await ctx.put(`/api/users/${username}/password`, {
    headers: { 'x-session-token': first.token, 'x-csrf-token': first.csrf },
    data: { current_password: initialPassword, new_password: newPassword },
  });
  return apiLogin(ctx, username, newPassword);
}

async function getHeapMB(page: Page): Promise<number> {
  return page.evaluate(() =>
    'memory' in performance ? (performance as any).usedJSHeapSize / (1024 * 1024) : 0,
  );
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

async function gotoDesktop(page: Page, tokens: Tokens, lang?: string) {
  await seedBrowser(page, tokens, lang);
  await page.goto('/desktop');
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

async function openLaunchpadApp(page: Page, appName: string) {
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator(`.lp-card[title="${appName}"]`);
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
}

async function gotoVpn(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openLaunchpadApp(page, 'VPN');
  await page.waitForSelector('[data-testid="vpn-page"]', { timeout: 10_000 });
  await expect(page.locator('[data-testid="vpn-page"] [data-testid="vpn-loading"]')).toHaveCount(0, { timeout: 15_000 });
}

/** The VPN window as a scoping root — `[data-testid="vpn-page"]` is unique to this page. */
function win(page: Page) {
  return page.locator('[data-testid="vpn-page"]');
}

function closeWindowButton(page: Page) {
  return page.locator('.hv-cl-btn-close').first();
}

let adminTokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) {
    throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  }
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
  await ctx.dispose();
});

test.afterEach(async ({ page }, testInfo) => {
  const heapMB = await getHeapMB(page).catch(() => 0);
  if (heapMB > 0) {
    // eslint-disable-next-line no-console
    console.log(`[MEM] ${testInfo.title}: ${heapMB.toFixed(1)} MB heap after test`);
  }
});

// ===========================================================================
// 1. Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — VPN window opens from the Applications launcher', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — header shows "VPN"', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title')).toHaveText('VPN');
  });

  test('1.3 — adapts to capability state: shows tabs, or the "no support" empty state', async ({
    page,
  }) => {
    await gotoVpn(page, adminTokens);
    const hasTabs = (await win(page).locator('[role="tablist"]').count()) > 0;
    const hasEmptyState = (await win(page).locator('[data-testid="vpn-empty-title"]', { hasText: /no.*support/i }).count()) > 0;
    expect(hasTabs || hasEmptyState).toBe(true);
  });

  test('1.4 — the 4 stat tiles render (when the page has capabilities)', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const tiles = win(page).locator('[data-testid="vpn-stat-card"]');
    if ((await tiles.count()) === 0) test.skip(true, 'no WireGuard/OpenVPN support on this host');
    await expect(tiles).toHaveCount(4);
  });
});

// ===========================================================================
// 2. Form fields
// ===========================================================================

test.describe('2 — Form fields', () => {
  test('2.1 — the "Add device" modal exposes a required name field', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const addBtn = win(page).locator('[data-testid="vpn-btn"]', { hasText: /New WireGuard|new_wireguard/i });
    if ((await addBtn.count()) === 0) test.skip(true, 'WireGuard not available on this host');
    await addBtn.first().click();
    await expect(page.locator('#wg-peer-name')).toHaveAttribute('type', 'text');
    await page.keyboard.press('Escape');
  });

  test('2.2 — the "Add OpenVPN" modal exposes name/file/username/password fields', async ({
    page,
  }) => {
    await gotoVpn(page, adminTokens);
    const openvpnTab = win(page).locator('.hv-winhdr-tab', { hasText: 'OpenVPN' });
    if ((await openvpnTab.count()) === 0) test.skip(true, 'OpenVPN not available on this host');
    await openvpnTab.click();
    const addBtn = win(page).locator('[data-testid="vpn-btn"]', { hasText: /New Connection|new_connection/i });
    await addBtn.first().click();
    await expect(page.locator('#ovpn-name')).toHaveAttribute('type', 'text');
    await expect(page.locator('#ovpn-file')).toHaveAttribute('type', 'file');
    await expect(page.locator('#ovpn-user')).toHaveAttribute('type', 'text');
    await expect(page.locator('#ovpn-pass')).toHaveAttribute('type', 'password');
    await page.keyboard.press('Escape');
  });
});

// ===========================================================================
// 3. Validation — direct fetch, mirroring tests/vpn_integration.rs
// ===========================================================================

test.describe('3 — Validation', () => {
  test('3.1 — server rejects an empty WireGuard device name', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/vpn/wireguard/peers', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ name: '' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.2 — server rejects an OpenVPN name containing path traversal', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/vpn/openvpn', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ name: '../../etc/cron.d/evil', config_content: 'client\n' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.3 — server returns 404 for an unknown connection id', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const status = await page.evaluate(
      async ({ token }) => {
        const r = await fetch('/api/vpn/connections/totally-fake-connection-id', {
          headers: { 'x-session-token': token },
        });
        return r.status;
      },
      { token: adminTokens.token },
    );
    expect(status).toBe(404);
  });
});

// ===========================================================================
// 4. All user interactions
// ===========================================================================

test.describe('4 — All user interactions', () => {
  test('4.1 — clicking a tab switches the active tab and its content', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const openvpnTab = win(page).locator('.hv-winhdr-tab', { hasText: 'OpenVPN' });
    if ((await openvpnTab.count()) === 0) test.skip(true, 'OpenVPN not available on this host');
    await openvpnTab.click();
    await expect(openvpnTab).toHaveAttribute('aria-selected', 'true');
  });

  test('4.2 — the Add-device modal opens and Cancel closes it', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const addBtn = win(page).locator('[data-testid="vpn-btn"]', { hasText: /New WireGuard|new_wireguard/i });
    if ((await addBtn.count()) === 0) test.skip(true, 'WireGuard not available on this host');
    await addBtn.first().click();
    await expect(page.locator('#wg-peer-name')).toBeVisible();
    await page.locator('button', { hasText: /Cancel|cancel/i }).first().click();
    await expect(page.locator('#wg-peer-name')).toHaveCount(0);
  });

  test('4.3 — Create device stays disabled until a name is typed', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const addBtn = win(page).locator('[data-testid="vpn-btn"]', { hasText: /New WireGuard|new_wireguard/i });
    if ((await addBtn.count()) === 0) test.skip(true, 'WireGuard not available on this host');
    await addBtn.first().click();
    const createBtn = page.locator('button', { hasText: /Create Device|create_device/i });
    await expect(createBtn).toBeDisabled();
    await page.locator('#wg-peer-name').fill('e2e-temp-name');
    await expect(createBtn).toBeEnabled();
    await page.keyboard.press('Escape');
  });

  test('4.4 — uploading an .ovpn file populates the name and shows a byte count', async ({
    page,
  }) => {
    await gotoVpn(page, adminTokens);
    const openvpnTab = win(page).locator('.hv-winhdr-tab', { hasText: 'OpenVPN' });
    if ((await openvpnTab.count()) === 0) test.skip(true, 'OpenVPN not available on this host');
    await openvpnTab.click();
    await win(page).locator('[data-testid="vpn-btn"]', { hasText: /New Connection|new_connection/i }).first().click();
    await page.locator('#ovpn-file').setInputFiles({
      name: 'e2e-office.ovpn',
      mimeType: 'text/plain',
      buffer: Buffer.from('client\ndev tun\nremote example.com 1194\n'),
    });
    await expect(page.locator('#ovpn-name')).toHaveValue('e2e-office');
    await expect(page.locator('[data-testid="vpn-file-preview"]')).toBeVisible();
    await page.keyboard.press('Escape');
  });
});

// ===========================================================================
// 5. Submission flows — guard/no-op paths (real lifecycle is section 13)
// ===========================================================================

test.describe('5 — Submission flows', () => {
  test('5.1 — Create device with an empty name does not fire a request', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const addBtn = win(page).locator('[data-testid="vpn-btn"]', { hasText: /New WireGuard|new_wireguard/i });
    if ((await addBtn.count()) === 0) test.skip(true, 'WireGuard not available on this host');
    await addBtn.first().click();
    let requestFired = false;
    page.on('request', (req) => {
      if (req.url().includes('/api/vpn/wireguard/peers') && req.method() === 'POST') requestFired = true;
    });
    // The button is disabled for an empty name — force-clicking (if it were
    // enabled) is not attempted; this only confirms the guard holds.
    const createBtn = page.locator('button', { hasText: /Create Device|create_device/i });
    await expect(createBtn).toBeDisabled();
    await page.waitForTimeout(300);
    expect(requestFired).toBe(false);
    await page.keyboard.press('Escape');
  });

  test('5.2 — Create connection stays disabled until a config file is loaded', async ({
    page,
  }) => {
    await gotoVpn(page, adminTokens);
    const openvpnTab = win(page).locator('.hv-winhdr-tab', { hasText: 'OpenVPN' });
    if ((await openvpnTab.count()) === 0) test.skip(true, 'OpenVPN not available on this host');
    await openvpnTab.click();
    await win(page).locator('[data-testid="vpn-btn"]', { hasText: /New Connection|new_connection/i }).first().click();
    const createBtn = page.locator('button', { hasText: /^Create Connection$|create_connection/i });
    await expect(createBtn).toBeDisabled();
    await page.keyboard.press('Escape');
  });
});

// ===========================================================================
// 6. Navigation
// ===========================================================================

test.describe('6 — Navigation', () => {
  test('6.1 — the window closes and can be reopened', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const closeBtn = closeWindowButton(page);
    if ((await closeBtn.count()) === 0) test.skip(true, 'no close control found for this window chrome');
    await closeBtn.click();
    await expect(page.locator('[data-testid="vpn-page"]')).toHaveCount(0);
    await openLaunchpadApp(page, 'VPN');
    await expect(win(page)).toBeVisible();
  });

  test('6.2 — switching tabs preserves the window (no reload)', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const openvpnTab = win(page).locator('.hv-winhdr-tab', { hasText: 'OpenVPN' });
    if ((await openvpnTab.count()) === 0) test.skip(true, 'only one/no tab available');
    const marker = 'e2e-nav-marker';
    await win(page).evaluate((el, m) => ((el as any)[m] = true), marker);
    await openvpnTab.click();
    await win(page).locator('.hv-winhdr-tab', { hasText: 'WireGuard' }).click();
    const stillThere = await win(page).evaluate((el, m) => Boolean((el as any)[m]), marker);
    expect(stillThere).toBe(true);
  });
});

// ===========================================================================
// 7. Responsive
// ===========================================================================

test.describe('7 — Responsive', () => {
  for (const [w, h, label] of [
    [360, 800, '360px (mobile)'],
    [768, 1024, '768px (tablet)'],
    [1920, 1080, '1920px (desktop)'],
  ] as const) {
    test(`7.${label} — renders without horizontal overflow`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: h });
      await gotoVpn(page, adminTokens);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
      );
      expect(overflow).toBe(false);
    });
  }
});

// ===========================================================================
// 8. Accessibility
// ===========================================================================

test.describe('8 — Accessibility', () => {
  test('8.1 — the tab bar exposes tablist/tab roles (when tabs exist)', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const tablist = win(page).locator('[role="tablist"]');
    if ((await tablist.count()) === 0) test.skip(true, 'no VPN support on this host — no tabs');
    await expect(win(page).locator('[role="tab"]').first()).toHaveAttribute(
      'aria-selected',
      'true',
    );
  });

  test('8.2 — the Add-device field has an associated label', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const addBtn = win(page).locator('[data-testid="vpn-btn"]', { hasText: /New WireGuard|new_wireguard/i });
    if ((await addBtn.count()) === 0) test.skip(true, 'WireGuard not available on this host');
    await addBtn.first().click();
    await expect(page.locator('label[for="wg-peer-name"]')).toBeVisible();
    await page.keyboard.press('Escape');
  });

  test('8.3 — no duplicate element ids on the page', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const dupes = await page.evaluate(() => {
      const ids = Array.from(document.querySelectorAll('[id]')).map((e) => e.id);
      const seen = new Set<string>();
      const dup: string[] = [];
      for (const id of ids) {
        if (seen.has(id)) dup.push(id);
        seen.add(id);
      }
      return dup;
    });
    expect(dupes).toEqual([]);
  });
});

// ===========================================================================
// 9. Performance
// ===========================================================================

test.describe('9 — Performance', () => {
  test('9.1 — the window opens in a reasonable time', async ({ page }) => {
    const start = Date.now();
    await gotoVpn(page, adminTokens);
    const elapsed = Date.now() - start;
    console.log(`[PERF] VPN window ready in ${elapsed}ms`);
    expect(elapsed).toBeLessThan(15_000);
  });

  test('9.2 — opening the VPN window does not add an excessive number of DOM nodes', async ({
    page,
  }) => {
    await gotoDesktop(page, adminTokens);
    const before = await page.evaluate(() => document.querySelectorAll('*').length);
    await openLaunchpadApp(page, 'VPN');
    await page.waitForSelector('[data-testid="vpn-page"]', { timeout: 10_000 });
    const after = await page.evaluate(() => document.querySelectorAll('*').length);
    const delta = after - before;
    console.log(`[PERF] VPN window added ${delta} DOM nodes`);
    expect(delta).toBeLessThan(2000);
  });
});

// ===========================================================================
// 10. Security
// ===========================================================================

test.describe('10 — Security', () => {
  test('10.1 — the VPN API rejects a missing session token', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/vpn/capabilities');
      return r.status;
    });
    expect(status).toBe(401);
  });

  test('10.2 — the VPN API rejects a tampered session token', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/vpn/capabilities', {
        headers: { 'x-session-token': 'totally-bogus-token-value' },
      });
      return r.status;
    });
    expect(status).toBe(401);
  });

  test('10.3 — mutating endpoints require admin, not just any session', async ({
    page,
    playwright,
  }) => {
    await gotoVpn(page, adminTokens);
    const ctx = await newCtx(playwright);
    const username = `e2evpnnonadmin${Date.now().toString(36)}`;
    const create = await ctx.post('/api/users', {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
      data: {
        username,
        password: 'NonAdminPass123!',
        fullname: 'E2E Non Admin',
        email: `${username}@example.com`,
        role_id: 1,
      },
    });
    if (create.status() !== 200 && create.status() !== 201) {
      await ctx.dispose();
      test.skip(true, 'could not provision a throwaway non-admin user');
    }
    const { token: userToken } = await apiLoginAsViewer(ctx, username, 'NonAdminPass123!');

    const status = await page.evaluate(
      async (token) => {
        const r = await fetch('/api/vpn/wireguard/peers', {
          method: 'POST',
          headers: { 'x-session-token': token, 'Content-Type': 'application/json' },
          body: JSON.stringify({ name: 'x' }),
        });
        return r.status;
      },
      userToken,
    );
    expect(status).toBe(403);

    await ctx.delete(`/api/users/${username}`, {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    });
    await ctx.dispose();
  });

  test('10.4 — no session token is ever placed in the URL', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    expect(page.url()).not.toContain(adminTokens.token);
  });

  test('10.5 — an XSS payload in the device-name field is never rendered as markup', async ({
    page,
  }) => {
    await gotoVpn(page, adminTokens);
    const addBtn = win(page).locator('[data-testid="vpn-btn"]', { hasText: /New WireGuard|new_wireguard/i });
    if ((await addBtn.count()) === 0) test.skip(true, 'WireGuard not available on this host');
    await addBtn.first().click();
    const payload = '<img src=x onerror="window.__vpnXss=true">';
    await page.locator('#wg-peer-name').fill(payload);
    const fired = await page.evaluate(() => (window as any).__vpnXss === true);
    expect(fired).toBe(false);
    await page.keyboard.press('Escape');
  });
});

// ===========================================================================
// 11. Keyboard
// ===========================================================================

test.describe('11 — Keyboard', () => {
  test('11.1 — Enter in the device-name field submits when a name is present', async ({
    page,
  }) => {
    await gotoVpn(page, adminTokens);
    const addBtn = win(page).locator('[data-testid="vpn-btn"]', { hasText: /New WireGuard|new_wireguard/i });
    if ((await addBtn.count()) === 0) test.skip(true, 'WireGuard not available on this host');
    await addBtn.first().click();
    let requestFired = false;
    page.on('request', (req) => {
      if (req.url().includes('/api/vpn/wireguard/peers') && req.method() === 'POST') requestFired = true;
    });
    await page.locator('#wg-peer-name').fill(`e2e-kbd-${Date.now().toString(36)}`);
    await page.locator('#wg-peer-name').press('Enter');
    await page.waitForTimeout(1_000);
    expect(requestFired).toBe(true);
    // Whatever the real outcome (host may lack sudo), close whatever is open.
    await page.keyboard.press('Escape').catch(() => {});
  });

  test('11.2 — Escape closes the Add-device modal', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const addBtn = win(page).locator('[data-testid="vpn-btn"]', { hasText: /New WireGuard|new_wireguard/i });
    if ((await addBtn.count()) === 0) test.skip(true, 'WireGuard not available on this host');
    await addBtn.first().click();
    await expect(page.locator('#wg-peer-name')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.locator('#wg-peer-name')).toHaveCount(0);
  });
});

// ===========================================================================
// 12. Console & network
// ===========================================================================

test.describe('12 — Console & network', () => {
  test('12.1 — no uncaught JS errors while browsing tabs', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await gotoVpn(page, adminTokens);
    const openvpnTab = win(page).locator('.hv-winhdr-tab', { hasText: 'OpenVPN' });
    if ((await openvpnTab.count()) > 0) {
      await openvpnTab.click();
      await win(page).locator('.hv-winhdr-tab', { hasText: 'WireGuard' }).click();
    }
    expect(errors).toEqual([]);
  });

  test('12.2 — no failed (4xx/5xx) requests during a normal page load', async ({ page }) => {
    const failed: string[] = [];
    page.on('response', (res) => {
      if (res.status() >= 400 && res.url().includes('/api/')) {
        failed.push(`${res.status()} ${res.url()}`);
      }
    });
    await gotoVpn(page, adminTokens);
    expect(failed).toEqual([]);
  });
});

// ===========================================================================
// 13. Real (safe) lifecycle — additive/reversible only, never real OpenVPN
//     tunneling. Skips cleanly wherever the host can't support it.
// ===========================================================================

test.describe('13 — Real device lifecycle (best-effort, always cleaned up)', () => {
  test('13.1 — create → verify listed → revoke a throwaway WireGuard device', async ({
    page,
  }) => {
    const name = `e2evpn${Date.now().toString(36)}`;
    let peerId: string | null = null;

    const createStatus: { status: number; id?: string } = await page.evaluate(
      async ({ token, csrf, name }) => {
        const r = await fetch('/api/vpn/wireguard/peers', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ name }),
        });
        const body = await r.json().catch(() => ({}));
        return { status: r.status, id: body?.data?.id };
      },
      { token: adminTokens.token, csrf: adminTokens.csrf, name },
    );

    if (createStatus.status !== 200) {
      test.skip(
        true,
        `could not create a throwaway WireGuard peer (status ${createStatus.status}) — likely no sudoers grant for wg on this host`,
      );
    }
    peerId = createStatus.id ?? null;

    try {
      await gotoVpn(page, adminTokens);
      await expect(win(page).locator('[data-testid="vpn-card-name"]', { hasText: name })).toBeVisible({
        timeout: 10_000,
      });
    } finally {
      if (peerId) {
        const delStatus = await page.evaluate(
          async ({ token, csrf, id }) => {
            const r = await fetch(`/api/vpn/wireguard/peers/${id}`, {
              method: 'DELETE',
              headers: { 'x-session-token': token, 'x-csrf-token': csrf },
            });
            return r.status;
          },
          { token: adminTokens.token, csrf: adminTokens.csrf, id: peerId },
        );
        expect(delStatus).toBe(200);
      }
    }
  });

  test('13.2 — create → verify listed → delete a throwaway OpenVPN connection record', async ({
    page,
  }) => {
    const name = `e2evpnovpn${Date.now().toString(36)}`;

    const createStatus = await page.evaluate(
      async ({ token, csrf, name }) => {
        const r = await fetch('/api/vpn/openvpn', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({
            name,
            config_content: 'client\ndev tun\nremote 192.0.2.1 1194\n',
          }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf, name },
    );

    if (createStatus !== 200) {
      test.skip(
        true,
        `could not create a throwaway OpenVPN config (status ${createStatus}) — /etc/openvpn likely not writable by this process`,
      );
    }

    try {
      await gotoVpn(page, adminTokens);
      const openvpnTab = win(page).locator('.hv-winhdr-tab', { hasText: 'OpenVPN' });
      await openvpnTab.click();
      await expect(win(page).locator('[data-testid="vpn-card-name"]', { hasText: name })).toBeVisible({
        timeout: 10_000,
      });
    } finally {
      const delStatus = await page.evaluate(
        async ({ token, csrf, name }) => {
          const r = await fetch(`/api/vpn/openvpn/${name}`, {
            method: 'DELETE',
            headers: { 'x-session-token': token, 'x-csrf-token': csrf },
          });
          return r.status;
        },
        { token: adminTokens.token, csrf: adminTokens.csrf, name },
      );
      expect(delStatus).toBe(200);
    }
  });
});

// ===========================================================================
// 14. Edge cases & memory regression
// ===========================================================================

test.describe('14 — Edge cases & memory regression', () => {
  test('14.1 — a unicode device name is accepted by the field without throwing', async ({
    page,
  }) => {
    await gotoVpn(page, adminTokens);
    const addBtn = win(page).locator('[data-testid="vpn-btn"]', { hasText: /New WireGuard|new_wireguard/i });
    if ((await addBtn.count()) === 0) test.skip(true, 'WireGuard not available on this host');
    await addBtn.first().click();
    await page.locator('#wg-peer-name').fill("配列-🔧-John's iPhone");
    await expect(page.locator('#wg-peer-name')).toHaveValue("配列-🔧-John's iPhone");
    await page.keyboard.press('Escape');
  });

  test('14.2 — a very long device name does not break layout', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const addBtn = win(page).locator('[data-testid="vpn-btn"]', { hasText: /New WireGuard|new_wireguard/i });
    if ((await addBtn.count()) === 0) test.skip(true, 'WireGuard not available on this host');
    await addBtn.first().click();
    await page.locator('#wg-peer-name').fill('a'.repeat(300));
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
    );
    expect(overflow).toBe(false);
    await page.keyboard.press('Escape');
  });

  test('14.3 — rapidly switching tabs does not crash the page', async ({ page }) => {
    await gotoVpn(page, adminTokens);
    const openvpnTab = win(page).locator('.hv-winhdr-tab', { hasText: 'OpenVPN' });
    const wgTab = win(page).locator('.hv-winhdr-tab', { hasText: 'WireGuard' });
    if ((await openvpnTab.count()) === 0) test.skip(true, 'only one/no tab available');
    for (let i = 0; i < 8; i++) {
      await (i % 2 === 0 ? openvpnTab : wgTab).click();
    }
    await expect(win(page)).toBeVisible();
  });

  test('14.4 — clearing localStorage mid-session and reloading does not crash', async ({
    page,
  }) => {
    await gotoVpn(page, adminTokens);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await page.waitForTimeout(1_000);
    const bodyText = await page.locator('body').innerText().catch(() => '');
    expect(bodyText.length).toBeGreaterThan(0);
  });

  test('14.5 — opening and closing the Add-device modal ×5 does not leak significant heap', async ({
    page,
  }) => {
    await gotoVpn(page, adminTokens);
    const addBtn = win(page).locator('[data-testid="vpn-btn"]', { hasText: /New WireGuard|new_wireguard/i });
    if ((await addBtn.count()) === 0) test.skip(true, 'WireGuard not available on this host');
    const before = await getHeapMB(page).catch(() => 0);
    for (let i = 0; i < 5; i++) {
      await addBtn.first().click();
      await expect(page.locator('#wg-peer-name')).toBeVisible();
      await page.keyboard.press('Escape');
      await expect(page.locator('#wg-peer-name')).toHaveCount(0);
    }
    const after = await getHeapMB(page).catch(() => 0);
    if (before > 0 && after > 0) {
      const delta = after - before;
      console.log(`[MEM] 5x modal open/close delta: ${delta.toFixed(1)} MB`);
      expect(delta).toBeLessThan(15);
    }
  });
});
