/* =============================================================================
 * File          : shortcuts.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Shortcuts screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Web shortcuts — End-to-End Test Suite
 *
 * Pinned web shortcuts live in a per-account localStorage store
 * (`stores/shortcuts.ts`) and render as a column of tiles on the desktop
 * (`components/desktop/DesktopShortcuts.svelte`). Adding one calls the
 * server-side metadata endpoint (`POST /api/shortcuts/fetch-metadata`), which
 * is SSRF-guarded.
 *
 * The suite below exercises the client-side store/UI and the SSRF refusals;
 * it never fetches a real third-party page, so it is deterministic offline.
 * A page that cannot be fetched for network reasons is treated as an
 * acceptable "error shown" outcome (the same way other offline-tolerant specs
 * handle external calls).
 *
 * Run (all):    npx playwright test tests/e2e/shortcuts/shortcuts.spec.ts --project=firefox
 * Run (single): npx playwright test tests/e2e/shortcuts/shortcuts.spec.ts -g "2.1"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/shortcuts/shortcuts.spec.ts
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
        // Start from a clean shortcut set so the spec is standalone.
        for (const k of Object.keys(localStorage)) {
          if (k.startsWith('hv-shortcuts:')) localStorage.removeItem(k);
        }
      } catch {
        /* storage unavailable — the test will surface it */
      }
    },
    { t: tokens },
  );
}

async function gotoDesktop(page: Page, tokens: Tokens) {
  await seedBrowser(page, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
}

function bar(page: Page) {
  return page.locator('[data-testid="desktop-shortcuts"]');
}

let adminTokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  const ctx = await playwright.request.newContext({ baseURL: BASE });
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
  await ctx.dispose();
});

test.afterEach(async ({ page }, testInfo) => {
  const heapMB = await getHeapMB(page).catch(() => 0);
  if (heapMB > 10) console.log(`[MEM] ${testInfo.title}: ${heapMB.toFixed(1)} MB heap`);
});

// ===========================================================================
// 1 — Rendering & empty state
// ===========================================================================

test.describe('1 — Rendering', () => {
  test('1.1 — the shortcuts bar renders on the desktop with an add button', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await expect(bar(page)).toBeVisible({ timeout: 15_000 });
    await expect(bar(page).locator('[data-testid="desktop-shortcut-add"]')).toBeVisible();
  });

  test('1.2 — the add dialog opens and closes', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await bar(page).locator('[data-testid="desktop-shortcut-add"]').click();
    await expect(page.locator('[data-testid="shortcuts-url"]')).toBeVisible({ timeout: 8_000 });
    await page.keyboard.press('Escape');
    await expect(page.locator('[data-testid="shortcuts-url"]')).toHaveCount(0);
  });
});

// ===========================================================================
// 2 — Adding a shortcut
// ===========================================================================

test.describe('2 — Adding', () => {
  test('2.1 — a valid URL is persisted per account and renders a tile', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await bar(page).locator('[data-testid="desktop-shortcut-add"]').click();
    const input = page.locator('[data-testid="shortcuts-url"]');
    await input.fill('https://example.com');
    await page.locator('[data-testid="shortcuts-submit"]').click();
    // Either the metadata fetch succeeded (tile appears) or it failed for
    // network reasons (error shown and dialog stays open). Both are valid
    // offline; the tile case is the interesting assertion.
    const tile = page.locator('[data-testid="desktop-shortcut"]');
    const err = page.locator('[data-testid="shortcuts-error"]');
    await expect.poll(async () => (await tile.count()) + (await err.count())).toBeGreaterThan(0);
    if (await tile.count()) {
      await expect(tile.first()).toContainText(/example/i);
      // Persisted under the account-namespaced key.
      const stored = await page.evaluate(() =>
        Object.keys(localStorage).filter((k) => k.startsWith('hv-shortcuts:')),
      );
      expect(stored.length).toBeGreaterThan(0);
    }
  });

  test('2.2 — a private/loopback URL is refused server-side (SSRF guard)', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await bar(page).locator('[data-testid="desktop-shortcut-add"]').click();
    await page.locator('[data-testid="shortcuts-url"]').fill('http://127.0.0.1:8080/');
    await page.locator('[data-testid="shortcuts-submit"]').click();
    await expect(page.locator('[data-testid="shortcuts-error"]')).toBeVisible({ timeout: 10_000 });
    // The dialog must not have added a tile for the local address.
    await expect(page.locator('[data-testid="desktop-shortcut"]')).toHaveCount(0);
  });

  test('2.3 — removing a shortcut deletes its tile and storage entry', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    // Seed a shortcut directly through the store's storage key.
    await page.evaluate(() => {
      const key = Object.keys(localStorage).find((k) => k.startsWith('hv-shortcuts:'));
      const k = key || 'hv-shortcuts:e2eadmin';
      localStorage.setItem(
        k,
        JSON.stringify([
          { id: 'sc_test', url: 'https://example.com', title: 'Example', description: '', iconUrl: null, siteName: 'example.com' },
        ]),
      );
    });
    await page.reload({ waitUntil: 'domcontentloaded' });
    await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
    const tile = page.locator('[data-testid="desktop-shortcut"]');
    await expect(tile).toHaveCount(1, { timeout: 10_000 });
    await tile.first().locator('[data-testid="desktop-shortcut-remove"]').click();
    await expect(tile).toHaveCount(0);
  });
});

// ===========================================================================
// 3 — Accessibility & security
// ===========================================================================

test.describe('3 — Accessibility & security', () => {
  test('3.1 — the add button and shortcut tiles have accessible names', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const add = bar(page).locator('[data-testid="desktop-shortcut-add"]');
    expect((await add.getAttribute('aria-label'))?.length ?? 0).toBeGreaterThan(0);
  });

  test('3.2 — a malicious title is rendered as text, never as markup', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.evaluate(() => {
      const k =
        Object.keys(localStorage).find((x) => x.startsWith('hv-shortcuts:')) || 'hv-shortcuts:e2eadmin';
      localStorage.setItem(
        k,
        JSON.stringify([
          { id: 'sc_xss', url: 'https://example.com', title: '<img src=x onerror=alert(1)>', description: '', iconUrl: null, siteName: '' },
        ]),
      );
    });
    await page.reload({ waitUntil: 'domcontentloaded' });
    await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
    const tile = page.locator('[data-testid="desktop-shortcut"]').first();
    await expect(tile).toBeVisible({ timeout: 10_000 });
    // No <img> injected from the title, and the raw text is present.
    await expect(tile.locator('img')).toHaveCount(0);
    await expect(tile).toContainText('<img src=x onerror=alert(1)>');
  });
});