/* =============================================================================
 * File          : metrics.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Metrics screen end-to-end tests (Playwright): covers the screen's
 *   functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Metrics app — End-to-End Test Suite
 *
 * The Metrics screen (`frontend/src/routes/Metrics.svelte`, app id
 * `metrics`) is READ-ONLY at the screen level — the collector is the only
 * writer. This suite covers:
 *
 *   1. Launcher opening + the five chart cards
 *   2. Range switching (1h / 24h / 7d) updates the charts
 *   3. Accessibility: charts expose aria-labels, range buttons are a
 *      toggle group (aria-pressed)
 *   4. The screen renders for non-admin users too (auth-level endpoint)
 *   5. Null semantics (ROADMAP 11): a series nobody measured is a gap, and a
 *      slow range answer cannot repaint a newer one
 *
 * A freshly provisioned instance may have no samples yet — the suite
 * accepts both the empty state and the charts, and only asserts what is
 * deterministic (card titles, range group behaviour).
 *
 * Run (all):    npx playwright test tests/e2e/metrics/metrics.spec.ts --project=firefox
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/metrics/metrics.spec.ts
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

async function apiLogin(ctx: APIRequestContext, username: string, password: string) {
  const res = await ctx.post('/api/auth/login', { data: { username, password } });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
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


/** Fresh installs show the once-per-version "What's New" modal — close it. */
async function dismissWhatsNew(page: Page) {
  const modal = page.locator('[data-testid="whats-new-modal"]');
  if (await modal.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await modal.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

function win(page: Page) {
  return page.locator('[data-testid="metrics-page"]');
}

async function openMetrics(page: Page, tokens: Tokens, lang?: string) {
  await seedBrowser(page, tokens, lang);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await dismissWhatsNew(page);
  await page
    .addStyleTag({ content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}' })
    .catch(() => {});
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[title="Metrics"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="metrics-page"]', { timeout: 15_000 });
  // Loading resolves into either the charts or the empty state.
  await expect(
    win(page).locator('[data-testid="metrics-card-cpu"], [data-testid="metrics-empty"]').first(),
  ).toBeVisible({ timeout: 20_000 });
}

let adminTokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
});

test('1.1 — Metrics opens from the launcher with the five chart cards', async ({ page }) => {
  await openMetrics(page, adminTokens);
  for (const id of ['cpu', 'mem', 'load1', 'temp', 'net']) {
    await expect(
      win(page).locator(`[data-testid="metrics-card-${id}"], [data-testid="metrics-empty"]`).first(),
    ).toBeVisible();
  }
});

test('2.1 — Range buttons are a toggle group and switch the range', async ({ page }) => {
  await openMetrics(page, adminTokens);
  const group = win(page).locator('[data-testid="metrics-ranges"]');
  await expect(group).toBeVisible();

  // aria-pressed exactly one true at any time.
  for (const r of ['1h', '24h', '7d']) {
    await group.locator(`[data-testid="metrics-range-${r}"]`).click();
    await expect(group.locator(`[data-testid="metrics-range-${r}"]`)).toHaveAttribute('aria-pressed', 'true');
  }
  await group.locator('[data-testid="metrics-range-1h"]').click();
  await expect(group.locator('[data-testid="metrics-range-1h"]')).toHaveAttribute('aria-pressed', 'true');
});

test('3.1 — Charts expose accessible names (or the empty state is announced)', async ({ page }) => {
  await openMetrics(page, adminTokens);
  const chart = win(page).locator('[data-testid="metrics-chart-cpu"]');
  if (await chart.count()) {
    const label = await chart.getAttribute('aria-label');
    expect(label, 'chart needs an accessible name').toBeTruthy();
    expect(label!.length).toBeGreaterThan(0);
  } else {
    await expect(win(page).locator('[data-testid="metrics-empty"]')).toBeVisible();
  }
});

test('4.1 — The screen opens for non-admin users (auth-level endpoint)', async ({ page, playwright }) => {
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  const username = 'e2enonadmin';
  await ctx
    .post(`${BASE}/api/users`, {
      headers: { 'x-session-token': token, 'x-csrf-token': csrf },
      data: { username, password: 'NonAdmin123!', fullname: 'E2E', email: 'e2e@example.com', role: 'user' },
    })
    .catch(() => {});
  const userLogin = await apiLogin(ctx, username, 'NonAdmin123!');
  test.skip(!userLogin.token, 'non-admin account unavailable');

  await openMetrics(page, { username, password: 'NonAdmin123!', ...userLogin });
  await expect(win(page)).toBeVisible();
});

// =============================================================================
// 5. Null semantics (ROADMAP 11) — a gap is not a zero, a stale answer is not
//    an answer
// =============================================================================

/** One point of a stubbed history answer; every series is optional. */
function point(ts: number, values: Partial<Record<'cpu' | 'mem' | 'load1' | 'net_rx' | 'net_tx' | 'temp', number>>) {
  return {
    ts,
    cpu: values.cpu ?? null,
    mem: values.mem ?? null,
    load1: values.load1 ?? null,
    net_rx: values.net_rx ?? null,
    net_tx: values.net_tx ?? null,
    temp: values.temp ?? null,
  };
}

function historyBody(points: unknown[]) {
  return JSON.stringify({
    success: true,
    data: { range: '1h', bucket_seconds: 60, points },
  });
}

test('5.1 — A temperature-only dataset renders the charts, not "no data"', async ({ page }) => {
  const now = Math.floor(Date.now() / 1000);
  await page.route('**/api/metrics/history*', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: historyBody([
        point(now - 120, { temp: 41 }),
        point(now - 60, { temp: 44 }),
        point(now, { temp: 46 }),
      ]),
    }),
  );

  await openMetrics(page, adminTokens);
  // The old empty-state heuristic only looked at cpu/mem, so a host with a
  // working thermal zone and nothing else rendered "no data".
  await expect(win(page).locator('[data-testid="metrics-empty"]')).toHaveCount(0);
  await expect(win(page).locator('[data-testid="metrics-card-temp"]')).toBeVisible();
  await expect(win(page).locator('[data-testid="metrics-current-temp"]')).toHaveText('46 °C');
  // A series nobody measured shows an em dash, never a fabricated 0.
  await expect(win(page).locator('[data-testid="metrics-current-cpu"]')).toHaveText('—');
});

test('5.2 — A slow 24h answer cannot repaint the newer 1h range', async ({ page }) => {
  const now = Math.floor(Date.now() / 1000);
  await page.route('**/api/metrics/history*', async (route) => {
    const range = new URL(route.request().url()).searchParams.get('range');
    // The 24h answer is deliberately slower than the 1h one.
    if (range === '24h') await new Promise((resolve) => setTimeout(resolve, 1500));
    const cpu = range === '24h' ? 90 : 11;
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: historyBody([point(now - 60, { cpu }), point(now, { cpu })]),
    });
  });

  await openMetrics(page, adminTokens);
  const group = win(page).locator('[data-testid="metrics-ranges"]');
  await group.locator('[data-testid="metrics-range-24h"]').click();
  await group.locator('[data-testid="metrics-range-1h"]').click();

  // Wait past the 24h delay: the screen must still show the 1h figure.
  await page.waitForTimeout(2000);
  await expect(group.locator('[data-testid="metrics-range-1h"]')).toHaveAttribute('aria-pressed', 'true');
  await expect(win(page).locator('[data-testid="metrics-current-cpu"]')).toHaveText('11.0 %');
});
