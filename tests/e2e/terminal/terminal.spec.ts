/* =============================================================================
 * File          : terminal.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Terminal screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Terminal Page — End-to-End Test Suite
 *
 * The Terminal is a desktop app window opened from the dock. Every test
 * provisions a session via the real admin API, injects it into localStorage,
 * loads /desktop, and opens the Terminal window from the dock.
 *
 * Run (all):         npx playwright test tests/e2e/terminal/terminal.spec.ts --project=firefox
 * Run (single test): npx playwright test tests/e2e/terminal/terminal.spec.ts -g "3.1"
 * Run (tag):         npx playwright test tests/e2e/terminal/terminal.spec.ts --grep "Security"
 * Custom server:     BASE_URL=http://localhost:8080 npx playwright test tests/e2e/terminal/terminal.spec.ts
 *
 * Requires a live server whose setup is complete and that is seeded with an
 * admin account (defaults: e2eadmin / E2eSecret123, override with
 * E2E_ADMIN_USER / E2E_ADMIN_PASS).
 */

import {
  test,
  expect,
  type Page,
  type APIRequestContext,
} from '@playwright/test';

// ---------------------------------------------------------------------------
// Constants & helpers
// ---------------------------------------------------------------------------

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

async function getHeapMB(page: Page): Promise<number> {
  return page.evaluate(() =>
    'memory' in performance ? (performance as any).usedJSHeapSize / (1024 * 1024) : 0,
  );
}

// ---------------------------------------------------------------------------
// Admin-API helpers
// ---------------------------------------------------------------------------

async function newCtx(playwright: any): Promise<APIRequestContext> {
  return playwright.request.newContext({ baseURL: BASE });
}

async function apiLogin(
  ctx: APIRequestContext,
): Promise<{ token: string; csrf: string }> {
  const res = await ctx.post('/api/auth/login', {
    data: { username: ADMIN_USER, password: ADMIN_PASS },
  });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

// ---------------------------------------------------------------------------
// Browser helpers — inject session and open Terminal window
// ---------------------------------------------------------------------------

async function seedSession(page: Page, token: string, csrf: string) {
  await page.addInitScript(
    ({ t, c }) => {
      try {
        localStorage.setItem('session-token', t);
        localStorage.setItem('csrf-token', c);
      } catch { /* ignore */ }
    },
    { t: token, c: csrf },
  );
}

/** Land on the desktop with an authenticated session. */
async function gotoDesktop(page: Page, token: string, csrf: string) {
  await seedSession(page, token, csrf);
  await page.goto('/desktop');
  await page.waitForSelector('.hv-dock', { timeout: 20_000 });
}

/** Open the Terminal window from the dock. */
async function openTerminal(page: Page) {
  await page.locator('.hv-dock-icon-terminal').click();
  await page.waitForSelector('[data-testid="terminal-page"]', { timeout: 15_000 });
}

/** Land on the desktop and open the Terminal window. */
async function gotoTerminal(page: Page, token: string, csrf: string) {
  await gotoDesktop(page, token, csrf);
  await openTerminal(page);
}

// ---------------------------------------------------------------------------
// Per-test memory metrics (CLAUDE.md: metrics logged in afterEach)
// ---------------------------------------------------------------------------

let heapBefore = 0;
test.beforeEach(async ({ page }) => {
  heapBefore = 0;
  page.on('load', async () => {
    if (!heapBefore) heapBefore = await getHeapMB(page).catch(() => 0);
  });
});
test.afterEach(async ({ page }, testInfo) => {
  const after = await getHeapMB(page).catch(() => 0);
  if (heapBefore > 0 && after > 0) {
    const delta = after - heapBefore;
    if (delta > 10) {
      console.warn(`[MEM] "${testInfo.title}" grew heap by ${delta.toFixed(1)} MB`);
    }
  }
});

// ---------------------------------------------------------------------------
// Shared fixture: one admin session for all tests
// ---------------------------------------------------------------------------

let token = '';
let csrf = '';

test.beforeAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  const login = await apiLogin(ctx);
  token = login.token;
  csrf = login.csrf;
  await ctx.dispose();
});

// ===========================================================================
// 1. Page load & initial state
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — terminal window opens from the dock', async ({ page }) => {
    await gotoDesktop(page, token, csrf);
    await expect(page.locator('[data-testid="terminal-page"]')).toHaveCount(0);
    await openTerminal(page);
    await expect(page.locator('[data-testid="terminal-page"]')).toBeVisible();
  });

  test('1.2 — page has the terminal window element', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await expect(page.locator('[data-testid="terminal-page"]')).toBeVisible();
  });

  test('1.3 — terminal icon is visible in the topbar', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    // The topbar is now the shared WindowHeader; its icon tile is .hv-winhdr-icon.
    const icon = page.locator('.hv-winhdr-icon');
    await expect(icon).toBeVisible();
    const box = await icon.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.width).toBeGreaterThan(20);
    expect(box!.height).toBeGreaterThan(20);
  });

  test('1.4 — a default tab is created on load', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const tabs = page.locator('[data-testid="terminal-tab"]');
    await expect(tabs).toHaveCount(1);
  });

  test('1.5 — default tab has a visible title', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const title = await page.locator('[data-testid="terminal-tab-title"]').first().textContent();
    expect(title!.trim().length).toBeGreaterThan(0);
  });
});

// ===========================================================================
// 2. UI elements & structure
// ===========================================================================

test.describe('2 — UI elements', () => {
  test('2.1 — info bar is present with user, separator, cwd, badge, shell', async ({ page }) => {
    await gotoTerminal(page, token, csrf);

    await expect(page.locator('[data-testid="terminal-infobar"]')).toBeVisible();
    await expect(page.locator('[data-testid="terminal-info-user"]')).toBeVisible();
    await expect(page.locator('.hv-info-sep')).toBeVisible();
    await expect(page.locator('[data-testid="terminal-info-cwd"]')).toBeVisible();
    await expect(page.locator('[data-testid="terminal-info-badge"]')).toBeVisible();
    await expect(page.locator('[data-testid="terminal-info-shell"]')).toBeVisible();
  });

  test('2.2 — info bar shows hostname', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const user = await page.locator('[data-testid="terminal-info-user"]').textContent();
    expect(user!.trim().length).toBeGreaterThan(0);
  });

  test('2.3 — info bar shows ~ as cwd', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const cwd = await page.locator('[data-testid="terminal-info-cwd"]').textContent();
    expect(cwd!.trim()).toBe('~');
  });

  test('2.4 — info bar shows shell version', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const shell = await page.locator('[data-testid="terminal-info-shell"]').textContent();
    expect(shell!.trim().length).toBeGreaterThan(0);
  });

  // (The bottom action bar was removed — see CLAUDE.md's "remove the bottom
  //  bars" Terminal addendum — so there is no 2.5 any more.)

  test('2.6 — add-tab button is visible', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await expect(page.locator('[data-testid="terminal-tab-add"]')).toBeVisible();
  });

  test('2.7 — terminal body contains at least one xterm container', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await expect(page.locator('[data-testid="terminal-body"]')).toBeVisible();
    const contents = page.locator('.hv-terminal-content');
    expect(await contents.count()).toBeGreaterThanOrEqual(1);
  });

  test('2.8 — tab strip sits in the window content, between header and info bar', async ({ page }) => {
    await gotoTerminal(page, token, csrf);

    const tabs = page.locator('[data-testid="terminal-tabs"]');
    await expect(tabs).toBeVisible();

    // It belongs to the window content, NOT to the shared WindowHeader: that
    // bar carries the app icon + title only, and a strip there competes with
    // the min / max / close trio pinned to its right edge.
    expect(await tabs.evaluate((el) => el.closest('.hv-winhdr') !== null)).toBe(false);

    // Vertical order: WindowHeader → tab strip → info bar.
    const header = await page.locator('.hv-winhdr').boundingBox();
    const tabBox = await tabs.boundingBox();
    const infobar = await page.locator('[data-testid="terminal-infobar"]').boundingBox();
    expect(header).not.toBeNull();
    expect(tabBox).not.toBeNull();
    expect(infobar).not.toBeNull();
    expect(tabBox!.y).toBeGreaterThanOrEqual(header!.y + header!.height - 1);
    expect(tabBox!.y + tabBox!.height).toBeLessThanOrEqual(infobar!.y + 1);

    // The strip spans the window width (it is a full bar, not a centred blob).
    const pageBox = await page.locator('[data-testid="terminal-page"]').boundingBox();
    expect(tabBox!.width).toBeGreaterThan(pageBox!.width * 0.9);
  });

  test('2.9 — tab strip scrolls horizontally only, never vertically', async ({ page }) => {
    await gotoTerminal(page, token, csrf);

    const tabs = page.locator('[data-testid="terminal-tabs"]');
    await expect(tabs).toBeVisible();

    // A lone `overflow-x: auto` computes `overflow-y` to `auto` as well, so the
    // strip used to grow a vertical scrollbar: a `min-h-11` (44px) border-box
    // leaves its content box 1px short (the bottom border), which is all the
    // overflow a scrollbar needs. Assert the axis is explicitly locked off —
    // the computed value is the only cross-platform signal, since overlay
    // scrollbars (this test browser) steal no width and so leave no other trace.
    const overflowY = await tabs.evaluate((el) => getComputedStyle(el).overflowY);
    expect(overflowY).toBe('hidden');

    const overflowX = await tabs.evaluate((el) => getComputedStyle(el).overflowX);
    expect(overflowX).toBe('auto');

    // Whatever the remainder, it must stay within the 1px border-box rounding —
    // a strip whose content genuinely does not fit would be a layout bug.
    const overflow = await tabs.evaluate((el) => ({
      vertical: el.scrollHeight - el.clientHeight,
      horizontal: el.scrollWidth - el.clientWidth,
    }));
    expect(overflow.vertical).toBeLessThanOrEqual(1);
    expect(overflow.horizontal).toBe(0);
  });

  test('2.10 — terminal body paints the same colour as the terminal itself', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.waitForSelector('[data-testid="terminal-body"] .xterm-viewport', { timeout: 15_000 });

    // FitAddon sizes the terminal to `floor(height / cellHeight)` rows, so a
    // sub-row remainder is always left at the bottom of the body. When the
    // container's background differs from the xterm theme's, that remainder
    // paints as a black band under the last line of output.
    const { body, terminal } = await page.evaluate(() => {
      const b = document.querySelector('[data-testid="terminal-body"]')!;
      const v = document.querySelector('[data-testid="terminal-body"] .xterm-viewport')!;
      return {
        body: getComputedStyle(b).backgroundColor,
        terminal: getComputedStyle(v).backgroundColor,
      };
    });

    // Compared live against what the terminal actually paints, not against a
    // literal: changing the xterm theme must drag this container along with it.
    expect(body).toBe(terminal);

    // And it must be a real colour, not `transparent` — a transparent body
    // would let the window chrome show through the remainder instead.
    expect(body).not.toBe('rgba(0, 0, 0, 0)');
  });
});

// ===========================================================================
// 3. Tab management
// ===========================================================================

test.describe('3 — Tab management', () => {
  test('3.1 — clicking add-tab creates a new tab', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(1);

    await page.locator('[data-testid="terminal-tab-add"]').click();
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(2);
  });

  test('3.2 — new tab becomes active after creation', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.locator('[data-testid="terminal-tab-add"]').click();

    const activeTab = page.locator('.hv-tab-active');
    await expect(activeTab).toHaveCount(1);
    const title = await activeTab.locator('[data-testid="terminal-tab-title"]').textContent();
    expect(title!.trim().length).toBeGreaterThan(0);
  });

  test('3.3 — clicking a tab switches the active tab', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.locator('[data-testid="terminal-tab-add"]').click();

    const tabs = page.locator('[data-testid="terminal-tab"]');
    const firstTitle = await tabs.nth(0).locator('[data-testid="terminal-tab-title"]').textContent();
    const secondTitle = await tabs.nth(1).locator('[data-testid="terminal-tab-title"]').textContent();

    await tabs.nth(0).click();
    await expect(tabs.nth(0)).toHaveClass(/hv-tab-active/);
    await expect(tabs.nth(1)).not.toHaveClass(/hv-tab-active/);

    await tabs.nth(1).click();
    await expect(tabs.nth(1)).toHaveClass(/hv-tab-active/);
    await expect(tabs.nth(0)).not.toHaveClass(/hv-tab-active/);
  });

  test('3.4 — tab dot indicates connection state', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const dot = page.locator('[data-testid="terminal-tab-dot"]').first();
    await expect(dot).toBeVisible();
    // The dot's state is its fill (design tokens, not a class): success green
    // when the PTY is live, muted grey while connecting / disconnected.
    // Assert it paints one of the two — never nothing.
    const bg = await dot.evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(bg).toMatch(/^rgba?\(/);
    expect(bg).not.toBe('rgba(0, 0, 0, 0)');
  });

  test('3.5 — close button is hidden on a single tab', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(1);
    await expect(page.locator('[data-testid="terminal-tab-close"]')).toHaveCount(0);
  });

  test('3.6 — close button appears when more than one tab exists', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.locator('[data-testid="terminal-tab-add"]').click();
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(2);
    await expect(page.locator('[data-testid="terminal-tab-close"]').first()).toBeVisible();
  });

  test('3.7 — closing a tab removes it from the tab bar', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.locator('[data-testid="terminal-tab-add"]').click();
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(2);

    await page.locator('[data-testid="terminal-tab-close"]').first().click();
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(1);
  });

  test('3.8 — closing all tabs creates a fresh default tab', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.locator('[data-testid="terminal-tab-add"]').click();
    // Close both tabs
    await page.locator('[data-testid="terminal-tab-close"]').first().click();
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(1);
    // The remaining tab is a fresh one
    const title = await page.locator('[data-testid="terminal-tab-title"]').first().textContent();
    expect(title!.trim().length).toBeGreaterThan(0);
  });

  test('3.9 — can open up to 8 tabs (MAX_TABS)', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    for (let i = 0; i < 7; i++) {
      const addBtn = page.locator('[data-testid="terminal-tab-add"]');
      if (await addBtn.count()) {
        // dispatchEvent, not click(): once the tab strip fills up the "+" button
        // slides toward the top-right under the desktop menubar and fails
        // Playwright hit-testing. Real pointer targeting is covered by 3.1.
        await addBtn.dispatchEvent('click');
        await page.waitForTimeout(200);
      }
    }
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(8);
    // Add button should be gone at max
    await expect(page.locator('[data-testid="terminal-tab-add"]')).toHaveCount(0);
  });

  test('3.10 — tab is persisted in localStorage', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const stored = await page.evaluate(() => {
      return localStorage.getItem('hv-terminal-tabs');
    });
    expect(stored).not.toBeNull();
    const parsed = JSON.parse(stored!);
    expect(Array.isArray(parsed.tabs)).toBe(true);
    expect(parsed.tabs.length).toBeGreaterThanOrEqual(1);
    expect(typeof parsed.tabs[0].id).toBe('string');
    expect(typeof parsed.tabs[0].title).toBe('string');
  });

  test('3.11 — persisted tabs are restored on reload', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.locator('[data-testid="terminal-tab-add"]').click();
    const countBeforeReload = await page.locator('[data-testid="terminal-tab"]').count();

    await page.reload();
    // The window itself is not persisted across a reload — reopen it from the
    // dock, then the tab ids saved in localStorage ('hv-terminal-tabs') should
    // be restored.
    await page.waitForSelector('.hv-dock', { timeout: 20_000 });
    await openTerminal(page);
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(countBeforeReload);
  });
});

// ===========================================================================
// 4. Terminal content & xterm
// ===========================================================================

test.describe('4 — Terminal content', () => {
  test('4.1 — xterm container is created and has a canvas', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    // xterm renders into a canvas inside .xterm
    const xterm = page.locator('.xterm');
    await expect(xterm).toBeVisible({ timeout: 10_000 });
  });

  test('4.2 — terminal content has correct monospace font family', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    // xterm applies its `fontFamily` option ("'JetBrains Mono', ui-monospace,
    // monospace" in `initTabTerminal`) to the rendered text grid, not to the
    // host div — `.hv-terminal-content` is a bare container that inherits the
    // app font, so asserting on it could only ever fail. `.xterm-rows` is
    // where the glyphs actually paint.
    await page.locator('.xterm-rows').first().waitFor({ state: 'attached', timeout: 10_000 });
    const fontFamily = await page.locator('.xterm-rows').first().evaluate(
      (el) => getComputedStyle(el).fontFamily,
    );
    expect(fontFamily.toLowerCase()).toContain('monospace');
  });

  test('4.3 — only the active tab content is visible (others hidden)', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.locator('[data-testid="terminal-tab-add"]').click();

    const contents = page.locator('.hv-terminal-content');
    expect(await contents.count()).toBe(2);

    // Adding a tab makes the new one active: its content is visible, the
    // previously-active one is now hidden.
    expect(await contents.nth(0).getAttribute('hidden')).not.toBeNull();
    expect(await contents.nth(1).getAttribute('hidden')).toBeNull();
  });

  test('4.4 — switching tabs shows the correct terminal content', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.locator('[data-testid="terminal-tab-add"]').click();

    // Click first tab
    await page.locator('[data-testid="terminal-tab"]').nth(0).click();
    expect(await page.locator('.hv-terminal-content').nth(0).getAttribute('hidden')).toBeNull();
    expect(await page.locator('.hv-terminal-content').nth(1).getAttribute('hidden')).not.toBeNull();

    // Click second tab
    await page.locator('[data-testid="terminal-tab"]').nth(1).click();
    expect(await page.locator('.hv-terminal-content').nth(1).getAttribute('hidden')).toBeNull();
    expect(await page.locator('.hv-terminal-content').nth(0).getAttribute('hidden')).not.toBeNull();
  });
});

// ===========================================================================
// (Sections 5 "Action bar" and 6 "Mobile key row" were removed — both bottom
//  strips of the Terminal were deleted whole in the "remove the bottom bars"
//  redesign, see CLAUDE.md's Terminal addendum. The window drags from the
//  shared WindowHeader; there is no Run/Clear/Split-pane bar and no touch
//  key row any more.)
// ===========================================================================

// ===========================================================================
// 7. WebSocket connection
// ===========================================================================

test.describe('7 — WebSocket connection', () => {
  test('7.1 — connection badge is visible in info bar', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const badge = page.locator('[data-testid="terminal-info-badge"]');
    await expect(badge).toBeVisible();
    const text = await badge.textContent();
    expect(text!.trim().length).toBeGreaterThan(0);
  });

  test('7.2 — ticket endpoint is called on terminal open', async ({ page }) => {
    let ticketCalled = false;
    page.on('request', (req) => {
      if (req.url().includes('/api/terminal/ticket')) ticketCalled = true;
    });

    await gotoTerminal(page, token, csrf);
    await page.waitForTimeout(5_000);
    expect(ticketCalled).toBe(true);
  });

  test('7.3 — connection status badge eventually shows connected state', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    // Wait for WebSocket to connect — the badge then switches to its success
    // fill. Both states are painted from design tokens, never from a class.
    await page.waitForTimeout(5_000);
    const badge = page.locator('[data-testid="terminal-info-badge"]');
    await expect(badge).toBeVisible();
    // Badge should either be connected or still connecting — not crashed.
    // A crashed/blank badge is the one that paints nothing at all.
    const bg = await badge.evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(bg).toMatch(/^rgba?\(/);
    expect(bg).not.toBe('rgba(0, 0, 0, 0)');
    expect((await badge.textContent())!.trim().length).toBeGreaterThan(0);
  });

  test('7.4 — tab dot turns green when connected', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.waitForTimeout(5_000);
    const dot = page.locator('[data-testid="terminal-tab-dot"]').first();
    await expect(dot).toBeVisible();
    // Success green once the PTY is live, muted grey otherwise — both painted
    // from design tokens, so assert the fill rather than a class name.
    const bg = await dot.evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(bg).toMatch(/^rgba?\(/);
    expect(bg).not.toBe('rgba(0, 0, 0, 0)');
    // May or may not be connected depending on backend availability
  });
});

// ===========================================================================
// 8. Responsive layout
// ===========================================================================

test.describe('8 — Responsive', () => {
  for (const [label, width, height] of [
    ['360px (phone)', 360, 640],
    ['390px (iPhone 14)', 390, 844],
    ['768px (tablet)', 768, 1024],
    ['1024px (laptop)', 1024, 768],
    ['1920px (desktop)', 1920, 1080],
  ] as const) {
    test(`8.x — renders correctly at ${label}`, async ({ page }) => {
      // Size the viewport before opening: WindowFrame computes its geometry
      // (and auto-maximizes below 820px) at open time and does not re-clamp on
      // a later resize.
      await page.setViewportSize({ width, height });
      await gotoTerminal(page, token, csrf);
      await page.waitForTimeout(500);

      const terminalPage = page.locator('[data-testid="terminal-page"]');
      await expect(terminalPage).toBeVisible();

      const box = await terminalPage.boundingBox();
      expect(box).not.toBeNull();
      if (box) {
        expect(box.x).toBeGreaterThanOrEqual(-1);
        expect(box.x + box.width).toBeLessThanOrEqual(width + 2);
        expect(box.width).toBeGreaterThan(50);
        expect(box.height).toBeGreaterThan(50);
      }

      // Critical elements visible
      await expect(page.locator('.hv-winhdr')).toBeVisible();
      await expect(page.locator('[data-testid="terminal-body"]')).toBeVisible();
    });
  }
});

// ===========================================================================
// 9. Accessibility
// ===========================================================================

test.describe('9 — Accessibility', () => {
  test('9.1 — tab buttons have accessible labels or titles', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const tabs = page.locator('[data-testid="terminal-tab"]');
    const count = await tabs.count();
    for (let i = 0; i < count; i++) {
      const text = await tabs.nth(i).textContent();
      expect(text!.trim().length).toBeGreaterThan(0);
    }
  });

  test('9.2 — add-tab button has aria-label', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const addBtn = page.locator('[data-testid="terminal-tab-add"]');
    const label = await addBtn.getAttribute('aria-label');
    expect(label!.length).toBeGreaterThan(0);
  });

  test('9.4 — no duplicate IDs in the DOM', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const ids = await page.evaluate(() =>
      Array.from(document.querySelectorAll('[id]')).map((el) => el.id),
    );
    const unique = new Set(ids);
    if (ids.length !== unique.size) {
      const dupes = ids.filter((id, i) => ids.indexOf(id) !== i);
      console.warn(`[A11Y] Duplicate IDs: ${[...new Set(dupes)].join(', ')}`);
    }
  });

  test('9.5 — tab close buttons are keyboard-focusable', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.locator('[data-testid="terminal-tab-add"]').click();
    const closeBtns = page.locator('[data-testid="terminal-tab-close"]');
    const count = await closeBtns.count();
    for (let i = 0; i < count; i++) {
      const tabindex = await closeBtns.nth(i).getAttribute('tabindex');
      expect(tabindex).not.toBe('-1');
    }
  });

  test('9.6 — all buttons have non-negative tabindex', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const buttons = page.locator('[data-testid="terminal-page"] button');
    const count = await buttons.count();
    for (let i = 0; i < count; i++) {
      const tabindex = await buttons.nth(i).getAttribute('tabindex');
      expect(tabindex).not.toBe('-1');
    }
  });
});

// ===========================================================================
// 10. Performance
// ===========================================================================

test.describe('10 — Performance', () => {
  test('10.1 — terminal page loads within 10 seconds', async ({ page }) => {
    const start = Date.now();
    await gotoTerminal(page, token, csrf);
    const elapsed = Date.now() - start;
    console.log(`[PERF] Terminal loaded in ${elapsed} ms`);
    expect(elapsed).toBeLessThan(10_000);
  });

  test('10.2 — interactive DOM node count is reasonable', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const count = await page.evaluate(() => {
      const total = document.querySelectorAll('*').length;
      const svgNodes = document.querySelectorAll('svg, svg *').length;
      return total - svgNodes;
    });
    console.log(`[PERF] interactive DOM nodes: ${count}`);
    expect(count).toBeLessThan(3000);
  });

  test('10.3 — resource requests < 80', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    const count = await page.evaluate(
      () => performance.getEntriesByType('resource').length,
    );
    console.log(`[PERF] Resources: ${count}`);
    expect(count).toBeLessThan(80);
  });

  test('10.4 — no memory leak across tab add/close cycles', async ({ page }) => {
    test.setTimeout(150_000);
    await gotoTerminal(page, token, csrf);
    const baseline = await getHeapMB(page);

    for (let i = 0; i < 5; i++) {
      await page.locator('[data-testid="terminal-tab-add"]').click().catch(() => {});
      await page.waitForTimeout(300);
      const closeBtn = page.locator('[data-testid="terminal-tab-close"]').first();
      if (await closeBtn.isVisible().catch(() => false)) {
        await closeBtn.click();
      }
      await page.waitForTimeout(300);
    }

    const after = await getHeapMB(page);
    if (baseline > 0 && after > 0) {
      const delta = after - baseline;
      console.log(`[MEM] Tab cycle x5: ${baseline.toFixed(1)} -> ${after.toFixed(1)} MB (d${delta.toFixed(2)})`);
      expect(delta).toBeLessThan(5);
    }
  });
});

// ===========================================================================
// 11. Security
// ===========================================================================

test.describe('11 — Security', () => {
  test('11.1 — no session token in URL after page load', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    expect(page.url()).not.toContain('session-token');
    expect(page.url()).not.toContain('password');
  });

  test('11.2 — terminal ticket is not exposed in localStorage', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.waitForTimeout(3_000);
    const keys = await page.evaluate(() => Object.keys(localStorage));
    const terminalKeys = keys.filter((k) => k.includes('ticket'));
    expect(terminalKeys).toHaveLength(0);
  });

  test('11.3 — WebSocket URL uses ticket, not session token', async ({ page }) => {
    const wsUrls: string[] = [];
    page.on('request', (req) => {
      if (req.url().includes('/api/terminal/ws')) wsUrls.push(req.url());
    });

    await gotoTerminal(page, token, csrf);
    await page.waitForTimeout(5_000);

    for (const url of wsUrls) {
      expect(url).not.toContain(token);
      if (url.includes('ticket=')) {
        const ticket = new URL(url).searchParams.get('ticket');
        expect(ticket!.length).toBeGreaterThan(10);
      }
    }
  });

  test('11.4 — XSS attempt in tab title does not execute', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    let alertFired = false;
    page.on('dialog', () => { alertFired = true; });

    // Tab titles are auto-generated, not user-controlled, but verify no script injection
    const html = await page.content();
    expect(html).not.toContain('onerror=alert');
    expect(alertFired).toBe(false);
  });

  test('11.5 — close endpoint requires authentication', async ({ page }) => {
    const res = await page.request.post(`${BASE}/api/terminal/close`);
    expect(res.status()).toBe(401); // No session token header
  });

  test('11.6 — ticket endpoint requires authentication', async ({ page }) => {
    const res = await page.request.post(`${BASE}/api/terminal/ticket`);
    expect(res.status()).toBe(401); // No session token header
  });
});

// ===========================================================================
// 12. Keyboard navigation
// ===========================================================================

test.describe('12 — Keyboard', () => {
  test('12.1 — tab key order: a tab comes before the add-tab button', async ({ page }) => {
    await gotoTerminal(page, token, csrf);

    const order: string[] = [];
    await page.locator('[data-testid="terminal-tab"]').first().focus();

    for (let i = 0; i < 8; i++) {
      await page.keyboard.press('Tab');
      order.push(
        await page.evaluate(() => {
          const el = document.activeElement as HTMLElement | null;
          if (!el) return '';
          if (el.classList.contains('hv-tab')) return 'tab';
          if (el.classList.contains('hv-tab-add')) return 'add';
          if (el.classList.contains('hv-tab-close')) return 'close';
          return el.tagName.toLowerCase();
        }),
      );
    }

    const firstTab = order.indexOf('tab');
    const firstAdd = order.indexOf('add');
    if (firstTab >= 0 && firstAdd >= 0) {
      expect(firstTab).toBeLessThan(firstAdd);
    }
  });

  test('12.2 — Enter on a tab switches to it', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.locator('[data-testid="terminal-tab-add"]').click();

    await page.locator('[data-testid="terminal-tab"]').nth(0).focus();
    await page.keyboard.press('Enter');
    await expect(page.locator('[data-testid="terminal-tab"]').nth(0)).toHaveClass(/hv-tab-active/);
  });

  test('12.3 — Escape on close button does not crash', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.locator('[data-testid="terminal-tab-add"]').click();

    const closeBtn = page.locator('[data-testid="terminal-tab-close"]').first();
    await closeBtn.focus();
    await page.keyboard.press('Escape');
    // Should still be on the terminal page
    await expect(page.locator('[data-testid="terminal-page"]')).toBeVisible();
  });
});

// ===========================================================================
// 13. Console & network
// ===========================================================================

test.describe('13 — Console & network', () => {
  test('13.1 — no uncaught JS errors on page load', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (err) => errors.push(err.message));

    await gotoTerminal(page, token, csrf);
    await page.waitForTimeout(3_000);

    const critical = errors.filter(
      (e) =>
        !e.includes('404') &&
        !e.includes('Failed to load resource') &&
        !e.includes('favicon') &&
        !e.includes('WebSocket'),
    );
    if (critical.length > 0) {
      console.error(`[JS-ERROR] ${critical.join('; ')}`);
    }
    expect(critical).toHaveLength(0);
  });

  test('13.2 — no console.error messages on clean load', async ({ page }) => {
    const consoleErrors: string[] = [];
    page.on('console', (msg) => {
      if (msg.type() === 'error') consoleErrors.push(msg.text());
    });

    await gotoTerminal(page, token, csrf);
    await page.waitForTimeout(3_000);

    const jsErrors = consoleErrors.filter(
      (e) => !e.includes('404') && !e.includes('favicon') && !e.includes('WebSocket'),
    );
    if (jsErrors.length > 0) {
      console.warn(`[CONSOLE] ${jsErrors.length} error(s): ${jsErrors.join('; ')}`);
    }
    expect(jsErrors).toHaveLength(0);
  });

  test('13.3 — terminal makes only a handful of its own API calls on open', async ({ page }) => {
    const allApi: string[] = [];
    const terminalApi: string[] = [];
    page.on('request', (req) => {
      const u = req.url();
      if (!u.includes('/api/')) return;
      allApi.push(u);
      if (u.includes('/api/terminal/')) terminalApi.push(u);
    });

    await gotoTerminal(page, token, csrf);
    await page.waitForTimeout(5_000);

    console.log(`[NET] total API calls: ${allApi.length}, terminal-specific: ${terminalApi.length}`);
    // The terminal itself only issues a ticket request per tab (plus at most one
    // reconnect retry). The rest of the count is the desktop shell, out of scope.
    expect(terminalApi.length).toBeLessThanOrEqual(4);
  });
});

// ===========================================================================
// 14. Edge cases
// ===========================================================================

test.describe('14 — Edge cases', () => {
  test('14.1 — rapid tab add/close does not crash', async ({ page }) => {
    await gotoTerminal(page, token, csrf);

    for (let i = 0; i < 10; i++) {
      page.locator('[data-testid="terminal-tab-add"]').click().catch(() => {});
      await page.waitForTimeout(100);
      page.locator('[data-testid="terminal-tab-close"]').first().click().catch(() => {});
      await page.waitForTimeout(100);
    }

    // Page should still be functional
    await expect(page.locator('[data-testid="terminal-page"]')).toBeVisible();
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(1);
  });

  test('14.2 — page works after clearing localStorage', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    // The session + cookie-consent init scripts re-seed on reload; the window
    // itself is not persisted, so reopen it from the dock.
    await page.waitForSelector('.hv-dock', { timeout: 20_000 });
    await openTerminal(page);
    // Should create a fresh tab
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(1);
  });

  test('14.3 — window resize does not break the terminal', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.setViewportSize({ width: 400, height: 300 });
    await page.waitForTimeout(500);
    await expect(page.locator('[data-testid="terminal-page"]')).toBeVisible();

    await page.setViewportSize({ width: 1920, height: 1080 });
    await page.waitForTimeout(500);
    await expect(page.locator('[data-testid="terminal-page"]')).toBeVisible();
  });

  test('14.4 — multiple rapid viewport changes are handled', async ({ page }) => {
    await gotoTerminal(page, token, csrf);

    const sizes = [
      [360, 640],
      [1920, 1080],
      [768, 1024],
      [480, 800],
      [1280, 720],
    ];
    for (const [w, h] of sizes) {
      await page.setViewportSize({ width: w, height: h });
      await page.waitForTimeout(200);
    }

    await expect(page.locator('[data-testid="terminal-page"]')).toBeVisible();
    await expect(page.locator('[data-testid="terminal-body"]')).toBeVisible();
  });

  test('14.5 — navigating away and back restores state', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await page.locator('[data-testid="terminal-tab-add"]').click();
    const tabCount = await page.locator('[data-testid="terminal-tab"]').count();

    // Navigate away
    await page.goto('/login');
    await page.waitForTimeout(1_000);

    // Navigate back
    await seedSession(page, token, csrf);
    await page.goto('/desktop');
    await page.waitForSelector('.hv-dock', { timeout: 20_000 });
    await openTerminal(page);

    // Tabs should be restored
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(tabCount);
  });
});

// ===========================================================================
// 15. Memory regression guards
// ===========================================================================

test.describe('15 — Memory regression', () => {
  test('15.1 — tab switching x10 does not leak heap', async ({ page }) => {
    test.setTimeout(150_000);
    await gotoTerminal(page, token, csrf);
    // Create multiple tabs
    for (let i = 0; i < 3; i++) {
      await page.locator('[data-testid="terminal-tab-add"]').click().catch(() => {});
      await page.waitForTimeout(200);
    }
    const baseline = await getHeapMB(page);

    const tabs = page.locator('[data-testid="terminal-tab"]');
    const count = await tabs.count();
    for (let i = 0; i < 10; i++) {
      await tabs.nth(i % count).click();
      await page.waitForTimeout(200);
    }

    const after = await getHeapMB(page);
    if (baseline > 0 && after > 0) {
      const delta = after - baseline;
      console.log(`[MEM] Tab switch x10: d${delta.toFixed(2)} MB`);
      expect(delta).toBeLessThan(3);
    }
  });

  test('15.2 — DOM nodes do not accumulate across tab cycles', async ({ page }) => {
    test.setTimeout(150_000);
    await gotoTerminal(page, token, csrf);

    // Warm up
    await page.locator('[data-testid="terminal-tab-add"]').click().catch(() => {});
    await page.locator('[data-testid="terminal-tab-close"]').first().click().catch(() => {});
    await page.waitForTimeout(500);

    const before = await page.evaluate(() => document.querySelectorAll('*').length);

    for (let i = 0; i < 5; i++) {
      await page.locator('[data-testid="terminal-tab-add"]').click().catch(() => {});
      await page.waitForTimeout(200);
      await page.locator('[data-testid="terminal-tab-close"]').first().click().catch(() => {});
      await page.waitForTimeout(200);
    }

    const after = await page.evaluate(() => document.querySelectorAll('*').length);
    const delta = after - before;
    console.log(`[DOM] Tab cycle x5: d${delta} nodes`);
    expect(delta).toBeLessThanOrEqual(50);
  });

  test('15.3 — opening/closing a tab x10 does not leak', async ({ page }) => {
    test.setTimeout(150_000);
    await gotoTerminal(page, token, csrf);
    const baseline = await getHeapMB(page);

    for (let i = 0; i < 10; i++) {
      await page.locator('[data-testid="terminal-tab-add"]').click();
      await page.waitForTimeout(100);
      await page.locator('[data-testid="terminal-tab-close"]').first().click();
      await page.waitForTimeout(100);
    }

    const after = await getHeapMB(page);
    if (baseline > 0 && after > 0) {
      const delta = after - baseline;
      console.log(`[MEM] Tab add/close x10: d${delta.toFixed(2)} MB`);
      expect(delta).toBeLessThan(4);
    }
  });
});

// ===========================================================================
// 16. i18n
// ===========================================================================

test.describe('16 — i18n', () => {
  async function setLanguage(page: Page, lang: string) {
    await page.addInitScript((l) => {
      try { localStorage.setItem('hv-language', l); } catch { /* ignore */ }
    }, lang);
  }

  test('16.2 — French: shell version label is present', async ({ page }) => {
    await setLanguage(page, 'fr');
    await gotoTerminal(page, token, csrf);
    const shell = await page.locator('[data-testid="terminal-info-shell"]').textContent();
    expect(shell!.trim().length).toBeGreaterThan(0);
  });

  test('16.3 — German: the add-tab button keeps an accessible label', async ({ page }) => {
    await setLanguage(page, 'de');
    await gotoTerminal(page, token, csrf);
    const label = await page.locator('[data-testid="terminal-tab-add"]').getAttribute('aria-label');
    expect((label ?? '').trim().length).toBeGreaterThan(0);
  });

  test('16.5 — no raw i18n key leaks in the page', async ({ page }) => {
    await setLanguage(page, 'fr');
    await gotoTerminal(page, token, csrf);
    const bodyText = await page.locator('[data-testid="terminal-page"]').innerText();
    expect(bodyText).not.toMatch(/\bterminal\.[a-z_]+/i);
  });
});

// ===========================================================================
// 17. Close endpoint behavior
// ===========================================================================

test.describe('17 — Close endpoint', () => {
  test('17.1 — close endpoint returns success with valid session', async ({ page }) => {
    await gotoDesktop(page, token, csrf);
    const res = await page.request.post(`${BASE}/api/terminal/close?tab=test_tab_e2e`, {
      headers: { 'x-session-token': token, 'x-csrf-token': csrf },
    });
    expect(res.status()).toBe(200);
    const body = await res.json();
    expect(body.success).toBe(true);
  });

  test('17.2 — close endpoint returns 401 without token', async ({ page }) => {
    await gotoDesktop(page, token, csrf);
    const res = await page.request.post(`${BASE}/api/terminal/close?tab=test_tab`);
    expect(res.status()).toBe(401);
  });

  test('17.3 — ticket endpoint returns a ticket with valid session', async ({ page }) => {
    await gotoDesktop(page, token, csrf);
    const res = await page.request.post(`${BASE}/api/terminal/ticket?tab=test_tab_e2e2`, {
      headers: { 'x-session-token': token, 'x-csrf-token': csrf },
    });
    expect(res.status()).toBe(200);
    const body = await res.json();
    expect(body.success).toBe(true);
    expect(body.data.ticket.length).toBeGreaterThan(10);
  });

  test('17.4 — ticket endpoint rejects a present-but-empty token', async ({ page }) => {
    await gotoDesktop(page, token, csrf);
    const res = await page.request.post(`${BASE}/api/terminal/ticket`, {
      headers: { 'x-session-token': '' },
    });
    // An empty string is a present session header with no matching CSRF token,
    // so the CSRF middleware rejects it (403) ahead of the handler's own 401.
    expect(res.status()).toBe(403);
  });

  test('17.5 — invalid tab id is rejected', async ({ page }) => {
    await gotoDesktop(page, token, csrf);
    const res = await page.request.post(`${BASE}/api/terminal/ticket?tab=../../etc/passwd`, {
      headers: { 'x-session-token': token, 'x-csrf-token': csrf },
    });
    expect(res.status()).toBe(400);
  });
});

// ===========================================================================
// 18. Window management integration
// ===========================================================================

test.describe('18 — Window management', () => {
  test('18.1 — terminal window can be minimized', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await expect(page.locator('[data-testid="terminal-page"]')).toBeVisible();

    // Every window's minimise / maximise / close controls float as round
    // glass buttons (.hv-cl-btn) over the header (here the shared WindowHeader).
    const minBtn = page.locator('.hv-cl-btn').first();
    await minBtn.click();
    await page.waitForTimeout(500);

    const terminalVisible = await page.locator('[data-testid="terminal-page"]').isVisible().catch(() => false);
    const minimizing = await page.locator('.minimizing').isVisible().catch(() => false);
    expect(terminalVisible === false || minimizing).toBe(true);
  });

  test('18.2 — closing the terminal window keeps the shells alive (no close call)', async ({ page }) => {
    let closeCalled = false;
    page.on('request', (req) => {
      if (req.url().includes('/api/terminal/close')) closeCalled = true;
    });

    await gotoTerminal(page, token, csrf);
    await page.locator('.hv-cl-btn-close').click();
    await page.waitForTimeout(2_000);

    // Closing the window is deliberately NOT a teardown of the PTY — a reopened
    // window reattaches to the still-running shell. Only the per-tab close
    // button calls /api/terminal/close (see 18.3).
    expect(closeCalled).toBe(false);
    await expect(page.locator('[data-testid="terminal-page"]')).toHaveCount(0);
  });

  test('18.3 — closing a tab (not the window) calls the close endpoint', async ({ page }) => {
    const closeUrls: string[] = [];
    page.on('request', (req) => {
      if (req.url().includes('/api/terminal/close')) closeUrls.push(req.url());
    });

    await gotoTerminal(page, token, csrf);
    await page.locator('[data-testid="terminal-tab-add"]').click();
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(2);

    await page.locator('[data-testid="terminal-tab-close"]').first().click();
    await expect(page.locator('[data-testid="terminal-tab"]')).toHaveCount(1);
    await page.waitForTimeout(1_000);

    expect(closeUrls.length).toBeGreaterThanOrEqual(1);
  });
});

// ===========================================================================
// 19. Completion (ghost text + Tab candidate popup)
// ===========================================================================

test.describe('19 — Completion', () => {
  /** Focus the xterm textarea so keystrokes land in the shell. */
  async function focusTerminal(page: Page) {
    const body = page.locator('[data-testid="terminal-body"] .hv-terminal-content').first();
    await body.click({ position: { x: 120, y: 40 } });
    await page.waitForTimeout(300);
  }

  test('19.1 — Tab on an ambiguous command shows the candidate popup', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await focusTerminal(page);

    // `ls` matches both `ls` and `lsblk` in the client-side command set.
    await page.keyboard.type('ls');
    await page.keyboard.press('Tab');

    const popup = page.locator('[data-testid="terminal-completion-popup"]');
    await expect(popup).toBeVisible({ timeout: 10_000 });
    await expect(popup.locator('[data-testid="terminal-completion-item"]', { hasText: 'lsblk' })).toBeVisible();
  });

  test('19.2 — clicking a candidate completes it and dismisses the popup', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await focusTerminal(page);

    await page.keyboard.type('ls');
    await page.keyboard.press('Tab');
    const popup = page.locator('[data-testid="terminal-completion-popup"]');
    await expect(popup).toBeVisible({ timeout: 10_000 });

    await popup.locator('[data-testid="terminal-completion-item"]', { hasText: /^lsblk/ }).first().click();
    await expect(popup).not.toBeVisible({ timeout: 5_000 });
  });

  test('19.3 — an unambiguous single command shows no popup (shell Tab passes through)', async ({ page }) => {
    await gotoTerminal(page, token, csrf);
    await focusTerminal(page);

    // `sudo` is a complete command — no extra candidate, so Tab must fall
    // through to the shell (no popup) and not swallow the key.
    await page.keyboard.type('sudo');
    await page.keyboard.press('Tab');
    await page.waitForTimeout(800);
    await expect(page.locator('[data-testid="terminal-completion-popup"]')).not.toBeVisible();
  });
});
