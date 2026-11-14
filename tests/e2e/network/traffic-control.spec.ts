/* =============================================================================
 * File          : traffic-control.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Traffic-control screen end-to-end tests (Playwright): opening the app,
 *   the live status, per-service rate editing, the weekly schedule editor,
 *   pause-all, apply-now and the save round-trip.
 *
 *   Every backend call is intercepted with `page.route`, so the suite never
 *   runs `tc` on the machine that hosts the test run and the assertions are
 *   deterministic. The real endpoint, auth matrix and validation live in
 *   `tests/traffic_control_integration.rs`; the command pipeline lives in the
 *   `traffic_control.rs` unit tests.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/network/traffic-control.spec.ts
 * =============================================================================
 */

import { test, expect, type Page, type APIRequestContext, type Route } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

interface Tokens {
  token: string;
  csrf: string;
}

async function apiLogin(ctx: APIRequestContext): Promise<Tokens> {
  const res = await ctx.post('/api/auth/login', {
    data: { username: ADMIN_USER, password: ADMIN_PASS },
  });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

async function gotoDesktop(page: Page, tokens: Tokens) {
  await page.addInitScript((t) => {
    try {
      localStorage.setItem('session-token', t.token);
      localStorage.setItem('csrf-token', t.csrf);
    } catch {
      /* storage unavailable — the test will surface it */
    }
  }, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  // Monitor widgets overlap the dock on this layout — hide them so clicks land.
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

/** `[data-testid="traffic-page"]` is unique to TrafficControl.svelte. */
function win(page: Page) {
  return page.locator('[data-testid="traffic-page"]');
}

/**
 * A freshly seeded server answers `GET /api/whats-new` with `seen: false`, so
 * the desktop opens a bits-ui modal whose full-screen overlay swallows the
 * first dock click. Close it if it is there.
 */
async function dismissWhatsNew(page: Page) {
  const modal = page.locator('[data-testid="whats-new-modal"]');
  if (await modal.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await modal.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

const SERVICES = [
  { id: 'smb', ports: [445, 139] },
  { id: 'nfs', ports: [2049] },
  { id: 'ftp', ports: [21, 20] },
  { id: 'webdav', ports: [8081] },
  { id: 'sftp', ports: [2222] },
];

function capabilities() {
  return {
    tc_available: true,
    ifb_available: false,
    services: SERVICES.map((s) => ({
      id: s.id,
      ports: s.ports,
      supports_upload_limit: true,
      supports_download_limit: false,
      note: 'Limits traffic leaving HomeVault.',
    })),
    management_ports: [22, 80, 443, 8443],
  };
}

function baseSettings() {
  return {
    enabled: true,
    interface: 'eth0',
    paused: false,
    services: SERVICES.map((s) => ({ service: s.id, rate_kbps: 0 })),
    schedule: [],
    updated_at: '2026-11-01T10:00:00Z',
  };
}

function payload(settings: any, state: any) {
  return {
    settings,
    capabilities: capabilities(),
    interfaces: [
      { name: 'eth0', type: 'ethernet', status: 'up', ipv4: '192.168.1.10' },
      { name: 'wlan0', type: 'wifi', status: 'down', ipv4: null },
    ],
    state,
  };
}

const SHAPING_ON = {
  shaping: true,
  reason: null,
  reason_code: null,
  commands: ['sudo tc qdisc replace dev eth0 root handle 1: htb default 30'],
};

/** Intercepts every traffic-control call and records what the page sent. */
interface Recorder {
  puts: any[];
  pauses: any[];
  applies: number;
}

async function mockApi(
  page: Page,
  options: { settings?: any; state?: any; putStatus?: number; putMessage?: string } = {},
): Promise<Recorder> {
  const recorder: Recorder = { puts: [], pauses: [], applies: 0 };
  let settings = options.settings ?? baseSettings();
  const state = options.state ?? SHAPING_ON;

  await page.route('**/api/network/traffic-control**', async (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    const method = request.method();
    // The route pattern also matches the `/pause` and `/apply` children.
    const isPause = url.pathname.endsWith('/pause');
    const isApply = url.pathname.endsWith('/apply');

    if (method === 'GET' && !isPause && !isApply) {
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: payload(settings, state) }),
      });
    }

    if (method === 'PUT') {
      const body = request.postDataJSON();
      recorder.puts.push(body);
      if (options.putStatus && options.putStatus >= 400) {
        return route.fulfill({
          status: options.putStatus,
          contentType: 'application/json',
          body: JSON.stringify({
            success: false,
            message: options.putMessage ?? 'Refused',
            data: null,
          }),
        });
      }
      settings = { ...body, updated_at: '2026-11-01T10:05:00Z' };
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: {
            ...payload(settings, state),
            apply: { applied: true, reason: null, commands: ['sudo tc qdisc replace'], failed: null },
          },
        }),
      });
    }

    if (method === 'POST' && isPause) {
      const body = request.postDataJSON();
      recorder.pauses.push(body);
      settings = { ...settings, paused: !!body.paused };
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: {
            settings,
            state: body.paused
              ? { ...state, shaping: false, reason: 'All transfers are paused', reason_code: 'paused' }
              : state,
            apply: { applied: false, reason: null, commands: [], failed: null },
          },
        }),
      });
    }

    if (method === 'POST' && isApply) {
      recorder.applies += 1;
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: {
            settings,
            state,
            apply: { applied: true, reason: null, commands: ['sudo tc qdisc replace'], failed: null },
          },
        }),
      });
    }

    return route.continue();
  });

  return recorder;
}

let tokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  const ctx = await playwright.request.newContext({ baseURL: BASE });
  tokens = await apiLogin(ctx);
  if (tokens.token) {
    // Acknowledge the release once, so the What's New overlay never sits
    // between a test and its first dock click.
    await ctx
      .post('/api/whats-new/seen', {
        headers: { 'X-Session-Token': tokens.token, 'X-CSRF-Token': tokens.csrf },
        data: {},
      })
      .catch(() => null);
  }
  await ctx.dispose();
  if (!tokens.token) {
    throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  }
});

test.beforeEach(() => {
  test.setTimeout(60_000);
});

async function openTraffic(page: Page) {
  await gotoDesktop(page, tokens);
  await dismissWhatsNew(page);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[data-app-id="traffic-control"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="traffic-page"]', { timeout: 15_000 });
  await expect(win(page).locator('[data-testid="traffic-content"]')).toBeVisible({
    timeout: 15_000,
  });
}

// ===========================================================================
// 1. Page load and status
// ===========================================================================

test.describe('1 — Load and status', () => {
  test('1.1 — the app opens from the launcher and lists every service', async ({ page }) => {
    await mockApi(page);
    await openTraffic(page);
    await expect(win(page)).toBeVisible();
    for (const service of SERVICES) {
      await expect(win(page).locator(`[data-testid="traffic-service-${service.id}"]`)).toBeVisible();
      await expect(win(page).locator(`[data-testid="traffic-rate-${service.id}"]`)).toBeVisible();
    }
  });

  test('1.2 — the status region reports shaping on', async ({ page }) => {
    await mockApi(page);
    await openTraffic(page);
    const status = win(page).locator('[data-testid="traffic-status"]');
    await expect(status).toHaveAttribute('role', 'status');
    await expect(status).toContainText('Shaping is active');
  });

  test('1.3 — a stopped policy explains why', async ({ page }) => {
    await mockApi(page, {
      settings: { ...baseSettings(), paused: true },
      state: { shaping: false, reason: 'All transfers are paused', reason_code: 'paused', commands: [] },
    });
    await openTraffic(page);
    const status = win(page).locator('[data-testid="traffic-status"]');
    await expect(status).toContainText('Shaping is stopped');
    await expect(status).toContainText('All transfers are paused');
  });
});

// ===========================================================================
// 2. Editing and saving
// ===========================================================================

test.describe('2 — Editing', () => {
  test('2.1 — a rate edit is sent as kbit/s for every service', async ({ page }) => {
    const recorder = await mockApi(page);
    await openTraffic(page);

    const smb = win(page).locator('[data-testid="traffic-rate-smb"]');
    await smb.fill('20000');
    await win(page).locator('[data-testid="traffic-save"]').click();

    await expect.poll(() => recorder.puts.length).toBe(1);
    const body = recorder.puts[0];
    expect(body.interface).toBe('eth0');
    expect(body.enabled).toBe(true);
    const smbRow = body.services.find((s: any) => s.service === 'smb');
    expect(smbRow.rate_kbps).toBe(20000);
    // All known services travel, `0` meaning "unlimited".
    expect(body.services).toHaveLength(SERVICES.length);
  });

  test('2.2 — a refused save surfaces the server reason', async ({ page }) => {
    await mockApi(page, {
      putStatus: 400,
      putMessage: "'lo' is not a valid interface name",
    });
    await openTraffic(page);
    await win(page).locator('[data-testid="traffic-save"]').click();
    await expect(win(page).locator('[role="alert"]').first()).toContainText(
      "'lo' is not a valid interface name",
    );
  });
});

// ===========================================================================
// 3. Schedule
// ===========================================================================

test.describe('3 — Schedule', () => {
  test('3.1 — a window can be added and its days toggled', async ({ page }) => {
    const recorder = await mockApi(page);
    await openTraffic(page);

    await expect(win(page).locator('[data-testid="traffic-schedule-empty"]')).toBeVisible();
    await win(page).locator('[data-testid="traffic-add-window"]').click();
    await expect(win(page).locator('[data-testid="traffic-window-0"]')).toBeVisible();

    // A new window defaults to Monday–Friday; Saturday starts disabled.
    const saturday = win(page).locator('[data-testid="traffic-day-0-5"]');
    await expect(saturday).toHaveAttribute('aria-pressed', 'false');
    await saturday.click();
    await expect(saturday).toHaveAttribute('aria-pressed', 'true');

    await win(page).locator('[data-testid="traffic-save"]').click();
    await expect.poll(() => recorder.puts.length).toBe(1);
    const window = recorder.puts[0].schedule[0];
    expect(window.days).toContain(5);
    expect(window.start_minute).toBe(540);
    expect(window.end_minute).toBe(1020);
  });

  test('3.2 — a window can be removed again', async ({ page }) => {
    const recorder = await mockApi(page);
    await openTraffic(page);
    await win(page).locator('[data-testid="traffic-add-window"]').click();
    await expect(win(page).locator('[data-testid="traffic-window-0"]')).toBeVisible();
    await win(page).locator('[data-testid="traffic-remove-window-0"]').click();
    await expect(win(page).locator('[data-testid="traffic-window-0"]')).toHaveCount(0);
    await expect(win(page).locator('[data-testid="traffic-schedule-empty"]')).toBeVisible();
    expect(recorder.puts).toHaveLength(0);
  });
});

// ===========================================================================
// 4. Pause-all and apply-now
// ===========================================================================

test.describe('4 — Pause and apply', () => {
  test('4.1 — pause-all posts the new state and updates the status', async ({ page }) => {
    const recorder = await mockApi(page);
    await openTraffic(page);
    await win(page).locator('[data-testid="traffic-pause"]').click();
    await expect.poll(() => recorder.pauses.length).toBe(1);
    expect(recorder.pauses[0].paused).toBe(true);
    await expect(win(page).locator('[data-testid="traffic-status"]')).toContainText(
      'All transfers are paused',
    );
    // The button flips to Resume.
    await expect(win(page).locator('[data-testid="traffic-resume"]')).toBeVisible();
  });

  test('4.2 — apply-now re-applies without saving', async ({ page }) => {
    const recorder = await mockApi(page);
    await openTraffic(page);
    await win(page).locator('[data-testid="traffic-apply"]').click();
    await expect.poll(() => recorder.applies).toBe(1);
    expect(recorder.puts).toHaveLength(0);
  });
});

// ===========================================================================
// 5. Accessibility
// ===========================================================================

test.describe('5 — Accessibility', () => {
  test('5.1 — weekday toggles meet the 44px target and expose aria-pressed', async ({ page }) => {
    await mockApi(page);
    await openTraffic(page);
    await win(page).locator('[data-testid="traffic-add-window"]').click();
    const day = win(page).locator('[data-testid="traffic-day-0-0"]');
    const box = await day.boundingBox();
    expect(box).not.toBeNull();
    // Firefox reports fractional layout geometry on some DPRs: a 44 CSS px
    // target can come back as 43.9999… Keep a sub-pixel tolerance instead of
    // demanding exact integer pixels.
    expect(box!.width).toBeGreaterThanOrEqual(43.5);
    expect(box!.height).toBeGreaterThanOrEqual(43.5);
    await expect(day).toHaveAttribute('aria-pressed', 'true');
  });

  test('5.2 — every rate input is named', async ({ page }) => {
    await mockApi(page);
    await openTraffic(page);
    for (const service of SERVICES) {
      const input = win(page).locator(`[data-testid="traffic-rate-${service.id}"]`);
      const name = await input.evaluate((el) => {
        const labelled = (el as HTMLInputElement).labels?.[0]?.textContent?.trim() ?? '';
        return labelled;
      });
      expect(name.length).toBeGreaterThan(0);
    }
  });
});
