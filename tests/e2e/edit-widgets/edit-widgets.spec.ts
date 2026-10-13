/**
 * Edit Widgets — End-to-End Test Suite
 *
 * The Edit Widgets app (id `edit-widgets`) is a drag & drop layout editor for
 * the desktop widgets. It has two zones: a **Featured** zone capped at 3
 * widgets, and the full catalog list with visibility toggles. The layout is
 * per-account (`stores/widgetLayout.ts`, localStorage) and visibility stays
 * owned by `monitorStore`.
 *
 * The suite covers the cap, the star toggle, keyboard reordering (the
 * accessible alternative to dragging) and the visibility toggle. It restores
 * the account's widget layout in `afterAll`.
 *
 * Run (all):    npx playwright test tests/e2e/edit-widgets/edit-widgets.spec.ts --project=firefox
 * Run (single): npx playwright test tests/e2e/edit-widgets/edit-widgets.spec.ts -g "2.1"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/edit-widgets/edit-widgets.spec.ts
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

async function seedBrowser(page: Page, tokens: Tokens) {
  await page.addInitScript(
    ({ t }) => {
      try {
        localStorage.setItem('session-token', t.token);
        localStorage.setItem('csrf-token', t.csrf);
        for (const k of Object.keys(localStorage)) {
          if (k.startsWith('hv-widget-layout:')) localStorage.removeItem(k);
        }
      } catch {
        /* storage unavailable — the test will surface it */
      }
    },
    { t: tokens },
  );
}

async function openEditWidgets(page: Page, tokens: Tokens) {
  await seedBrowser(page, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[title="Edit Widgets"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="edit-widgets-page"]', { timeout: 15_000 });
}

function win(page: Page) {
  return page.locator('[data-testid="edit-widgets-page"]');
}

let adminTokens: Tokens;
let savedLayout: string | null = null;

test.beforeAll(async ({ playwright }) => {
  const ctx = await playwright.request.newContext({ baseURL: BASE });
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
  const res = await ctx.get('/api/monitor-layout', { headers: { 'x-session-token': token } });
  savedLayout = (await res.json().catch(() => ({})))?.data?.layout_json ?? null;
  await ctx.dispose();
});

test.afterAll(async ({ playwright }) => {
  if (savedLayout === null) return;
  const ctx = await playwright.request.newContext({ baseURL: BASE });
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
  if (heapMB > 10) console.log(`[MEM] ${testInfo.title}: ${heapMB.toFixed(1)} MB heap`);
});

// ===========================================================================
// 1 — Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — the editor opens with both zones and the full catalog', async ({ page }) => {
    await openEditWidgets(page, adminTokens);
    await expect(win(page).locator('[data-testid="edit-widgets-featured"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="edit-widgets-all"]')).toBeVisible();
    // The catalog has 11 widgets.
    await expect(win(page).locator('[data-testid="edit-widgets-tile"]')).toHaveCount(11);
  });

  test('1.2 — the featured zone starts empty for a fresh account', async ({ page }) => {
    await openEditWidgets(page, adminTokens);
    await expect(win(page).locator('[data-testid="edit-widgets-featured-empty"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="edit-widgets-count"]')).toContainText('0 / 3');
  });
});

// ===========================================================================
// 2 — Featured cap (max 3)
// ===========================================================================

test.describe('2 — Featured cap', () => {
  test('2.1 — starring widgets features them and updates the counter', async ({ page }) => {
    await openEditWidgets(page, adminTokens);
    const stars = win(page).locator('[data-testid="edit-widgets-feature"]');
    await stars.nth(0).click();
    await stars.nth(1).click();
    await expect(win(page).locator('[data-testid="edit-widgets-count"]')).toContainText('2 / 3');
    await expect(win(page).locator('[data-testid="edit-widgets-featured-tile"]')).toHaveCount(2);
  });

  test('2.2 — a fourth feature is refused with a visible cap message', async ({ page }) => {
    await openEditWidgets(page, adminTokens);
    const stars = win(page).locator('[data-testid="edit-widgets-feature"]');
    await stars.nth(0).click();
    await stars.nth(1).click();
    await stars.nth(2).click();
    await expect(win(page).locator('[data-testid="edit-widgets-count"]')).toContainText('3 / 3');
    await stars.nth(3).click();
    await expect(win(page).locator('[data-testid="edit-widgets-cap"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="edit-widgets-count"]')).toContainText('3 / 3');
    await expect(win(page).locator('[data-testid="edit-widgets-featured-tile"]')).toHaveCount(3);
  });

  test('2.3 — unfeaturing frees a slot', async ({ page }) => {
    await openEditWidgets(page, adminTokens);
    const stars = win(page).locator('[data-testid="edit-widgets-feature"]');
    await stars.nth(0).click();
    await stars.nth(1).click();
    await stars.nth(2).click();
    // Unfeature one from the featured zone.
    await win(page).locator('[data-testid="edit-widgets-unfeature"]').first().click();
    await expect(win(page).locator('[data-testid="edit-widgets-count"]')).toContainText('2 / 3');
    // A new star now succeeds.
    await stars.nth(3).click();
    await expect(win(page).locator('[data-testid="edit-widgets-count"]')).toContainText('3 / 3');
  });
});

// ===========================================================================
// 3 — Reordering (keyboard alternative to drag)
// ===========================================================================

test.describe('3 — Reordering', () => {
  test('3.1 — the down button swaps a widget with the next one', async ({ page }) => {
    await openEditWidgets(page, adminTokens);
    const names = win(page).locator('[data-testid="edit-widgets-tile"] div.text-c-base');
    const before = await names.allInnerTexts();
    await win(page).locator('[data-testid="edit-widgets-down"]').first().click();
    const after = await names.allInnerTexts();
    expect(after[0]).toBe(before[1]);
    expect(after[1]).toBe(before[0]);
  });

  test('3.2 — the first up button is disabled (no-op at the edge)', async ({ page }) => {
    await openEditWidgets(page, adminTokens);
    await expect(win(page).locator('[data-testid="edit-widgets-up"]').first()).toBeDisabled();
  });
});

// ===========================================================================
// 4 — Visibility toggle
// ===========================================================================

test.describe('4 — Visibility', () => {
  test('4.1 — toggling visibility flips aria-pressed', async ({ page }) => {
    await openEditWidgets(page, adminTokens);
    const btn = win(page).locator('[data-testid="edit-widgets-visibility"]').first();
    const before = await btn.getAttribute('aria-pressed');
    await btn.click();
    await expect(btn).toHaveAttribute('aria-pressed', before === 'true' ? 'false' : 'true');
    // Restore.
    await btn.click();
  });
});

// ===========================================================================
// 5 — Accessibility
// ===========================================================================

test.describe('5 — Accessibility', () => {
  test('5.1 — every icon button has an accessible name', async ({ page }) => {
    await openEditWidgets(page, adminTokens);
    const buttons = win(page).locator('[data-testid^="edit-widgets-"] button');
    const count = await buttons.count();
    expect(count).toBeGreaterThan(0);
    for (let i = 0; i < count; i++) {
      const label = await buttons.nth(i).getAttribute('aria-label');
      expect(label, `button ${i} needs an aria-label`).toBeTruthy();
    }
  });

  test('5.2 — the featured zone is a labelled section', async ({ page }) => {
    await openEditWidgets(page, adminTokens);
    const heading = win(page).locator('#ew-featured-h');
    await expect(heading).toBeVisible();
    await expect(win(page).locator('[aria-labelledby="ew-featured-h"]')).toHaveCount(1);
  });
});
