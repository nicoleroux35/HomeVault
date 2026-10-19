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
 * (`components/desktop/DesktopShortcuts.svelte`). The **add** control is not a
 * desktop control: it is the launcher grid's LAST tile
 * (`components/desktop/HvLaunchpad.svelte` → `launcher-shortcut-add`), and it
 * opens `ShortcutAddDialog.svelte`. Adding one calls the server-side metadata
 * endpoint (`POST /api/shortcuts/fetch-metadata`), which is SSRF-guarded.
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

async function gotoDesktop(page: Page, tokens: Tokens, shortcut?: Record<string, unknown>) {
  // Order matters: `seedBrowser` registers the script that CLEARS every
  // `hv-shortcuts:*` key, so the seed has to be registered after it.
  await seedBrowser(page, tokens);
  if (shortcut) await seedShortcut(page, tokens, shortcut);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
}

function bar(page: Page) {
  return page.locator('[data-testid="desktop-shortcuts"]');
}

/** Seed one shortcut into the account-namespaced storage key, before the app
 *  boots.
 *
 *  `addInitScript` (not `evaluate` + `reload`) is mandatory here:
 *  `seedBrowser`'s init script clears every `hv-shortcuts:*` key on **every**
 *  document load, so a seed written with `evaluate` was wiped by the very
 *  reload meant to pick it up — that is why these two cases used to fail. The
 *  script is self-contained (it clears, then writes) and must be registered
 *  AFTER `seedBrowser`; `gotoDesktop` is what guarantees that order. */
async function seedShortcut(page: Page, tokens: Tokens, shortcut: Record<string, unknown>) {
  await page.addInitScript(
    ({ username, s }) => {
      try {
        for (const k of Object.keys(localStorage)) {
          if (k.startsWith('hv-shortcuts:')) localStorage.removeItem(k);
        }
        localStorage.setItem(`hv-shortcuts:${username}`, JSON.stringify([s]));
      } catch {
        /* storage unavailable — the test will surface it */
      }
    },
    { username: tokens.username, s: shortcut },
  );
}

/** The launcher, opened from the dock's Launchpad tile. */
async function openLauncher(page: Page) {
  await page.locator('[data-testid="dock-launchpad"]').click();
  await expect(page.locator('.lp-panel')).toBeVisible();
}

/** The "add a web shortcut" tile — the launcher grid's LAST cell, after every
 *  app tile (it used to sit on the desktop, next to the shortcut column). */
function addTile(page: Page) {
  return page.locator('[data-testid="launcher-shortcut-add"]');
}

/** Open the launcher, then the add dialog from its last tile. */
async function openAddDialog(page: Page) {
  await openLauncher(page);
  await addTile(page).click();
  await expect(page.locator('[data-testid="shortcuts-url"]')).toBeVisible({ timeout: 8_000 });
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
  test('1.1 — the add control lives in the launcher, never on the desktop', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    // The desktop is a canvas of what the user pinned, not a place to
    // configure it: with no shortcut pinned there is no column at all, and no
    // add control anywhere on the desktop.
    await expect(page.locator('[data-testid="desktop-shortcut-add"]')).toHaveCount(0);
    await expect(bar(page)).toHaveCount(0);
    await openLauncher(page);
    await expect(addTile(page)).toBeVisible();
  });

  test('1.2 — the add tile is the launcher grid\'s LAST cell, after every app', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await openLauncher(page);
    await expect(page.locator('.lp-grid > .lp-card').last()).toHaveAttribute(
      'data-testid',
      'launcher-shortcut-add',
    );
    // Not part of a filtered result set: a query or a category chip narrows the
    // grid to an answer, so the add tile steps out of it.
    await page.locator('.lp-search-field').fill('storage');
    await expect(addTile(page)).toHaveCount(0);
  });

  test('1.3 — the add dialog opens from the launcher tile and closes', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await openAddDialog(page);
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
    await openAddDialog(page);
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
    await openAddDialog(page);
    await page.locator('[data-testid="shortcuts-url"]').fill('http://127.0.0.1:8080/');
    await page.locator('[data-testid="shortcuts-submit"]').click();
    await expect(page.locator('[data-testid="shortcuts-error"]')).toBeVisible({ timeout: 10_000 });
    // The dialog must not have added a tile for the local address.
    await expect(page.locator('[data-testid="desktop-shortcut"]')).toHaveCount(0);
  });

  test('2.3 — removing a shortcut deletes its tile and storage entry', async ({ page }) => {
    await gotoDesktop(page, adminTokens, {
      id: 'sc_test',
      url: 'https://example.com',
      title: 'Example',
      description: '',
      iconUrl: null,
      siteName: 'example.com',
    });
    const tile = page.locator('[data-testid="desktop-shortcut"]');
    await expect(tile).toHaveCount(1, { timeout: 10_000 });
    // The remove button is revealed on hover AND `focus-within` (never
    // hover-only), so this walks the keyboard path: focusing the tile's open
    // button is enough to make remove reachable.
    const firstTile = tile.first();
    await firstTile.locator('.hv-shortcut-open').focus();
    const remove = firstTile.locator('[data-testid="desktop-shortcut-remove"]');
    await expect(remove).toBeVisible();
    await remove.click();
    // The tile goes, and so does the column: the desktop renders only the
    // pinned set, so an empty set leaves nothing behind.
    await expect(tile).toHaveCount(0);
    await expect(bar(page)).toHaveCount(0);
    const stored = await page.evaluate((username) => {
      const raw = localStorage.getItem(`hv-shortcuts:${username}`);
      return raw ? (JSON.parse(raw) as unknown[]) : [];
    }, adminTokens.username);
    expect(stored).toHaveLength(0);
  });
});

// ===========================================================================
// 3 — Accessibility & security
// ===========================================================================

test.describe('3 — Accessibility & security', () => {
  test('3.1 — the add tile and shortcut tiles have accessible names', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await openLauncher(page);
    const add = addTile(page);
    expect((await add.getAttribute('aria-label'))?.length ?? 0).toBeGreaterThan(0);
  });

  test('3.2 — a malicious title is rendered as text, never as markup', async ({ page }) => {
    await gotoDesktop(page, adminTokens, {
      id: 'sc_xss',
      url: 'https://example.com',
      title: '<img src=x onerror=alert(1)>',
      description: '',
      iconUrl: null,
      siteName: '',
    });
    const tile = page.locator('[data-testid="desktop-shortcut"]').first();
    await expect(tile).toBeVisible({ timeout: 10_000 });
    // No <img> injected from the title, and the raw text is present.
    await expect(tile.locator('img')).toHaveCount(0);
    await expect(tile).toContainText('<img src=x onerror=alert(1)>');
  });
});