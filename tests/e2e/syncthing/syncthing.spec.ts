/* =============================================================================
 * File          : syncthing.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Syncthing screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Syncthing app — End-to-End Test Suite
 *
 * Not a route: a window opened from the desktop's Applications launcher
 * (id `syncthing`, "Syncthing" tile). It surfaces a locally-running
 * Syncthing daemon's devices/folders/connections and can restart it or
 * pause/resume a paired device, via `src/handlers/syncthing.rs`.
 *
 * **Safety model:** the 2026-07-16 audit's headline finding was that all
 * 10 handlers had **no auth check at all** — `restart` (full daemon
 * restart) and pause/resume of any paired device included. This suite
 * verifies the fix: every endpoint requires an **admin** session, and the
 * body-taking ones reject a malformed body with 422 before any HTTP call.
 * It never restarts a real daemon or pauses a real device.
 *
 * A full device/folder lifecycle needs a running Syncthing instance with a
 * paired peer, which this environment does not have — so section 13 is
 * **opt-in** (`SYNCTHING_E2E=1`) and, even then, only does read-only calls
 * plus a folder rescan against whatever instance is up. Without a daemon
 * the page shows its "not available" state, which this suite tests as a
 * first-class case.
 *
 * The management flow added by ROADMAP 18.1 (add/remove a folder or a device)
 * is exercised in section 7 against a **mocked daemon**: the read endpoints are
 * fulfilled with fixtures and every write is recorded, so the assertions are
 * about what the screen sent and what it did with the answer. Nothing in this
 * file ever writes a real daemon's configuration.
 *
 * Run (all):     npx playwright test tests/e2e/syncthing/syncthing.spec.ts --project=firefox
 * Run (single):  npx playwright test tests/e2e/syncthing/syncthing.spec.ts -g "1.1"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/syncthing/syncthing.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const RUN_LIVE = process.env.SYNCTHING_E2E === '1';

/** A structurally valid device ID (56 base-32 characters, eight groups). */
const DEVICE_ID = 'AAAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH';

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

async function gotoSyncthing(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openLaunchpadApp(page, 'Syncthing');
  await page.waitForSelector('[data-testid="syncthing-page"]', { timeout: 10_000 });
  await expect(page.locator('[data-testid="syncthing-page"] [data-testid="syncthing-loading"]')).toHaveCount(0, { timeout: 15_000 });
}

function win(page: Page) {
  return page.locator('[data-testid="syncthing-page"]');
}

// ---------------------------------------------------------------------------
// A fake daemon (ROADMAP 18.1)
// ---------------------------------------------------------------------------
//
// The management flow has to be exercised without touching a real Syncthing
// instance: creating a folder writes the daemon's persisted configuration, and
// a CI host has no daemon at all (the screen would show "not available").
// These routes answer the read endpoints with fixture data and record every
// write, so a test can assert *what was sent* and *what the screen did with the
// answer* — the behaviour, not the envelope.

interface DaemonMock {
  folders: any[];
  devices: any[];
  /** Every non-GET request the screen issued, in order. */
  writes: { method: string; path: string; body: any }[];
  /** Override the answer of a folder write (to test a refusal). */
  folderWrite?: (body: any) => { status?: number; body: unknown } | null;
  /** Override the answer of a device write (to test a refusal). */
  deviceWrite?: (body: any) => { status?: number; body: unknown } | null;
}

function folderFixture(over: Record<string, unknown> = {}) {
  return {
    id: 'photos',
    label: 'Photos',
    path: '/mnt/pools/data/photos',
    paused: false,
    global_bytes: 1024,
    global_files: 10,
    local_bytes: 1024,
    local_files: 10,
    need_bytes: 0,
    need_files: 0,
    in_sync_files: 10,
    receive_only: false,
    devices: [],
    ...over,
  };
}

function deviceFixture(over: Record<string, unknown> = {}) {
  return {
    device_id: DEVICE_ID,
    name: 'Laptop',
    addresses: ['dynamic'],
    compression: 'metadata',
    paused: false,
    connected: true,
    address: '10.0.0.5:22000',
    client_version: 'v1.27.2',
    completion: 100,
    in_bytes_total: 2048,
    out_bytes_total: 1024,
    ...over,
  };
}

function newMock(): DaemonMock {
  return { folders: [], devices: [], writes: [] };
}

async function installDaemonMock(page: Page, mock: DaemonMock) {
  const json = (body: unknown, status = 200) => ({
    status,
    contentType: 'application/json',
    body: JSON.stringify(body),
  });
  const success = (data: unknown) => ({ success: true, data, error: null });

  await page.route('**/api/syncthing/status*', (route) =>
    route.fulfill(
      json(
        success({
          available: true,
          version: 'v1.27.2',
          my_id: 'SELF-SELF',
          uptime: 3600,
          last_seen: null,
        }),
      ),
    ),
  );

  const handler = (kind: 'folders' | 'devices') => async (route: any) => {
    const request = route.request();
    const method = request.method();
    const path = new URL(request.url()).pathname;

    if (method === 'GET') {
      return route.fulfill(json(success(kind === 'folders' ? mock.folders : mock.devices)));
    }

    const body = request.postDataJSON?.() ?? {};
    mock.writes.push({ method, path, body });

    const override = kind === 'folders' ? mock.folderWrite : mock.deviceWrite;
    const custom = override?.(body);
    if (custom) return route.fulfill(json(custom.body, custom.status ?? 200));

    if (method === 'POST') {
      const created =
        kind === 'folders'
          ? folderFixture({ id: body.id, label: body.label, path: body.path })
          : deviceFixture({ device_id: body.device_id, name: body.name });
      (kind === 'folders' ? mock.folders : mock.devices).push(created);
      return route.fulfill(json(success(created)));
    }

    // DELETE /api/syncthing/{kind}/{id}
    const id = decodeURIComponent(path.split('/').pop() || '');
    if (kind === 'folders') mock.folders = mock.folders.filter((f) => f.id !== id);
    else mock.devices = mock.devices.filter((d) => d.device_id !== id);
    return route.fulfill(json(success('removed')));
  };

  await page.route('**/api/syncthing/folders*', handler('folders'));
  await page.route('**/api/syncthing/devices*', handler('devices'));
}

/** The open dialog (bits-ui portals it to <body>). */
function dialog(page: Page) {
  return page.locator('[role="dialog"]');
}

async function openTab(page: Page, label: RegExp) {
  await win(page).locator('.hv-winhdr-tab', { hasText: label }).first().click();
}

async function api(
  page: Page,
  t: Tokens | null,
  method: string,
  path: string,
  body?: unknown,
) {
  return page.evaluate(
    async ({ t, method, path, body }) => {
      const headers: Record<string, string> = {};
      if (t) {
        headers['x-session-token'] = t.token;
        headers['x-csrf-token'] = t.csrf;
      }
      if (body !== undefined) headers['Content-Type'] = 'application/json';
      const r = await fetch(path, {
        method,
        headers,
        body: body !== undefined ? JSON.stringify(body) : undefined,
      });
      const j = await r.json().catch(() => ({}));
      return { http: r.status, success: j?.success, error: j?.error || '', data: j?.data };
    },
    { t, method, path, body },
  );
}

let adminTokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
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
  test('1.1 — Syncthing window opens from the Applications launcher', async ({ page }) => {
    await gotoSyncthing(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — the loading spinner clears (no stuck spinner)', async ({ page }) => {
    await gotoSyncthing(page, adminTokens);
    await expect(win(page).locator('[data-testid="syncthing-loading"]')).toHaveCount(0);
  });

  test('1.3 — header shows "Syncthing" and 3 tabs', async ({ page }) => {
    await gotoSyncthing(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title')).toContainText('Syncthing');
    await expect(win(page).locator('.hv-winhdr-tab')).toHaveCount(3);
  });

  test('1.4 — adapts to state: stats grid (daemon up) or the "not available" panel', async ({
    page,
  }) => {
    await gotoSyncthing(page, adminTokens);
    const stats = await win(page).locator('[data-testid="syncthing-stats"]').count();
    const empty = await win(page).locator('[data-testid="syncthing-empty"]').count();
    expect(stats + empty).toBeGreaterThan(0);
  });

  test('1.5 — the Restart button is disabled while the daemon is unavailable', async ({ page }) => {
    await gotoSyncthing(page, adminTokens);
    const st = await api(page, adminTokens, 'GET', '/api/syncthing/status');
    const btn = win(page).locator('[data-testid="syncthing-btn-primary"]');
    if (st.data?.available) {
      await expect(btn).toBeEnabled();
    } else {
      await expect(btn).toBeDisabled();
    }
  });
});

// ===========================================================================
// 2. Tab navigation
// ===========================================================================

test.describe('2 — Tab navigation', () => {
  for (const label of [/device/i, /folder/i, /overview|aperçu|übersicht|resumen/i]) {
    test(`2.${label.source.slice(0, 8)} — switching tabs does not crash`, async ({ page }) => {
      await gotoSyncthing(page, adminTokens);
      await win(page).locator('.hv-winhdr-tab', { hasText: label }).first().click();
      await expect(win(page)).toBeVisible();
    });
  }
});

// ===========================================================================
// 3. Security — every endpoint is admin-only
// ===========================================================================

test.describe('3 — Security', () => {
  test('3.1 — every endpoint rejects a missing session token', async ({ page }) => {
    await gotoSyncthing(page, adminTokens);
    const results = await page.evaluate(async () => {
      const eps: [string, string, any][] = [
        ['GET', '/api/syncthing/status', null],
        ['GET', '/api/syncthing/config', null],
        ['GET', '/api/syncthing/devices', null],
        ['GET', '/api/syncthing/folders', null],
        ['GET', '/api/syncthing/connections', null],
        ['GET', '/api/syncthing/errors', null],
        ['POST', '/api/syncthing/restart', null],
        ['POST', '/api/syncthing/pause', { device: 'X' }],
        ['POST', '/api/syncthing/resume', { device: 'X' }],
        ['POST', '/api/syncthing/scan', { folder: 'default' }],
        // ROADMAP 18.1 — the management routes are the same privilege bar.
        ['POST', '/api/syncthing/folders', { id: 'x', path: '/tmp/x' }],
        ['DELETE', '/api/syncthing/folders/x', null],
        ['POST', '/api/syncthing/devices', { device_id: DEVICE_ID }],
        ['DELETE', `/api/syncthing/devices/${DEVICE_ID}`, null],
      ];
      const out: Record<string, boolean> = {};
      for (const [m, p, b] of eps) {
        const r = await fetch(p, {
          method: m,
          headers: b ? { 'Content-Type': 'application/json' } : {},
          body: b ? JSON.stringify(b) : undefined,
        });
        const j = await r.json().catch(() => ({}));
        out[m + ' ' + p] = j?.success === false;
      }
      return out;
    });
    for (const [k, denied] of Object.entries(results)) {
      expect(denied, `${k} must be denied`).toBe(true);
    }
  });

  test('3.2 — a tampered token is rejected', async ({ page }) => {
    await gotoSyncthing(page, adminTokens);
    const r = await api(page, { ...adminTokens, token: adminTokens.token + 'x' }, 'GET', '/api/syncthing/devices');
    expect(r.success).toBe(false);
    expect(r.error).toMatch(/invalid or expired/i);
  });

  test('3.3 — a non-admin session cannot enumerate devices or restart the daemon', async ({
    playwright,
  }) => {
    const ctx = await newCtx(playwright);
    const adminHeaders = { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf };
    const uname = `stviewer${Date.now().toString(36)}`;
    const created = await ctx.post('/api/users', {
      headers: adminHeaders,
      data: { username: uname, password: 'Viewer12345!', fullname: 'V', email: '', role_id: 1, groups: [], home_enabled: false },
    });
    if (!created.ok()) {
      await ctx.dispose();
      test.skip(true, `could not create a non-admin user (${created.status()})`);
    }
    try {
      const { token, csrf } = await apiLoginAsViewer(ctx, uname, 'Viewer12345!');
      const h = { 'x-session-token': token, 'x-csrf-token': csrf };
      for (const [method, path, data] of [
        ['get', '/api/syncthing/devices', undefined],
        ['get', '/api/syncthing/config', undefined],
        ['post', '/api/syncthing/restart', {}],
        ['post', '/api/syncthing/pause', { device: 'X' }],
      ] as const) {
        const r = await (ctx as any)[method](path, data ? { headers: h, data } : { headers: h });
        const body = await r.json().catch(() => ({}));
        expect(body?.error || '', `${method} ${path}`).toMatch(/admin privileges/i);
      }
    } finally {
      await ctx.delete(`/api/users/${uname}`, { headers: adminHeaders }).catch(() => {});
      await ctx.dispose();
    }
  });

  test('3.4 — no session token appears in any URL', async ({ page }) => {
    const urls: string[] = [];
    page.on('request', (r) => urls.push(r.url()));
    await gotoSyncthing(page, adminTokens);
    await page.waitForTimeout(1000);
    for (const u of urls) expect(u).not.toContain(adminTokens.token);
  });
});

// ===========================================================================
// 4. Validation — malformed bodies + the daemon-down error path
// ===========================================================================

test.describe('4 — Validation', () => {
  test('4.1 — pause/resume/scan reject a body missing the required field (422)', async ({ page }) => {
    await gotoSyncthing(page, adminTokens);
    for (const [path, body] of [
      ['/api/syncthing/pause', { not_device: 'x' }],
      ['/api/syncthing/resume', {}],
      ['/api/syncthing/scan', { not_folder: 'x' }],
    ] as const) {
      const r = await api(page, adminTokens, 'POST', path, body);
      expect(r.http, path).toBe(422);
    }
  });

  test('4.2 — pause/scan with a valid body fail cleanly when the daemon is down (no 500)', async ({
    page,
  }) => {
    await gotoSyncthing(page, adminTokens);
    const st = await api(page, adminTokens, 'GET', '/api/syncthing/status');
    if (st.data?.available) {
      test.skip(true, 'a Syncthing daemon is actually running — the down-path test does not apply');
    }
    for (const [path, body] of [
      ['/api/syncthing/pause', { device: 'AAAA-BBBB' }],
      ['/api/syncthing/scan', { folder: 'default' }],
    ] as const) {
      const r = await api(page, adminTokens, 'POST', path, body);
      expect(r.http, path).toBe(200);
      expect(r.success, path).toBe(false);
      expect(r.error, path).toMatch(/failed to connect to syncthing|syncthing api error/i);
    }
  });

  test('4.3 — status always returns a well-formed envelope with an `available` bool', async ({
    page,
  }) => {
    await gotoSyncthing(page, adminTokens);
    const r = await api(page, adminTokens, 'GET', '/api/syncthing/status');
    expect(r.http).toBe(200);
    expect(r.success).toBe(true);
    expect(typeof r.data?.available).toBe('boolean');
  });

  test('4.4 — an unsendable API key is explained, not turned into "install Syncthing"', async ({
    page,
  }) => {
    // ROADMAP 18.2: the key in config.xml used to panic the handler (a control
    // character or a non-ASCII byte), and `status` reported the daemon as "not
    // running". The server now answers a named error and the screen shows it —
    // and must not simultaneously offer to install a daemon that is installed.
    const reason =
      'Syncthing API key in config.xml is not a valid HTTP header value (it contains control characters or non-ASCII bytes); regenerate it in Syncthing\'s GUI or remove the <apikey> line';
    await page.route('**/api/syncthing/status*', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: false, data: null, error: reason }),
      }),
    );
    try {
      await gotoSyncthing(page, adminTokens);
      const alert = win(page).locator('[data-testid="syncthing-error"]');
      await expect(alert).toBeVisible({ timeout: 8_000 });
      await expect(alert).toContainText('not a valid HTTP header value');
      await expect(win(page).locator('[data-testid="syncthing-empty"]')).toHaveCount(0);
    } finally {
      await page.unroute('**/api/syncthing/status*');
    }
  });

  test('4.5 — a refused scan shows the server reason instead of doing nothing', async ({ page }) => {
    // ROADMAP 18.2 / X.3: scanFolder's `else {}` was empty, so a refusal was
    // indistinguishable from a slow request.
    const ok = (body: unknown) =>
      JSON.stringify({ success: true, data: body, error: null });
    await page.route('**/api/syncthing/status*', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: ok({ available: true, version: 'v1.27.2', my_id: 'AAAA-BBBB', uptime: 3600, last_seen: null }),
      }),
    );
    await page.route('**/api/syncthing/devices*', (route) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: ok([]) }),
    );
    await page.route('**/api/syncthing/folders*', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: ok([
          {
            id: 'default', label: 'Default Folder', path: '/srv/sync/default', paused: false,
            global_bytes: 1000, global_files: 10, local_bytes: 1000, local_files: 10,
            need_bytes: 0, need_files: 0, in_sync_files: 10, receive_only: false, devices: [],
          },
        ]),
      }),
    );
    await page.route('**/api/syncthing/scan*', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: false, data: null, error: 'Syncthing API error (500): scan refused' }),
      }),
    );
    try {
      await gotoSyncthing(page, adminTokens);
      const scan = win(page).locator('[data-testid="syncthing-action-btn"]').first();
      await expect(scan).toBeVisible({ timeout: 8_000 });
      await scan.click();
      const alert = win(page).locator('[data-testid="syncthing-error"]');
      await expect(alert).toBeVisible({ timeout: 8_000 });
      await expect(alert).toContainText('scan refused');
    } finally {
      await page.unroute('**/api/syncthing/status*');
      await page.unroute('**/api/syncthing/devices*');
      await page.unroute('**/api/syncthing/folders*');
      await page.unroute('**/api/syncthing/scan*');
    }
  });
});

// ===========================================================================
// 5. Responsive
// ===========================================================================

test.describe('5 — Responsive', () => {
  for (const [w, h, label] of [
    [360, 800, '360px'],
    [768, 1024, '768px'],
    [1920, 1080, '1920px'],
  ] as const) {
    test(`5.${label} — no horizontal overflow`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: h });
      await gotoSyncthing(page, adminTokens);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
      );
      expect(overflow).toBe(false);
    });
  }
});

// ===========================================================================
// 6. Accessibility & console
// ===========================================================================

test.describe('6 — Accessibility & console', () => {
  test('6.1 — the tab bar exposes tablist/tab roles', async ({ page }) => {
    await gotoSyncthing(page, adminTokens);
    await expect(win(page).locator('[role="tablist"]')).toHaveCount(1);
    expect(await win(page).locator('[role="tab"]').count()).toBeGreaterThanOrEqual(3);
  });

  test('6.2 — no duplicate element ids', async ({ page }) => {
    await gotoSyncthing(page, adminTokens);
    const dupes = await page.evaluate(() => {
      const seen = new Set<string>();
      const dup: string[] = [];
      for (const el of Array.from(document.querySelectorAll('[id]'))) {
        if (seen.has(el.id)) dup.push(el.id);
        seen.add(el.id);
      }
      return dup;
    });
    expect(dupes).toEqual([]);
  });

  test('6.3 — no uncaught JS errors browsing all tabs', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await gotoSyncthing(page, adminTokens);
    for (const label of [/device/i, /folder/i, /overview|aperçu/i]) {
      await win(page).locator('.hv-winhdr-tab', { hasText: label }).first().click().catch(() => {});
      await page.waitForTimeout(300);
    }
    expect(errors).toEqual([]);
  });

  test('6.4 — no failed syncthing requests during a normal load', async ({ page }) => {
    // handlers answer HTTP 200 even when the daemon is down (the failure is
    // in the body), so a 4xx/5xx here would be a real regression.
    const bad: string[] = [];
    page.on('response', (r) => {
      if (r.url().includes('/api/syncthing/') && r.status() >= 400) bad.push(`${r.status()} ${r.url()}`);
    });
    await gotoSyncthing(page, adminTokens);
    await page.waitForTimeout(2500);
    expect(bad).toEqual([]);
  });
});

// ===========================================================================
// 7. Folder and device management (ROADMAP 18.1)
// ===========================================================================
//
// Before this landed, the screen could read the daemon's topology and
// pause/resume a paired device — but there was no route and no control to add
// or remove a folder or a device, so a share or a peer could not be onboarded
// from HomeVault at all. These tests pin the behaviour the new dialogs promise:
// the values that are sent, the server's reason staying inside the dialog, and
// a removal that names its consequence and only happens after confirmation.

test.describe('7 — Folder and device management', () => {
  test('7.1 — the Folders tab offers "Add folder", and Create waits for the two required fields', async ({
    page,
  }) => {
    const mock = newMock();
    await installDaemonMock(page, mock);
    await gotoSyncthing(page, adminTokens);
    await openTab(page, /folder/i);

    const add = win(page).locator('[data-testid="syncthing-add-folder"]');
    await expect(add).toBeVisible({ timeout: 8_000 });

    await add.click();
    await expect(dialog(page)).toBeVisible();
    // The ID is the folder's identity, the path is where its files live: the
    // dialog cannot sensibly submit without both.
    const submit = page.locator('[data-testid="syncthing-folder-submit"]');
    await expect(submit).toBeDisabled();

    await page.locator('[data-testid="syncthing-folder-id-input"]').fill('photos');
    await expect(submit).toBeDisabled(); // path still missing
    await page.locator('#hv-syncthing-folder-path').fill('/mnt/pools/data/photos');
    await expect(submit).toBeEnabled();
    // Nothing was sent just by typing.
    expect(mock.writes).toHaveLength(0);
  });

  test('7.2 — creating a folder sends the trimmed values and the new folder appears', async ({
    page,
  }) => {
    const mock = newMock();
    mock.devices = [deviceFixture()];
    await installDaemonMock(page, mock);
    await gotoSyncthing(page, adminTokens);
    await openTab(page, /folder/i);
    await win(page).locator('[data-testid="syncthing-add-folder"]').click();

    await page.locator('[data-testid="syncthing-folder-id-input"]').fill('  photos  ');
    await page.locator('[data-testid="syncthing-folder-label-input"]').fill('Photos');
    await page.locator('#hv-syncthing-folder-path').fill('/mnt/pools/data/photos');
    await page.locator('[data-testid="syncthing-folder-type-select"]').selectOption('sendonly');
    await page.locator('[data-testid="syncthing-folder-submit"]').click();

    await expect(dialog(page)).toHaveCount(0, { timeout: 8_000 });
    expect(mock.writes).toHaveLength(1);
    expect(mock.writes[0].method).toBe('POST');
    expect(mock.writes[0].path).toBe('/api/syncthing/folders');
    expect(mock.writes[0].body.id).toBe('photos'); // trimmed server-side
    expect(mock.writes[0].body.label).toBe('Photos');
    expect(mock.writes[0].body.path).toBe('/mnt/pools/data/photos');
    expect(mock.writes[0].body.type).toBe('sendonly');
    // The one paired device was pre-ticked, so the folder is shared with it.
    expect(mock.writes[0].body.devices).toEqual([DEVICE_ID]);

    // Visible feedback (simplicity rule 5) and the refreshed list.
    await expect(win(page).locator('[data-testid="syncthing-success"]')).toContainText('Photos');
    await expect(win(page).locator('[data-testid="syncthing-folder-list-row"]')).toHaveCount(1);
  });

  test('7.3 — a refused create keeps the dialog open and shows the server reason in it', async ({
    page,
  }) => {
    const mock = newMock();
    mock.folderWrite = () => ({
      body: {
        success: false,
        data: null,
        error: "Folder path '/etc' must be inside a storage location HomeVault manages",
      },
    });
    await installDaemonMock(page, mock);
    await gotoSyncthing(page, adminTokens);
    await openTab(page, /folder/i);
    await win(page).locator('[data-testid="syncthing-add-folder"]').click();

    await page.locator('[data-testid="syncthing-folder-id-input"]').fill('etc');
    await page.locator('#hv-syncthing-folder-path').fill('/etc');
    await page.locator('[data-testid="syncthing-folder-submit"]').click();

    // The reason is next to the field that is wrong, not in a page banner the
    // user would have to correlate with a dialog they cannot see.
    const formError = page.locator('[data-testid="syncthing-form-error"]');
    await expect(formError).toBeVisible({ timeout: 8_000 });
    await expect(formError).toContainText('must be inside a storage location');
    await expect(dialog(page)).toBeVisible();
    await expect(page.locator('[data-testid="syncthing-folder-id-input"]')).toHaveValue('etc');
    await expect(win(page).locator('[data-testid="syncthing-error"]')).toHaveCount(0);
  });

  test('7.4 — a malformed device ID is explained inside the pairing dialog', async ({ page }) => {
    const mock = newMock();
    mock.deviceWrite = () => ({
      body: {
        success: false,
        data: null,
        error: 'Device ID must be 56 characters (dashes are optional), got 9',
      },
    });
    await installDaemonMock(page, mock);
    await gotoSyncthing(page, adminTokens);
    await openTab(page, /device/i);
    await win(page).locator('[data-testid="syncthing-add-device"]').click();

    await page.locator('[data-testid="syncthing-device-id-input"]').fill('AAAA-BBBB');
    await page.locator('[data-testid="syncthing-device-name-input"]').fill('Laptop');
    await page.locator('[data-testid="syncthing-device-submit"]').click();

    await expect(page.locator('[data-testid="syncthing-form-error"]')).toContainText(
      '56 characters',
    );
    expect(mock.writes[0].body.device_id).toBe('AAAA-BBBB');
    await expect(dialog(page)).toBeVisible();
  });

  test('7.5 — removing a folder names its consequence, sends nothing on Cancel, deletes on confirm', async ({
    page,
  }) => {
    const mock = newMock();
    mock.folders = [folderFixture()];
    await installDaemonMock(page, mock);
    await gotoSyncthing(page, adminTokens);
    await openTab(page, /folder/i);

    await win(page).locator('[data-testid="syncthing-folder-list-row"]').first().click();
    await win(page).locator('[data-testid="syncthing-remove-folder"]').click();

    // The dialog must say what happens to the files — a home user has to be
    // able to decide without guessing (simplicity rule 6).
    await expect(dialog(page)).toBeVisible();
    await expect(dialog(page)).toContainText('Photos');
    await expect(page.locator('[data-testid="syncthing-remove-desc"]')).toContainText(
      'Files already on this appliance',
    );

    await dialog(page).getByRole('button', { name: /cancel/i }).click();
    await expect(dialog(page)).toHaveCount(0);
    expect(mock.writes).toHaveLength(0);

    await win(page).locator('[data-testid="syncthing-remove-folder"]').click();
    await page.locator('[data-testid="syncthing-confirm-remove"]').click();

    await expect(dialog(page)).toHaveCount(0, { timeout: 8_000 });
    expect(mock.writes).toHaveLength(1);
    expect(mock.writes[0].method).toBe('DELETE');
    expect(mock.writes[0].path).toBe('/api/syncthing/folders/photos');
    await expect(win(page).locator('[data-testid="syncthing-folder-list-row"]')).toHaveCount(0);
    await expect(win(page).locator('[data-testid="syncthing-success"]')).toContainText('Photos');
  });

  test('7.6 — removing a device asks for confirmation and unpairs it', async ({ page }) => {
    const mock = newMock();
    mock.devices = [deviceFixture()];
    await installDaemonMock(page, mock);
    await gotoSyncthing(page, adminTokens);
    await openTab(page, /device/i);

    await win(page).locator('[data-testid="syncthing-device-list-row"]').first().click();
    await win(page).locator('[data-testid="syncthing-remove-device"]').click();
    await expect(page.locator('[data-testid="syncthing-remove-desc"]')).toContainText(
      'Folders and their files stay on disk',
    );
    await page.locator('[data-testid="syncthing-confirm-remove"]').click();

    await expect(dialog(page)).toHaveCount(0, { timeout: 8_000 });
    expect(mock.writes).toHaveLength(1);
    expect(mock.writes[0].method).toBe('DELETE');
    expect(mock.writes[0].path).toBe(`/api/syncthing/devices/${DEVICE_ID}`);
    await expect(win(page).locator('[data-testid="syncthing-device-list-row"]')).toHaveCount(0);
    await expect(win(page).locator('[data-testid="syncthing-empty"]')).toBeVisible();
  });

  test('7.7 — the management actions are labelled buttons, reachable by keyboard', async ({
    page,
  }) => {
    const mock = newMock();
    mock.folders = [folderFixture()];
    mock.devices = [deviceFixture()];
    await installDaemonMock(page, mock);
    await gotoSyncthing(page, adminTokens);

    await openTab(page, /folder/i);
    const addFolder = win(page).locator('[data-testid="syncthing-add-folder"]');
    await expect(addFolder).toHaveRole('button');
    await expect(addFolder).toHaveAccessibleName(/add folder/i);
    // Keyboard-only entry point: focus it and press Enter.
    await addFolder.focus();
    await page.keyboard.press('Enter');
    await expect(dialog(page)).toBeVisible();
    // Every field has a label and the dialog's title names the task.
    await expect(dialog(page)).toContainText(/add a folder/i);
    await expect(page.locator('[data-testid="syncthing-folder-id-input"]')).toHaveAttribute(
      'id',
      'hv-syncthing-folder-id',
    );
    await page.keyboard.press('Escape');
    await expect(dialog(page)).toHaveCount(0);

    await openTab(page, /device/i);
    const addDevice = win(page).locator('[data-testid="syncthing-add-device"]');
    await expect(addDevice).toHaveRole('button');
    await expect(addDevice).toHaveAccessibleName(/add device/i);
  });
});

// ===========================================================================
// 13. Live daemon (opt-in) — read-only + a folder rescan
// ===========================================================================

test.describe('13 — Live daemon (opt-in)', () => {
  test.skip(!RUN_LIVE, 'set SYNCTHING_E2E=1 with a running Syncthing instance to exercise the live paths');

  test('13.1 — read config/devices/folders and trigger a rescan', async ({ page }) => {
    await gotoSyncthing(page, adminTokens);
    const st = await api(page, adminTokens, 'GET', '/api/syncthing/status');
    if (!st.data?.available) test.skip(true, 'no Syncthing daemon reachable');

    for (const path of [
      '/api/syncthing/config',
      '/api/syncthing/devices',
      '/api/syncthing/folders',
      '/api/syncthing/connections',
    ]) {
      const r = await api(page, adminTokens, 'GET', path);
      expect(r.http, path).toBe(200);
      expect(r.success, `${path}: ${r.error}`).toBe(true);
    }

    const folders = await api(page, adminTokens, 'GET', '/api/syncthing/folders');
    const first = (folders.data as any[])?.[0]?.id;
    if (first) {
      const scan = await api(page, adminTokens, 'POST', '/api/syncthing/scan', { folder: first });
      expect(scan.success, `scan: ${scan.error}`).toBe(true);
    }
  });
});

// ===========================================================================
// 14. Edge cases & memory
// ===========================================================================

test.describe('14 — Edge cases & memory', () => {
  test('14.1 — clearing localStorage mid-session and reloading does not crash', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await gotoSyncthing(page, adminTokens);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await expect(page.locator('.hv-login-page, [data-testid="syncthing-page"], #app')).toBeAttached({ timeout: 15_000 });
    await page.waitForTimeout(800);
    expect(errors).toEqual([]);
  });

  test('14.2 — open/close the window ×5 without significant heap growth', async ({ page }) => {
    test.setTimeout(150_000);
    await gotoSyncthing(page, adminTokens);
    const base = await getHeapMB(page);
    for (let i = 0; i < 5; i++) {
      await page.locator('.hv-cl-btn-close').first().click();
      await expect(win(page)).toHaveCount(0);
      await openLaunchpadApp(page, 'Syncthing');
      await page.waitForSelector('[data-testid="syncthing-page"]', { timeout: 10_000 });
    }
    const after = await getHeapMB(page);
    if (base > 0 && after > 0) {
      console.log(`[MEM] heap ${base.toFixed(1)} → ${after.toFixed(1)} MB`);
      expect(after - base).toBeLessThan(12);
    }
  });
});
