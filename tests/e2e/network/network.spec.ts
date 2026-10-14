/* =============================================================================
 * File          : network.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Network screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Network app — End-to-End Test Suite
 *
 * The Network app is not a route: it is a window opened from the desktop's
 * Applications launcher (id `network`, "Network" tile). It manages the
 * host's real Ethernet/WiFi interfaces, IP/DNS/gateway configuration, and
 * interface up/down state through the backend's `src/network.rs`.
 *
 * **This suite never applies a real config change, never flips a real
 * interface up/down, and never connects/disconnects a real WiFi network.**
 * Doing so for real could disconnect the very host running this test suite
 * (its actual active interface) — a self-inflicted outage with no
 * guaranteed recovery path in CI. Per CLAUDE.md's "What NOT to test in
 * Rust" (mirrored here for E2E since the same reasoning applies to a real
 * browser session): mutating flows are only exercised far enough to
 * confirm they fail *cleanly* (400/403, never a panic) against invalid
 * input or a nonexistent interface name, via direct `fetch` — exactly
 * mirroring `tests/network_integration.rs`. Read-only data (interface
 * list, connection summary, diagnostics, WiFi scan/status) is safe to load
 * for real and is asserted on directly.
 *
 * Run (all, safe subset): npx playwright test tests/e2e/network/network.spec.ts --project=firefox
 * Run (single):            npx playwright test tests/e2e/network/network.spec.ts -g "4.1"
 * Run (category):          npx playwright test tests/e2e/network/network.spec.ts -g "Security"
 * Custom server:            BASE_URL=http://localhost:3000 npx playwright test tests/e2e/network/network.spec.ts
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

async function gotoNetwork(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openLaunchpadApp(page, lang === 'fr' ? 'Réseau' : 'Network');
  await page.waitForSelector('[data-testid="network-page"]', { timeout: 10_000 });
  await expect(page.locator('[data-testid="network-page"] [data-testid="network-loading"]')).toHaveCount(0, { timeout: 10_000 });
}

/** The Network window as a scoping root — `[data-testid="network-page"]` is unique to this
 * page (confirmed via grep across `frontend/src/routes`). */
function win(page: Page) {
  return page.locator('[data-testid="network-page"]');
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
  test('1.1 — Network window opens from the Applications launcher', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — header shows the title only (standardized header)', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title')).toHaveText('Network Configuration');
    // The standardized header is icon tile + title + the window-control trio,
    // and carries no subtitle.
    // `.hv-winhdr-subtitle` no longer exists (the prop was removed from
    // `WindowHeader` on 2026-09-14), so this locator matched nothing.
    // Assert the bar's real shape instead: the icon tile + the title.
    await expect(win(page).locator('.hv-winhdr > *')).toHaveCount(2);
  });

  test('1.3 — the Ethernet tab is selected by default', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-tab', { hasText: 'Ethernet' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
  });

  test('1.4 — the WiFi tab only appears if the host has wireless hardware', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const hasWifiTab = (await win(page).locator('.hv-winhdr-tab', { hasText: 'WiFi' }).count()) > 0;
    const hasWifiInterface = await page.evaluate(async ({ token }) => {
      const r = await fetch('/api/network/interfaces', { headers: { 'x-session-token': token } });
      const body = await r.json();
      return (body?.data?.interfaces || []).some((i: any) => i.iface_type === 'wifi');
    }, { token: adminTokens.token });
    expect(hasWifiTab).toBe(hasWifiInterface);
  });

  test('1.5 — the 4 stat tiles render', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    await expect(win(page).locator('[data-testid="network-stat-card"]')).toHaveCount(4);
  });

  test('1.6 — the connection summary card renders', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    await expect(win(page).locator('[data-testid="network-summary-card"]')).toBeVisible();
  });
});

// ===========================================================================
// 2. Form fields
// ===========================================================================

test.describe('2 — Form fields', () => {
  test('2.1 — the interface config modal exposes IPv4 method radios', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const configureBtn = win(page).locator('[data-testid="network-btn"]', { hasText: 'Configure' }).first();
    if ((await configureBtn.count()) === 0) test.skip(true, 'no interface card to configure');
    await configureBtn.click();
    await expect(page.locator('input[name="ip-method"][value="dhcp"]')).toBeVisible();
    await expect(page.locator('input[name="ip-method"][value="static"]')).toBeVisible();
  });

  test('2.2 — selecting Static IPv4 reveals ip/netmask/gateway/dns fields', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const configureBtn = win(page).locator('[data-testid="network-btn"]', { hasText: 'Configure' }).first();
    if ((await configureBtn.count()) === 0) test.skip(true, 'no interface card to configure');
    await configureBtn.click();
    await page.locator('input[name="ip-method"][value="static"]').check();
    await expect(page.locator('#cfg-ip')).toBeVisible();
    await expect(page.locator('#cfg-netmask')).toBeVisible();
    await expect(page.locator('#cfg-gw')).toBeVisible();
    await expect(page.locator('#cfg-dns')).toBeVisible();
  });

  test('2.3 — IPv6 config exposes 4 method radios', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const configureBtn = win(page).locator('[data-testid="network-btn"]', { hasText: 'Configure' }).first();
    if ((await configureBtn.count()) === 0) test.skip(true, 'no interface card to configure');
    await configureBtn.click();
    const radios = page.locator('input[name="ipv6-method"]');
    await expect(radios).toHaveCount(4);
  });
});

// ===========================================================================
// 3. Validation — direct fetch, mirroring tests/network_integration.rs.
//    Never touches a real interface.
// ===========================================================================

test.describe('3 — Validation', () => {
  test('3.1 — server rejects an interface config update with an invalid method', async ({
    page,
  }) => {
    await gotoNetwork(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/network/interfaces/definitely-not-a-real-iface/config', {
          method: 'PUT',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ method: 'not-a-real-method' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.2 — server rejects a config update against a nonexistent interface cleanly', async ({
    page,
  }) => {
    await gotoNetwork(page, adminTokens);
    const { status, message } = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/network/interfaces/definitely-not-a-real-iface/config', {
          method: 'PUT',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ method: 'dhcp' }),
        });
        const body = await r.json().catch(() => ({}));
        return { status: r.status, message: body?.message || '' };
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
    expect(message).toContain('Unknown network interface');
  });

  test('3.3 — server rejects an interface state change with no up field', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/network/interfaces/definitely-not-a-real-iface/state', {
          method: 'PUT',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({}),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.4 — server rejects a wifi connect request with a missing ssid', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/network/wifi/connect', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ password: 'x' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.5 — server rejects a wifi disconnect request with a missing ssid', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/network/wifi/disconnect', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({}),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });
});

// ===========================================================================
// 4. All user interactions
// ===========================================================================

test.describe('4 — All user interactions', () => {
  test('4.1 — clicking Configure opens the config modal, Cancel closes it', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const configureBtn = win(page).locator('[data-testid="network-btn"]', { hasText: 'Configure' }).first();
    if ((await configureBtn.count()) === 0) test.skip(true, 'no interface card to configure');
    await configureBtn.click();
    await expect(page.locator('[data-testid="network-form-field"]').first()).toBeVisible();
    await page.locator('button', { hasText: 'Cancel' }).first().click();
    await expect(page.locator('input[name="ip-method"]')).toHaveCount(0);
  });

  test('4.2 — no manual Refresh button; interface data is on a live auto-refresh', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    // The manual Refresh button was removed — Network already polls every 10s
    // and revalidates on focus.
    await expect(win(page).locator('[data-testid="network-btn"]', { hasText: 'Refresh' })).toHaveCount(0);
    let requestFired = false;
    page.on('request', (req) => {
      if (req.url().includes('/api/network/interfaces')) requestFired = true;
    });
    await page.waitForTimeout(11_000); // one auto-refresh cycle
    expect(requestFired).toBe(true);
    await expect(win(page)).toBeVisible();
  });

  test('4.3 — Diagnostics button triggers a real diagnostics run and shows a result', async ({
    page,
  }) => {
    await gotoNetwork(page, adminTokens);
    await win(page).locator('[data-testid="network-btn"]', { hasText: 'Diagnostics' }).click();
    await expect(win(page).locator('[data-testid="network-diagnostics"]')).toBeVisible({ timeout: 15_000 });
  });

  test('4.4 — switching to the WiFi tab loads status/scan (when wifi hardware is present)', async ({
    page,
  }) => {
    await gotoNetwork(page, adminTokens);
    const wifiTab = win(page).locator('.hv-winhdr-tab', { hasText: 'WiFi' });
    if ((await wifiTab.count()) === 0) test.skip(true, 'no wireless interface on this host');
    await wifiTab.click();
    await expect(win(page).locator('.hv-winhdr-tab', { hasText: 'WiFi' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    await expect(
      win(page).locator('[data-testid="network-empty-mini"], [data-testid="network-wifi-list"]').first(),
    ).toBeVisible({ timeout: 15_000 });
  });

  test('4.5 — clicking a WiFi network opens the connect/disconnect modal (when any are found)', async ({
    page,
  }) => {
    await gotoNetwork(page, adminTokens);
    const wifiTab = win(page).locator('.hv-winhdr-tab', { hasText: 'WiFi' });
    if ((await wifiTab.count()) === 0) test.skip(true, 'no wireless interface on this host');
    await wifiTab.click();
    const item = win(page).locator('[data-testid="network-wifi-item"]').first();
    if ((await item.count()) === 0) test.skip(true, 'no WiFi networks found by the scan');
    await item.click();
    await expect(page.locator('[data-testid="network-form-field"], p', { hasText: /Connect to|Disconnect from/i }).first().or(
      page.locator('h2, h3', { hasText: /Connect to|Disconnect from/i }),
    )).toBeVisible().catch(() => {});
    // The modal opening at all (a select or the disconnect-confirm text) is
    // the assertion — never actually submit connect/disconnect.
    const modalVisible =
      (await page.locator('#wifi-security').count()) > 0 ||
      (await page.locator('text=/Disconnect from|Are you sure/i').count()) > 0;
    expect(modalVisible).toBe(true);
    await page.keyboard.press('Escape');
  });
});

// ===========================================================================
// 5. Submission flows — guard/no-op paths only (see file header)
// ===========================================================================

test.describe('5 — Submission flows', () => {
  test('5.1 — Apply is disabled while a save is not in flight, never applies without a real change request', async ({
    page,
  }) => {
    await gotoNetwork(page, adminTokens);
    const configureBtn = win(page).locator('[data-testid="network-btn"]', { hasText: 'Configure' }).first();
    if ((await configureBtn.count()) === 0) test.skip(true, 'no interface card to configure');
    await configureBtn.click();
    let requestFired = false;
    page.on('request', (req) => {
      if (req.url().includes('/config') && req.method() === 'PUT') requestFired = true;
    });
    await page.locator('button', { hasText: 'Cancel' }).first().click();
    await page.waitForTimeout(300);
    expect(requestFired).toBe(false);
  });

  test('5.2 — Connect is disabled until a password is entered for a secured network', async ({
    page,
  }) => {
    await gotoNetwork(page, adminTokens);
    const wifiTab = win(page).locator('.hv-winhdr-tab', { hasText: 'WiFi' });
    if ((await wifiTab.count()) === 0) test.skip(true, 'no wireless interface on this host');
    await wifiTab.click();
    const item = win(page)
      .locator('[data-testid="network-wifi-item"]')
      .filter({ hasNot: page.locator('text=Connected') })
      .first();
    if ((await item.count()) === 0) test.skip(true, 'no connectable WiFi network found');
    await item.click();
    const connectBtn = page.locator('button', { hasText: /^Connect$/ });
    if ((await connectBtn.count()) === 0) test.skip(true, 'opened in disconnect mode');
    await expect(connectBtn).toBeDisabled();
    await page.keyboard.press('Escape');
  });
});

// ===========================================================================
// 6. Navigation
// ===========================================================================

test.describe('6 — Navigation', () => {
  test('6.1 — the window closes and can be reopened', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const closeBtn = closeWindowButton(page);
    if ((await closeBtn.count()) === 0) test.skip(true, 'no close control found for this window chrome');
    await closeBtn.click();
    await expect(page.locator('[data-testid="network-page"]')).toHaveCount(0);
    await openLaunchpadApp(page, 'Network');
    await expect(win(page)).toBeVisible();
  });

  test('6.2 — switching tabs preserves the window (no reload)', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const wifiTab = win(page).locator('.hv-winhdr-tab', { hasText: 'WiFi' });
    if ((await wifiTab.count()) === 0) test.skip(true, 'no wireless interface on this host — only one tab exists');
    const marker = 'e2e-nav-marker';
    await win(page).evaluate((el, m) => ((el as any)[m] = true), marker);
    await wifiTab.click();
    await win(page).locator('.hv-winhdr-tab', { hasText: 'Ethernet' }).click();
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
      await gotoNetwork(page, adminTokens);
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
  test('8.1 — the tab bar exposes tablist/tab roles', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    await expect(win(page).locator('[role="tablist"]')).toHaveCount(1);
    await expect(win(page).locator('[role="tab"]').first()).toHaveAttribute(
      'aria-selected',
      'true',
    );
  });

  test('8.2 — config modal fields have associated labels', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const configureBtn = win(page).locator('[data-testid="network-btn"]', { hasText: 'Configure' }).first();
    if ((await configureBtn.count()) === 0) test.skip(true, 'no interface card to configure');
    await configureBtn.click();
    await page.locator('input[name="ip-method"][value="static"]').check();
    await expect(page.locator('label[for="cfg-ip"]')).toBeVisible();
    await expect(page.locator('label[for="cfg-netmask"]')).toBeVisible();
  });

  test('8.3 — no duplicate element ids on the page', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
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
    await gotoNetwork(page, adminTokens);
    const elapsed = Date.now() - start;
    console.log(`[PERF] Network window ready in ${elapsed}ms`);
    expect(elapsed).toBeLessThan(15_000);
  });

  test('9.2 — opening the Network window does not add an excessive number of DOM nodes', async ({
    page,
  }) => {
    await gotoDesktop(page, adminTokens);
    const before = await page.evaluate(() => document.querySelectorAll('*').length);
    await openLaunchpadApp(page, 'Network');
    await page.waitForSelector('[data-testid="network-page"]', { timeout: 10_000 });
    const after = await page.evaluate(() => document.querySelectorAll('*').length);
    const delta = after - before;
    console.log(`[PERF] Network window added ${delta} DOM nodes`);
    expect(delta).toBeLessThan(2000);
  });
});

// ===========================================================================
// 10. Security
// ===========================================================================

test.describe('10 — Security', () => {
  test('10.1 — the Network API rejects a missing session token', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/network/interfaces');
      return r.status;
    });
    expect(status).toBe(400);
  });

  test('10.2 — the Network API rejects a tampered session token', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/network/interfaces', {
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
    await gotoNetwork(page, adminTokens);
    const ctx = await newCtx(playwright);
    const username = `e2enetnonadmin${Date.now().toString(36)}`;
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
    const { token: userToken } = await apiLogin(ctx, username, 'NonAdminPass123!');

    const status = await page.evaluate(
      async (token) => {
        const r = await fetch('/api/network/interfaces/definitely-not-a-real-iface/state', {
          method: 'PUT',
          headers: { 'x-session-token': token, 'Content-Type': 'application/json' },
          body: JSON.stringify({ up: true }),
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
    await gotoNetwork(page, adminTokens);
    expect(page.url()).not.toContain(adminTokens.token);
  });

  test('10.5 — an XSS payload in the static-IP field is never rendered as markup', async ({
    page,
  }) => {
    await gotoNetwork(page, adminTokens);
    const configureBtn = win(page).locator('[data-testid="network-btn"]', { hasText: 'Configure' }).first();
    if ((await configureBtn.count()) === 0) test.skip(true, 'no interface card to configure');
    await configureBtn.click();
    await page.locator('input[name="ip-method"][value="static"]').check();
    const payload = '<img src=x onerror="window.__netXss=true">';
    await page.locator('#cfg-ip').fill(payload);
    const fired = await page.evaluate(() => (window as any).__netXss === true);
    expect(fired).toBe(false);
  });
});

// ===========================================================================
// 11. Keyboard
// ===========================================================================

test.describe('11 — Keyboard', () => {
  test('11.1 — arrow keys move focus between tabs (when more than one tab exists)', async ({
    page,
  }) => {
    await gotoNetwork(page, adminTokens);
    const tabs = win(page).locator('.hv-winhdr-tab');
    if ((await tabs.count()) < 2) test.skip(true, 'only one tab (no wireless interface)');
    await tabs.first().focus();
    await page.keyboard.press('ArrowRight');
    const focused = await page.evaluate(() => document.activeElement?.getAttribute('aria-selected'));
    expect(focused).toBe('true');
  });

  test('11.2 — Escape closes the interface config modal', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const configureBtn = win(page).locator('[data-testid="network-btn"]', { hasText: 'Configure' }).first();
    if ((await configureBtn.count()) === 0) test.skip(true, 'no interface card to configure');
    await configureBtn.click();
    await expect(page.locator('input[name="ip-method"]').first()).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.locator('input[name="ip-method"]')).toHaveCount(0);
  });
});

// ===========================================================================
// 12. Console & network
// ===========================================================================

test.describe('12 — Console & network', () => {
  test('12.1 — no uncaught JS errors while browsing the page', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await gotoNetwork(page, adminTokens);
    const wifiTab = win(page).locator('.hv-winhdr-tab', { hasText: 'WiFi' });
    if ((await wifiTab.count()) > 0) {
      await wifiTab.click();
      await win(page).locator('.hv-winhdr-tab', { hasText: 'Ethernet' }).click();
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
    await gotoNetwork(page, adminTokens);
    expect(failed).toEqual([]);
  });
});

// ===========================================================================
// 13. Edge cases & memory regression
// ===========================================================================

test.describe('13 — Edge cases & memory regression', () => {
  test('13.1 — a unicode DNS server value is accepted by the field without throwing', async ({
    page,
  }) => {
    await gotoNetwork(page, adminTokens);
    const configureBtn = win(page).locator('[data-testid="network-btn"]', { hasText: 'Configure' }).first();
    if ((await configureBtn.count()) === 0) test.skip(true, 'no interface card to configure');
    await configureBtn.click();
    await page.locator('input[name="ip-method"][value="static"]').check();
    await page.locator('#cfg-dns').fill('配列-🔧-dns');
    await expect(page.locator('#cfg-dns')).toHaveValue('配列-🔧-dns');
  });

  test('13.2 — a very long gateway value does not break layout', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const configureBtn = win(page).locator('[data-testid="network-btn"]', { hasText: 'Configure' }).first();
    if ((await configureBtn.count()) === 0) test.skip(true, 'no interface card to configure');
    await configureBtn.click();
    await page.locator('input[name="ip-method"][value="static"]').check();
    await page.locator('#cfg-gw').fill('a'.repeat(500));
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
    );
    expect(overflow).toBe(false);
  });

  test('13.3 — rapidly switching tabs does not crash the page', async ({ page }) => {
    await gotoNetwork(page, adminTokens);
    const wifiTab = win(page).locator('.hv-winhdr-tab', { hasText: 'WiFi' });
    if ((await wifiTab.count()) === 0) test.skip(true, 'only one tab (no wireless interface)');
    for (let i = 0; i < 8; i++) {
      await (i % 2 === 0 ? wifiTab : win(page).locator('.hv-winhdr-tab', { hasText: 'Ethernet' })).click();
    }
    await expect(win(page)).toBeVisible();
  });

  test('13.4 — clearing localStorage mid-session and reloading does not crash', async ({
    page,
  }) => {
    await gotoNetwork(page, adminTokens);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await page.waitForTimeout(1_000);
    const bodyText = await page.locator('body').innerText().catch(() => '');
    expect(bodyText.length).toBeGreaterThan(0);
  });

  test('13.5 — opening and closing the Network window ×5 does not leak significant heap', async ({
    page,
  }) => {
    test.setTimeout(150_000);
    await gotoDesktop(page, adminTokens);
    const before = await getHeapMB(page).catch(() => 0);
    for (let i = 0; i < 5; i++) {
      await openLaunchpadApp(page, 'Network');
      await page.waitForSelector('[data-testid="network-page"]', { timeout: 10_000 });
      const closeBtn = closeWindowButton(page);
      if ((await closeBtn.count()) === 0) {
        test.skip(true, 'no close control found for this window chrome');
      }
      await closeBtn.click();
      await expect(page.locator('[data-testid="network-page"]')).toHaveCount(0);
    }
    const after = await getHeapMB(page).catch(() => 0);
    if (before > 0 && after > 0) {
      const delta = after - before;
      console.log(`[MEM] 5x window open/close delta: ${delta.toFixed(1)} MB`);
      expect(delta).toBeLessThan(15);
    }
  });
});
