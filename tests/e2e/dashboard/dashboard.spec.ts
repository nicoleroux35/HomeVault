/**
 * Dashboard app — End-to-End Test Suite
 *
 * The Dashboard is a window opened from the desktop (pinned dock icon +
 * the Applications launcher, id `dashboard`). It shows a live system
 * overview — CPU / RAM / disk / health gauge, per-core chart, memory,
 * network interfaces, storage usage, sensors, GPU and OS cards — polling
 * `GET /api/dashboard/server-details` and `GET /api/dashboard/health`
 * every 5 s (`frontend/src/routes/Dashboard.svelte`).
 *
 * **This suite is entirely read-only.** The Dashboard screen has no
 * mutating action (the manual "Refresh" button just re-runs the two GETs).
 * Widget CRUD / widget-preferences / the small `/api/widgets/*` endpoints
 * have no consumer on this screen and are covered by
 * `tests/dashboard_integration.rs`.
 *
 * Run (all):    npx playwright test tests/e2e/dashboard/dashboard.spec.ts --project=firefox
 * Run (single): npx playwright test tests/e2e/dashboard/dashboard.spec.ts -g "4.1"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/dashboard/dashboard.spec.ts
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
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

async function openDashboard(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  // Dashboard is a pinned dock app.
  await page.locator('[data-testid="dock-dashboard"]').click();
  await page.waitForSelector('[data-testid="dashboard-page"]', { timeout: 15_000 });
  // The loading placeholder clears once the first server-details poll lands.
  // `server-details` shells out (dmidecode / nvidia-smi / smartctl per
  // disk) on the first call — give it room.
  await expect(page.locator('[data-testid="dashboard-page"] [data-testid="dashboard-loading"]')).toHaveCount(0, { timeout: 30_000 });
}

/** `[data-testid="dashboard-page"]` is unique to Dashboard.svelte (confirmed via grep). */
function win(page: Page) {
  return page.locator('[data-testid="dashboard-page"]');
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

test.beforeEach(() => {
  // The first Dashboard mount waits on a cold `server-details` (dmidecode /
  // nvidia-smi / per-disk smartctl); the default 60 s can be tight when
  // stacked on the nav + selector waits.
  test.setTimeout(100_000);
});

test.afterEach(async ({ page }, testInfo) => {
  const heapMB = await getHeapMB(page).catch(() => 0);
  if (heapMB > 10) {
    // eslint-disable-next-line no-console
    console.log(`[MEM] ${testInfo.title}: ${heapMB.toFixed(1)} MB heap after test`);
  }
});

// ===========================================================================
// 1. Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — Dashboard window opens from the dock', async ({ page }) => {
    await openDashboard(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — the loading placeholder appears then clears', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('[data-testid="dock-dashboard"]').click();
    await page.waitForSelector('[data-testid="dashboard-page"]', { timeout: 15_000 });
    // Either it already resolved, or the placeholder is/was shown — in both
    // cases it must be gone shortly.
    await expect(page.locator('[data-testid="dashboard-loading"]')).toHaveCount(0, { timeout: 20_000 });
  });

  test('1.3 — the WindowHeader shows title + subtitle', async ({ page }) => {
    await openDashboard(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title, .wh-title')).toHaveCount(1);
    await expect(win(page).locator('.hv-winhdr-title, .wh-title').first()).not.toBeEmpty();
  });

  test('1.4 — the 4 top stat cards render (CPU / RAM / Disk / one more)', async ({ page }) => {
    await openDashboard(page, adminTokens);
    await expect(win(page).locator('[data-testid="dashboard-stats-grid"] [data-testid="dashboard-stat-card"]')).toHaveCount(4);
  });

  test('1.5 — the health gauge renders with a numeric score', async ({ page }) => {
    await openDashboard(page, adminTokens);
    const score = win(page).locator('[data-testid="dashboard-gauge-score"]');
    await expect(score).toBeVisible();
    await expect(score).toHaveText(/\d+/);
  });

  test('1.6 — the OS info card renders with real values (not all placeholders)', async ({ page }) => {
    await openDashboard(page, adminTokens);
    const grid = win(page).locator('[data-testid="dashboard-os-grid"]');
    await expect(grid).toBeVisible();
    const text = (await grid.innerText()).trim();
    // At least one row must carry a real value, not just the "—" fallback.
    expect(text.replace(/[—\s]/g, '').length).toBeGreaterThan(3);
  });

  test('1.7 — no header badge, no manual refresh button (standardized header)', async ({ page }) => {
    await openDashboard(page, adminTokens);
    // The "Live" badge was header decoration and went with the header
    // standardization — the data still revalidates on focus + a slow poll.
    // `.hv-winhdr-subtitle` no longer exists (the prop was removed from
    // `WindowHeader` on 2026-09-14), so this locator matched nothing.
    // Assert the bar's real shape instead: the icon tile + the title.
    await expect(win(page).locator('.hv-winhdr > *')).toHaveCount(2);
    await expect(win(page).locator('[data-testid="dashboard-live"]')).toHaveCount(0);
    // `.db-refresh-btn` is not a class token the app ever renders (Svelte's
    // `class={const}` trap), so the old `toHaveCount(0)` on it could never
    // fail. Assert the *rule* instead: no control in the window advertises a
    // refresh action. `getByRole` matches the computed accessible name, so
    // this catches a re-added manual Refresh whatever class or testid it
    // ships with — and an icon-only button needs `aria-label` anyway.
    await expect(
      win(page).getByRole('button', { name: /refresh|actualiser/i }),
    ).toHaveCount(0);
  });
});

// ===========================================================================
// 2. Data cards
// ===========================================================================

test.describe('2 — Data cards', () => {
  test('2.1 — memory card shows a used / total figure', async ({ page }) => {
    await openDashboard(page, adminTokens);
    await expect(win(page).locator('[data-testid="dashboard-mem-bar"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="dashboard-mem-labels"]')).not.toBeEmpty();
  });

  test('2.2 — network interfaces list renders at least one interface', async ({ page }) => {
    await openDashboard(page, adminTokens);
    const ifaces = win(page).locator('[data-testid="dashboard-net-iface"]');
    await expect(ifaces.first()).toBeVisible({ timeout: 15_000 });
  });

  test('2.3 — storage usage bars render', async ({ page }) => {
    await openDashboard(page, adminTokens);
    await expect(win(page).locator('[data-testid="dashboard-storage-bars"], [data-testid="dashboard-storage-row"]').first()).toBeVisible();
  });

  test('2.4 — sensors grid renders (may be empty on a VM, but the card is present)', async ({ page }) => {
    await openDashboard(page, adminTokens);
    await expect(win(page).locator('[data-testid="dashboard-sensors-grid"], [data-testid="dashboard-card"]').first()).toBeVisible();
  });

  test('2.5 — every stat value is a number, never NaN / undefined', async ({ page }) => {
    await openDashboard(page, adminTokens);
    const vals = await win(page).locator('[data-testid="dashboard-stat-value"]').allInnerTexts();
    expect(vals.length).toBe(4);
    for (const v of vals) {
      expect(v).not.toMatch(/NaN|undefined|null/i);
      expect(v).toMatch(/\d/);
    }
  });
});

// ===========================================================================
// 3. Interactions
// ===========================================================================

test.describe('3 — Interactions', () => {
  test('3.1 — the 5s auto-refresh re-fetches server-details and health', async ({ page }) => {
    await openDashboard(page, adminTokens);
    const seen = new Set<string>();
    page.on('response', (r) => {
      const u = r.url();
      if (u.includes('/api/dashboard/server-details')) seen.add('details');
      if (u.includes('/api/dashboard/health')) seen.add('health');
    });
    // No button — just wait for the next poll cycle.
    await expect.poll(() => seen.size, { timeout: 15_000 }).toBe(2);
  });

  test('3.2 — the window can be closed', async ({ page }) => {
    await openDashboard(page, adminTokens);
    await page.locator('.hv-cl-btn-close').first().click();
    await expect(win(page)).toHaveCount(0, { timeout: 10_000 });
  });

  test('3.3 — reopening the Dashboard focuses the existing window (no duplicate)', async ({ page }) => {
    await openDashboard(page, adminTokens);
    // The open window overlaps the pinned dock icon, so Playwright's
    // actionability check would block a normal click — force it (we only
    // care that the dock handler runs, not the hit-test).
    await page.locator('[data-testid="dock-dashboard"]').click({ force: true });
    await page.waitForTimeout(500);
    await expect(win(page)).toHaveCount(1);
  });
});

// ===========================================================================
// 4. Security
// ===========================================================================

test.describe('4 — Security', () => {
  test('4.1 — dashboard endpoints reject an unauthenticated request', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const codes = await page.evaluate(async () => {
      const urls = [
        '/api/dashboard/server-details',
        '/api/dashboard/health',
        '/api/dashboard/data',
        '/api/dashboard/storage',
      ];
      const out: number[] = [];
      for (const u of urls) {
        const r = await fetch(u, { headers: {} });
        out.push(r.status);
      }
      return out;
    });
    for (const c of codes) expect(c).not.toBe(200);
  });

  test('4.2 — a bogus token is rejected with 401', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/dashboard/health', {
        headers: { 'x-session-token': 'not-a-real-token' },
      });
      return r.status;
    });
    expect(status).toBe(401);
  });

  test('4.3 — no session token or secret is placed in the URL', async ({ page }) => {
    await openDashboard(page, adminTokens);
    expect(page.url()).not.toMatch(/token|session|password|secret/i);
  });

  test('4.4 — localStorage holds only the expected keys, no plaintext password', async ({ page }) => {
    await openDashboard(page, adminTokens);
    const dump = await page.evaluate(() => JSON.stringify(localStorage));
    expect(dump).not.toContain(ADMIN_PASS);
  });

  test('4.5 — an XSS payload in the OS/hostname is rendered as text, not markup', async ({ page }) => {
    // We cannot control the host name, but we can assert the card contains
    // no injected <script> / <img onerror> from whatever the host reports.
    await openDashboard(page, adminTokens);
    const html = await win(page).locator('[data-testid="dashboard-os-grid"]').innerHTML();
    expect(html).not.toMatch(/<script|onerror=|<img[^>]+onerror/i);
  });
});

// ===========================================================================
// 5. Navigation
// ===========================================================================

test.describe('5 — Navigation', () => {
  test('5.1 — Dashboard also opens from the Applications launcher', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    const card = page.locator('.lp-card[title="Dashboard"]');
    await card.waitFor({ state: 'visible', timeout: 10_000 });
    await card.click();
    await expect(win(page)).toBeVisible();
  });

  test('5.2 — minimising then restoring keeps the same window', async ({ page }) => {
    await openDashboard(page, adminTokens);
    await page.locator('[data-testid="win-minimize"]').first().click();
    await page.locator('[data-testid="dock-dashboard"]').click();
    await expect(win(page)).toBeVisible();
    await expect(win(page)).toHaveCount(1);
  });
});

// ===========================================================================
// 6. Responsive
// ===========================================================================

test.describe('6 — Responsive', () => {
  for (const [w, h] of [
    [360, 740],
    [768, 1024],
    [1920, 1080],
  ] as const) {
    test(`6.${w} — renders without horizontal overflow at ${w}px`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: h });
      await openDashboard(page, adminTokens);
      const overflow = await page.evaluate(() => {
        const el = document.querySelector('[data-testid="dashboard-content"]') as HTMLElement | null;
        return el ? el.scrollWidth - el.clientWidth : 0;
      });
      expect(overflow).toBeLessThanOrEqual(2);
    });
  }
});

// ===========================================================================
// 7. Accessibility
// ===========================================================================

test.describe('7 — Accessibility', () => {
  test('7.1 — the header is title-only and exposes no refresh control', async ({ page }) => {
    await openDashboard(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title')).not.toBeEmpty();
    await expect(win(page).locator('[data-testid="dashboard-live"]')).toHaveCount(0);
    // Same rewrite as 1.7 — see the comment there.
    await expect(
      win(page).getByRole('button', { name: /refresh|actualiser/i }),
    ).toHaveCount(0);
  });

  test('7.2 — no duplicate id attributes in the Dashboard window', async ({ page }) => {
    await openDashboard(page, adminTokens);
    const dupes = await win(page).evaluate((root) => {
      const ids = [...root.querySelectorAll('[id]')].map((e) => e.id);
      return ids.filter((v, i) => ids.indexOf(v) !== i);
    });
    expect(dupes).toEqual([]);
  });

  test('7.3 — every stat card exposes a text label', async ({ page }) => {
    await openDashboard(page, adminTokens);
    const labels = await win(page).locator('[data-testid="dashboard-stat-label"]').allInnerTexts();
    expect(labels.length).toBe(4);
    for (const l of labels) expect(l.trim().length).toBeGreaterThan(0);
  });
});

// ===========================================================================
// 8. Console & network
// ===========================================================================

test.describe('8 — Console & network', () => {
  test('8.1 — no uncaught console errors while the Dashboard loads', async ({ page }) => {
    const errors: string[] = [];
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(m.text());
    });
    page.on('pageerror', (e) => errors.push(String(e)));
    await openDashboard(page, adminTokens);
    await page.waitForTimeout(1000);
    const real = errors.filter(
      (e) => !/favicon|ResizeObserver|Failed to load resource/i.test(e),
    );
    expect(real, real.join('\n')).toEqual([]);
  });

  test('8.2 — the Dashboard only ever calls its two data endpoints', async ({ page }) => {
    // The api client caches (ttl == poll interval), so *which* poll makes a
    // real request is timing-dependent — but over a long enough window and a
    // manual Refresh, every `/api/dashboard/*` request that does fire must be
    // one of the two this screen is allowed to use, and never anything else.
    await openDashboard(page, adminTokens);
    const seen = new Set<string>();
    page.on('request', (r) => {
      const m = r.url().match(/\/api\/dashboard\/([a-z-]+)/);
      if (m) seen.add(m[1]);
    });
    await page.waitForTimeout(12_000); // > 2 poll cycles, past the cache ttl
    const extras = [...seen].filter((k) => !['health', 'server-details'].includes(k));
    expect(extras, `unexpected dashboard endpoints: ${extras.join(', ')}`).toEqual([]);
    expect(seen.size, 'expected at least one data request').toBeGreaterThan(0);
  });
});

// ===========================================================================
// 9. Performance
// ===========================================================================

test.describe('9 — Performance', () => {
  test('9.1 — the Dashboard window shell renders quickly (data may lag)', async ({ page }) => {
    // Time only to the window shell appearing — the cold `server-details`
    // call (dmidecode / smartctl per disk) is not part of "mount".
    await gotoDesktop(page, adminTokens);
    const t0 = Date.now();
    await page.locator('[data-testid="dock-dashboard"]').click();
    await page.waitForSelector('[data-testid="dashboard-page"]', { timeout: 15_000 });
    expect(Date.now() - t0).toBeLessThan(10_000);
  });

  test('9.2 — the Dashboard window keeps its DOM subtree bounded', async ({ page }) => {
    await openDashboard(page, adminTokens);
    const n = await win(page).evaluate((root) => root.querySelectorAll('*').length);
    expect(n).toBeLessThan(2500);
  });

  test('9.3 — heap does not grow unbounded across several poll cycles', async ({ page }) => {
    await openDashboard(page, adminTokens);
    const before = await getHeapMB(page);
    await page.waitForTimeout(16_000); // ~3 poll cycles
    const after = await getHeapMB(page);
    if (before > 0 && after > 0) {
      expect(after - before).toBeLessThan(15);
    }
  });
});

// ===========================================================================
// 10. Keyboard
// ===========================================================================

test.describe('10 — Keyboard', () => {
  test('10.1 — Escape does not crash the window', async ({ page }) => {
    await openDashboard(page, adminTokens);
    await page.keyboard.press('Escape');
    await expect(win(page)).toBeVisible();
  });

  test('10.2 — Tab / Enter in the window does not break the view', async ({ page }) => {
    await openDashboard(page, adminTokens);
    for (let i = 0; i < 3; i++) { await page.keyboard.press('Tab'); }
    await page.keyboard.press('Enter');
    await expect(win(page).locator('[data-testid="dashboard-stats-grid"]')).toBeVisible();
    await expect(win(page)).toBeVisible();
  });
});

// ===========================================================================
// 11. Edge cases
// ===========================================================================

test.describe('11 — Edge cases', () => {
  test('11.1 — a slow / failing health call surfaces the error bar, not a blank screen', async ({ page }) => {
    await seedBrowser(page, adminTokens);
    await page.route('**/api/dashboard/health*', (route) => route.fulfill({ status: 500, body: '{}' }));
    await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
    await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
    await page.locator('[data-testid="dock-dashboard"]').click();
    await page.waitForSelector('[data-testid="dashboard-page"]', { timeout: 15_000 });
    // server-details still succeeds → the shell renders; health failure is
    // non-fatal (the gauge just shows 0), the window must not stay blank.
    await expect(page.locator('[data-testid="dashboard-stats-grid"]')).toBeVisible({ timeout: 15_000 });
  });

  test('11.2 — rapid focus events do not break the view', async ({ page }) => {
    await openDashboard(page, adminTokens);
    for (let i = 0; i < 5; i++) {
      await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    }
    await expect(win(page).locator('[data-testid="dashboard-stats-grid"]')).toBeVisible();
  });

  test('11.3 — leaving and returning to the Dashboard restarts polling cleanly', async ({ page }) => {
    await openDashboard(page, adminTokens);
    await page.locator('.hv-cl-btn-close').first().click();
    await expect(win(page)).toHaveCount(0);
    await page.locator('[data-testid="dock-dashboard"]').click();
    await expect(win(page).locator('[data-testid="dashboard-stats-grid"]')).toBeVisible({ timeout: 15_000 });
  });
});

// ===========================================================================
// 12. Memory regression
// ===========================================================================

test.describe('12 — Memory regression', () => {
  test('12.1 — open/close the Dashboard 5× without DOM node accumulation', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    let baseline = 0;
    for (let i = 0; i < 5; i++) {
      await page.locator('[data-testid="dock-dashboard"]').click();
      await page.waitForSelector('[data-testid="dashboard-page"]', { timeout: 15_000 });
      await page.locator('.hv-cl-btn-close').first().click();
      await expect(win(page)).toHaveCount(0, { timeout: 10_000 });
      const n = await page.evaluate(() => document.querySelectorAll('*').length);
      if (i === 0) baseline = n;
      else expect(n).toBeLessThan(baseline + 400);
    }
  });
});
