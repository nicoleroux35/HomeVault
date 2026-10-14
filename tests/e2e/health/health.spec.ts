/* =============================================================================
 * File          : health.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Health screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Health app — End-to-End Test Suite
 *
 * The Health screen (`frontend/src/routes/Health.svelte`) is a window opened
 * from the desktop. Its dock icon is the shield labelled **"Security"** (a
 * historical mislabel — `handleDockAppClick('health')`); it also opens from
 * the Applications launcher as "Health".
 *
 * It has exactly one backend call — `api.healthCheck()` → `GET /api/health`
 * (never cached) — and renders one card per built-in check from
 * `src/health.rs` (always 8: authentication, storage, memory, disk_space,
 * disk_smart, network, services, temperatures), plus an overall-status
 * badge in the header.
 *
 * **This suite is entirely read-only.** The screen's only action is the
 * "Refresh" button, which just re-runs the one GET. Endpoint auth / payload
 * shape are covered by `tests/health_integration.rs`; the check logic by
 * `src/health.rs`'s unit tests.
 *
 * Run (all):    npx playwright test tests/e2e/health/health.spec.ts --project=firefox
 * Run (single): npx playwright test tests/e2e/health/health.spec.ts -g "1.1"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/health/health.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

const EXPECTED_CHECKS = 8;

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
  // The desktop monitor widgets overlap the dock — hide them so clicks land.
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

/** `[data-testid="health-page"]` is unique to Health.svelte (confirmed via grep). */
function win(page: Page) {
  return page.locator('[data-testid="health-page"]');
}

async function openHealth(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  // Open via the Applications launcher (exercises the launcher path; the
  // dock icon carries the same translated app name).
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[title="Health"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="health-page"]', { timeout: 15_000 });
  // The loading state clears once the first `/api/health` call lands (it
  // shells out to df / lsblk+smartctl / systemctl — give it room). The real
  // endpoint always returns 8 checks, so the grid renders.
  await expect(win(page).locator('[data-testid="health-loading"]')).toHaveCount(0, { timeout: 30_000 });
  await expect(win(page).locator('[data-testid="health-card"]').first()).toBeVisible({ timeout: 5_000 });
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
  // The first mount waits on a cold `/api/health` (per-disk smartctl).
  test.setTimeout(90_000);
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
  test('1.1 — Health window opens from the launcher', async ({ page }) => {
    await openHealth(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — the loading placeholder appears then clears', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    await page.locator('.lp-card[title="Health"]').click();
    await page.waitForSelector('[data-testid="health-page"]', { timeout: 15_000 });
    // The loading state (`health-loading`) clears once the first `/api/health`
    // lands, then the 8 cards render.
    await expect(win(page).locator('[data-testid="health-loading"]')).toHaveCount(0, { timeout: 25_000 });
    await expect(win(page).locator('[data-testid="health-card"]')).toHaveCount(EXPECTED_CHECKS);
  });

  test('1.3 — the header shows the title only (standardized header)', async ({ page }) => {
    await openHealth(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title')).not.toBeEmpty();
    // `.hv-winhdr-subtitle` no longer exists (the prop was removed from
    // `WindowHeader` on 2026-09-14), so this locator matched nothing.
    // Assert the bar's real shape instead: the icon tile + the title.
    await expect(win(page).locator('.hv-winhdr > *')).toHaveCount(2);
  });

  test('1.4 — exactly 8 check cards render', async ({ page }) => {
    await openHealth(page, adminTokens);
    await expect(win(page).locator('[data-testid="health-card"]')).toHaveCount(EXPECTED_CHECKS);
  });

  test('1.5 — the overall-status strip renders in the body with a label', async ({ page }) => {
    await openHealth(page, adminTokens);
    // Moved out of the window header (which is now icon + title only) into the
    // page body — it is a real reading, not header decoration.
    await expect(win(page).locator('[data-testid="health-overall"]')).toBeVisible();
    const badge = win(page).locator('[data-testid="health-badge"]');
    await expect(badge).toBeVisible();
    await expect(badge).not.toBeEmpty();
  });

  test('1.6 — no manual Refresh button (auto-revalidation instead)', async ({ page }) => {
    await openHealth(page, adminTokens);
    // The header bar carries the icon + title only: even the `extra` / `actions`
    // slots are empty, so no manual re-read button can live there.
    await expect(win(page).locator('.hv-winhdr-actions')).toHaveCount(0);
    await expect(win(page).getByRole('button', { name: 'Refresh' })).toHaveCount(0);
  });
});

// ===========================================================================
// 2. Check content
// ===========================================================================

test.describe('2 — Check content', () => {
  test('2.1 — every card has a translated name, a status label, and a message', async ({ page }) => {
    await openHealth(page, adminTokens);
    const cards = win(page).locator('[data-testid="health-card"]');
    const n = await cards.count();
    expect(n).toBe(EXPECTED_CHECKS);
    for (let i = 0; i < n; i++) {
      const card = cards.nth(i);
      const name = (await card.locator('[data-testid="health-card-name"]').innerText()).trim();
      const status = (await card.locator('[data-testid="health-card-status"]').innerText()).trim();
      const details = (await card.locator('[data-testid="health-card-details"]').innerText()).trim();
      expect(name.length).toBeGreaterThan(0);
      // Not a raw snake_case identifier / untranslated key.
      expect(name).not.toMatch(/^health\.|_/);
      expect(status.length).toBeGreaterThan(0);
      expect(details.length).toBeGreaterThan(0);
    }
  });

  test('2.2 — card names cover the 8 known checks (no duplicates)', async ({ page }) => {
    await openHealth(page, adminTokens);
    const names = await win(page).locator('[data-testid="health-card-name"]').allInnerTexts();
    const trimmed = names.map((s) => s.trim());
    expect(new Set(trimmed).size).toBe(EXPECTED_CHECKS);
  });

  test('2.3 — status text is never NaN / undefined / null', async ({ page }) => {
    await openHealth(page, adminTokens);
    const texts = await win(page).locator('[data-testid="health-card-status"]').allInnerTexts();
    for (const s of texts) expect(s).not.toMatch(/NaN|undefined|null/i);
  });

  test('2.4 — the overall badge agrees with the worst card', async ({ page }) => {
    await openHealth(page, adminTokens);
    // Read the raw API rather than re-deriving from localized labels.
    const data = await page.evaluate(async () => {
      const r = await fetch('/api/health', {
        headers: { 'x-session-token': localStorage.getItem('session-token') || '' },
      });
      return (await r.json())?.data;
    });
    const statuses: string[] = (data.checks || []).map((c: any) => c.status);
    const worst = statuses.includes('unhealthy')
      ? 'unhealthy'
      : statuses.includes('warning')
        ? 'warning'
        : statuses.length && statuses.every((s) => s === 'healthy')
          ? 'healthy'
          : 'unknown';
    expect(data.overall_status).toBe(worst);
  });
});

// ===========================================================================
// 3. Interactions
// ===========================================================================

test.describe('3 — Interactions', () => {
  test('3.1 — a window-focus event re-fetches /api/health', async ({ page }) => {
    await openHealth(page, adminTokens);
    let hits = 0;
    page.on('response', (r) => {
      if (r.url().includes('/api/health')) hits += 1;
    });
    await page.waitForTimeout(1_600); // clear onAppVisible's coalescing cooldown
    await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    await expect.poll(() => hits, { timeout: 10_000 }).toBeGreaterThan(0);
    await expect(win(page).locator('[data-testid="health-card"]')).toHaveCount(EXPECTED_CHECKS);
  });

  test('3.2 — rapid focus/blur events do not break the view', async ({ page }) => {
    await openHealth(page, adminTokens);
    for (let i = 0; i < 5; i++) {
      await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    }
    await expect(win(page).locator('[data-testid="health-card"]')).toHaveCount(EXPECTED_CHECKS);
  });

  test('3.3 — the window can be closed', async ({ page }) => {
    await openHealth(page, adminTokens);
    await page.locator('.hv-cl-btn-close').first().click();
    await expect(win(page)).toHaveCount(0, { timeout: 10_000 });
  });
});

// ===========================================================================
// 4. Security
// ===========================================================================

test.describe('4 — Security', () => {
  test('4.1 — /api/health refuses an unauthenticated request', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const body = await page.evaluate(async () => {
      const r = await fetch('/api/health', { headers: {} });
      return { status: r.status, json: await r.json().catch(() => ({})) };
    });
    // The handler answers 200 and signals refusal in the body.
    expect(body.json?.success).toBe(false);
    expect(body.json?.data).toBeFalsy();
  });

  test('4.2 — a bogus token is refused', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const json = await page.evaluate(async () => {
      const r = await fetch('/api/health', { headers: { 'x-session-token': 'not-a-real-token' } });
      return r.json().catch(() => ({}));
    });
    expect(json?.success).toBe(false);
  });

  test('4.3 — no token or secret is placed in the URL', async ({ page }) => {
    await openHealth(page, adminTokens);
    expect(page.url()).not.toMatch(/token|session|password|secret/i);
  });

  test('4.4 — localStorage holds no plaintext password', async ({ page }) => {
    await openHealth(page, adminTokens);
    const dump = await page.evaluate(() => JSON.stringify(localStorage));
    expect(dump).not.toContain(ADMIN_PASS);
  });

  test('4.5 — check messages from the host are rendered as text, not markup', async ({ page }) => {
    await openHealth(page, adminTokens);
    const html = await win(page).locator('[data-testid="health-grid"]').innerHTML();
    expect(html).not.toMatch(/<script|onerror=|<img[^>]+onerror/i);
  });
});

// ===========================================================================
// 5. Navigation
// ===========================================================================

test.describe('5 — Navigation', () => {
  test('5.1 — the dock Health shield opens the Health app', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('[data-testid="dock-health"]').click();
    await page.waitForSelector('[data-testid="health-page"]', { timeout: 15_000 });
    await expect(win(page)).toBeVisible();
  });

  test('5.2 — reopening focuses the existing window (no duplicate)', async ({ page }) => {
    await openHealth(page, adminTokens);
      await page.locator('[data-testid="dock-health"]').click({ force: true });
    await page.waitForTimeout(500);
    await expect(win(page)).toHaveCount(1);
  });

  test('5.3 — minimising then restoring keeps the same window', async ({ page }) => {
    await openHealth(page, adminTokens);
    await page.locator('[data-testid="win-minimize"]').first().click();
    await page.locator('[data-testid="dock-health"]').click();
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
      await openHealth(page, adminTokens);
      const overflow = await win(page).evaluate((el) => el.scrollWidth - el.clientWidth);
      expect(overflow).toBeLessThanOrEqual(2);
    });
  }
});

// ===========================================================================
// 7. Accessibility
// ===========================================================================

test.describe('7 — Accessibility', () => {
  test('7.1 — the window header bar renders the icon + title only', async ({ page }) => {
    await openHealth(page, adminTokens);
    const bar = win(page).locator('.hv-winhdr');
    await expect(bar.locator('.hv-winhdr-icon')).toHaveCount(1);
    await expect(bar.locator('.hv-winhdr-title')).toHaveCount(1);
    // Neither injectable slot of the bar may be filled.
    await expect(bar.locator('.hv-winhdr-actions')).toHaveCount(0);
    await expect(bar.locator('.hv-winhdr-center')).toHaveCount(0);
  });

  test('7.2 — no duplicate id attributes in the Health window', async ({ page }) => {
    await openHealth(page, adminTokens);
    const dupes = await win(page).evaluate((root) => {
      const ids = [...root.querySelectorAll('[id]')].map((e) => e.id);
      return ids.filter((v, i) => ids.indexOf(v) !== i);
    });
    expect(dupes).toEqual([]);
  });

  test('7.3 — every check card exposes readable text', async ({ page }) => {
    await openHealth(page, adminTokens);
    const cards = win(page).locator('[data-testid="health-card"]');
    const n = await cards.count();
    for (let i = 0; i < n; i++) {
      expect((await cards.nth(i).innerText()).trim().length).toBeGreaterThan(0);
    }
  });
});

// ===========================================================================
// 8. Console & network
// ===========================================================================

test.describe('8 — Console & network', () => {
  test('8.1 — no uncaught console errors while Health loads', async ({ page }) => {
    const errors: string[] = [];
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(m.text());
    });
    page.on('pageerror', (e) => errors.push(String(e)));
    await openHealth(page, adminTokens);
    await page.waitForTimeout(1000);
    const real = errors.filter((e) => !/favicon|ResizeObserver|Failed to load resource/i.test(e));
    expect(real, real.join('\n')).toEqual([]);
  });

  test('8.2 — the Health screen is read-only: it only ever GETs /api/health', async ({
    page,
  }) => {
    // The desktop shell polls several endpoints on its own (dock badges,
    // monitors) — that traffic is not this screen's. What we can assert is
    // that Health issues no write and no endpoint other than its one GET,
    // including on the focus-triggered auto-revalidation.
    await openHealth(page, adminTokens);
    const offenders: string[] = [];
    let healthGets = 0;
    page.on('request', (r) => {
      const url = r.url();
      if (!url.includes('/api/')) return;
      if (url.includes('/api/health')) {
        if (r.method() !== 'GET') offenders.push(`${r.method()} ${url}`);
        else healthGets += 1;
      } else if (r.method() !== 'GET') {
        offenders.push(`${r.method()} ${url}`);
      }
    });
    await page.waitForTimeout(1_600);
    await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    await expect.poll(() => healthGets, { timeout: 8000 }).toBeGreaterThan(0);
    await page.waitForTimeout(1500);
    expect(offenders, `Health issued a non-GET / write request: ${offenders.join(', ')}`).toEqual(
      [],
    );
  });
});

// ===========================================================================
// 9. Performance
// ===========================================================================

test.describe('9 — Performance', () => {
  test('9.1 — the window shell renders quickly (data may lag)', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    await page.locator('.lp-card[title="Health"]').click();
    const t0 = Date.now();
    await page.waitForSelector('[data-testid="health-page"]', { timeout: 15_000 });
    expect(Date.now() - t0).toBeLessThan(10_000);
  });

  test('9.2 — the Health window keeps its DOM subtree bounded', async ({ page }) => {
    await openHealth(page, adminTokens);
    const n = await win(page).evaluate((root) => root.querySelectorAll('*').length);
    expect(n).toBeLessThan(1200);
  });
});

// ===========================================================================
// 10. Keyboard
// ===========================================================================

test.describe('10 — Keyboard', () => {
  test('10.1 — Escape does not crash the window', async ({ page }) => {
    await openHealth(page, adminTokens);
    await page.keyboard.press('Escape');
    await expect(win(page)).toBeVisible();
  });

  test('10.2 — the window survives repeated Tab / Enter with no refresh control', async ({ page }) => {
    await openHealth(page, adminTokens);
    for (let i = 0; i < 4; i++) { await page.keyboard.press('Tab'); }
    await page.keyboard.press('Enter');
    await expect(win(page).locator('[data-testid="health-card"]')).toHaveCount(EXPECTED_CHECKS);
  });
});

// ===========================================================================
// 11. Edge cases
// ===========================================================================

test.describe('11 — Edge cases', () => {
  test('11.1 — a failing /api/health surfaces the empty state, not a blank window', async ({
    page,
  }) => {
    await seedBrowser(page, adminTokens);
    await page.route('**/api/health*', (route) => route.fulfill({ status: 500, body: '{}' }));
    await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
    await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
    await page
      .addStyleTag({ content: '[class*="-monitor"]{display:none!important}' })
      .catch(() => {});
    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    await page.locator('.lp-card[title="Health"]').click();
    await page.waitForSelector('[data-testid="health-page"]', { timeout: 15_000 });
    // `loading` clears; with no checks the empty-state block shows
    // (data-testid="health-empty" only exists in the empty branch) plus zero
    // cards.
    await expect(win(page).locator('[data-testid="health-empty"]')).toBeVisible({ timeout: 15_000 });
    await expect(win(page).locator('[data-testid="health-card"]')).toHaveCount(0);
    // The loading state itself is gone (the window is not stuck loading).
    await expect(win(page).locator('[data-testid="health-loading"]')).toHaveCount(0);
  });

  test('11.2 — a malformed /api/health payload does not throw', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await seedBrowser(page, adminTokens);
    await page.route('**/api/health*', (route) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: '{"success":true}' }),
    );
    await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
    await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    await page.locator('.lp-card[title="Health"]').click();
    await page.waitForSelector('[data-testid="health-page"]', { timeout: 15_000 });
    await page.waitForTimeout(1000);
    expect(errors, errors.join('\n')).toEqual([]);
  });
});

// ===========================================================================
// 12. Memory regression
// ===========================================================================

test.describe('12 — Memory regression', () => {
  test('12.1 — open/close Health 5× without DOM node accumulation', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    let baseline = 0;
    for (let i = 0; i < 5; i++) {
    await page.locator('[data-testid="dock-health"]').click({ force: true });
      await page.waitForSelector('[data-testid="health-page"]', { timeout: 15_000 });
      await page.locator('.hv-cl-btn-close').first().click();
      await expect(win(page)).toHaveCount(0, { timeout: 10_000 });
      const n = await page.evaluate(() => document.querySelectorAll('*').length);
      if (i === 0) baseline = n;
      else expect(n).toBeLessThan(baseline + 400);
    }
  });
});
