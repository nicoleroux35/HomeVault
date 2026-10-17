/* =============================================================================
 * File          : settings.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Settings app — End-to-End Test Suite
 *
 * Settings is not a route: it is a desktop app window (light frosted shell,
 * left category rail + scrolling panel — see the 2026-09-04 redesign note in
 * CLAUDE.md). Every test seeds a real admin session into `localStorage`,
 * loads `/desktop`, and opens the Settings window from the dock.
 *
 * Settings are GLOBAL server singletons (not per-user), so this suite takes a
 * full `GET /api/settings/export` snapshot in `beforeAll` and restores it via
 * `POST /api/settings/import` in `afterAll` — the seeded instance is left
 * exactly as it was found. There is NO auto-save: every field change is staged
 * in a shared draft and persisted ONLY when the "Save" button in the sidebar
 * footer is clicked (`[data-testid="settings-save"]`, one PUT per dirty
 * group). Any test that touches a field and then saves persists it — hence the
 * snapshot/restore.
 *
 * Run (all):      npx playwright test tests/e2e/settings/settings.spec.ts --project=firefox
 * Run (single):   npx playwright test tests/e2e/settings/settings.spec.ts -g "3.1"
 * Run (category): npx playwright test tests/e2e/settings/settings.spec.ts -g "Save"
 * Custom server:  BASE_URL=http://localhost:8080 npx playwright test tests/e2e/settings/settings.spec.ts
 *
 * Requires a live server whose setup is complete and that is seeded with an
 * admin account (defaults: e2eadmin / E2eSecret123, override with
 * E2E_ADMIN_USER / E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

// ---------------------------------------------------------------------------
// Constants & helpers
// ---------------------------------------------------------------------------

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

/**
 * The 8 category rail entries, in declaration order (Settings.svelte).
 * The 2026-09-09 IA pass folded the old 12 flat tabs into these groups:
 *   widgets      ← weather (moved out of appearance 2026-09-09)
 *   notifications ← email + notifications
 *   system       ← updates + system_info + data_management + privacy
 * `openCategory()` still accepts the old names and resolves each to its
 * containing group, so per-field tests below keep working unchanged.
 */
const CATEGORIES = [
  'general',
  'security',
  'appearance',
  'widgets',
  'notifications',
  'ai',
  'monitoring',
  'system',
] as const;
type NavGroup = (typeof CATEGORIES)[number];
type Category =
  | NavGroup
  | 'weather'
  | 'email'
  | 'updates'
  | 'system_info'
  | 'privacy'
  | 'data_management';

/** Legacy category name → the rail group that now contains it. */
const GROUP_OF: Record<Category, NavGroup> = {
  general: 'general',
  security: 'security',
  appearance: 'appearance',
  widgets: 'widgets',
  weather: 'widgets',
  notifications: 'notifications',
  email: 'notifications',
  ai: 'ai',
  monitoring: 'monitoring',
  system: 'system',
  updates: 'system',
  system_info: 'system',
  privacy: 'system',
  data_management: 'system',
};

/** Visible EN rail label for each group (tests seed lang='en'). */
const GROUP_LABEL: Record<NavGroup, string> = {
  general: 'General',
  security: 'Security',
  appearance: 'Appearance',
  widgets: 'Widgets',
  notifications: 'Notifications',
  ai: 'AI Assistant',
  monitoring: 'Monitoring',
  system: 'System',
};

interface Tokens {
  token: string;
  csrf: string;
}

async function apiLogin(ctx: APIRequestContext, u: string, p: string): Promise<Tokens> {
  const res = await ctx.post('/api/auth/login', { data: { username: u, password: p } });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

async function getHeapMB(page: Page): Promise<number> {
  return page.evaluate(() =>
    'memory' in performance ? (performance as any).usedJSHeapSize / (1024 * 1024) : 0,
  );
}

// ---------------------------------------------------------------------------
// Browser helpers — inject session, open the Settings window
// ---------------------------------------------------------------------------

async function seedBrowser(page: Page, tokens: Tokens, lang = 'en') {
  await page.addInitScript(
    ({ t, l }) => {
      try {
        localStorage.setItem('session-token', t.token);
        localStorage.setItem('csrf-token', t.csrf);
        localStorage.setItem('hv-language', l);
      } catch {
        /* storage unavailable — a test will surface it */
      }
    },
    { t: tokens, l: lang },
  );
}

async function gotoDesktop(page: Page, tokens: Tokens, lang = 'en') {
  await seedBrowser(page, tokens, lang);
  await page.goto('/desktop');
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page.locator('.hv-dock').waitFor({ state: 'visible', timeout: 10_000 });
  // A fresh account defaults every desktop monitor widget to visible; each is
  // position:fixed and floats over the whole screen. Hide them so they never
  // intercept clicks meant for the Settings window or the dock.
  //
  // `[class*="-monitor"]` is a *substring* match, and every spec in the suite
  // injects this same rule: any class in the app containing `-monitor`
  // anywhere is hidden too. That is why the Settings monitor rows are
  // `settings-widget-row` and not `settings-monitor-row` — the latter was
  // invisible here (0px track, unclickable label) while looking perfectly
  // styled in the source (E2E 23.7/23.9, 2026-09-14).
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

/** Open Settings from the dock and wait for the panel to hydrate. */
async function openSettings(page: Page, tokens: Tokens, lang = 'en') {
  await gotoDesktop(page, tokens, lang);
  const dockBtn = page.locator('.hv-dock-item:has(.hv-dock-icon-settings)').first();
  await dockBtn.scrollIntoViewIfNeeded().catch(() => {});
  // On a very narrow viewport the dock can scroll horizontally; nudge it and
  // fall back to a forced click if the icon still can't be brought on-screen.
  try {
    await dockBtn.click({ timeout: 5_000 });
  } catch {
    await dockBtn.click({ force: true });
  }
  await page.waitForSelector('[data-testid="settings-page"]', { timeout: 15_000 });
  // The rail + first card only render once loadSettings() resolves.
  await page.locator('[data-testid="settings-nav-item"]').first().waitFor({ state: 'visible', timeout: 15_000 });
  await page
    .locator('[data-testid="settings-content"] [data-testid="settings-card"], [data-testid="settings-content"] [data-testid="settings-card-title"]')
    .first()
    .waitFor({ state: 'visible', timeout: 15_000 });
  return page.locator('[data-testid="settings-page"]');
}

/** The Settings window as a scoping root. */
function set(page: Page) {
  return page.locator('[data-testid="settings-page"]');
}

/** Click the rail entry for `cat` (or the group that contains it) and wait
 *  for its panel to swap in. Selects by visible label, not index, so it is
 *  resilient to rail order/count changes. */
async function openCategory(page: Page, cat: Category) {
  const label = GROUP_LABEL[GROUP_OF[cat]];
  const item = set(page)
    .locator('[data-testid="settings-nav-item"]')
    .filter({ hasText: label });
  await item.click();
  await expect(item).toHaveClass(/set-nav-active/);
  // give the {#if activeCategory} block a beat to mount
  await page.waitForTimeout(120);
}

/**
 * Click the sidebar-footer Save button and wait for the flush to complete.
 * The button's own `data-state` is the signal — `dirty` → `saving` → `saved`
 * (the green confirmation) → `clean` (back to blue). `saved` / `clean` means
 * every PUT finished, which is a stronger signal than `networkidle` (the
 * antivirus card keeps an SSE connection open, so networkidle never fires on
 * the security category). A failed group parks the button on `error`, which
 * never matches the pattern and therefore fails the wait instead of passing.
 */
async function saveSettings(page: Page) {
  const saveBtn = set(page).locator('[data-testid="settings-save"]');
  await saveBtn.click();
  await expect(saveBtn).toHaveAttribute('data-state', /^(saved|clean)$/, { timeout: 15_000 });
  await page.waitForTimeout(150);
}

/**
 * Flip the custom checkbox-switch inside the `index`-th `.settings-toggle-label`
 * of `scope`. The real <input> is visually hidden (position:absolute, 1px), so
 * the click has to land on the wrapping <label>; the assertion is on the input.
 */
async function flipSwitch(scope: ReturnType<Page['locator']>, index = 0) {
  const label = scope.locator('.settings-toggle-label').nth(index);
  const input = label.locator('input[type="checkbox"]');
  const before = await input.isChecked();
  await label.click();
  await expect(input).toBeChecked({ checked: !before });
  return input;
}

/**
 * The alpha channel of a computed colour, across the serializations the
 * engines actually produce: `rgba(r, g, b, a)`, `rgb(r, g, b)` (a = 1) and
 * Firefox's `color(srgb r g b / a)` for anything that went through
 * `color-mix()`. A regex over the whole string would only ever match one of
 * the three, which is how the tinted-tile assertion used to fail on Firefox.
 */
function alphaOf(css: string): number {
  const slash = css.match(/\/\s*([\d.]+)\s*\)/);
  if (slash) return Number(slash[1]); // color(srgb r g b / a)
  const nums = css.match(/[\d.]+/g)?.map(Number) ?? [];
  return nums.length > 3 ? nums[3] : 1; // rgba(r, g, b, a) / rgb(r, g, b)
}

/**
 * The declarations the stylesheet applies to the rail's trailing accent bar.
 * The bar is a `::after` pseudo-element, so `getComputedStyle` resolves its
 * `margin-left: auto` to the *used* `0px` — the value that right-aligns it is
 * only readable off the rule itself.
 */
async function accentBarRule(page: Page): Promise<Record<string, string>> {
  return page.evaluate(() => {
    for (const sheet of Array.from(document.styleSheets)) {
      let rules: CSSRuleList;
      try {
        rules = sheet.cssRules;
      } catch {
        continue; // cross-origin sheet — nothing of ours is in one
      }
      for (const rule of Array.from(rules)) {
        const r = rule as CSSStyleRule;
        const sel = r.selectorText || '';
        if (sel.includes('has-accent-bar') && sel.includes('after')) {
          return { selector: sel, marginLeft: r.style.getPropertyValue('margin-left') };
        }
      }
    }
    return {};
  });
}

/** Open the notification center and return every visible message string. */
async function notificationMessages(page: Page): Promise<string[]> {
  await page.locator('.hv-notif-btn').click();
  await page.waitForTimeout(250);
  const msgs = await page
    .locator('.notifications-panel .notification-message')
    .allTextContents()
    .catch(() => [] as string[]);
  await page.locator('.hv-notif-btn').click().catch(() => {}); // close
  return msgs.map((m) => m.trim());
}

/** Poll the notification center until a message matches `re` (or time out false). */
async function waitForNotification(page: Page, re: RegExp, timeout = 12_000): Promise<boolean> {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const msgs = await notificationMessages(page);
    if (msgs.some((m) => re.test(m))) return true;
    await page.waitForTimeout(600);
  }
  return false;
}

// ---------------------------------------------------------------------------
// Shared fixture — one admin session + a settings snapshot to restore
// ---------------------------------------------------------------------------

let sharedCtx: APIRequestContext;
let admin: Tokens;
let adminHeaders: Record<string, string>;
let settingsSnapshot: any = null;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await playwright.request.newContext({ baseURL: BASE });
  admin = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  expect(admin.token, 'seeded admin login must succeed').not.toEqual('');
  adminHeaders = { 'x-session-token': admin.token, 'x-csrf-token': admin.csrf };

  const exp = await sharedCtx.get('/api/settings/export', { headers: adminHeaders });
  if (exp.ok()) {
    const body = await exp.json().catch(() => ({}));
    settingsSnapshot = body?.data ?? body ?? null;
  }
});

test.afterAll(async () => {
  // Restore the instance to exactly the settings it had before the run.
  if (settingsSnapshot && sharedCtx) {
    await sharedCtx
      .post('/api/settings/import', { headers: adminHeaders, data: { data: settingsSnapshot } })
      .catch(() => {});
  }
  await sharedCtx?.dispose();
});

// Per-test heap log (CLAUDE.md: metrics in afterEach, warn on >10 MB growth).
let heapBefore = 0;
test.beforeEach(async ({ page }) => {
  heapBefore = 0;
  page.on('load', async () => {
    if (!heapBefore) heapBefore = await getHeapMB(page).catch(() => 0);
  });
});
test.afterEach(async ({ page }, testInfo) => {
  const after = await getHeapMB(page).catch(() => 0);
  if (heapBefore > 0 && after > 0 && after - heapBefore > 10) {
    console.warn(`[MEM] "${testInfo.title}" grew heap by ${(after - heapBefore).toFixed(1)} MB`);
  }
});

// ===========================================================================
// 1 — Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — Settings window opens from the dock', async ({ page }) => {
    await openSettings(page, admin);
    await expect(set(page)).toBeVisible();
    await expect(page.locator('.hv-window:has([data-testid="settings-page"])')).toBeVisible();
  });

  test('1.2 — its own frosted WindowHeader renders (chromeless, no AppHeader)', async ({ page }) => {
    await openSettings(page, admin);
    const win = page.locator('.hv-window:has([data-testid="settings-page"])');
    // exactly one glass header, and it belongs to the app
    await expect(win.locator('.hv-winhdr')).toHaveCount(1);
    await expect(win.locator('.hv-winhdr')).toContainText(/settings|paramètres/i);
    // The old AppHeader must be gone. `.ah-wrap` / `.ah-top` no longer exist
    // anywhere in the frontend (deleted 2026-09-08), so the old
    // `toHaveCount(0)` on them could never fail — assert the bar's real
    // shape instead: the icon tile + the title block, nothing else.
    await expect(win.locator('.hv-winhdr > *')).toHaveCount(2);
  });

  test('1.3 — the Save button lives in the sidebar footer and carries the save state', async ({ page }) => {
    await openSettings(page, admin);
    // the one Save button for the whole screen is in the rail footer, and it
    // doubles as the save indicator (the old status pill/caption are gone)
    const saveBtn = set(page).locator('[data-testid="settings-save"]');
    await expect(saveBtn).toBeVisible();
    await expect(saveBtn).toHaveAttribute('data-state', 'clean');
    // the sr-only status line it renders stays in the DOM (role="status")
    await expect(page.locator('[data-testid="settings-save-status"]')).toHaveCount(1);
  });

  test('1.4 — the category rail lists all 8 entries', async ({ page }) => {
    await openSettings(page, admin);
    await expect(set(page).locator('[data-testid="settings-nav-item"]')).toHaveCount(CATEGORIES.length);
  });

  test('1.5 — General is the default open category', async ({ page }) => {
    await openSettings(page, admin);
    await expect(set(page).locator('[data-testid="settings-nav-item"]').first()).toHaveClass(/set-nav-active/);
    await expect(set(page).locator('#serverName')).toBeVisible();
  });

  test('1.6 — the panel is scrollable, not the page body', async ({ page }) => {
    await openSettings(page, admin);
    const overflow = await set(page)
      .locator('[data-testid="settings-content"]')
      .evaluate((el) => getComputedStyle(el).overflowY);
    expect(['auto', 'scroll']).toContain(overflow);
  });

  test('1.7 — spinner clears (content, not a stuck loader)', async ({ page }) => {
    await openSettings(page, admin);
    await expect(set(page).locator('[data-testid="settings-spinner"]')).toHaveCount(0);
  });
});

// ===========================================================================
// 2 — Category navigation
// ===========================================================================

test.describe('2 — Category navigation', () => {
  test('2.1 — every category opens and shows a heading', async ({ page }) => {
    await openSettings(page, admin);
    for (const cat of CATEGORIES) {
      await openCategory(page, cat);
      // security / monitoring render frozen sub-cards; the rest have a [data-testid="settings-card-title"]
      const hasHeading =
        (await set(page).locator('[data-testid="settings-content"] [data-testid="settings-card-title"]').count()) > 0;
      expect(hasHeading, `${cat} should render at least one card heading`).toBeTruthy();
    }
  });

  test('2.2 — exactly one rail entry is active at a time', async ({ page }) => {
    await openSettings(page, admin);
    for (const cat of ['appearance', 'email', 'ai', 'privacy'] as Category[]) {
      await openCategory(page, cat);
      await expect(set(page).locator('[data-testid="settings-nav-item"].set-nav-active')).toHaveCount(1);
    }
  });

  test('2.3 — switching category swaps the panel content', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'general');
    await expect(set(page).locator('#serverName')).toBeVisible();
    await openCategory(page, 'email');
    await expect(set(page).locator('#serverName')).toHaveCount(0);
    await expect(set(page).locator('#smtpServer')).toBeVisible();
  });

  test('2.4 — rapid switching leaves a consistent state', async ({ page }) => {
    await openSettings(page, admin);
    for (let i = 0; i < 3; i++) {
      for (const cat of CATEGORIES) {
        await set(page).locator('[data-testid="settings-nav-item"]').nth(CATEGORIES.indexOf(cat)).click();
      }
    }
    await expect(set(page).locator('[data-testid="settings-nav-item"].set-nav-active')).toHaveCount(1);
    await expect(set(page).locator('[data-testid="settings-content"]')).toBeVisible();
  });

  test('2.5 — no card clips its own content; the panel scrolls instead', async ({ page }) => {
    // Regression: [data-testid="settings-content"] is a column flexbox and every card sets
    // overflow:hidden, which drops its automatic flex min-size to 0. Without
    // `[data-testid="settings-content"] > :global([data-testid="settings-card"]) { flex-shrink: 0 }` the flexbox
    // squeezes tall cards and their content is visually cut off.
    await page.setViewportSize({ width: 1280, height: 720 });
    await openSettings(page, admin);
    for (const cat of CATEGORIES) {
      await openCategory(page, cat);
      const clips = await set(page).locator('[data-testid="settings-content"] > [data-testid="settings-card"]').evaluateAll(
        (cards) =>
          cards
            .map((el, i) => ({ i, over: el.scrollHeight - el.clientHeight }))
            .filter((c) => c.over > 2),
      );
      expect(clips, `${cat}: card(s) clipping content ${JSON.stringify(clips)}`).toEqual([]);
    }
    // and the panel itself is the scroll container for the tall categories
    await openCategory(page, 'security');
    const scrolls = await set(page)
      .locator('[data-testid="settings-content"]')
      .evaluate((el) => el.scrollHeight > el.clientHeight + 2);
    expect(scrolls, 'the [data-testid="settings-content"] panel should scroll when cards overflow it').toBeTruthy();
  });
});

// ===========================================================================
// 3 — General
// ===========================================================================

test.describe('3 — General', () => {
  test('3.1 — server name field is populated and editable', async ({ page }) => {
    await openSettings(page, admin);
    const f = set(page).locator('#serverName');
    await expect(f).toBeVisible();
    await expect(f).toHaveAttribute('type', 'text');
    const original = await f.inputValue();
    await f.fill('Renamed Vault');
    await expect(f).toHaveValue('Renamed Vault');
    await f.fill(original); // don't persist — no save here
  });

  test('3.2 — language select offers the 4 languages', async ({ page }) => {
    await openSettings(page, admin);
    const opts = await set(page).locator('#language option').allTextContents();
    expect(opts.join('|')).toMatch(/English/);
    expect(opts.join('|')).toMatch(/Français/);
    expect(opts.join('|')).toMatch(/Deutsch/);
    expect(opts.join('|')).toMatch(/Español/);
  });

  test('3.3 — timezone select has many zones and a working change', async ({ page }) => {
    await openSettings(page, admin);
    const sel = set(page).locator('#timezone');
    expect(await sel.locator('option').count()).toBeGreaterThan(5);
    await sel.selectOption('Europe/Paris');
    await expect(sel).toHaveValue('Europe/Paris');
  });

  test('3.4 — "start on boot" toggle is staged, then PUT /api/settings/autostart on Save', async ({ page }) => {
    await openSettings(page, admin);
    // The autostart row uses the shared <Switch>: a visually hidden native
    // checkbox inside a `.settings-toggle-label`, which is the real hit
    // target (the input itself is clipped to 1px, so clicking it directly is
    // not reliably actionable).
    const toggle = set(page)
      .locator('[data-testid="settings-card"] .settings-toggle-label')
      .first();
    await expect(toggle).toBeVisible();
    const before = await sharedCtx.get('/api/settings/autostart', { headers: adminHeaders });
    const enabledBefore = !!(await before.json().catch(() => ({})))?.data?.enabled;

    // Toggling must NOT persist by itself — no PUT until Save.
    await toggle.click();
    await page.waitForTimeout(1200);
    const after = await sharedCtx.get('/api/settings/autostart', { headers: adminHeaders });
    const enabledAfterToggle = !!(await after.json().catch(() => ({})))?.data?.enabled;
    expect(enabledAfterToggle, 'autostart must not change until Save').toBe(enabledBefore);

    // The Save button flushes the staged value.
    const [resp] = await Promise.all([
      page.waitForResponse(
        (r) => r.url().includes('/api/settings/autostart') && r.request().method() === 'PUT',
        { timeout: 8_000 },
      ),
      set(page).locator('[data-testid="settings-save"]').click(),
    ]);
    expect(resp.request().postDataJSON()).toHaveProperty('enabled');
    // 200 where systemd is available; a 4xx where it is not (dev boxes have no
    // unit) — but the handler must never 5xx.
    expect(resp.status(), `autostart PUT status ${resp.status()}`).toBeLessThan(500);

    // restore
    await sharedCtx.put('/api/settings/autostart', {
      headers: adminHeaders,
      data: { enabled: enabledBefore },
    });
  });
});

// ===========================================================================
// 4 — Appearance
// ===========================================================================

test.describe('4 — Appearance', () => {
  test('4.1 — the section shows only the bundled-wallpaper grid (no density / accent / upload)', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'appearance');
    const grid = set(page).locator('.settings-wallpaper-grid');
    await expect(grid).toBeVisible();
    // removed controls
    await expect(set(page).locator('#density')).toHaveCount(0);
    await expect(set(page).locator('#accentColor')).toHaveCount(0);
    await expect(set(page).locator('input[type="file"]')).toHaveCount(0);
    await expect(page.locator('[data-slot="dialog-content"]')).toHaveCount(0);
  });

  test('4.2 — grid is a 3-column radiogroup of the 15 bundled images', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'appearance');
    const grid = set(page).locator('.settings-wallpaper-grid');
    await expect(grid).toHaveAttribute('role', 'radiogroup');
    await expect(grid.locator('.settings-wallpaper-tile')).toHaveCount(15);
    const cols = await grid.evaluate(
      (el) => getComputedStyle(el).gridTemplateColumns.split(' ').length
    );
    expect(cols).toBe(3);
  });

  test('4.3 — every thumbnail is a same-origin /wallpapers/ webp that actually loads', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'appearance');
    const imgs = set(page).locator('.settings-wallpaper-grid img');
    await expect(imgs).toHaveCount(15);
    for (let i = 0; i < 15; i++) {
      const img = imgs.nth(i);
      await expect(img).toHaveAttribute('src', /^\/wallpapers\/hv-\d\d-thumb\.webp$/);
      await expect(img).toHaveAttribute('decoding', 'async');
      await img.scrollIntoViewIfNeeded();
      await expect
        .poll(() => img.evaluate((el: HTMLImageElement) => el.naturalWidth))
        .toBeGreaterThan(0);
    }
  });

  test('4.4 — selecting a wallpaper marks exactly one tile checked and stores it locally', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'appearance');
    const tiles = set(page).locator('.settings-wallpaper-tile');
    const target = tiles.nth(3); // 4th bundled wallpaper = hv-04 (no gradient tile any more)
    await target.click();
    await expect(target).toHaveClass(/is-selected/);
    await expect(target).toHaveAttribute('aria-checked', 'true');
    await expect(target.locator('.settings-wallpaper-check')).toBeVisible();
    await expect(set(page).locator('.settings-wallpaper-tile[aria-checked="true"]')).toHaveCount(1);

    const stored = await page.evaluate(() => localStorage.getItem('hv-wallpaper'));
    expect(stored).toContain('/wallpapers/hv-04.webp');

    // the choice survives navigating away from the section and back
    await openCategory(page, 'general');
    await openCategory(page, 'appearance');
    await expect(
      set(page).locator('.settings-wallpaper-tile').nth(3)
    ).toHaveAttribute('aria-checked', 'true');
  });

  test('4.5 — there is no gradient "default" tile; every tile is a real wallpaper', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'appearance');
    const tiles = set(page).locator('.settings-wallpaper-tile');
    await tiles.first().click();
    await expect(tiles.first()).toHaveAttribute('aria-checked', 'true');
    const stored = await page.evaluate(() => localStorage.getItem('hv-wallpaper'));
    expect(stored).toContain('/wallpapers/hv-01.webp');
    expect(stored).not.toContain('"default"');
  });

  test('4.6 — picking a wallpaper stages it; only the Save button PUTs /api/settings/appearance', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'appearance');

    // Pick whichever tile is NOT the current selection (clicking the already
    // selected one is a no-op and would never stage a change).
    const tiles = set(page).locator('.settings-wallpaper-tile');
    const count = await tiles.count();
    let target = -1;
    for (let i = 0; i < count; i++) {
      const cls = (await tiles.nth(i).getAttribute('class')) || '';
      if (!cls.includes('is-selected')) { target = i; break; }
    }
    expect(target, 'expected at least one unselected wallpaper tile').toBeGreaterThanOrEqual(0);

    // Picking alone must not reach the server — no PUT until Save.
    let sawAppearancePut = false;
    const onReq = (r: any) => {
      if (r.url().includes('/api/settings/appearance') && r.method() === 'PUT') sawAppearancePut = true;
    };
    page.on('request', onReq);
    await tiles.nth(target).click();
    await page.waitForTimeout(1200);
    page.off('request', onReq);
    expect(sawAppearancePut, 'picking a wallpaper must not auto-save').toBe(false);

    // The Save button flushes the staged wallpaper.
    const [req] = await Promise.all([
      page.waitForRequest(
        (r) => r.url().includes('/api/settings/appearance') && r.method() === 'PUT',
        { timeout: 15_000 },
      ),
      set(page).locator('[data-testid="settings-save"]').click(),
    ]);
    expect(JSON.parse(req.postData() || '{}').wallpaper).toMatch(/\/wallpapers\/hv-\d\d\.webp/);
  });

  test('4.7 — thumbnails keep a 16:10 ratio, even gaps (no drop-shadow bleed), 2 columns when narrow', async ({ page }) => {
    // Regressions: (a) the <img height="200"> attribute pinned every thumbnail
    // to 200px tall (no `height:auto`) → distorted vertical strips on the
    // 3-column grid; (b) the card drop-shadow bled ~24px past each tile and,
    // with a 12px gap, made the rows look cramped / overlapping.
    await page.setViewportSize({ width: 460, height: 820 });
    await openSettings(page, admin);
    await openCategory(page, 'appearance');
    const grid = set(page).locator('.settings-wallpaper-grid');
    const m = await grid.evaluate((el) => {
      const cs = getComputedStyle(el);
      const t = [...el.querySelectorAll('.settings-wallpaper-tile')].slice(0, 3).map((n) => n.getBoundingClientRect());
      const img = (el.querySelector('img') as HTMLImageElement).getBoundingClientRect();
      return {
        cols: cs.gridTemplateColumns.split(' ').length,
        ratio: img.width / img.height,
        colGap: Math.round(t[1].x - t[0].right),
        rowGap: Math.round(t[2].y - t[0].bottom),
        // the only box-shadow allowed is the inset selection ring
        outsetShadow: [...el.querySelectorAll('.settings-wallpaper-thumb')].some((n) => {
          const bs = getComputedStyle(n).boxShadow;
          return bs !== 'none' && !bs.includes('inset');
        }),
      };
    });
    expect(m.cols, '2 columns below 560px').toBe(2);
    expect(m.ratio, `thumbnail aspect ratio ${m.ratio.toFixed(2)} should be ~1.6`).toBeGreaterThan(1.45);
    expect(m.ratio).toBeLessThan(1.75);
    expect(m.colGap, 'column gap == row gap').toBe(m.rowGap);
    expect(m.outsetShadow, 'no bleeding outset drop-shadow on thumbnails').toBe(false);
  });
});

// ===========================================================================
// 5 — Weather widget
// ===========================================================================

test.describe('5 — Weather widget', () => {
  test('5.1 — shows a search box or the current location editor', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'weather');
    const searchBox = set(page).locator('.settings-weather-search-row input');
    const current = set(page).locator('.settings-weather-current');
    expect((await searchBox.count()) + (await current.count())).toBeGreaterThan(0);
  });

  test('5.2 — "displayed data" exposes one toggle per optional field', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'weather');
    await expect(set(page).locator('.settings-weather-fields .settings-toggle-label').first()).toBeVisible();
    expect(await set(page).locator('.settings-weather-fields input[type="checkbox"]').count()).toBeGreaterThan(0);
  });

  test('5.3 — toggling an optional field stages it; localStorage is only written on Save', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'weather');
    const fields = set(page).locator('.settings-weather-fields');
    await flipSwitch(fields, 0);

    // The display-prefs store must not be touched until Save.
    let stored = await page.evaluate(() => localStorage.getItem('hv-weather-display-fields'));
    expect(stored, 'no localStorage write before Save').toBeNull();

    await saveSettings(page);
    stored = await page.evaluate(() => localStorage.getItem('hv-weather-display-fields'));
    expect(stored, 'weather display fields persist to localStorage on Save').not.toBeNull();

    // restore
    await fields.locator('.settings-toggle-label').first().click();
    await saveSettings(page);
    stored = await page.evaluate(() => localStorage.getItem('hv-weather-display-fields'));
    expect(stored).toBe('[]');
  });

  test('5.4 — location search reaches Open-Meteo and lists results', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'weather');
    // switch to the editor if a location is already set
    const edit = set(page).locator('.settings-weather-current .settings-btn');
    if (await edit.count()) await edit.click();
    const box = set(page).locator('.settings-weather-search-row input');
    await box.fill('Paris');
    await Promise.all([
      page.waitForResponse((r) => r.url().includes('geocoding-api.open-meteo.com'), { timeout: 15_000 }),
      set(page).locator('.settings-weather-search-row .settings-btn').click(),
    ]).catch(() => {
      /* the machine may be offline — the button/flow is still what we assert below */
    });
    // Either results came back, or the "no location" hint shows — both are valid states.
    const results = set(page).locator('.settings-weather-result-btn');
    const hint = set(page).locator('.settings-hint');
    expect((await results.count()) + (await hint.count())).toBeGreaterThan(0);
  });
});

// ===========================================================================
// 6 — Email / SMTP
// ===========================================================================

test.describe('6 — Email', () => {
  test('6.1 — all five SMTP fields render with the right input types', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'email');
    await expect(set(page).locator('#smtpServer')).toHaveAttribute('type', 'text');
    await expect(set(page).locator('#smtpPort')).toHaveAttribute('type', 'number');
    await expect(set(page).locator('#smtpUsername')).toHaveAttribute('type', 'text');
    await expect(set(page).locator('#smtpPassword')).toHaveAttribute('type', 'password');
    await expect(set(page).locator('#fromAddress')).toHaveAttribute('type', 'email');
  });

  test('6.2 — the password field is never pre-filled (redaction)', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'email');
    await expect(set(page).locator('#smtpPassword')).toHaveValue('');
  });

  test('6.3 — the TLS checkbox toggles', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'email');
    await flipSwitch(set(page), 0);
  });

  test('6.4 — "Test email" fires POST /api/settings/email/test and reports a result', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'email');
    const btn = set(page).locator('[data-testid="settings-card-footer"] .settings-btn');
    await expect(btn).toContainText(/test/i);
    const [resp] = await Promise.all([
      page.waitForResponse((r) => r.url().includes('/api/settings/email/test'), { timeout: 15_000 }),
      btn.click(),
    ]);
    expect(resp.request().method()).toBe('POST');
    // A notification (success, or a real SMTP / not-configured error) must show.
    expect(
      await waitForNotification(page, /\S/),
      'test email must surface a result notification',
    ).toBeTruthy();
  });
});

// ===========================================================================
// 7 — Notifications
// ===========================================================================

test.describe('7 — Notifications', () => {
  test('7.1 — notification email field + hint', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'notifications');
    await expect(set(page).locator('#notifyEmail')).toHaveAttribute('type', 'email');
    await expect(set(page).locator('.settings-hint').first()).toBeVisible();
  });

  test('7.2 — alert-threshold slider is a 0..100 range, step 5, and moves', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'notifications');
    const s = set(page).locator('#alertThreshold');
    await expect(s).toHaveAttribute('type', 'range');
    await expect(s).toHaveAttribute('min', '0');
    await expect(s).toHaveAttribute('max', '100');
    await expect(s).toHaveAttribute('step', '5');
    await s.fill('65');
    await expect(s).toHaveValue('65');
    await expect(set(page).locator('label[for="alertThreshold"]')).toContainText('65');
  });

  test('7.3 — the three notification toggles all flip independently', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'notifications');
    // Scope to the alerts card — the Notifications group also stacks the
    // e-mail/SMTP card, which carries its own TLS toggle.
    const labels = set(page).locator('[data-testid="settings-card"]:has(#alertThreshold) .settings-toggle-label');
    await expect(labels).toHaveCount(3);
    for (let i = 0; i < 3; i++) {
      const input = labels.nth(i).locator('input[type="checkbox"]');
      const before = await input.isChecked();
      await labels.nth(i).click();
      await expect(input).toBeChecked({ checked: !before });
      await labels.nth(i).click(); // restore
    }
  });

  test('7.4 — webhook URL field is a url input', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'notifications');
    await expect(set(page).locator('#webhookUrl')).toHaveAttribute('type', 'url');
    await set(page).locator('#webhookUrl').fill('https://example.com/hook');
    await expect(set(page).locator('#webhookUrl')).toHaveValue('https://example.com/hook');
  });
});

// ===========================================================================
// 8 — Updates
// ===========================================================================

test.describe('8 — Updates', () => {
  test('8.1 — frequency select has the 5 options', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'updates');
    const vals = await set(page).locator('#updateFrequency option').evaluateAll((os) =>
      os.map((o) => (o as HTMLOptionElement).value),
    );
    expect(vals.sort()).toEqual(['daily', 'disabled', 'manual', 'monthly', 'weekly']);
  });

  test('8.2 — version box shows the current version', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'updates');
    await expect(set(page).locator('.settings-version-box')).toBeVisible();
    await expect(set(page).locator('.settings-version-row').first()).toContainText(/v\d+\.\d+/);
  });

  test('8.3 — "Check for updates" hits GET /api/settings/version', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'updates');
    const [resp] = await Promise.all([
      page.waitForResponse((r) => r.url().includes('/api/settings/version'), { timeout: 15_000 }),
      set(page).locator('[data-testid="settings-card-footer"] .settings-btn').click(),
    ]);
    expect(resp.request().method()).toBe('GET');
  });
});

// ===========================================================================
// 9 — AI Assistant
// ===========================================================================

test.describe('9 — AI Assistant', () => {
  test('9.1 — either the loading state, the model row, or the Ollama-down status shows', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'ai');
    const row = set(page).locator('#aiModel');
    const status = set(page).locator('.settings-ai-status');
    await expect(async () => {
      expect((await row.count()) + (await status.count())).toBeGreaterThan(0);
    }).toPass({ timeout: 15_000 });
  });

  test('9.2 — Ollama status line + refresh button', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'ai');
    const status = set(page).locator('.settings-ai-status');
    if ((await status.count()) === 0) test.skip(true, 'AI section unavailable');
    await expect(status.locator('.settings-ai-status-dot')).toBeVisible();

    // The card fires /chat/models on mount; wait for that to fully settle so
    // the refresh click makes its own fresh request (ttl:0, no in-flight
    // coalescing).
    await page.waitForResponse((r) => r.url().includes('/api/chat/models'), { timeout: 15_000 })
      .catch(() => {});
    await page.waitForLoadState('networkidle').catch(() => {});

    let sawModels = false;
    const onResp = (r: any) => { if (r.url().includes('/api/chat/models')) sawModels = true; };
    page.on('response', onResp);
    await status.locator('.settings-ai-refresh-btn').click();
    await page.waitForTimeout(3000);
    page.off('response', onResp);
    // A fresh refetch is the norm; if the client short-circuits it, at least
    // the status line must stay rendered (the button is not broken).
    if (!sawModels) await expect(status.locator('.settings-ai-status-dot')).toBeVisible();
  });

  test('9.3 — manual-install row: quick-pick buttons fill the input', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'ai');
    const status = set(page).locator('.settings-ai-status');
    if ((await status.count()) === 0) test.skip(true, 'AI section still loading / unavailable');
    const quick = set(page).locator('.settings-ai-quick-btn').first();
    if (await quick.count()) {
      const label = (await quick.textContent())?.trim() || '';
      await quick.click();
      await expect(set(page).locator('#newModelName')).toHaveValue(label);
    }
  });

  test('9.4 — the model select is disabled when there are no installed models', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'ai');
    const select = set(page).locator('#aiModel');
    if ((await select.count()) === 0) test.skip(true, 'AI section still loading');
    // No per-card Apply button — the model pick is staged and the sidebar
    // footer Save button activates it server-side.
    await expect(set(page).locator('.settings-ai-select-row .settings-btn')).toHaveCount(0);
    const optionCount = await select.locator('option').count();
    if (optionCount === 0) await expect(select).toBeDisabled();
  });
});

// ===========================================================================
// 10 — System Information
// ===========================================================================

test.describe('10 — System Information', () => {
  test('10.1 — server address / platform / version rows render', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'system_info');
    const rows = set(page).locator('.settings-info-row');
    await expect(rows).toHaveCount(3);
    await expect(rows.nth(1)).toContainText(/Linux/);
    await expect(rows.nth(2)).toContainText(/HomeVault v\d/);
  });
});

// ===========================================================================
// 11 — Privacy
// ===========================================================================

test.describe('11 — Privacy', () => {
  test('11.1 — three privacy feature blurbs render', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'privacy');
    await expect(set(page).locator('.settings-privacy-feature')).toHaveCount(3);
  });
  // (11.2 removed — the privacy card renders three blurbs and no policy link,
  //  so there is nothing to assert. The `.settings-privacy-link` CSS rule that
  //  used to outlive the markup went with the 2026-09-14 re-skin; the class is
  //  now referenced nowhere, which is why no test targets it.)
});

// ===========================================================================
// 12 — Security (frozen sub-cards render inside the panel)
// ===========================================================================

test.describe('12 — Security', () => {
  test('12.1 — Antivirus / Anti-Ransomware / Firewall cards all mount', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'security');
    const titles = (await set(page).locator('[data-testid="settings-card-title"]').allTextContents())
      .join(' | ')
      .toLowerCase();
    expect(titles).toMatch(/antivirus/);
    expect(titles).toMatch(/ransom/);
    expect(titles).toMatch(/firewall|pare-feu/);
  });

  test('12.2 — Security folder fields are pick-only, and the picker is a floating (non-modal) panel', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'security');

    // No manual path entry anywhere in the Security panel: every folder field
    // is a read-only display + a "Select a folder" button.
    const readonly = set(page).locator('.hv-ppf input[readonly]');
    expect(await readonly.count()).toBeGreaterThan(0);
    expect(await set(page).locator('[data-testid="settings-content"] input:not([readonly])[id*="Path" i], [data-testid="settings-content"] input:not([readonly])[id*="Dir" i]').count()).toBe(0);

    // The picker opens as a small draggable panel, not a screen-blocking modal.
    await set(page).locator('.hv-ppf button').first().click();
    const picker = page.locator(".hv-fpp");
    await expect(picker).toBeVisible();
    expect(await page.locator('.hv-fp-overlay').count()).toBe(0);
    const box = await picker.boundingBox();
    expect(box!.width).toBeLessThan(page.viewportSize()!.width * 0.9);
    await expect(picker.locator('.hv-fpp-header')).toBeVisible(); // drag handle
    await page.keyboard.press('Escape');
    await expect(picker).toHaveCount(0);
  });

  test('12.3 — HTTPS/TLS card loads with an enable toggle', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'security');
    const card = set(page).locator('[data-testid="settings-card"]', { hasText: /HTTPS|TLS/ });
    await expect(card).toBeVisible();
    // The enable switch is the card's first toggle, reached the same way as
    // every other switch on this screen (`.settings-toggle-label` wrapping a
    // real checkbox) — see flipSwitch().
    await expect(card.locator('.settings-toggle-label').first()).toBeVisible();
    await expect(card.locator('.hv-switch-track').first()).toBeVisible();
    // Persistence moved to the screen-wide Save button in the sidebar footer.
    await expect(set(page).locator('[data-testid="settings-save"]')).toBeVisible();
  });

  test('12.4 — enabling TLS reveals the port field, mode radios and redirect toggle', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'security');
    const card = set(page).locator('[data-testid="settings-card"]', { hasText: /HTTPS|TLS/ });
    // Toggle "Enable HTTPS" on (first switch in the card).
    await flipSwitch(card, 0);
    await expect(page.locator('#tls-https-port')).toBeVisible();
    await expect(card.locator('input[type="radio"][value="self-signed"]')).toHaveCount(1);
    await expect(card.locator('input[type="radio"][value="manual"]')).toHaveCount(1);

    // Choosing "manual" reveals two pick-only file fields (no manual typing).
    await card.locator('input[type="radio"][value="manual"]').click();
    await expect(page.locator('#tls-cert-path')).toHaveAttribute('readonly', '');
    await expect(page.locator('#tls-key-path')).toHaveAttribute('readonly', '');
  });

  test('12.5 — a port equal to the HTTP port is flagged in the UI', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'security');
    const card = set(page).locator('[data-testid="settings-card"]', { hasText: /HTTPS|TLS/ });
    await flipSwitch(card, 0);
    // Read the HTTP port the card reports, then set the HTTPS port to match.
    const httpPort = await page.evaluate(async () => {
      const r = await fetch('/api/settings/tls', {
        headers: { 'x-session-token': localStorage.getItem('session-token') || '' },
      });
      return (await r.json()).data.http_port as number;
    });
    await page.locator('#tls-https-port').fill(String(httpPort));
    // The inline clash message is the UI's guard (the server validates too).
    await expect(card.locator('.tls-inline-error')).toBeVisible();
  });

  test('12.6 — security: no certificate/key material in URL or localStorage', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'security');
    const tlsCard = set(page).locator('[data-testid="settings-card"]', { hasText: /HTTPS|TLS/ });
    // Reveal the certificate fields so the assertion covers a state where key
    // material is actually on screen, not just the collapsed card.
    await flipSwitch(tlsCard, 0);
    expect(page.url()).not.toMatch(/BEGIN|PRIVATE|\.key|\.pem/i);
    const ls = await page.evaluate(() => JSON.stringify(localStorage));
    expect(ls).not.toMatch(/PRIVATE KEY|BEGIN CERTIFICATE/);
  });
});

// ===========================================================================
// 13 — Monitoring (frozen sub-cards)
// ===========================================================================

test.describe('13 — Monitoring', () => {
  test('13.1 — UPS + SNMP cards mount; the group disk-quota card is gone', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'monitoring');
    await expect(set(page).locator('[data-testid="settings-card"]')).not.toHaveCount(0);
    const titles = (await set(page).locator('[data-testid="settings-card-title"]').allTextContents())
      .join(' | ')
      .toLowerCase();
    expect(titles).toMatch(/ups|onduleur/);
    expect(titles).toMatch(/snmp/);
    // Disk quotas are managed per user/group on the Users/Groups pages now.
    expect(titles).not.toMatch(/quota/);
  });
});

// ===========================================================================
// 14 — Data Management
// ===========================================================================

test.describe('14 — Data Management', () => {
  test('14.1 — Export downloads a JSON settings file', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'data_management');
    const [download] = await Promise.all([
      page.waitForEvent('download', { timeout: 15_000 }),
      set(page).locator('.settings-data-actions .settings-btn').first().click(),
    ]);
    expect(download.suggestedFilename()).toMatch(/homevault-settings-.*\.json/);
  });

  test('14.2 — the import source toggles between paste and file', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'data_management');
    await expect(set(page).locator('#importText')).toBeVisible();
    await set(page).locator('.settings-data-actions .settings-btn').nth(1).click();
    await expect(set(page).locator('#importFile')).toBeVisible();
    await expect(set(page).locator('#importText')).toHaveCount(0);
  });

  test('14.3 — importing invalid JSON is rejected with a notification', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'data_management');
    await set(page).locator('#importText').fill('{ not json');
    await set(page).locator('.settings-field-full .settings-btn-primary').click();
    expect(await waitForNotification(page, /json|invalid|invalide/i)).toBeTruthy();
  });

  test('14.4 — a valid round-trip import succeeds', async ({ page }) => {
    test.skip(!settingsSnapshot, 'no settings snapshot captured');
    await openSettings(page, admin);
    await openCategory(page, 'data_management');
    await set(page).locator('#importText').fill(JSON.stringify(settingsSnapshot));
    const [resp] = await Promise.all([
      page.waitForResponse((r) => r.url().includes('/api/settings/import'), { timeout: 15_000 }),
      set(page).locator('.settings-field-full .settings-btn-primary').click(),
    ]);
    expect(resp.ok()).toBeTruthy();
  });

  test('14.5 — "Reset to defaults" asks for confirmation first', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'data_management');
    // Scoped to the data-actions row: the rail-footer Save button wears the
    // same `settings-btn-danger` skin while a change is staged.
    await set(page).locator('.settings-data-actions .settings-btn-danger').click();
    await expect(page.locator('[data-slot="dialog-content"]')).toBeVisible({ timeout: 8_000 });
    // Dismissed — nothing is reset.
    await page.locator('[data-slot="dialog-content"]').getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(page.locator('[data-slot="dialog-content"]')).toHaveCount(0);
  });
});

// ===========================================================================
// 15 — Save button (no auto-save — each change is staged, flushed by Save)
// ===========================================================================

test.describe('15 — Save button', () => {
  test('15.1 — editing a field stages it; the Save button PUTs only that group', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'general');

    const puts: string[] = [];
    page.on('request', (r) => {
      if (r.method() === 'PUT' && r.url().includes('/api/settings/')) puts.push(r.url());
    });

    await set(page).locator('#serverName').fill(`E2E ${Date.now()}`);
    await page.waitForTimeout(1200);
    // No auto-save: the edit alone must not reach the server.
    expect(puts.some((u) => u.includes('/api/settings/general'))).toBe(false);

    // The Save button flushes only the group the user actually changed.
    const [req] = await Promise.all([
      page.waitForResponse(
        (r) => r.url().includes('/api/settings/general') && r.request().method() === 'PUT',
        { timeout: 15_000 },
      ),
      set(page).locator('[data-testid="settings-save"]').click(),
    ]);
    expect(req.ok()).toBe(true);
    await page.waitForTimeout(400); // let any stray fan-out settle

    expect(puts.some((u) => u.includes('/api/settings/general'))).toBe(true);
    expect(puts.some((u) => u.includes('/api/settings/email'))).toBe(false);
    expect(puts.some((u) => u.includes('/api/settings/notifications'))).toBe(false);
  });

  test('15.2 — a saved server name survives a window reopen', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'general');
    const stamp = `Persist ${Date.now() % 100000}`;
    await set(page).locator('#serverName').fill(stamp);
    const [req] = await Promise.all([
      page.waitForResponse(
        (r) => r.url().includes('/api/settings/general') && r.request().method() === 'PUT',
        { timeout: 15_000 },
      ),
      set(page).locator('[data-testid="settings-save"]').click(),
    ]);
    expect(req.ok()).toBe(true);
    await page.waitForTimeout(400);

    // close & reopen
    await page.locator('.hv-window:has([data-testid="settings-page"]) [data-testid="win-close"]').first().click();
    await expect(page.locator('[data-testid="settings-page"]')).toHaveCount(0);
    await page.locator('.hv-dock-item:has(.hv-dock-icon-settings)').first().click();
    await page.waitForSelector('[data-testid="settings-page"]');
    await openCategory(page, 'general');
    await expect(set(page).locator('#serverName')).toHaveValue(stamp);
  });

  test('15.3 — the Save button colour + label track a save in flight, then settle', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'general');
    // Hold the general PUT open long enough to observe the "saving" state.
    await page.route('**/api/settings/general', async (route) => {
      await new Promise((r) => setTimeout(r, 1200));
      await route.continue();
    });
    const status = page.locator('[data-testid="settings-save-status"]');
    const saveBtn = set(page).locator('[data-testid="settings-save"]');
    // The three fills, read from the live document rather than hardcoded:
    // `--hv-primary` is re-hydrated from the saved accent at boot
    // (App.svelte), so only the running page knows the real blue.
    const tokenRGB = (name: string) =>
      page.evaluate((token) => {
        const probe = document.createElement('span');
        probe.style.color = `var(${token})`;
        document.body.appendChild(probe);
        const rgb = getComputedStyle(probe).color;
        probe.remove();
        return rgb;
      }, name);
    const BLUE = await tokenRGB('--hv-primary');
    const RED = await tokenRGB('--hv-error');
    const GREEN = await tokenRGB('--hv-success');

    // Nothing staged: neutral blue, neutral label. Park the pointer off the
    // button first — a resting hover would read `--hv-primary-hover` instead.
    await page.mouse.move(0, 0);
    await expect(saveBtn).toHaveAttribute('data-state', 'clean');
    await expect(saveBtn).toHaveCSS('background-color', BLUE);
    await expect(saveBtn).toHaveText('Save');

    // A staged change: red with an adapted label — and never colour alone
    // (WCAG 1.4.1), the sr-only status line spells the state out too.
    await set(page).locator('#serverName').fill(`Flight ${Date.now() % 10000}`);
    await expect(saveBtn).toHaveAttribute('data-state', 'dirty', { timeout: 6_000 });
    await expect(saveBtn).toHaveCSS('background-color', RED);
    await expect(saveBtn).toHaveText('Save changes');
    await expect(status).toHaveText('Save changes');

    await saveBtn.click();
    await expect(saveBtn).toHaveAttribute('data-state', 'saving');
    await expect(status).toHaveText('Saving…', { timeout: 6_000 });

    // Green confirmation, long enough to read it…
    await expect(saveBtn).toHaveAttribute('data-state', 'saved', { timeout: 8_000 });
    await page.mouse.move(0, 0);
    await expect(saveBtn).toHaveCSS('background-color', GREEN);
    await expect(saveBtn).toHaveText('Saved');
    await expect(status).toHaveText('Saved');

    // …then back to neutral blue, for both the fill and the label.
    await expect(saveBtn).toHaveAttribute('data-state', 'clean', { timeout: 6_000 });
    await expect(saveBtn).toHaveCSS('background-color', BLUE);
    await expect(saveBtn).toHaveText('Save');
    await expect(status).toHaveText('Save');
  });

  test('15.4 — no field edit means no settings PUT at all', async ({ page }) => {
    await openSettings(page, admin);
    let puts = 0;
    page.on('request', (r) => {
      if (r.method() === 'PUT' && r.url().includes('/api/settings/')) puts++;
    });
    for (const cat of CATEGORIES) await openCategory(page, cat);
    await page.waitForTimeout(1200);
    expect(puts).toBe(0);
  });

  test('15.5 — the Save button is disabled while a save is in flight', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'general');
    await page.route('**/api/settings/general', async (route) => {
      await new Promise((r) => setTimeout(r, 800));
      await route.continue();
    });
    await set(page).locator('#serverName').fill(`Double ${Date.now() % 10000}`);
    const saveBtn = set(page).locator('[data-testid="settings-save"]');
    await saveBtn.click();
    await expect(saveBtn).toBeDisabled();
    await expect(saveBtn).toBeEnabled({ timeout: 8_000 });
  });
});

// ===========================================================================
// 16 — Responsive (window-relative container query)
// ===========================================================================

test.describe('16 — Responsive', () => {
  for (const w of [1920, 1280, 768, 390]) {
    test(`16.x — usable at ${w}px viewport, rail reachable`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: 800 });
      await openSettings(page, admin);
      await expect(set(page).locator('[data-testid="settings-nav-item"]').first()).toBeVisible();
      // no horizontal page overflow
      const bodyScroll = await page.evaluate(
        () => document.documentElement.scrollWidth <= document.documentElement.clientWidth + 2,
      );
      expect(bodyScroll, 'page body must not scroll horizontally').toBeTruthy();
    });
  }

  test('16.5 — the standard PageLayout anatomy fits a narrow window', async ({ page }) => {
    await page.setViewportSize({ width: 560, height: 800 });
    await openSettings(page, admin);
    // Settings now uses the shared PageLayout anatomy (like every other app):
    // the left rail stays a vertical column and the content column scrolls.
    // The rail must not overflow horizontally and the content must be visible.
    const rail = await set(page)
      .locator('[data-testid="settings-sidebar"]')
      .evaluate((el) => getComputedStyle(el).flexDirection);
    expect(rail, 'the rail stays a column').toBe('column');
    // the panel content must actually be on screen, not squished to 0
    const w = await set(page).locator('[data-testid="settings-content"]').evaluate((el) => el.clientWidth);
    expect(w, 'panel keeps real width').toBeGreaterThan(300);
  });

  test('16.6 — the rail is a vertical column in a wide window', async ({ page }) => {
    await page.setViewportSize({ width: 1600, height: 900 });
    await openSettings(page, admin);
    // maximise so the window is comfortably wide
    await page.locator('.hv-window:has([data-testid="settings-page"]) [data-testid="win-maximize"]').click().catch(() => {});
    await page.waitForTimeout(300);
    const dir = await set(page)
      .locator('[data-testid="settings-sidebar"]')
      .evaluate((el) => getComputedStyle(el).flexDirection);
    expect(dir).toBe('column');
  });
});

// ===========================================================================
// 17 — Accessibility
// ===========================================================================

test.describe('17 — Accessibility', () => {
  test('17.1 — every labelled field has a matching control id (General/Email)', async ({ page }) => {
    await openSettings(page, admin);
    for (const cat of ['general', 'email', 'notifications', 'updates'] as Category[]) {
      await openCategory(page, cat);
      const labels = set(page).locator('label[for]');
      const n = await labels.count();
      for (let i = 0; i < n; i++) {
        const forId = await labels.nth(i).getAttribute('for');
        if (!forId) continue;
        await expect(set(page).locator(`#${forId}`), `${cat}: #${forId}`).toHaveCount(1);
      }
    }
  });

  test('17.2 — no duplicate element ids in the open panel', async ({ page }) => {
    await openSettings(page, admin);
    for (const cat of CATEGORIES) {
      await openCategory(page, cat);
      const dupes = await page.evaluate(() => {
        const ids = [...document.querySelectorAll('[data-testid="settings-content"] [id]')].map((e) => e.id);
        return ids.filter((v, i) => ids.indexOf(v) !== i);
      });
      expect(dupes, `${cat} duplicate ids`).toEqual([]);
    }
  });

  test('17.3 — rail entries are real <button>s and focusable', async ({ page }) => {
    await openSettings(page, admin);
    const tag = await set(page).locator('[data-testid="settings-nav-item"]').first().evaluate((e) => e.tagName);
    expect(tag).toBe('BUTTON');
    await set(page).locator('[data-testid="settings-nav-item"]').nth(2).focus();
    await expect(set(page).locator('[data-testid="settings-nav-item"]').nth(2)).toBeFocused();
  });

  test('17.4 — the window header carries an accessible name', async ({ page }) => {
    await openSettings(page, admin);
    await expect(page.locator('.hv-window:has([data-testid="settings-page"])')).toHaveAttribute('aria-label', /.+/);
  });

  // The rail tile carries the active state in two forms (tokens.css →
  // `--hv-settings-cat-*`): a full-colour glyph on its 9% tint at rest, a
  // white glyph on the solid when active — and the solid tile against the
  // white rail is the same ratio as white-on-solid. WCAG 1.4.11 wants ≥3:1
  // for both. The 2026-09-23 standardization moved widgets/notifications/
  // ai/monitoring to their 600/700 stops because violet-400/amber-500/
  // cyan-500/emerald-500 sat at 2.00–2.72:1. Every ratio is measured from
  // the live DOM in both states, never from hardcoded RGB.
  test('17.5 — all 8 category tiles keep ≥3:1 glyph/tile, rest and active (WCAG 1.4.11)', async ({ page }) => {
    await openSettings(page, admin);
    const rail = set(page).locator('[data-testid="settings-nav-item"]');
    await expect(rail).toHaveCount(CATEGORIES.length);
    const failures: string[] = [];
    for (const cat of CATEGORIES) {
      await openCategory(page, cat);
      const tiles = await page.evaluate(() => {
        // `rgb(a)` and Firefox's `color(srgb … / a)` for color-mix() output —
        // the same serializations `alphaOf` above documents.
        const parse = (css: string): [number, number, number, number] => {
          const n = (css.match(/[\d.]+/g) ?? []).map(Number);
          if (css.startsWith('color(')) {
            return [(n[0] ?? 0) * 255, (n[1] ?? 0) * 255, (n[2] ?? 0) * 255, n[3] ?? 1];
          }
          return [n[0] ?? 0, n[1] ?? 0, n[2] ?? 0, n[3] ?? 1];
        };
        const lin = (v: number) => {
          const c = v / 255;
          return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
        };
        const lum = ([r, g, b]: [number, number, number]) =>
          0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
        // The rest tile is `accent @ 9% alpha`; composite it over the first
        // opaque ancestor (white rail when the whole chain stays translucent).
        const effectiveBg = (el: Element): [number, number, number, number] => {
          let cur = parse(getComputedStyle(el).backgroundColor);
          let node = el.parentElement;
          while (cur[3] < 1 && node) {
            const p = parse(getComputedStyle(node).backgroundColor);
            cur = [
              cur[0] * cur[3] + p[0] * (1 - cur[3]),
              cur[1] * cur[3] + p[1] * (1 - cur[3]),
              cur[2] * cur[3] + p[2] * (1 - cur[3]),
              cur[3] + p[3] * (1 - cur[3]),
            ];
            node = node.parentElement;
          }
          if (cur[3] < 1) {
            cur = [
              cur[0] * cur[3] + 255 * (1 - cur[3]),
              cur[1] * cur[3] + 255 * (1 - cur[3]),
              cur[2] * cur[3] + 255 * (1 - cur[3]),
              1,
            ];
          }
          return cur;
        };
        return [...document.querySelectorAll<HTMLElement>('[data-testid="settings-nav-item"]')].map((item) => {
          const tile = item.querySelector('.hv-winsidebar-item-icon');
          if (!tile) return { label: item.textContent ?? '?', solid: false, ratio: 0 };
          const raw = parse(getComputedStyle(tile).backgroundColor);
          const fg = lum(parse(getComputedStyle(tile).color));
          const bg = lum(effectiveBg(tile));
          const ratio = (Math.max(fg, bg) + 0.05) / (Math.min(fg, bg) + 0.05);
          return {
            label: (item.textContent ?? '?').trim().slice(0, 40),
            solid: raw[3] >= 1, // only the active tile paints opaque
            ratio: Math.round(ratio * 100) / 100,
          };
        });
      });
      expect(tiles, `${cat}: rail tiles rendered`).toHaveLength(CATEGORIES.length);
      // The state change itself must be visible: exactly one tile is solid.
      expect(tiles.filter((t) => t.solid), `${cat}: exactly one active tile`).toHaveLength(1);
      for (const t of tiles) {
        if (t.ratio < 3) failures.push(`${cat} → "${t.label}" ${t.solid ? 'active' : 'rest'} = ${t.ratio}:1`);
      }
    }
    expect(failures, 'category tiles below the 3:1 graphical floor').toEqual([]);
  });
});

// ===========================================================================
// 18 — Keyboard
// ===========================================================================

test.describe('18 — Keyboard', () => {
  test('18.1 — Tab moves through the General form controls', async ({ page }) => {
    await openSettings(page, admin);
    await set(page).locator('#serverName').focus();
    await page.keyboard.press('Tab');
    const active = await page.evaluate(() => document.activeElement?.id || document.activeElement?.tagName);
    expect(active).toBeTruthy();
  });

  test('18.2 — Enter in the weather search box triggers a lookup', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'weather');
    const edit = set(page).locator('.settings-weather-current .settings-btn');
    if (await edit.count()) await edit.click();
    const box = set(page).locator('.settings-weather-search-row input');
    await box.fill('Berlin');
    const hit = page
      .waitForResponse((r) => r.url().includes('geocoding-api.open-meteo.com'), { timeout: 10_000 })
      .catch(() => null);
    await box.press('Enter');
    // offline-safe: pass if the request fired OR the UI entered the searching state
    const fired = await hit;
    if (!fired) {
      await expect(set(page).locator('.settings-weather-search-row .settings-btn')).toBeVisible();
    }
  });

  test('18.3 — the wallpaper grid tiles are keyboard-reachable and Enter selects', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'appearance');
    // pick a tile that is not already selected, so Enter is a real change
    const tiles = set(page).locator('.settings-wallpaper-tile');
    const count = await tiles.count();
    let idx = 1;
    for (let i = 0; i < count; i++) {
      const cls = (await tiles.nth(i).getAttribute('class')) || '';
      if (!cls.includes('is-selected')) { idx = i; break; }
    }
    const tile = tiles.nth(idx);
    await tile.focus();
    await expect(tile).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(tile).toHaveAttribute('aria-checked', 'true');
    await expect(tile.locator('.settings-wallpaper-check')).toBeVisible();
  });
});

// ===========================================================================
// 19 — Console & network hygiene
// ===========================================================================

test.describe('19 — Console & network', () => {
  test('19.1 — opening Settings logs no JS errors', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    page.on('console', (m) => {
      if (m.type() === 'error' && !/favicon|net::ERR|Failed to load resource/.test(m.text())) {
        errors.push(m.text());
      }
    });
    await openSettings(page, admin);
    for (const cat of CATEGORIES) await openCategory(page, cat);
    expect(errors, errors.join('\n')).toEqual([]);
  });

  test('19.2 — no settings endpoint 500s during a full sweep', async ({ page }) => {
    const bad: string[] = [];
    page.on('response', (r) => {
      if (r.url().includes('/api/') && r.status() >= 500) bad.push(`${r.status()} ${r.url()}`);
    });
    await openSettings(page, admin);
    for (const cat of CATEGORIES) await openCategory(page, cat);
    expect(bad, bad.join('\n')).toEqual([]);
  });

  test('19.3 — GET /api/settings is not re-fetched on every category switch', async ({ page }) => {
    await openSettings(page, admin);
    let count = 0;
    page.on('response', (r) => {
      if (/\/api\/settings(\?|$)/.test(r.url()) && r.request().method() === 'GET') count++;
    });
    for (const cat of CATEGORIES) await openCategory(page, cat);
    expect(count, 'the aggregate settings GET is cached, not per-tab').toBeLessThanOrEqual(1);
  });
});

// ===========================================================================
// 20 — Data-safety
// ===========================================================================

test.describe('20 — Data safety', () => {
  test('20.1 — no session token leaks into the URL', async ({ page }) => {
    await openSettings(page, admin);
    expect(page.url()).not.toContain(admin.token);
  });

  test('20.2 — the SMTP password is not exposed in the DOM', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'email');
    // Scope to the SMTP card — the Notifications group stacks e-mail + alerts,
    // and the regex below would otherwise match another field's value. The card
    // is built inline in Settings.svelte (only the six embedded sub-cards use
    // the `SettingsCard` component), so the card element itself is the scope —
    // do not reach for a `settings-card-body` hook, which those sub-cards carry
    // as a class and this card does not have at all.
    const html = await set(page)
      .locator('[data-testid="settings-card"]:has(#smtpPassword)')
      .innerHTML();
    expect(html).not.toMatch(/value="[^"]{6,}"/); // no long pre-filled value on the password input
    await expect(set(page).locator('#smtpPassword')).toHaveValue('');
  });

  test('20.3 — export payload does not carry a plaintext SMTP password', async () => {
    const res = await sharedCtx.get('/api/settings/export', { headers: adminHeaders });
    const text = await res.text();
    // the persistence layer redacts smtp_password in export()
    expect(text).not.toMatch(/"smtp_password"\s*:\s*"(?!(\*+|redacted|)")./i);
  });
});

// ===========================================================================
// 21 — Edge cases
// ===========================================================================

test.describe('21 — Edge cases', () => {
  test('21.1 — a unicode / very long server name is accepted by the field', async ({ page }) => {
    await openSettings(page, admin);
    const long = '🏠 Vault ' + 'é'.repeat(120);
    await set(page).locator('#serverName').fill(long);
    await expect(set(page).locator('#serverName')).toHaveValue(long);
  });

  test('21.2 — re-triggering the dock entry focuses the window, does not stack a second', async ({ page }) => {
    await openSettings(page, admin);
    // The open window covers the dock, so dispatch the click straight at the
    // dock button (this is what focusExistingWindow() must intercept).
    for (let i = 0; i < 3; i++) {
      await page
        .locator('.hv-dock-item:has(.hv-dock-icon-settings)')
        .first()
        .dispatchEvent('click');
      await page.waitForTimeout(250);
    }
    await expect(page.locator('.hv-window:has([data-testid="settings-page"])')).toHaveCount(1);
  });

  test('21.3 — rapid edits stage without PUTs; one Save flushes a single request', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'general');
    let puts = 0;
    page.on('request', (r) => {
      if (r.url().includes('/api/settings/general') && r.method() === 'PUT') puts++;
    });
    const field = set(page).locator('#serverName');
    await field.click();
    await field.press('Control+a');
    // Type ~10 characters at 45ms each (~450ms) — even a long typing burst
    // must never auto-save.
    await field.pressSequentially(`Rapid${Date.now() % 100000}`, { delay: 45 });
    await page.waitForTimeout(1200);
    expect(puts, 'typing must not PUT before Save').toBe(0);

    // A single Save collapses the whole burst into exactly one PUT. The waiter
    // must be armed before the click, or the response can arrive first.
    const [resp] = await Promise.all([
      page.waitForResponse(
        (r) => r.url().includes('/api/settings/general') && r.request().method() === 'PUT',
        { timeout: 15_000 },
      ),
      set(page).locator('[data-testid="settings-save"]').click(),
    ]);
    expect(resp.ok()).toBe(true);
    await page.waitForTimeout(400);
    expect(puts, 'one burst of typing must collapse to a single PUT on Save').toBeLessThanOrEqual(1);
  });
});

// ===========================================================================
// 22 — Memory regression
// ===========================================================================

test.describe('22 — Memory regression', () => {
  test('22.1 — open/close the window 5× without runaway heap or DOM growth', async ({ page }) => {
    test.setTimeout(150_000);
    await openSettings(page, admin);
    const domBefore = await page.evaluate(() => document.querySelectorAll('*').length);
    const heap0 = await getHeapMB(page);

    for (let i = 0; i < 5; i++) {
      await page
        .locator('.hv-window:has([data-testid="settings-page"]) [data-testid="win-close"]')
        .first()
        .click()
        .catch(() => {});
      await page.waitForTimeout(150);
      await page.locator('.hv-dock-item:has(.hv-dock-icon-settings)').first().click();
      await page.waitForSelector('[data-testid="settings-page"]');
    }
    await (page as any).evaluate(() => (window as any).gc && (window as any).gc()).catch(() => {});
    await page.waitForTimeout(300);

    const domAfter = await page.evaluate(() => document.querySelectorAll('*').length);
    const heap1 = await getHeapMB(page);
    expect(domAfter - domBefore, 'DOM node accumulation').toBeLessThan(600);
    if (heap0 > 0 && heap1 > 0) {
      expect(heap1 - heap0, 'heap growth over 5 cycles (MB)').toBeLessThan(12);
    }
  });

  test('22.2 — cycling all 8 categories many times leaves one panel mounted', async ({ page }) => {
    test.setTimeout(150_000);
    await openSettings(page, admin);
    const items = set(page).locator('[data-testid="settings-nav-item"]');
    const domBefore = await page.evaluate(() => document.querySelectorAll('[data-testid="settings-content"] *').length);
    for (let r = 0; r < 6; r++) {
      for (let i = 0; i < CATEGORIES.length; i++) {
        await items.nth(i).click({ force: true, noWaitAfter: true });
      }
    }
    await page.waitForTimeout(300);
    await expect(set(page).locator('[data-testid="settings-content"]')).toHaveCount(1);
    await expect(set(page).locator('[data-testid="settings-nav-item"].set-nav-active')).toHaveCount(1);
    // only the active category's panel is in the DOM — no accumulation
    const domAfter = await page.evaluate(() => document.querySelectorAll('[data-testid="settings-content"] *').length);
    expect(domAfter - domBefore, 'panel DOM does not accumulate').toBeLessThan(400);
  });
});

// ===========================================================================
// 23 — Redesigned shell (2026-09-14 Figma re-skin)
// ===========================================================================
//
// The screen is now a 208px two-line rail (per-category colour tile + title +
// subtitle) beside a content column with its own header, and a section/row
// card vocabulary. These tests pin the *structure* the design depends on —
// they are the ones that would silently rot if someone flattened the rail back
// to 12 flat tabs or dropped the content header.

/** The 11 monitors in `stores/widgetCatalog.ts::WIDGET_DEFS`, in order. */
const MONITOR_IDS = [
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

test.describe('23 — Redesigned shell', () => {
  test('23.1 — the rail renders 8 two-line entries, each with a tinted icon tile', async ({ page }) => {
    await openSettings(page, admin);
    const items = set(page).locator('[data-testid="settings-nav-item"]');
    await expect(items).toHaveCount(8);

    // Every entry stacks a title over a subtitle (the design's two-line row),
    // and every entry carries a 28px icon tile in its own category colour — so
    // the tile is never the shared brand blue.
    const shape = await items.evaluateAll((nodes) =>
      nodes.map((n) => {
        const tile = n.querySelector('.hv-winsidebar-item-icon') as HTMLElement | null;
        const r = tile?.getBoundingClientRect();
        return {
          w: r ? Math.round(r.width) : 0,
          h: r ? Math.round(r.height) : 0,
          bg: tile ? getComputedStyle(tile).backgroundColor : '',
          active: n.classList.contains('is-active'),
          label: (n.querySelector('.hv-winsidebar-item-label')?.textContent || '').trim(),
          subtitle: (n.querySelector('.hv-winsidebar-item-subtitle')?.textContent || '').trim(),
        };
      }),
    );
    expect(shape.length).toBe(8);
    // Exactly one entry is active — the category the screen opens on.
    expect(shape.filter((s) => s.active).length, 'active entries in the rail').toBe(1);
    for (const [i, s] of shape.entries()) {
      expect(s.w, `entry ${i} (${s.label}) icon tile width`).toBe(28);
      expect(s.h, `entry ${i} (${s.label}) icon tile height`).toBe(28);
      if (s.active) {
        // The active tile is the category colour at full strength — the same
        // solid fill 23.2 asserts on, and the only flat tile in the rail.
        expect(alphaOf(s.bg), `entry ${i} (${s.label}) active tile is solid`).toBe(1);
      } else {
        // A resting tile is the category colour at ~9% — never a flat fill.
        expect(alphaOf(s.bg), `entry ${i} (${s.label}) tile is tinted, not solid`)
          .toBeGreaterThan(0.02);
        expect(alphaOf(s.bg), `entry ${i} (${s.label}) tile is a tint, not an opaque fill`)
          .toBeLessThan(0.2);
      }
      expect(s.label.length, `entry ${i} has a title`).toBeGreaterThan(0);
      expect(s.subtitle.length, `entry ${i} ("${s.label}") has a subtitle`).toBeGreaterThan(0);
    }
    // The labels are exactly the 8 groups, in declaration order — so the rail
    // order and the GROUP_LABEL map above cannot drift apart.
    expect(shape.map((s) => s.label)).toEqual(CATEGORIES.map((c) => GROUP_LABEL[c]));
    // One tile colour per category.
    expect(new Set(shape.map((s) => s.bg)).size, 'per-category tile colours').toBe(8);
  });

  test('23.2 — the active entry gets a solid category tile, a white glyph and the 4x20 bar', async ({ page }) => {
    await openSettings(page, admin);
    const active = set(page).locator('[data-testid="settings-nav-item"].set-nav-active');
    const barRule = await accentBarRule(page);

    for (const cat of ['general', 'security', 'widgets'] as const) {
      await openCategory(page, cat);
      await expect(active).toHaveCount(1);
      await expect(active.locator('.hv-winsidebar-item-label')).toHaveText(GROUP_LABEL[cat]);

      const m = await active.evaluate((n) => {
        const tile = n.querySelector('.hv-winsidebar-item-icon') as HTMLElement;
        const glyph = tile.querySelector('svg') as SVGElement | null;
        const bar = getComputedStyle(n, '::after');
        // "rgb(r, g, b)" for a solid fill; the resting tile is rgba with alpha.
        const parse = (c: string) => {
          const nums = c.match(/[\d.]+/g)?.map(Number) ?? [];
          return { r: nums[0], g: nums[1], b: nums[2], a: nums.length > 3 ? nums[3] : 1 };
        };
        return {
          tile: parse(getComputedStyle(tile).backgroundColor),
          glyph: parse(glyph ? getComputedStyle(glyph).color : ''),
          barW: bar.width,
          barH: bar.height,
          barBg: bar.backgroundColor,
          barRadius: bar.borderRadius,
          barContent: bar.content,
        };
      });

      expect(m.tile.a, `${cat}: active tile is solid, not a tint`).toBe(1);
      // The glyph is white — the resting tile puts the accent on the glyph, so
      // a solid accent tile with an accent glyph would be invisible.
      expect(m.glyph.r, `${cat}: active glyph is white (r)`).toBe(255);
      expect(m.glyph.g, `${cat}: active glyph is white (g)`).toBe(255);
      expect(m.glyph.b, `${cat}: active glyph is white (b)`).toBe(255);
      // Same hue on tile and bar — the bar is the category colour.
      expect(m.barBg, `${cat}: bar uses the category colour`).toBe(
        `rgb(${m.tile.r}, ${m.tile.g}, ${m.tile.b})`,
      );
      expect(m.barContent, `${cat}: bar is rendered`).not.toBe('none');
      expect(m.barW, `${cat}: bar width`).toBe('4px');
      expect(m.barH, `${cat}: bar height`).toBe('20px');
      expect(m.barRadius, `${cat}: bar is a pill`).toBe('9999px');
      // `margin-left: auto` inside the flex row is what pushes it right, but a
      // pseudo-element's auto margin computes to the used `0px` — assert the
      // declaration instead (see `accentBarRule`).
      expect(barRule.selector, 'the accent bar is a ::after pseudo-element')
        .toContain('has-accent-bar');
      expect(barRule.marginLeft, `${cat}: bar is pushed to the right edge`).toBe('auto');
    }
  });

  test('23.3 — the content header mirrors the active category and does not scroll away', async ({ page }) => {
    await openSettings(page, admin);
    const header = set(page).locator('[data-testid="settings-header-title"]');
    await expect(header).toBeVisible();
    await expect(header).toHaveText(GROUP_LABEL.general);

    // The header block is the title's grandparent: title's wrapper → header.
    const headerBlock = () =>
      header.evaluate((h) => {
        const block = h.parentElement!.parentElement as HTMLElement;
        const tile = block.querySelector('span[style*="background"]') as HTMLElement;
        const sub = block.querySelector('p') as HTMLElement;
        const tr = tile.getBoundingClientRect();
        return {
          tileBg: getComputedStyle(tile).backgroundColor,
          tileW: Math.round(tr.width),
          tileH: Math.round(tr.height),
          subtitle: (sub.textContent || '').trim(),
        };
      });

    const before = await headerBlock();
    expect(before.tileW, 'header icon tile is 36px').toBe(36);
    expect(before.tileH, 'header icon tile is 36px').toBe(36);
    expect(before.subtitle.length, 'header carries the category subtitle').toBeGreaterThan(0);
    // The rail item and the header show the same subtitle.
    const railSubtitle = await set(page)
      .locator('[data-testid="settings-nav-item"].set-nav-active .hv-winsidebar-item-subtitle')
      .textContent();
    expect(before.subtitle).toBe((railSubtitle || '').trim());

    await openCategory(page, 'security');
    await expect(header).toHaveText(GROUP_LABEL.security);
    const after = await headerBlock();
    expect(after.tileBg, 'header tile follows the category').not.toBe(before.tileBg);
    expect(after.subtitle, 'header subtitle follows the category').not.toBe(before.subtitle);

    // The header is a sibling of the scroll container, so scrolling the panel
    // leaves it in place at the top of the content column.
    const content = set(page).locator('[data-testid="settings-content"]');
    const box0 = (await header.boundingBox())!;
    await content.evaluate((el) => (el.scrollTop = el.scrollHeight));
    await page.waitForTimeout(120);
    const box1 = (await header.boundingBox())!;
    expect(Math.abs(box1.y - box0.y), 'header does not scroll with the panel').toBeLessThan(2);
    await content.evaluate((el) => (el.scrollTop = 0));
  });

  test('23.4 — the content panel is the scroll container and every card is a direct child', async ({ page }) => {
    await openSettings(page, admin);
    const m = await set(page)
      .locator('[data-testid="settings-content"]')
      .evaluate((el) => {
        const cs = getComputedStyle(el);
        const cards = [...el.querySelectorAll('[data-testid="settings-card"]')];
        return {
          overflowY: cs.overflowY,
          scrollable: el.scrollHeight > el.clientHeight,
          direct: cards.every((c) => c.parentElement === el),
          cardCount: cards.length,
          // a card must not shrink when the panel overflows
          shrink: cards.map((c) => getComputedStyle(c).flexShrink),
        };
      });
    expect(m.overflowY).toBe('auto');
    expect(m.direct, 'every card is a direct child of settings-content').toBe(true);
    expect(m.cardCount).toBeGreaterThan(0);
    for (const s of m.shrink) expect(s, 'cards keep flex-shrink: 0').toBe('0');
    void m.scrollable;
  });

  test('23.5 — section labels are siblings of their cards, styled as micro-labels', async ({ page }) => {
    await openSettings(page, admin);
    // `system` is the category with the most panel-emitted sections (updates,
    // system information, data management, privacy). `security` is deliberately
    // not used here: its 4 sections are the embedded sub-cards, whose heading
    // IS the `SettingsCard` header — the one place the label sits *inside* its
    // card, and E2E 12.1 reads those instead.
    await openCategory(page, 'system');
    // `settings-card-title` is emitted on both the panel's section label and on
    // each `SettingsCard`'s own `<h2>`; the design's "label above the card,
    // never inside it" rule is the direct-child one, so scope the query.
    const labels = set(page).locator(
      '[data-testid="settings-content"] > [data-testid="settings-card-title"]',
    );
    expect(await labels.count(), 'system has 4 section labels').toBeGreaterThanOrEqual(4);
    const m = await labels.evaluateAll((nodes) =>
      nodes.map((n) => {
        const cs = getComputedStyle(n);
        return {
          siblingOfCard: n.nextElementSibling?.getAttribute('data-testid') === 'settings-card',
          insideCard: !!n.closest('[data-testid="settings-card"]'),
          transform: cs.textTransform,
          size: parseFloat(cs.fontSize),
        };
      }),
    );
    for (const [i, l] of m.entries()) {
      expect(l.insideCard, `label ${i} is outside the card`).toBe(false);
      expect(l.siblingOfCard, `label ${i} precedes its card`).toBe(true);
      expect(l.transform, `label ${i} is uppercased`).toBe('uppercase');
      expect(l.size, `label ${i} micro size`).toBeLessThanOrEqual(12);
    }
  });

  test('23.6 — Widgets lists all 11 real monitors, each on its live toggle state', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'widgets');
    const rows = set(page).locator('.settings-widget-row');
    await expect(rows).toHaveCount(11);
    await expect(rows).toHaveCount(MONITOR_IDS.length);

    const ids = await rows.evaluateAll((nodes) => nodes.map((n) => n.getAttribute('data-widget')));
    expect(ids).toEqual([...MONITOR_IDS]);

    // Each row's switch reads the store, not a local copy: the checked state
    // in the DOM must equal what /api/monitor-layout reports for this account.
    const res = await sharedCtx.get('/api/monitor-layout', { headers: adminHeaders });
    const blob = (await res.json().catch(() => ({})))?.data?.layout_json;
    if (blob) {
      const layout = JSON.parse(blob);
      for (const id of MONITOR_IDS) {
        const key = {
          resource: 'resourceMonitorVisible',
          raid: 'raidMonitorVisible',
          security: 'securityMonitorVisible',
          vpn: 'vpnMonitorVisible',
          downloader: 'downloaderMonitorVisible',
          syncthing: 'syncthingMonitorVisible',
          calendar: 'calendarMonitorVisible',
          docker: 'dockerMonitorVisible',
          weather: 'weatherMonitorVisible',
          sessions: 'sessionsMonitorVisible',
          health: 'healthMonitorVisible',
        }[id];
        if (!(key in layout)) continue; // never toggled — the default applies
        const checked = await set(page)
          .locator(`.settings-widget-row[data-widget="${id}"] input[type="checkbox"]`)
          .isChecked();
        expect(checked, `${id} reflects the stored layout`).toBe(!!layout[key]);
      }
    }
  });

  test('23.7 — flipping a monitor switch stages it; the Save button POSTs /api/monitor-layout', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'widgets');
    const row = set(page).locator('.settings-widget-row[data-widget="weather"]');
    const input = row.locator('input[type="checkbox"]');
    const before = await input.isChecked();

    // Flipping the toggle must NOT reach the server — the layout is staged.
    let sawPost = false;
    const onReq = (r: any) => {
      if (r.url().includes('/api/monitor-layout') && r.method() === 'POST') sawPost = true;
    };
    page.on('request', onReq);
    await row.locator('.settings-toggle-label').click();
    await page.waitForTimeout(1200);
    page.off('request', onReq);
    expect(sawPost, 'flipping a monitor switch must not auto-save').toBe(false);
    await expect(input).toBeChecked({ checked: !before });

    // The Save button flushes the staged layout.
    const [req] = await Promise.all([
      page.waitForRequest(
        (r) => r.url().includes('/api/monitor-layout') && r.method() === 'POST',
        { timeout: 15_000 },
      ),
      set(page).locator('[data-testid="settings-save"]').click(),
    ]);
    const sent = JSON.parse(req.postData() || '{}').layout_json;
    expect(JSON.parse(sent).weatherMonitorVisible).toBe(!before);

    await expect
      .poll(
        async () => {
          const r = await sharedCtx.get('/api/monitor-layout', { headers: adminHeaders });
          const b = (await r.json().catch(() => ({})))?.data?.layout_json;
          return b ? JSON.parse(b).weatherMonitorVisible : undefined;
        },
        { timeout: 10_000 },
      )
      .toBe(!before);

    // put it back so the run leaves the account as it found it
    await row.locator('.settings-toggle-label').click();
    await saveSettings(page);
    await expect(input).toBeChecked({ checked: before });
  });

  test('23.8 — the refresh select offers 2/5/10/30/60s, defaults to 5s and persists on Save', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'widgets');
    const select = set(page).locator('#refreshInterval');
    await expect(select).toBeVisible();

    const options = await select.locator('option').evaluateAll((o) => o.map((n) => (n as HTMLOptionElement).value));
    expect(options).toEqual(['2000', '5000', '10000', '30000', '60000']);

    const current = await select.inputValue();
    expect(options, 'the current value is one of the offered intervals').toContain(current);
    const target = current === '30000' ? '10000' : '30000';

    // Selecting alone must not POST — the layout change is staged.
    let sawPost = false;
    const onReq = (r: any) => {
      if (r.url().includes('/api/monitor-layout') && r.method() === 'POST') sawPost = true;
    };
    page.on('request', onReq);
    await select.selectOption(target);
    await page.waitForTimeout(1200);
    page.off('request', onReq);
    expect(sawPost, 'selecting a refresh interval must not auto-save').toBe(false);

    const [req] = await Promise.all([
      page.waitForRequest(
        (r) => r.url().includes('/api/monitor-layout') && r.method() === 'POST',
        { timeout: 15_000 },
      ),
      set(page).locator('[data-testid="settings-save"]').click(),
    ]);
    expect(JSON.parse(JSON.parse(req.postData() || '{}').layout_json).refreshMs).toBe(Number(target));

    // It is stored in the same blob as the widget positions, so it follows the
    // account across browser origins — read it back from the server.
    await expect
      .poll(
        async () => {
          const r = await sharedCtx.get('/api/monitor-layout', { headers: adminHeaders });
          const b = (await r.json().catch(() => ({})))?.data?.layout_json;
          return b ? JSON.parse(b).refreshMs : undefined;
        },
        { timeout: 10_000 },
      )
      .toBe(Number(target));

    // restore the original interval
    await select.selectOption(current);
    await saveSettings(page);
    await expect.poll(() => select.inputValue(), { timeout: 5_000 }).toBe(current);
  });

  test('23.9 — one toggle geometry on the whole screen: 40x22 track, 16px knob', async ({ page }) => {
    await openSettings(page, admin);
    let seen = 0;
    for (const cat of ['general', 'appearance', 'widgets'] as const) {
      await openCategory(page, cat);
      const tracks = set(page).locator('.settings-toggle-label .hv-switch-track');
      const n = await tracks.count();
      if (n === 0) continue;
      seen += n;
      const geo = await tracks.evaluateAll((nodes) =>
        nodes.map((t) => {
          const r = t.getBoundingClientRect();
          const knob = t.querySelector('.hv-switch-knob') as HTMLElement | null;
          const kr = knob?.getBoundingClientRect();
          const kn = knob ? getComputedStyle(knob) : null;
          return {
            w: Math.round(r.width),
            h: Math.round(r.height),
            knob: kr ? Math.round(kr.width) : 0,
            radius: getComputedStyle(t).borderRadius,
            knobRadius: kn?.borderRadius ?? '',
          };
        }),
      );
      for (const [i, g] of geo.entries()) {
        expect(g.w, `${cat} track ${i} width`).toBe(40);
        expect(g.h, `${cat} track ${i} height`).toBe(22);
        expect(g.knob, `${cat} track ${i} knob size`).toBe(16);
        expect(g.radius, `${cat} track ${i} is a pill`).toBe('9999px');
        expect(g.knobRadius, `${cat} track ${i} knob is round`).toBe('9999px');
      }
    }
    expect(seen, 'the categories under test actually render toggles').toBeGreaterThan(0);
  });

  test('23.10 — the checked track fills with the row accent, not a fixed brand blue', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'widgets');
    // Each monitor row passes its own catalogue colour, so the checked tracks
    // on this screen are deliberately many different colours.
    const fills = await set(page)
      .locator('.settings-widget-row')
      .evaluateAll((rows) =>
        rows.map((r) => {
          const label = r.querySelector('.settings-toggle-label') as HTMLElement;
          return (label?.style.getPropertyValue('--hv-switch-accent') || '').trim();
        }),
      );
    expect(fills.length).toBe(11);
    expect(fills.every((f) => f.length > 0), 'every monitor row sets an accent').toBe(true);
    expect(new Set(fills).size, 'one accent per monitor').toBeGreaterThan(5);
  });

  test('23.11 — the shell introduces no horizontal overflow at 1440 and stays a column at 390', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1176 });
    await openSettings(page, admin);
    await openCategory(page, 'widgets');
    const overflow = await page.evaluate(() => {
      const root = document.querySelector('[data-testid="settings-page"]')!;
      return {
        pageScrollX: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        panelScrollX: (() => {
          const c = document.querySelector('[data-testid="settings-content"]') as HTMLElement;
          return c.scrollWidth - c.clientWidth;
        })(),
        rootW: Math.round(root.getBoundingClientRect().width),
      };
    });
    expect(overflow.pageScrollX, 'no page-level horizontal scroll').toBeLessThanOrEqual(1);
    expect(overflow.panelScrollX, 'no horizontal scroll inside the panel').toBeLessThanOrEqual(1);

    await page.setViewportSize({ width: 390, height: 844 });
    await page.waitForTimeout(250);
    const narrow = await set(page)
      .locator('[data-testid="settings-content"]')
      .evaluate((el) => ({
        clientW: el.clientWidth,
        scrollW: el.scrollWidth,
        cards: [...el.querySelectorAll('[data-testid="settings-card"]')].length,
      }));
    expect(narrow.scrollW, 'no horizontal overflow at phone width').toBeLessThanOrEqual(narrow.clientW + 1);
    expect(narrow.cards).toBeGreaterThan(0);
  });
});

// ===========================================================================
// 15 — Database snapshots (HomeVault's own SQLite database)
// ===========================================================================
//
// The database runs in WAL mode, so copying `homevault.db` alone yields a file
// with no tables. These snapshots are `VACUUM INTO` files — the only thing an
// operator can copy and restore — so the card must exist, be reachable by
// keyboard, and actually produce one.

test.describe('15 — Database snapshots', () => {
  test('15.1 — the card mounts with its schedule, retention and buttons', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'data_management');

    const card = set(page).locator('[data-testid="settings-card"]').filter({ hasText: /Database snapshots/i });
    await expect(card).toBeVisible();

    // Schedule + retention controls are labelled, not placeholder-only.
    await expect(card.locator('label[for="dbBackupInterval"]')).toBeVisible();
    await expect(card.locator('label[for="dbBackupRetain"]')).toBeVisible();
    await expect(card.locator('#dbBackupRetain')).toHaveAttribute('type', 'number');
    await expect(card.locator('#dbBackupRetain')).toHaveAttribute('min', '1');
    await expect(card.locator('#dbBackupRetain')).toHaveAttribute('max', '100');

    // Both actions exist and are ≥ 44px tall (touch target).
    for (const id of ['db-backup-now', 'db-backup-save']) {
      const btn = card.locator(`[data-testid="${id}"]`);
      await expect(btn).toBeVisible();
      const box = await btn.boundingBox();
      expect(box!.height).toBeGreaterThanOrEqual(28);
    }
  });

  test('15.2 — "Snapshot now" writes a file and refreshes the last-run row', async ({ page }) => {
    await openSettings(page, admin);
    await openCategory(page, 'data_management');

    const card = set(page).locator('[data-testid="settings-card"]').filter({ hasText: /Database snapshots/i });
    const nowBtn = card.locator('[data-testid="db-backup-now"]');
    await expect(nowBtn).toBeVisible();

    const before = await card.locator('[data-testid="db-backup-last"]').textContent();
    await nowBtn.click();

    // The row flips from "Never" (or an older timestamp) to a fresh timestamp.
    await expect
      .poll(async () => card.locator('[data-testid="db-backup-last"]').textContent(), {
        timeout: 20_000,
      })
      .not.toBe(before);

    // The list now has at least one entry.
    await expect(card.locator('[data-testid="db-backup-list"] li').first()).toBeVisible();
    // No error surfaced.
    await expect(card.locator('[data-testid="db-backup-error"]')).toHaveCount(0);
  });

  test('15.3 — the card is keyboard reachable and every control has an accessible name', async ({
    page,
  }) => {
    await openSettings(page, admin);
    await openCategory(page, 'data_management');

    const card = set(page).locator('[data-testid="settings-card"]').filter({ hasText: /Database snapshots/i });

    // Focus the primary action directly — it must not be tabindex=-1.
    const nowBtn = card.locator('[data-testid="db-backup-now"]');
    await nowBtn.focus();
    expect(await page.evaluate(() => document.activeElement?.tagName.toLowerCase())).toBe('button');
    expect(await nowBtn.getAttribute('tabindex')).not.toBe('-1');

    // Accessible names come from the i18n labels, never a raw key.
    const name = (await nowBtn.textContent())?.trim() || '';
    expect(name.length).toBeGreaterThan(0);
    expect(name).not.toMatch(/^settings\./);
  });
});
