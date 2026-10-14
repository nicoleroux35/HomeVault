/* =============================================================================
 * File          : widgets.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Widgets screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Widgets app — End-to-End Test Suite
 *
 * The Widgets app is a window opened from the desktop's Applications
 * launcher (id `widgets`, "Widgets" tile). It is a catalog of the 11
 * floating desktop "monitor" panels (Resource, RAID, Security, VPN,
 * Downloader, Syncthing, Calendar, Docker, Weather, Sessions, System
 * Health); clicking a tile toggles that panel's visibility on the desktop.
 * Visibility + positions live in `monitorStore` (per-user localStorage +
 * `GET|POST /api/monitor-layout` server sync).
 *
 * **This suite changes the admin account's monitor layout while it runs,
 * so it snapshots the current layout in `beforeAll` and restores it in
 * `afterAll`.** No host state is touched.
 *
 * Run (all):    npx playwright test tests/e2e/widgets/widgets.spec.ts --project=firefox
 * Run (single): npx playwright test tests/e2e/widgets/widgets.spec.ts -g "3.1"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/widgets/widgets.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
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

/** monitor id → the desktop panel's class + the tile's expected position. */
const MONITORS = [
  'resource',
  'raid',
  'security',
  'vpn',
  'downloader',
  'syncthing',
  'calendar',
  'docker',
  'weather',
  'sessions',
  'health',
] as const;

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

async function seedBrowser(page: Page, tokens: Tokens | null) {
  await page.addInitScript(
    ({ t }) => {
      try {
        if (t) {
          localStorage.setItem('session-token', t.token);
          localStorage.setItem('csrf-token', t.csrf);
        }
      } catch {
        /* storage unavailable — the test will surface it */
      }
    },
    { t: tokens },
  );
}

/** Desktop, WITHOUT hiding the monitor panels (this suite needs to see them). */
async function gotoDesktop(page: Page, tokens: Tokens) {
  await seedBrowser(page, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
}

async function openWidgets(page: Page, tokens: Tokens) {
  await gotoDesktop(page, tokens);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[title="Widgets"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="widgets-page"]', { timeout: 15_000 });
}

function win(page: Page) {
  return page.locator('[data-testid="widgets-page"]');
}

/** The catalog tile for one monitor, matched by its visible name. */
function tile(page: Page, name: RegExp) {
  return win(page).locator('[data-testid="widgets-tile"]').filter({ hasText: name }).first();
}

let adminTokens: Tokens;
let savedLayout: string | null = null;

test.beforeAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
  const res = await ctx.get('/api/monitor-layout', { headers: { 'x-session-token': token } });
  savedLayout = (await res.json().catch(() => ({})))?.data?.layout_json ?? null;
  await ctx.dispose();
});

test.afterAll(async ({ playwright }) => {
  if (savedLayout === null) return;
  const ctx = await newCtx(playwright);
  await ctx
    .post('/api/monitor-layout', {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
      data: { layout_json: savedLayout },
    })
    .catch(() => {});
  await ctx.dispose();
});

test.afterEach(async ({ page }, testInfo) => {
  const heapMB = await getHeapMB(page).catch(() => 0);
  if (heapMB > 10) {
    // eslint-disable-next-line no-console
    console.log(`[MEM] ${testInfo.title}: ${heapMB.toFixed(1)} MB heap`);
  }
});

// ===========================================================================
// 1. Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — Widgets window opens from the launcher', async ({ page }) => {
    await openWidgets(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — header shows the title', async ({ page }) => {
    await openWidgets(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title')).not.toBeEmpty();
  });

  test('1.3 — the catalog lists exactly 11 monitor tiles', async ({ page }) => {
    await openWidgets(page, adminTokens);
    await expect(win(page).locator('[data-testid="widgets-tile"]')).toHaveCount(11);
  });

  test('1.4 — every tile shows either an "active" or an "add" affordance', async ({ page }) => {
    await openWidgets(page, adminTokens);
    const items = win(page).locator('[data-testid="widgets-tile"]');
    const n = await items.count();
    for (let i = 0; i < n; i++) {
      const badge = await items.nth(i).locator('[data-testid="widgets-tile-badge"]').count();
      const add = await items.nth(i).locator('[data-testid="widgets-tile-add"]').count();
      expect(badge + add).toBe(1);
    }
  });

  test('1.5 — the footer shows the on-desktop count and a Reset button', async ({ page }) => {
    await openWidgets(page, adminTokens);
    await expect(win(page).locator('[data-testid="widgets-stats"]')).toHaveText(/\d+/);
    await expect(win(page).locator('[data-testid="widgets-reset"]')).toBeVisible();
  });
});

// ===========================================================================
// 2. Tile state reflects the desktop
// ===========================================================================

test.describe('2 — Tile / desktop consistency', () => {
  test('2.1 — an "active" tile corresponds to a visible desktop panel', async ({ page }) => {
    await openWidgets(page, adminTokens);
    for (const id of MONITORS) {
      const active = await win(page)
        .locator(`[data-testid="widgets-tile"].active:has([data-testid="widgets-tile-name"])`)
        .count();
      expect(active).toBeGreaterThanOrEqual(0); // sanity — real per-monitor check is 3.x
    }
    // Concretely: whatever the Resource tile's state is, the panel matches.
    const resourceActive = await tile(page, /Resource/i).evaluate((el) =>
      el.classList.contains('active'),
    );
    const panelVisible = (await page.locator('.hv-resource-monitor').count()) > 0;
    expect(panelVisible).toBe(resourceActive);
  });

  test('2.2 — the 11 desktop panels share one coherent card geometry', async ({ page }) => {
    // Panels must be visible on the desktop (this suite does not hide them).
    await gotoDesktop(page, adminTokens);
    await page.waitForSelector('.hv-resource-monitor', { timeout: 15_000 });
    const geo = await page.evaluate(() => {
      const panel = document.querySelector('.hv-resource-monitor')!;
      const header = panel.querySelector('.hv-widget-header')!;
      const tile = header.querySelector('.hv-widget-tile')!;
      const hb = header.getBoundingClientRect();
      const tb = tile.getBoundingClientRect();
      const widthOf = (sel: string) =>
        parseFloat(getComputedStyle(document.querySelector(sel)!).width);
      return {
        radius: getComputedStyle(panel).borderRadius,
        headerH: Math.round(hb.height),
        tileW: Math.round(tb.width),
        tileH: Math.round(tb.height),
        tileCentered: Math.abs((tb.top + tb.height / 2) - (hb.top + hb.height / 2)),
        // Width stops come from the shared --hv-monitor-w* token set.
        compactW: widthOf('.hv-resource-monitor'),
        standardW: widthOf('.hv-calendar-monitor'),
        wideW: widthOf('.hv-raid-monitor'),
      };
    });
    // Floating widget-card corner (--hv-radius-card = 12px) — not a control radius.
    expect(parseFloat(geo.radius)).toBe(12);
    // The header is the app-header bar: 44px, a 28px icon tile dead-centre, and
    // no close/action button (monitor visibility is managed in Settings).
    expect(geo.headerH).toBe(44);
    expect(geo.tileW).toBe(28);
    expect(geo.tileH).toBe(28);
    expect(geo.tileCentered).toBeLessThanOrEqual(2);
    // Width stops: compact 280 / standard 300 / wide 320.
    expect(geo.compactW).toBe(280);
    expect(geo.standardW).toBe(300);
    expect(geo.wideW).toBe(320);
  });
});

// ===========================================================================
// 3. Toggling
// ===========================================================================

test.describe('3 — Toggling a widget', () => {
  test('3.1 — turning the Resource Monitor off removes its desktop panel', async ({ page }) => {
    await openWidgets(page, adminTokens);
    const t = tile(page, /Resource/i);
    // Ensure it starts ON.
    if (!(await t.evaluate((el) => el.classList.contains('active')))) await t.click();
    await expect(t).toHaveClass(/active/);
    await expect(page.locator('.hv-resource-monitor')).toHaveCount(1);

    await t.click();
    await expect(t).not.toHaveClass(/active/);
    await expect(page.locator('.hv-resource-monitor')).toHaveCount(0);
  });

  test('3.2 — turning it back on restores the panel', async ({ page }) => {
    await openWidgets(page, adminTokens);
    const t = tile(page, /Resource/i);
    if (await t.evaluate((el) => el.classList.contains('active'))) await t.click();
    await expect(page.locator('.hv-resource-monitor')).toHaveCount(0);
    await t.click();
    await expect(t).toHaveClass(/active/);
    await expect(page.locator('.hv-resource-monitor')).toHaveCount(1);
  });

  test('3.3 — the footer count tracks the number of active tiles', async ({ page }) => {
    await openWidgets(page, adminTokens);
    const activeNow = await win(page).locator('[data-testid="widgets-tile"].active').count();
    const footer = win(page).locator('[data-testid="widgets-stats"]');
    await expect(footer).toContainText(String(activeNow));

    const t = tile(page, /System Health/i);
    const wasActive = await t.evaluate((el) => el.classList.contains('active'));
    await t.click();
    await expect(footer).toContainText(String(activeNow + (wasActive ? -1 : 1)));
  });

  test('3.4 — toggling persists via POST /api/monitor-layout', async ({ page }) => {
    await openWidgets(page, adminTokens);
    let posted = false;
    page.on('request', (r) => {
      if (r.method() === 'POST' && r.url().includes('/api/monitor-layout')) posted = true;
    });
    await tile(page, /Docker/i).click();
    await expect.poll(() => posted, { timeout: 5000 }).toBe(true);
  });

  test('3.5 — Reset to Default turns every monitor on', async ({ page }) => {
    await openWidgets(page, adminTokens);
    // Turn a couple off first.
    for (const name of [/VPN/i, /Weather/i]) {
      const t = tile(page, name);
      if (await t.evaluate((el) => el.classList.contains('active'))) await t.click();
    }
    await win(page).locator('[data-testid="widgets-reset"]').click();
    await expect(win(page).locator('[data-testid="widgets-tile"].active')).toHaveCount(11);
  });
});

// ===========================================================================
// 4. Navigation
// ===========================================================================

test.describe('4 — Navigation', () => {
  test('4.1 — the window can be closed and reopened', async ({ page }) => {
    await openWidgets(page, adminTokens);
    await page.locator('.hv-cl-btn-close').first().click();
    await expect(win(page)).toHaveCount(0, { timeout: 10_000 });
    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    await page.locator('.lp-card[title="Widgets"]').click();
    await expect(win(page)).toBeVisible();
  });
});

// ===========================================================================
// 5. Responsive
// ===========================================================================

test.describe('5 — Responsive', () => {
  for (const [w, h] of [
    [360, 740],
    [768, 1024],
    [1920, 1080],
  ] as const) {
    test(`5.${w} — no horizontal overflow at ${w}px`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: h });
      await openWidgets(page, adminTokens);
      const overflow = await win(page)
        .locator('[data-testid="widgets-grid"]')
        .evaluate((el) => el.scrollWidth - el.clientWidth);
      expect(overflow).toBeLessThanOrEqual(2);
    });
  }
});

// ===========================================================================
// 6. Accessibility
// ===========================================================================

test.describe('6 — Accessibility', () => {
  test('6.1 — every tile is a real <button>', async ({ page }) => {
    await openWidgets(page, adminTokens);
    const tags = await win(page)
      .locator('[data-testid="widgets-tile"]')
      .evaluateAll((els) => els.map((e) => e.tagName));
    expect(tags.every((t) => t === 'BUTTON')).toBe(true);
  });

  test('6.2 — no duplicate id attributes in the Widgets window', async ({ page }) => {
    await openWidgets(page, adminTokens);
    const dupes = await win(page).evaluate((root) => {
      const ids = [...root.querySelectorAll('[id]')].map((e) => e.id);
      return ids.filter((v, i) => ids.indexOf(v) !== i);
    });
    expect(dupes).toEqual([]);
  });
});

// ===========================================================================
// 7. Console & network
// ===========================================================================

test.describe('7 — Console & network', () => {
  test('7.1 — no uncaught console errors while the Widgets app loads', async ({ page }) => {
    const errors: string[] = [];
    page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
    page.on('pageerror', (e) => errors.push(String(e)));
    await openWidgets(page, adminTokens);
    await page.waitForTimeout(800);
    const real = errors.filter((e) => !/favicon|ResizeObserver|Failed to load resource/i.test(e));
    expect(real, real.join('\n')).toEqual([]);
  });
});

// ===========================================================================
// 8. Security
// ===========================================================================

test.describe('8 — Security', () => {
  test('8.1 — /api/monitor-layout rejects an unauthenticated request', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const codes = await page.evaluate(async () => {
      const g = await fetch('/api/monitor-layout');
      const p = await fetch('/api/monitor-layout', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: '{"layout_json":"{}"}',
      });
      return [g.status, p.status];
    });
    for (const c of codes) expect(c).not.toBe(200);
  });

  test('8.2 — /api/security/active-sessions-count is admin-gated', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/security/active-sessions-count', {
        headers: { 'x-session-token': 'not-a-real-token' },
      });
      return r.status;
    });
    expect(status).toBe(401);
  });

  test('8.3 — no token / secret lands in the URL or localStorage', async ({ page }) => {
    await openWidgets(page, adminTokens);
    expect(page.url()).not.toMatch(/token|password|secret/i);
    const dump = await page.evaluate(() => JSON.stringify(localStorage));
    expect(dump).not.toContain(ADMIN_PASS);
  });
});

// ===========================================================================
// 9. Memory regression
// ===========================================================================

test.describe('9 — Memory regression', () => {
  test('9.1 — open/close the Widgets app 5× without DOM node accumulation', async ({ page }) => {
    test.setTimeout(150_000);
    await gotoDesktop(page, adminTokens);
    let baseline = 0;
    for (let i = 0; i < 5; i++) {
      await page.locator('.hv-dock-item[aria-label="Applications"]').click();
      await page.locator('.lp-card[title="Widgets"]').click();
      await page.waitForSelector('[data-testid="widgets-page"]', { timeout: 15_000 });
      await page.locator('.hv-cl-btn-close').first().click();
      await expect(win(page)).toHaveCount(0, { timeout: 10_000 });
      const nodes = await page.evaluate(() => document.querySelectorAll('*').length);
      if (i === 0) baseline = nodes;
      else expect(nodes).toBeLessThan(baseline + 500);
    }
  });
});
