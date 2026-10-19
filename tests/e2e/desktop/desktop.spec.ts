/* =============================================================================
 * File          : desktop.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Desktop screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Desktop shell — End-to-End Test Suite
 *
 * `frontend/src/routes/Desktop.svelte` is the shell that loads right after
 * login and hosts every app: the wallpaper, the floating topbar pill
 * (clock / user pill / network popover / notifications / power), the dock
 * (pinned apps + running indicators + minimized-window buttons), the
 * Applications launcher, global search (Ctrl/⌘-K) and the window manager
 * (open / close / minimize / maximize / focus-dedup).
 *
 * **Safety: this suite never confirms a restart or a shutdown** — the power
 * menu's confirm button calls `POST /api/server/restart|shutdown`, which
 * reboots / powers off the host. Every power test stops at the confirmation
 * modal and cancels. It also never confirms a logout (that would end the
 * session mid-run). The `/api/server/*` auth guards are checked separately
 * by `tests/desktop_integration.rs`. The floating monitor panels are hidden
 * for this suite (they overlay the desktop) and are the Widgets suite's job.
 *
 * Run (all):    npx playwright test tests/e2e/desktop/desktop.spec.ts --project=firefox
 * Run (single): npx playwright test tests/e2e/desktop/desktop.spec.ts -g "4.1"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/desktop/desktop.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';
// The chrome keep-out geometry is mirrored in JS on purpose (initial window
// placement happens before layout exists) — section 14 pins the mirror to the
// CSS tokens it duplicates.
import { DEFAULT_SNAP_CONFIG } from '../../../frontend/src/lib/utils/snapUtils';

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
        // Pinned before the SPA boots (init script) so the pill never paints
        // in the default language first — section 15 measures label widths.
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
  // The floating monitor panels overlay the desktop and would intercept
  // clicks — hide them (they're the Widgets suite's concern).
  await page
    .addStyleTag({ content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}' })
    .catch(() => {});
}

// Language-independent dock lookup: the dock buttons carry data-testid
// hooks (dock-<appId>) because their aria-labels are translated
// (Tableau de bord / Dashboard / …) — matching on visible text would tie
// the suite to one UI language.
function dockApp(page: Page, id: string) {
  return page.locator(`[data-testid="dock-${id}"]`);
}

// Same reason for the topbar pill's three buttons: their accessible names are
// translated (Network / Réseau / Netzwerk …) and they are icon-only 44px
// circles since 2026-10-18, so neither visible text nor the old "power is the
// last .hv-topbar-btn" positional trick is a safe handle any more. The hooks are
// `topbar-network`, `topbar-notifications`, `topbar-power`. (The former topbar
// "Widgets" button was removed 2026-09-15 — adding/removing widgets lives in
// Settings now.)
function topbarBtn(page: Page, id: string) {
  return page.locator(`[data-testid="topbar-${id}"]`);
}

const TOPBAR_IDS = ['network', 'notifications', 'power'] as const;

// The desktop hydrates in two waves: the shell (topbar pill, dock) paints
// first, then the icon sprite (~24k <path> nodes) + monitor widgets mount
// ~200ms later. A DOM census taken in that window reads ~500 nodes and every
// later reading looks like a leak. Memory tests must wait for the second wave
// before taking their baseline: poll until the node count stops changing.
async function waitForDomSettle(page: Page) {
  let prev = -1;
  for (let i = 0; i < 40; i++) {
    const n = await page.evaluate(() => document.querySelectorAll('*').length);
    // The >1000 guard refuses an early "stable" reading from inside the
    // pre-hydration window itself (two polls can land on the same ~500).
    if (n === prev && n > 1000) return;
    prev = n;
    await page.waitForTimeout(250);
  }
}

// An open window (z-index 100+) covers the dock (z-index 50), so once a
// window is up a real pointer click on another dock icon is intercepted by
// the window. Dispatch the click straight at the button — we're exercising
// the dock handler, not pointer occlusion.
async function clickDock(page: Page, id: string) {
  await dockApp(page, id).dispatchEvent('click');
}

function windows(page: Page) {
  return page.locator('.hv-desktop .hv-window');
}

let adminTokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };

  // A freshly seeded server answers `GET /api/whats-new` with `seen: false` for
  // the first admin, and the desktop then opens a bits-ui modal whose full-screen
  // overlay swallows every pointer event. It landed on whichever test happened
  // to load the desktop first (measured: 14.7/14.8 and 16.2/16.4 failed with
  // "dialog-overlay intercepts pointer events" on a fresh scratch server, and
  // 15.5 could not even hover the Power button). Acknowledging the release once
  // here — with the same authenticated + CSRF'd call the modal's own close
  // button makes — takes the overlay out of every test's path, deterministically.
  // The session travels in `X-Session-Token`, not `Authorization`: that is what
  // `ApiClient.headers()` sends (see frontend/src/lib/services/api.ts).
  const seen = await ctx.post('/api/whats-new/seen', {
    headers: { 'X-Session-Token': token, 'X-CSRF-Token': csrf },
  });
  if (!seen.ok()) {
    throw new Error(
      `Could not acknowledge the release notes (${seen.status()}) — the What's New overlay would block pointer tests.`,
    );
  }
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
  test('1.1 — the desktop, dock and topbar pill all render', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await expect(page.locator('.hv-desktop')).toBeVisible();
    await expect(page.locator('.hv-dock')).toBeVisible();
    await expect(page.locator('.hv-menubar')).toBeVisible();
  });

  test('1.2 — a wallpaper layer is painted', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const painted = await page.evaluate(() => {
      const el =
        document.querySelector('[class*="hv-wallpaper"]') || document.querySelector('.hv-desktop-container');
      if (!el) return false;
      const bg = getComputedStyle(el as Element).backgroundImage;
      return bg !== 'none' || (el as HTMLElement).className.includes('wallpaper');
    });
    expect(painted).toBe(true);
  });

  test('1.3 — the topbar clock shows a time', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await expect(page.locator('.hv-menubar-time')).toHaveText(/\d/);
  });

  test('1.4 — the user pill shows the signed-in username', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await expect(page.locator('.hv-user-pill-name')).toContainText(ADMIN_USER);
  });

  test('1.5 — the pinned dock apps are present', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    // Every pinned tile, in dock order. `dock-terminal` was missing from this
    // list while being rendered right after `dashboard` — a pinned app no test
    // asserted the presence of.
    for (const id of ['dashboard', 'terminal', 'docker', 'health', 'chat', 'launchpad', 'settings']) {
      await expect(dockApp(page, id)).toBeVisible();
    }
  });
});

// ===========================================================================
// 2. Dock — opening apps
// ===========================================================================

test.describe('2 — Dock', () => {
  test('2.1 — clicking a pinned icon opens that app window with a running dot', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await expect(windows(page)).toHaveCount(0);
    await dockApp(page, 'dashboard').click();
    await expect(windows(page)).toHaveCount(1);
    await expect(dockApp(page, 'dashboard').locator('.hv-dock-dot')).toHaveCount(1);
  });

  test('2.2 — clicking the same icon again focuses, never opens a duplicate', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'dashboard').click();
    await expect(windows(page)).toHaveCount(1);
    await clickDock(page, 'dashboard');
    await page.waitForTimeout(400);
    await expect(windows(page)).toHaveCount(1);
  });

  test('2.3 — opening two different apps yields two windows', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'dashboard').click();
    await clickDock(page, 'docker');
    await expect(windows(page)).toHaveCount(2);
  });
});

// ===========================================================================
// 3. Window management
// ===========================================================================

test.describe('3 — Window management', () => {
  test('3.1 — a window can be closed', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'dashboard').click();
    await expect(windows(page)).toHaveCount(1);
    await page.locator('[data-testid="win-close"]').first().click();
    await expect(windows(page)).toHaveCount(0);
    await expect(dockApp(page, 'dashboard').locator('.hv-dock-dot')).toHaveCount(0);
  });

  test('3.2 — minimize sends the window to the dock, clicking it restores', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'dashboard').click();
    await page.locator('[data-testid="win-minimize"]').first().click();
    // The panel unmounts while minimized, but the running dot stays.
    await expect(windows(page)).toHaveCount(0);
    await expect(dockApp(page, 'dashboard').locator('.hv-dock-dot')).toHaveCount(1);
    await clickDock(page, 'dashboard');
    await expect(windows(page)).toHaveCount(1);
  });

  test('3.3 — maximize then restore keeps a single window', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'dashboard').click();
    await page.locator('[data-testid="win-maximize"]').first().click();
    // Maximized state is asserted structurally (.maximized), not via the
    // translated aria-label (Agrandir / Maximize / …).
    await expect(page.locator('.hv-desktop .hv-window.maximized')).toHaveCount(1);
    await page.locator('[data-testid="win-maximize"]').first().click();
    await expect(page.locator('.hv-desktop .hv-window.maximized')).toHaveCount(0);
    await expect(windows(page)).toHaveCount(1);
  });
});

// ===========================================================================
// 4. Topbar panels — mutually exclusive
// ===========================================================================

test.describe('4 — Topbar panels', () => {
  test('4.1 — the user pill opens the account menu', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('.hv-user-pill').click();
    await expect(page.locator('.hv-user-dropdown')).toBeVisible();
  });

  test('4.2 — opening the notifications panel closes the account menu', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('.hv-user-pill').click();
    await expect(page.locator('.hv-user-dropdown')).toBeVisible();
    await page.locator('.hv-notif-btn').click();
    await expect(page.locator('.hv-user-dropdown')).toHaveCount(0);
    await expect(page.locator('.hv-topbar-panel')).toBeVisible();
  });

  test('4.3 — clicking outside closes an open topbar panel', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('.hv-user-pill').click();
    await expect(page.locator('.hv-user-dropdown')).toBeVisible();
    await page.locator('.hv-desktop').click({ position: { x: 400, y: 400 } });
    await expect(page.locator('.hv-user-dropdown')).toHaveCount(0);
  });

  test('4.4 — all three topbar popovers share the same card chrome + anchor', async ({ page }) => {
    await gotoDesktop(page, adminTokens);

    // Read the standardized properties off `.hv-topbar-panel` for each popover.
    const readChrome = async () => {
      const el = page.locator('.hv-topbar-panel').first();
      await expect(el).toBeVisible();
      return el.evaluate((node) => {
        const c = getComputedStyle(node);
        return {
          borderRadius: c.borderRadius,
          background: c.backgroundColor,
          boxShadow: c.boxShadow,
          animationName: c.animationName,
          animationDuration: c.animationDuration,
          position: c.position,
          top: c.top,
          right: c.right,
        };
      });
    };

    await page.locator('.hv-user-pill').click();
    const account = await readChrome();
    await page.locator('.hv-desktop').click({ position: { x: 400, y: 400 } });

    await page.locator('.hv-connection-wrap .hv-topbar-btn').click();
    const net = await readChrome();
    await page.locator('.hv-desktop').click({ position: { x: 400, y: 400 } });

    await page.locator('.hv-notif-btn').click();
    const notifications = await readChrome();

    // Card look is identical across all three.
    expect(net.borderRadius).toBe(account.borderRadius);
    expect(notifications.borderRadius).toBe(account.borderRadius);
    expect(net.background).toBe(account.background);
    expect(notifications.background).toBe(account.background);
    expect(net.boxShadow).toBe(account.boxShadow);
    expect(notifications.boxShadow).toBe(account.boxShadow);
    expect(account.animationName).toBe('hvTopbarPanelAppear');
    expect(net.animationName).toBe('hvTopbarPanelAppear');
    expect(notifications.animationName).toBe('hvTopbarPanelAppear');
    expect(account.animationDuration).toBe('0.16s');

    // Anchoring (absolute, pinned to the pill's right edge) is identical too.
    for (const p of [account, net, notifications]) {
      expect(p.position).toBe('absolute');
      expect(p.right).toBe('0px');
    }
    expect(net.top).toBe(account.top);
    expect(notifications.top).toBe(account.top);
  });
});

// ===========================================================================
// 5. Applications launcher
// ===========================================================================

test.describe('5 — Applications launcher', () => {
  test('5.1 — opens from the dock and lists app tiles', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'launchpad').click();
    await expect(page.locator('.lp-panel')).toBeVisible();
    expect(await page.locator('.lp-card').count()).toBeGreaterThan(5);
  });

  test('5.2 — the search field filters the tiles', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'launchpad').click();
    const all = await page.locator('.lp-card').count();
    // "Storage" is a launcher tile (Terminal / Settings are dock-only, not
    // listed in the launcher).
    await page.locator('.lp-search-field').fill('storage');
    await expect.poll(() => page.locator('.lp-card').count()).toBeLessThan(all);
    await expect(page.locator('.lp-card[title="Storage"]')).toBeVisible();
  });

  test('5.3 — clicking a tile opens that app and closes the launcher', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'launchpad').click();
    await page.locator('.lp-card[title="System Journal"]').click();
    await expect(page.locator('.lp-overlay')).toHaveCount(0);
    // The Journal window opens in the window manager — assert on its dialog
    // (language-independent) rather than a page-root class (the old `.jr-page`
    // was renamed away with the PageLayout migration).
    await expect(page.locator('[role="dialog"][aria-label="System Journal"]')).toBeVisible({
      timeout: 15_000,
    });
  });

  test('5.4 — Escape closes the launcher', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'launchpad').click();
    await expect(page.locator('.lp-panel')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.locator('.lp-overlay')).toHaveCount(0);
  });

  // ── Redesign (Figma "Sans titre" node 23:6646, 2026-09-16) ──────────────
  // The launcher stopped being a centered opaque modal and became a
  // full-screen dimmed scrim with a chip row over one tile grid. 5.1–5.4
  // above still pass unchanged (same selectors); the tests below cover what
  // the new anatomy added.

  test('5.5 — the category chips narrow the one grid in place', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'launchpad').click();
    await expect(page.locator('.lp-panel')).toBeVisible();

    const all = await page.locator('.lp-card').count();
    const chips = page.locator('.lp-cat');
    const n = await chips.count();
    // "All apps" plus at least one real category.
    expect(n).toBeGreaterThan(1);

    // Exactly one chip is pressed at rest, and it is "All apps".
    await expect(page.locator('.lp-cat[aria-pressed="true"]')).toHaveCount(1);
    await expect(chips.nth(0)).toHaveAttribute('aria-pressed', 'true');

    // Find a category that genuinely narrows the grid (any single category
    // holding every app would make the assertion vacuous).
    let narrowed = -1;
    for (let i = 1; i < n; i++) {
      await chips.nth(i).click();
      const c = await page.locator('.lp-card').count();
      if (c > 0 && c < all) {
        narrowed = c;
        break;
      }
      await chips.nth(i).click(); // re-click clears the filter
    }
    expect(narrowed).toBeGreaterThan(0);
    // Still exactly one pressed chip — they are a radio group, not toggles.
    await expect(page.locator('.lp-cat[aria-pressed="true"]')).toHaveCount(1);
    await expect(chips.nth(0)).toHaveAttribute('aria-pressed', 'false');

    // "All apps" restores the full grid.
    await chips.nth(0).click();
    await expect(page.locator('.lp-card')).toHaveCount(all);
    await expect(page.locator('.lp-cat[aria-pressed="true"]')).toHaveCount(1);
  });

  test('5.6 — a query with no match shows the empty state, and Reset restores it', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'launchpad').click();
    const all = await page.locator('.lp-card').count();

    await page.locator('.lp-search-field').fill('zzzznope-not-an-app');
    await expect(page.locator('.lp-empty')).toBeVisible();
    await expect(page.locator('.lp-card')).toHaveCount(0);

    await page.locator('.lp-reset').click();
    await expect(page.locator('.lp-empty')).toHaveCount(0);
    await expect(page.locator('.lp-card')).toHaveCount(all);
    await expect(page.locator('.lp-search-field')).toHaveValue('');
  });

  test('5.7 — the launcher is a dimmed full-screen scrim, not an opaque card', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'launchpad').click();
    await expect(page.locator('.lp-panel')).toBeVisible();

    const overlay = await page.locator('.lp-overlay').evaluate((el) => {
      const s = getComputedStyle(el);
      return { position: s.position, bg: s.backgroundColor, blur: s.backdropFilter };
    });
    expect(overlay.position).toBe('fixed');

    // The mock blurs the wallpaper behind the scrim. This is not decorative:
    // declaring the `backdrop-filter` + `-webkit-backdrop-filter` pair makes
    // the CSS minifier keep ONLY the prefixed form, which Firefox ignores
    // (`CSS.supports('-webkit-backdrop-filter', …)` is false there) — the blur
    // silently resolved to `none` once already. Measure it, don't trust it.
    const blur = parseFloat((overlay.blur.match(/blur\(([\d.]+)px\)/) || [])[1] ?? '0');
    expect(blur).toBeGreaterThan(0);

    // Translucent dark ink: dimming the desktop behind it, not replacing it.
    const m = overlay.bg.match(/rgba?\(([^)]+)\)/);
    expect(m).not.toBeNull();
    const [r, g, b, a] = m![1].split(',').map((v) => parseFloat(v.trim()));
    expect(a).toBeGreaterThan(0.3);
    expect(a).toBeLessThan(1);
    expect(r + g + b).toBeLessThan(255); // dark, not a white wash

    // The content column itself paints no surface — the tiles sit on the scrim.
    const panelBg = await page.locator('.lp-panel').evaluate(
      (el) => getComputedStyle(el).backgroundColor,
    );
    expect(panelBg).toMatch(/rgba\(0, 0, 0, 0\)|transparent/);
  });

  test('5.8 — the tile label clears 4.5:1 over the brightest wallpaper', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'launchpad').click();
    await expect(page.locator('.lp-card-name').first()).toBeVisible();

    // The launcher composites three translucent layers over an unknown
    // wallpaper, so the contrast cannot be read off a single token. Worst
    // case for white ink is the brightest ground HomeVault can show (a white
    // photo wallpaper), so composite tile → scrim → white and measure.
    const measured = await page.locator('.lp-card-name').first().evaluate((el) => {
      const srgb = (c: number) => {
        c /= 255;
        return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
      };
      const lum = (c: number[]) =>
        0.2126 * srgb(c[0]) + 0.7152 * srgb(c[1]) + 0.0722 * srgb(c[2]);
      const parse = (color: string): number[] => {
        const p = color
          .match(/rgba?\(([^)]+)\)/)![1]
          .split(',')
          .map((v) => parseFloat(v.trim()));
        return [p[0], p[1], p[2], p.length > 3 ? p[3] : 1];
      };
      const over = (fg: number[], bg: number[]): number[] => {
        const a = fg[3];
        return [
          fg[0] * a + bg[0] * (1 - a),
          fg[1] * a + bg[1] * (1 - a),
          fg[2] * a + bg[2] * (1 - a),
          1,
        ];
      };
      const card = el.closest('.lp-card') as HTMLElement;
      const tile = parse(getComputedStyle(card).backgroundColor);
      const scrim = parse(
        getComputedStyle(document.querySelector('.lp-overlay') as HTMLElement).backgroundColor,
      );
      const ink = parse(getComputedStyle(el).color);
      const paper = [255, 255, 255, 1];
      const ground = over(tile, over(scrim, paper));
      const hi = Math.max(lum(ink), lum(ground));
      const lo = Math.min(lum(ink), lum(ground));
      return { ratio: (hi + 0.05) / (lo + 0.05), alpha: ink[3] };
    });

    expect(measured.alpha).toBe(1); // opaque ink — no alpha stacking
    expect(measured.ratio).toBeGreaterThanOrEqual(4.5);
  });

  test('5.9 — every launcher control keeps a 44px touch target', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'launchpad').click();
    await expect(page.locator('.lp-panel')).toBeVisible();

    // The chip paints 32px (the mock's own geometry) but must carry the
    // transparent 44px hit pad.
    const chip = await page.locator('.lp-cat').first().evaluate((el) => {
      const after = getComputedStyle(el, '::after');
      return {
        visible: el.getBoundingClientRect().height,
        pad: parseFloat(after.height),
        positioned: after.position,
      };
    });
    expect(chip.visible).toBeLessThan(44);
    expect(chip.pad).toBeGreaterThanOrEqual(44);
    expect(chip.positioned).toBe('absolute');

    // The search input holds the floor itself, inside the mock's 56px bar.
    const field = await page.locator('.lp-search-field').evaluate(
      (el) => el.getBoundingClientRect().height,
    );
    expect(field).toBeGreaterThanOrEqual(44);
    const bar = await page.locator('.lp-search').evaluate(
      (el) => el.getBoundingClientRect().height,
    );
    // Close-to, not exact: the panel animates in (`lpPop`, a composited
    // translate), and Chromium sometimes reports the composited box as
    // 55.999996 — a 4e-6px float artefact, not a layout change. Half a pixel
    // still fails if the bar really moves (it is 6px + the 44px field + 6px).
    expect(bar).toBeCloseTo(56, 0);

    // The tiles are the primary target — larger than the floor by design.
    const card = await page.locator('.lp-card').first().evaluate(
      (el) => el.getBoundingClientRect().height,
    );
    expect(card).toBeGreaterThanOrEqual(44);
  });

  test('5.10 — the launcher traps focus and returns it to the dock tile', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const trigger = dockApp(page, 'launchpad');
    await trigger.focus();
    await trigger.click();
    await expect(page.locator('.lp-panel')).toBeVisible();

    // Focus starts inside the dialog (WCAG 2.1.1 — the search box wins).
    expect(
      await page.evaluate(() => !!document.activeElement?.closest('.lp-panel')),
    ).toBe(true);

    // Tab cycles inside: no keyboard escape from the modal, no trap either.
    for (let i = 0; i < 30; i++) {
      await page.keyboard.press('Tab');
      const inside = await page.evaluate(() => !!document.activeElement?.closest('.lp-panel'));
      expect(inside, `focus escaped the launcher after ${i + 1} Tab presses`).toBe(true);
    }

    // Escape closes and restores focus to the trigger (WCAG 2.4.3).
    await page.keyboard.press('Escape');
    await expect(page.locator('.lp-overlay')).toHaveCount(0);
    expect(
      await page.evaluate(() => document.activeElement?.getAttribute('data-testid')),
    ).toBe('dock-launchpad');
  });

  test('5.11 — on a phone the chips scroll sideways and no tile is unreachable', async ({
    page,
  }) => {
    await page.setViewportSize({ width: 400, height: 800 });
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'launchpad').click();
    await expect(page.locator('.lp-panel')).toBeVisible();

    const phone = await page.evaluate(() => {
      const chips = document.querySelector('.lp-cats') as HTMLElement;
      // The panel is the scroller (see `.lp-panel` in the component): scrolling
      // it rather than the grid wrapper is what keeps the search bar, the chip
      // row and the grid in one content box, and so one width.
      const panel = document.querySelector('.lp-panel') as HTMLElement;
      const grid = document.querySelector('.lp-grid') as HTMLElement;
      return {
        chipWrap: getComputedStyle(chips).flexWrap,
        chipOverflowX: getComputedStyle(chips).overflowX,
        // `flex: 1 1 auto` on the desktop chip (see the component) must be
        // switched back off here: a grown chip would fill the row and there
        // would be nothing left to scroll to.
        chipGrow: getComputedStyle(document.querySelector('.lp-cat') as HTMLElement).flexGrow,
        chipScrolls: chips.scrollWidth > chips.clientWidth + 1,
        cols: getComputedStyle(grid).gridTemplateColumns.split(' ').length,
        panelScrolls: panel.scrollHeight > panel.clientHeight,
        panelOverflowY: getComputedStyle(panel).overflowY,
        // The whole point of the inner scroller: the page itself stays put.
        pageScrolls: document.documentElement.scrollHeight > window.innerHeight + 1,
        docScrollW: document.documentElement.scrollWidth,
        viewportW: window.innerWidth,
      };
    });

    // 640px breakpoint: a 3-column grid (the mock's 5 do not fit) and a chip
    // row that scrolls instead of wrapping onto a second line.
    expect(phone.cols).toBe(3);
    expect(phone.chipWrap).toBe('nowrap');
    expect(phone.chipOverflowX).toBe('auto');
    expect(phone.chipGrow).toBe('0');
    expect(phone.chipScrolls).toBe(true);
    expect(phone.panelOverflowY).toBe('auto');
    expect(phone.panelScrolls).toBe(true);
    // 21 tiles never fit 800px: they must scroll INSIDE the dialog, never by
    // scrolling the desktop behind it, and never sideways.
    expect(phone.pageScrolls).toBe(false);
    expect(phone.docScrollW).toBe(phone.viewportW);

    // Last tile is genuinely reachable once the inner column is scrolled.
    const last = page.locator('.lp-card').last();
    await last.scrollIntoViewIfNeeded();
    await expect(last).toBeInViewport();
  });

  test('5.12 — six columns, and the search bar and chips are exactly that wide', async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'launchpad').click();
    await expect(page.locator('.lp-panel')).toBeVisible();

    const measure = () =>
      page.evaluate(() => {
        const box = (sel: string) => {
          const el = document.querySelector(sel) as HTMLElement;
          const r = el.getBoundingClientRect();
          return { x: Math.round(r.x), w: Math.round(r.width), right: Math.round(r.right) };
        };
        const grid = document.querySelector('.lp-grid') as HTMLElement;
        const panel = document.querySelector('.lp-panel') as HTMLElement;
        const tracks = getComputedStyle(grid).gridTemplateColumns.split(' ');
        // Chips grouped into visual lines by their top offset; each line is
        // reduced to its leftmost and rightmost rendered edge, which is the
        // only thing that says how far across the row the chips actually go.
        const chips = [...document.querySelectorAll('.lp-cat')].map((el) => {
          const r = (el as HTMLElement).getBoundingClientRect();
          return {
            y: Math.round(r.y),
            left: Math.round(r.left),
            right: Math.round(r.right),
          };
        });
        const chipLines = [...new Set(chips.map((c) => c.y))]
          .sort((a, b) => a - b)
          .map((y) => {
            const line = chips.filter((c) => c.y === y);
            return {
              left: Math.min(...line.map((c) => c.left)),
              right: Math.max(...line.map((c) => c.right)),
            };
          });
        return {
          cols: tracks.length,
          trackPx: parseFloat(tracks[0]),
          grid: box('.lp-grid'),
          search: box('.lp-search'),
          cats: box('.lp-cats'),
          panel: box('.lp-panel'),
          chipCount: chips.length,
          chipLines,
          panelScrolls: panel.scrollHeight > panel.clientHeight,
        };
      });

    const m = await measure();

    // Six columns, and square-ish: the tiles must not be squeezed under the
    // 124px the icon tile + label need.
    expect(m.cols).toBe(6);
    expect(m.trackPx).toBeGreaterThanOrEqual(124);

    // The three modules are one width — the request that produced this test.
    // Sub-pixel rounding is allowed for, a layout drift is not.
    expect(Math.abs(m.search.w - m.grid.w)).toBeLessThanOrEqual(1);
    expect(Math.abs(m.cats.w - m.grid.w)).toBeLessThanOrEqual(1);
    // …and flush left, not merely equal in width.
    expect(m.search.x).toBe(m.grid.x);
    expect(m.cats.x).toBe(m.grid.x);
    // The panel is the column all three live in. Not an equality: the panel can
    // be a scrollbar gutter wider, that gutter being reserved inside it.
    expect(m.panel.w).toBeGreaterThanOrEqual(m.grid.w);

    // …and the chips *fill* that width, which is a second, separate thing: the
    // row is the grid's width whatever its contents, but a chip is only as
    // wide as its label, so with a short category list the chips used to stop
    // two thirds across and the module read as narrower than the tiles. Asserted
    // per rendered line, so it holds with or without a wrap (a full catalog has
    // up to 13 chips): every line of a `flex-grow` row is grown to the same
    // width, so each line must run edge to edge on its own.
    expect(m.chipCount, 'the chip row is never empty').toBeGreaterThanOrEqual(2);
    expect(m.chipLines.length).toBeGreaterThanOrEqual(1);
    for (const line of m.chipLines) {
      expect(Math.abs(line.left - m.cats.x)).toBeLessThanOrEqual(1);
      expect(Math.abs(line.right - m.cats.right)).toBeLessThanOrEqual(1);
    }

    // Same three modules, now with the tiles overflowing — the only state in
    // which they ever came apart: while the grid wrapper scrolled, a
    // layout-width scrollbar narrowed the grid without touching the other two.
    await page.setViewportSize({ width: 1440, height: 520 });
    await expect.poll(async () => (await measure()).panelScrolls).toBe(true);
    const tight = await measure();
    expect(tight.cols).toBe(6);
    expect(Math.abs(tight.search.w - tight.grid.w)).toBeLessThanOrEqual(1);
    expect(Math.abs(tight.cats.w - tight.grid.w)).toBeLessThanOrEqual(1);
    expect(tight.search.x).toBe(tight.grid.x);
    expect(tight.cats.x).toBe(tight.grid.x);
    for (const line of tight.chipLines) {
      expect(Math.abs(line.right - tight.cats.right)).toBeLessThanOrEqual(1);
    }
    // The search bar is sticky over the panel's scrollport, so the field stays
    // reachable while the tiles scroll under it.
    const stuck = await page.evaluate(() => {
      const panel = document.querySelector('.lp-panel') as HTMLElement;
      panel.scrollTop = panel.scrollHeight;
      return {
        scrollTop: panel.scrollTop,
        delta: Math.round(
          document.querySelector('.lp-search')!.getBoundingClientRect().y -
            panel.getBoundingClientRect().y,
        ),
      };
    });
    expect(stuck.scrollTop).toBeGreaterThan(0);
    expect(stuck.delta).toBe(0);
  });

  test('5.13 — the launcher search bar has a neutral focus ring, never the brand blue', async ({
    page,
  }) => {
    // The launcher's focus trap lands on the query field the moment it opens,
    // so whatever ring that state paints is on screen permanently — a brand
    // ring there read as a blue halo around the capsule instead of a "you just
    // tabbed here" affordance. It also DOUBLED: `.lp-search:focus-within`
    // painted one and base.css's global `input:focus` painted a second, on the
    // field inside the capsule. The bar now takes a 2px slate rim drawn
    // INSIDE the capsule (an outside ring merges into the white bar, and a
    // dark halo is invisible on the dark scrim), and the field paints nothing.
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'launchpad').click();
    await expect(page.locator('.lp-panel')).toBeVisible();

    const read = () =>
      page.evaluate(() => {
        const bar = document.querySelector('.lp-search') as HTMLElement;
        const field = document.querySelector('.lp-search-field') as HTMLElement;
        const lum = (c: number[]) => {
          const f = c.slice(0, 3).map((v) => {
            const s = v / 255;
            return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
          });
          return 0.2126 * f[0] + 0.7152 * f[1] + 0.0722 * f[2];
        };
        // Composite the first (ring) layer over the bar's own surface and
        // measure it against that surface — the change a focus ring must make.
        const first = getComputedStyle(bar).boxShadow.split(/,(?![^(]*\))/)[0];
        const m = first.match(/rgba?\(([^)]+)\)/);
        const parts = m ? m[1].split(',').map((v) => parseFloat(v)) : [];
        const alpha = parts.length === 4 ? parts[3] : 1;
        const surface = [255, 255, 255];
        const rim = surface.map((v, i) => Math.round(parts[i] * alpha + v * (1 - alpha)));
        const lr = lum(rim);
        const ls = lum(surface);
        return {
          active: document.activeElement?.className ?? '',
          barShadow: getComputedStyle(bar).boxShadow,
          fieldShadow: getComputedStyle(field).boxShadow,
          inset: /inset/.test(first),
          rim,
          contrast: (Math.max(lr, ls) + 0.05) / (Math.min(lr, ls) + 0.05),
        };
      });

    const focused = await read();
    // The field is the focused element on open — that is why the ring shows.
    expect(focused.active, 'the query field holds focus on open').toContain(
      'lp-search-field',
    );

    // No brand blue anywhere in the indicator (the old value was
    // `rgba(30, 98, 174, 0.9)` / `0.35`), and no ring on the field itself —
    // the wrapper owns it, so the capsule carries exactly one contour.
    expect(focused.barShadow, 'no brand-blue ring on the bar').not.toContain(
      '30, 98, 174',
    );
    expect(focused.fieldShadow, 'the field paints no ring of its own').toBe('none');

    // It is still a real indicator: a slate rim, drawn inside the capsule so
    // the bar keeps its silhouette, clearing the 3:1 floor (1.4.11 / 2.4.7)
    // against the white surface it marks.
    expect(focused.inset, 'the rim is drawn inside the capsule').toBe(true);
    expect(focused.contrast, 'rim contrast vs the bar surface').toBeGreaterThanOrEqual(3);

    // And it is a focus state, not a decoration: tabbing to a chip drops it.
    await page.keyboard.press('Tab');
    await expect(page.locator('.lp-cat-active')).toBeFocused();
    const blurred = await read();
    expect(blurred.barShadow, 'no rim once focus leaves the bar').not.toContain('inset');
  });

  test('5.14 — the tiles and the chips read alphabetically', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'launchpad').click();
    await expect(page.locator('.lp-card-name').first()).toBeVisible();

    // The grid is ONE A→Z list, not category groups (2026-10-22): the order is
    // the user's entry point into ~25 apps, so it has to be the visible label
    // order and it has to survive a filter. Compare against the browser's own
    // collation so the assertion tracks the locale the UI is rendered in.
    //
    // The "add a web shortcut" tile is excluded on purpose: it is the grid's
    // last cell by contract (`launcher-shortcut-add`, after every app — see
    // shortcuts.spec.ts 1.2), not a member of the A→Z run.
    const appNames = () =>
      page.evaluate(() =>
        [
          ...document.querySelectorAll(
            '.lp-grid > .lp-card:not(.lp-card-add) .lp-card-name',
          ),
        ].map((el) => el.textContent?.trim() ?? ''),
      );
    const order = await page.evaluate(() => {
      const names = [
        ...document.querySelectorAll('.lp-grid > .lp-card:not(.lp-card-add) .lp-card-name'),
      ].map((el) => el.textContent?.trim() ?? '');
      const chips = [...document.querySelectorAll('.lp-cat')]
        .slice(1) // "All apps" is pinned first, outside the alphabetical run
        .map((el) => el.textContent?.trim() ?? '');
      const collator = new Intl.Collator(document.documentElement.lang, {
        sensitivity: 'base',
        numeric: true,
      });
      return {
        names,
        chips,
        sortedNames: [...names].sort((a, b) => collator.compare(a, b)),
        sortedChips: [...chips].sort((a, b) => collator.compare(a, b)),
      };
    });

    expect(order.names.length).toBeGreaterThan(5);
    expect(order.names, 'tiles are A→Z').toEqual(order.sortedNames);
    expect(order.chips.length).toBeGreaterThan(1);
    expect(order.chips, 'category chips are A→Z').toEqual(order.sortedChips);

    // A filter keeps the same order instead of re-grouping the survivors.
    await page.locator('.lp-cat').nth(1).click();
    const filteredNames = await appNames();
    const filtered = await page.evaluate((names) => {
      const collator = new Intl.Collator(document.documentElement.lang, {
        sensitivity: 'base',
        numeric: true,
      });
      return { names, sorted: [...names].sort((a, b) => collator.compare(a, b)) };
    }, filteredNames);
    expect(filtered.names).toEqual(filtered.sorted);
  });
});

// ===========================================================================
// 6. Global search
// ===========================================================================

test.describe('6 — Global search', () => {
  test('6.1 — Ctrl+K opens the search overlay', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.keyboard.press('Control+k');
    await expect(page.locator('.search-overlay .search-input')).toBeVisible();
  });

  test('6.2 — typing a query surfaces matching items', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.keyboard.press('Control+k');
    await page.locator('.search-overlay .search-input').fill('settings');
    await expect(page.locator('.search-item').first()).toBeVisible({ timeout: 5000 });
  });

  test('6.3 — selecting a result opens the app and closes search', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.keyboard.press('Control+k');
    await page.locator('.search-overlay .search-input').fill('terminal');
    await page.locator('.search-item').first().click();
    await expect(page.locator('.search-overlay')).toHaveCount(0);
    await expect(windows(page)).toHaveCount(1);
  });

  test('6.4 — Escape closes search', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.keyboard.press('Control+k');
    await expect(page.locator('.search-overlay')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.locator('.search-overlay')).toHaveCount(0);
  });

  // The Settings categories are fed into this index from the single shared
  // catalog (`stores/settingsCatalog.ts`), so every rail group is reachable
  // by search and a hit opens Settings already on that category.
  test('6.5 — a settings-category synonym is searchable', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.keyboard.press('Control+k');
    await page.locator('.search-overlay .search-input').fill('smtp');
    // "smtp" is only in the notifications category's keyword list, never in
    // its label — it can only match through the catalog projection.
    await expect(page.locator('.search-item').first()).toBeVisible({ timeout: 5000 });
  });

  test('6.6 — selecting a settings category opens Settings on that category', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.keyboard.press('Control+k');
    await page.locator('.search-overlay .search-input').fill('ollama');
    await page.locator('.search-item').first().click();
    await expect(page.locator('.search-overlay')).toHaveCount(0);
    await page.waitForSelector('[data-testid="settings-page"]', { timeout: 15_000 });
    // The AI category (the only one carrying the "ollama" keyword) must be the
    // active rail item, proving the one-shot `requestSettingsCategory` was
    // consumed.
    await expect(
      page.locator('[data-testid="settings-nav-item"].set-nav-active')
    ).toContainText(/AI|assistant|Ollama/i);
  });

  test('6.7 — the wallpaper shortcut opens the appearance category', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.keyboard.press('Control+k');
    await page.locator('.search-overlay .search-input').fill('wallpaper');
    await page.locator('.search-item').first().click();
    await page.waitForSelector('[data-testid="settings-page"]', { timeout: 15_000 });
    await expect(
      page.locator('[data-testid="settings-nav-item"].set-nav-active')
    ).toContainText(/appearance|apparence/i);
  });
});

// ===========================================================================
// 7. Power menu (never confirms a destructive action)
// ===========================================================================

test.describe('7 — Power menu', () => {
  test('7.1 — the power button opens a menu with shutdown / restart / logout', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('[data-testid="topbar-power"]').click();
    await expect(page.locator('.hv-power-dialog')).toBeVisible();
    await expect(page.locator('.hv-power-option')).toHaveCount(3);
  });

  test('7.2 — choosing Restart shows a confirmation that can be cancelled — never confirmed', async ({ page }) => {
    let hitServer = false;
    page.on('request', (r) => {
      if (/\/api\/server\/(restart|shutdown)/.test(r.url())) hitServer = true;
    });
    await gotoDesktop(page, adminTokens);
    await page.locator('[data-testid="topbar-power"]').click();
    await page.locator('.hv-power-option', { hasText: /restart|redémarr/i }).click();
    // Second modal — the confirmation.
    await expect(page.locator('.hv-power-footer')).toBeVisible({ timeout: 5000 });
    await page.locator('.hv-power-btn-secondary').click(); // Cancel
    await expect(page.locator('.hv-power-footer')).toHaveCount(0);
    expect(hitServer, 'the E2E must never call /api/server/*').toBe(false);
  });

  test('7.3 — the power menu closes on Escape (the header and its X button were removed)', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('[data-testid="topbar-power"]').click();
    await expect(page.locator('.hv-power-dialog')).toBeVisible();
    // The dialog has no header/close button anymore — Escape (focusTrap) and
    // overlay-click are the close paths; assert the keyboard one.
    await page.keyboard.press('Escape');
    await expect(page.locator('.hv-power-dialog')).toHaveCount(0);
    // And the overlay click closes it too.
    await page.locator('[data-testid="topbar-power"]').click();
    await expect(page.locator('.hv-power-dialog')).toBeVisible();
    await page.locator('.hv-power-overlay').click({ position: { x: 5, y: 5 } });
    await expect(page.locator('.hv-power-dialog')).toHaveCount(0);
  });

  test('7.4 — the power dialog and the topbar popovers share the surface corner (--hv-radius-4xl, 18px)', async ({ page }) => {
    await gotoDesktop(page, adminTokens);

    // The account menu is the shared `.hv-topbar-panel` chrome — the same
    // floating surface family as the power dialog.
    await page.locator('.hv-user-pill').click();
    const panel = page.locator('.hv-topbar-panel').first();
    await expect(panel).toBeVisible();
    const panelRadius = await panel.evaluate((el) =>
      parseFloat(getComputedStyle(el).borderRadius),
    );
    await page.locator('.hv-desktop').click({ position: { x: 400, y: 400 } });

    await page.locator('[data-testid="topbar-power"]').click();
    const dialog = page.locator('.hv-power-dialog');
    await expect(dialog).toBeVisible();
    const dialogRadius = await dialog.evaluate((el) =>
      parseFloat(getComputedStyle(el).borderRadius),
    );

    // 12px: the popover and the dialog are containers like a window or a
    // widget, so they take the single surface stop rather than a taller one.
    expect(panelRadius, 'topbar popover surface corner').toBe(18);
    expect(dialogRadius, 'power dialog surface corner').toBe(18);
  });

  test('7.5 — the power sheet wears the dock\'s circular tiles', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('[data-testid="topbar-power"]').click();
    await expect(page.locator('.hv-power-dialog')).toBeVisible();

    // The tiles are the dock tile itself: same diameter token, same circle.
    const tileSize = await page.evaluate(() =>
      parseFloat(
        getComputedStyle(document.documentElement).getPropertyValue('--hv-dock-item-size'),
      ),
    );
    const controlCorner = await page.evaluate(() =>
      parseFloat(
        getComputedStyle(document.documentElement).getPropertyValue('--hv-radius-control'),
      ),
    );

    const tiles = page.locator('.hv-power-tile');
    await expect(tiles).toHaveCount(3);

    const measured = await tiles.evaluateAll((els) =>
      els.map((el) => {
        const style = getComputedStyle(el);
        const rect = el.getBoundingClientRect();
        return {
          radius: style.borderTopLeftRadius,
          // Computed box, not the rect: the dialog's 200ms `hvPowerAppear`
          // entrance scales the whole sheet, so a rect read mid-animation is
          // 0.95× and would report a tile smaller than the token.
          width: parseFloat(style.width),
          height: parseFloat(style.height),
          gradient: style.backgroundImage,
          top: rect.top,
          bottom: rect.bottom,
        };
      }),
    );

    for (const tile of measured) {
      expect(tile.radius, 'power tile is a perfect circle, like a dock tile').toBe('50%');
      expect(tile.width, 'tile diameter = --hv-dock-item-size').toBeCloseTo(tileSize, 1);
      expect(tile.height, 'tile box is square').toBeCloseTo(tile.width, 1);
      expect(tile.gradient, 'each action tile paints its own gradient').toContain('linear-gradient');
    }
    // Shutdown / restart / logout must not collapse into one flat tile.
    expect(new Set(measured.map((t) => t.gradient)).size, 'three distinct gradients').toBe(3);

    // Dock anatomy: the label sits UNDER the tile, not beside it.
    const labelBox = await page.locator('.hv-power-label').first().boundingBox();
    expect(labelBox!.y, 'label under the tile').toBeGreaterThanOrEqual(
      measured[0].bottom - 1,
    );

    // …and the button is still a ≥44px target on the one control corner.
    const button = await page.locator('.hv-power-option').first().evaluate((el) => ({
      height: el.getBoundingClientRect().height,
      radius: parseFloat(getComputedStyle(el).borderRadius),
    }));
    expect(button.height, 'touch target').toBeGreaterThanOrEqual(44);
    expect(button.radius, 'the button keeps the control corner').toBe(controlCorner);
  });
});

// ===========================================================================
// 8. Responsive
// ===========================================================================

test.describe('8 — Responsive', () => {
  test('8.1 — below 480px the topbar hides the date and username', async ({ page }) => {
    await page.setViewportSize({ width: 400, height: 800 });
    await gotoDesktop(page, adminTokens);
    // The pill is still there, but the date is hidden.
    await expect(page.locator('.hv-menubar')).toBeVisible();
    const dateVisible = await page.locator('.hv-menubar-date').isVisible().catch(() => false);
    expect(dateVisible).toBe(false);
  });

  test('8.2 — the desktop never scrolls horizontally', async ({ page }) => {
    for (const w of [360, 768, 1920]) {
      await page.setViewportSize({ width: w, height: 900 });
      await gotoDesktop(page, adminTokens);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(overflow, `${w}px`).toBeLessThanOrEqual(2);
    }
  });

  test('8.3 — each pill renders its own height token, and both are 72px above phone width', async ({ page }) => {
    // The two pills were once locked to ONE height token; they now keep
    // separate tokens so either can move alone (they have shipped 84/62,
    // 64/62 and — since the 2026-10-18 height pass — 72/72). This test
    // asserts both halves of that: each pill matches ITS OWN token, and above
    // phone width the two agree at 72px. Asserting only "they differ" would go
    // green if one token stopped being honoured.
    //
    // Phone is the documented exception: there the topbar pill (56px) is the
    // taller one and the dock drops to 52px (44px tiles + 4px of breathing
    // room), because a 72px dock would eat a 360px viewport. Asserting
    // equality at 400px would fail on a decision, not a regression.
    for (const w of [400, 768, 1440]) {
      await page.setViewportSize({ width: w, height: 900 });
      await gotoDesktop(page, adminTokens);
      const bars = await page.evaluate(() => {
        const px = (v: string) => parseFloat(v) || -1;
        const css = getComputedStyle(document.documentElement);
        const h = (sel: string) => {
          const el = document.querySelector(sel);
          return el ? Math.round(el.getBoundingClientRect().height) : -1;
        };
        return {
          topbar: h('.hv-menubar'),
          dock: h('.hv-dock'),
          topbarToken: px(css.getPropertyValue('--hv-chrome-topbar-h')),
          dockToken: px(css.getPropertyValue('--hv-chrome-dock-h')),
        };
      });
      expect(bars.topbarToken, `--hv-chrome-topbar-h @${w}px`).toBeGreaterThan(0);
      expect(bars.dockToken, `--hv-chrome-dock-h @${w}px`).toBeGreaterThan(0);
      // box-sizing:border-box on both, so the token IS the rendered height.
      expect(bars.topbar, `topbar vs its token @${w}px`).toBe(bars.topbarToken);
      expect(bars.dock, `dock vs its token @${w}px`).toBe(bars.dockToken);
      if (w > 480) {
        expect(bars.dock, `both pills are 72px @${w}px`).toBe(72);
        expect(bars.topbar, `both pills are 72px @${w}px`).toBe(72);
      } else {
        // Phone: compact, and the topbar is the taller one.
        expect(bars.topbar, `phone topbar @${w}px`).toBe(56);
        expect(bars.dock, `phone dock @${w}px`).toBe(52);
      }
    }
  });

  test('8.4 — the topbar profile avatar fills its frame (not centre-cropped)', async ({ page }) => {
    await page.setViewportSize({ width: 400, height: 800 });
    await gotoDesktop(page, adminTokens);
    const fit = await page.evaluate(() => {
      const wrap = document.querySelector('.hv-user-pill-avatar');
      const img = wrap?.querySelector('img.hv-user-avatar') as HTMLImageElement | null;
      if (!wrap || !img) return null;
      const wr = wrap.getBoundingClientRect();
      const ir = img.getBoundingClientRect();
      return { dw: Math.abs(wr.width - ir.width), dh: Math.abs(wr.height - ir.height) };
    });
    expect(fit).not.toBeNull();
    expect(fit!.dw).toBeLessThanOrEqual(1);
    expect(fit!.dh).toBeLessThanOrEqual(1);
  });

  test('8.5 — both floating pills are capsules with perfectly circular ends', async ({ page }) => {
    for (const w of [400, 768, 1440]) {
      await page.setViewportSize({ width: w, height: 900 });
      await gotoDesktop(page, adminTokens);
      const radii = await page.evaluate(() => {
        const read = (sel: string) => {
          const el = document.querySelector(sel) as HTMLElement | null;
          if (!el) return null;
          return {
            r: parseFloat(getComputedStyle(el).borderRadius),
            h: el.getBoundingClientRect().height,
          };
        };
        return { topbar: read('.hv-menubar'), dock: read('.hv-dock') };
      });
      // Capsule: the full radius token (9999px) is clamped by the browser to
      // half the box height, so BOTH ends render as exact semicircles — the
      // assertion is radius >= height/2, deliberately not the literal 9999px.
      for (const [name, box] of [
        ['dock', radii.dock],
        ['topbar', radii.topbar],
      ] as const) {
        expect(box, `${name} element @${w}px`).not.toBeNull();
        expect(box!.r, `${name} capsule radius @${w}px`).toBeGreaterThanOrEqual(box!.h / 2);
      }
    }
  });

  test('8.7 — the dock app icons are perfect circles of one uniform size', async ({ page }) => {
    for (const w of [400, 768, 1440]) {
      await page.setViewportSize({ width: w, height: 900 });
      await gotoDesktop(page, adminTokens);
      // The dock items stagger in (hvDockItemIn, ~750ms after mount, with the
      // `backwards` fill) and a mid-animation tile measures at 0.9× its size —
      // wait for the choreography to be over before reading any rectangle.
      // getAnimations() only lists running/pending animations, so an empty
      // list on every item means the entrance is done at any viewport.
      await page.waitForFunction(
        () => {
          const items = [...document.querySelectorAll('.hv-dock-container .hv-dock-item')];
          return (
            items.length > 0 && items.every((el) => (el as HTMLElement).getAnimations().length === 0)
          );
        },
        undefined,
        { timeout: 5_000 },
      );
      const tiles = await page.evaluate(() => {
        const icons = [...document.querySelectorAll('.hv-dock-container .hv-dock-icon')] as HTMLElement[];
        return icons.map((el) => {
          const b = el.getBoundingClientRect();
          const s = getComputedStyle(el);
          return {
            // `50%` on a square tile is the only value that renders a perfect
            // circle — a px radius would silently square it back on resize.
            radius: s.borderRadius,
            width: b.width,
            height: b.height,
          };
        });
      });
      expect(tiles.length, `dock icons @${w}px`).toBeGreaterThan(5);
      const sizes = new Set(tiles.map((t) => Math.round(t.width)));
      expect(sizes.size, `uniform icon size @${w}px`).toBe(1);
      for (const t of tiles) {
        expect(t.radius, `icon radius @${w}px`).toBe('50%');
        expect(t.width, `icon width @${w}px`).toBe(t.height);
      }
    }
  });

  test('8.6 — a maximized window clears the dock on tablet (601–900px)', async ({ page }) => {
    // Below 820px a new window opens already maximized (AUTO_MAXIMIZE_BREAKPOINT),
    // so on a tablet viewport the opened window IS the maximized state to test.
    await page.setViewportSize({ width: 800, height: 900 });
    await gotoDesktop(page, adminTokens);
    await clickDock(page, 'dashboard');
    const win = page.locator('.hv-window');
    await expect(win).toBeVisible();
    await expect(win).toHaveClass(/maximized/);
    const clear = await page.evaluate(() => {
      const w = document.querySelector('.hv-window')!.getBoundingClientRect();
      const d = document.querySelector('.hv-dock')!.getBoundingClientRect();
      return { windowBottom: Math.round(w.bottom), dockTop: Math.round(d.top) };
    });
    expect(clear.windowBottom, 'window bottom').toBeLessThanOrEqual(clear.dockTop);
  });
});

// ===========================================================================
// 9. Accessibility
// ===========================================================================

test.describe('9 — Accessibility', () => {
  test('9.1 — every dock item is a labelled <button>', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const info = await page.locator('.hv-dock-item').evaluateAll((els) =>
      els.map((e) => ({ tag: e.tagName, label: e.getAttribute('aria-label') })),
    );
    expect(info.length).toBeGreaterThan(5);
    for (const i of info) {
      expect(i.tag).toBe('BUTTON');
      expect(i.label && i.label.length).toBeGreaterThan(0);
    }
  });

  test('9.2 — no duplicate id attributes on the desktop', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const dupes = await page.evaluate(() => {
      const ids = [...document.querySelectorAll('[id]')].map((e) => e.id).filter(Boolean);
      return [...new Set(ids.filter((v, i) => ids.indexOf(v) !== i))];
    });
    expect(dupes).toEqual([]);
  });
});

// ===========================================================================
// 10. Console & network
// ===========================================================================

test.describe('10 — Console & network', () => {
  test('10.1 — no uncaught console errors while the desktop loads', async ({ page }) => {
    const errors: string[] = [];
    page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
    page.on('pageerror', (e) => errors.push(String(e)));
    await gotoDesktop(page, adminTokens);
    await page.waitForTimeout(1500);
    const real = errors.filter((e) => !/favicon|ResizeObserver|Failed to load resource|net::ERR/i.test(e));
    expect(real, real.join('\n')).toEqual([]);
  });

  test('10.2 — the shell never calls /api/server/restart or /shutdown on its own', async ({ page }) => {
    let hit = false;
    page.on('request', (r) => {
      if (/\/api\/server\/(restart|shutdown)/.test(r.url())) hit = true;
    });
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'dashboard').click();
    await page.waitForTimeout(1000);
    expect(hit).toBe(false);
  });
});

// ===========================================================================
// 11. Edge cases
// ===========================================================================

test.describe('11 — Edge cases', () => {
  test('11.1 — opening and closing several apps in a row leaves no stray windows', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    for (const id of ['dashboard', 'docker', 'dashboard']) {
      await clickDock(page, id);
      await page.waitForTimeout(150);
    }
    // 2 distinct apps → 2 windows (the repeat focuses, not duplicates).
    await expect(windows(page)).toHaveCount(2);
    for (let i = 0; i < 2; i++) {
      await page.locator('[data-testid="win-close"]').first().click();
    }
    await expect(windows(page)).toHaveCount(0);
  });

  test('11.2 — pressing Escape with nothing open does not break the desktop', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.keyboard.press('Escape');
    await page.keyboard.press('Escape');
    await expect(page.locator('.hv-desktop')).toBeVisible();
  });

  // W10: `?hv-render-error` arms WindowFrame's RenderErrorProbe, which throws
  // on first render. The window body must degrade to an announced fallback
  // (role="alert") while the chrome stays alive, and Reload must re-render
  // the real route content.
  test('11.3 — a crashing window body shows the alert fallback and Reload recovers it', async ({ page }) => {
    await seedBrowser(page, adminTokens);
    await page.goto('/desktop?hv-render-error', { waitUntil: 'domcontentloaded' });
    await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
    await page
      .addStyleTag({ content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}' })
      .catch(() => {});
    await clickDock(page, 'dashboard');
    await expect(windows(page)).toHaveCount(1);

    const fallback = page.locator('[data-testid="hv-render-error"]');
    await expect(fallback).toBeVisible();
    await expect(fallback).toHaveAttribute('role', 'alert');
    await expect(fallback).toContainText('This panel failed to render.');

    // The shell and the window chrome survive the crash.
    await expect(page.locator('.hv-desktop')).toBeVisible();
    await expect(windows(page).locator('[data-testid="win-close"]')).toBeVisible();

    // Reload disarms the probe and resets the boundary → real content back.
    await page.locator('[data-testid="hv-render-retry"]').click();
    await expect(fallback).toHaveCount(0);
    await expect(windows(page).locator('[data-testid="dashboard-page"]')).toBeVisible();
  });
});

// ===========================================================================
// 12. Memory regression
// ===========================================================================

test.describe('12 — Memory regression', () => {
  test('12.1 — open/close a window 5× without DOM node accumulation', async ({ page }) => {
    test.setTimeout(150_000);
    await gotoDesktop(page, adminTokens);
    await waitForDomSettle(page);
    let baseline = 0;
    for (let i = 0; i < 5; i++) {
      await dockApp(page, 'dashboard').click({ force: true });
      await expect(windows(page)).toHaveCount(1);
      await page.locator('[data-testid="win-close"]').first().click();
      await expect(windows(page)).toHaveCount(0);
      const n = await page.evaluate(() => document.querySelectorAll('*').length);
      if (i === 0) baseline = n;
      else expect(n).toBeLessThan(baseline + 400);
    }
  });
});

// ===========================================================================
// 13. Activity taskbar
//
// A dock button (only while background tasks run) with a badge = active-task
// count, toggling a panel flush above the dock — one row per task: name /
// progress bar / %. Display-only. The panel width tracks the dock.
//
// To get a real running task we POST /api/rag/index (admin, idempotent, no
// cleanup needed — it just reindexes the Chat knowledge base in the background)
// and poll /api/activity until it shows a row. If the box has RAG disabled and
// the job finishes before it can be observed, the UI assertions that need a
// live task are skipped for that run (documented, rare).
// ===========================================================================

test.describe('13 — Activity taskbar', () => {
  const activityIcon = (page: Page) => page.locator('.hv-dock-item .hv-dock-icon-activity');
  const activityBtn = (page: Page) =>
    page.locator('.hv-dock-item:has(.hv-dock-icon-activity)');
  const panel = (page: Page) => page.locator('.hv-activity-panel');

  /** Fire a background job and wait until /api/activity reports at least one. */
  async function ensureLiveTask(ctx: APIRequestContext): Promise<boolean> {
    const h = { 'x-session-token': adminTokens.token };
    for (let attempt = 0; attempt < 3; attempt++) {
      await ctx.post('/api/rag/index', { headers: h }).catch(() => {});
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) {
        const res = await ctx.get('/api/activity', { headers: h });
        const body = await res.json().catch(() => ({}));
        if (Array.isArray(body?.data) && body.data.length > 0) return true;
        await new Promise((r) => setTimeout(r, 200));
      }
    }
    return false;
  }

  /** Wait until /api/activity is empty again (best effort) so 13.1 is clean. */
  async function waitIdle(ctx: APIRequestContext): Promise<void> {
    const h = { 'x-session-token': adminTokens.token };
    for (let i = 0; i < 30; i++) {
      const res = await ctx.get('/api/activity', { headers: h });
      const body = await res.json().catch(() => ({}));
      if (Array.isArray(body?.data) && body.data.length === 0) return;
      await new Promise((r) => setTimeout(r, 500));
    }
  }

  test('13.1 — no activity dock button when nothing is running', async ({ page, playwright }) => {
    const ctx = await newCtx(playwright);
    await waitIdle(ctx);
    await ctx.dispose();

    await gotoDesktop(page, adminTokens);
    await expect(activityIcon(page)).toHaveCount(0);
    await expect(panel(page)).toHaveCount(0);
  });

  test('13.2 — a running task shows the dock button + count badge; click opens the panel', async ({
    page,
    playwright,
  }) => {
    const ctx = await newCtx(playwright);
    const live = await ensureLiveTask(ctx);
    test.skip(!live, 'no observable background task on this host');

    await gotoDesktop(page, adminTokens);
    const btn = activityBtn(page);
    await expect(btn).toBeVisible({ timeout: 6000 });
    await expect(btn.locator('.hv-dock-badge')).toHaveText(/^\d\+?$/);
    await expect(btn).toHaveAttribute('aria-expanded', 'false');

    await btn.click();
    await expect(panel(page)).toBeVisible();
    await expect(btn).toHaveAttribute('aria-expanded', 'true');
    await expect(panel(page)).toHaveAttribute('role', 'dialog');

    const dockBox = await page.locator('.hv-dock').boundingBox();
    const panelBox = await panel(page).boundingBox();
    expect(Math.abs(panelBox!.width - dockBox!.width)).toBeLessThan(3);
    expect(panelBox!.y + panelBox!.height).toBeLessThanOrEqual(dockBox!.y + 2);

    await ctx.dispose();
  });

  test('13.3 — each row: a name, a progressbar track and a % (blank when indeterminate)', async ({
    page,
    playwright,
  }) => {
    const ctx = await newCtx(playwright);
    const live = await ensureLiveTask(ctx);
    test.skip(!live, 'no observable background task on this host');

    await gotoDesktop(page, adminTokens);
    await activityBtn(page).click();
    const row = panel(page).locator('.hv-activity-row').first();
    await expect(row).toBeVisible();
    await expect(row.locator('.hv-activity-name')).not.toBeEmpty();
    await expect(row.locator('.hv-activity-track')).toHaveAttribute('role', 'progressbar');
    const pct = ((await row.locator('.hv-activity-pct').textContent()) ?? '').trim();
    expect(pct === '' || /^\d{1,3}%$/.test(pct)).toBeTruthy();

    await ctx.dispose();
  });

  test('13.4 — clicking a row does nothing (display-only)', async ({ page, playwright }) => {
    const ctx = await newCtx(playwright);
    const live = await ensureLiveTask(ctx);
    test.skip(!live, 'no observable background task on this host');

    await gotoDesktop(page, adminTokens);
    await activityBtn(page).click();
    const url = page.url();
    const winCount = await windows(page).count();
    await panel(page).locator('.hv-activity-row').first().click();
    await expect(panel(page)).toBeVisible();
    expect(page.url()).toBe(url);
    await expect(windows(page)).toHaveCount(winCount);

    await ctx.dispose();
  });

  test('13.5 — panel closes on Escape, outside click and a second button press', async ({
    page,
    playwright,
  }) => {
    const ctx = await newCtx(playwright);
    const live = await ensureLiveTask(ctx);
    test.skip(!live, 'no observable background task on this host');

    await gotoDesktop(page, adminTokens);
    const btn = activityBtn(page);

    await btn.click();
    await expect(panel(page)).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(panel(page)).toBeHidden();

    await btn.click();
    await expect(panel(page)).toBeVisible();
    await page.locator('.hv-desktop').click({ position: { x: 5, y: 5 } });
    await expect(panel(page)).toBeHidden();

    await btn.click();
    await expect(panel(page)).toBeVisible();
    await btn.click();
    await expect(panel(page)).toBeHidden();

    await ctx.dispose();
  });

  test('13.6 — security: no task detail / path in localStorage or the URL', async ({
    page,
    playwright,
  }) => {
    const ctx = await newCtx(playwright);
    const live = await ensureLiveTask(ctx);
    test.skip(!live, 'no observable background task on this host');

    await gotoDesktop(page, adminTokens);
    await activityBtn(page).click();
    await expect(panel(page)).toBeVisible();

    expect(page.url()).not.toMatch(/rag_index|activity|job/i);
    const ls = await page.evaluate(() => JSON.stringify(localStorage));
    expect(ls).not.toMatch(/rag_index|hv-activity|activityStore/);

    await ctx.dispose();
  });

  test('13.7 — the shell polls /api/activity and never /downloader/tasks on its own', async ({
    page,
  }) => {
    const calls: string[] = [];
    page.on('request', (r) => {
      const u = r.url();
      if (u.includes('/api/activity')) calls.push('activity');
      if (u.includes('/api/downloader/tasks') || u.includes('/api/downloader/global-stat')) {
        calls.push('downloader');
      }
    });
    await gotoDesktop(page, adminTokens);
    await page.waitForTimeout(3000);
    expect(calls.filter((c) => c === 'activity').length).toBeGreaterThan(0);
    expect(calls).not.toContain('downloader');
  });

  test('13.8 — responsive: the panel stays in the viewport at 360 / 768 / 1920', async ({
    page,
    playwright,
  }) => {
    const ctx = await newCtx(playwright);
    const live = await ensureLiveTask(ctx);
    test.skip(!live, 'no observable background task on this host');

    await gotoDesktop(page, adminTokens);
    for (const width of [360, 768, 1920]) {
      await page.setViewportSize({ width, height: 800 });
      await activityBtn(page).click();
      const box = await panel(page).boundingBox();
      expect(box!.x).toBeGreaterThanOrEqual(-1);
      expect(box!.x + box!.width).toBeLessThanOrEqual(width + 1);
      await page.keyboard.press('Escape');
    }
    await ctx.dispose();
  });
});

// ===========================================================================
// 14. Chrome geometry & material
// ===========================================================================
// The floating chrome (topbar pill + dock) is styled from a small set of
// dedicated `--hv-chrome-*` tokens, mirrored by hand in `snapUtils.ts`. These
// tests pin the parts that are easy to break silently: the JS/CSS lockstep,
// the token pair that drives each dock tile AND its running dot, the opaque
// paper material (a dark wallpaper must never wash the ink out), and the
// tooltip — which is pure CSS and previously had no coverage at all.

test.describe('14 — Chrome geometry & material', () => {
  test('14.1 — the dock and topbar keep-out in JS mirrors the CSS tokens', async ({ page }) => {
    // `snapUtils.ts` re-states the chrome geometry by hand because initial
    // placement happens before layout exists. That mirror has silently drifted
    // before (dockBottom was stale for months), so pin it to the tokens.
    await page.setViewportSize({ width: 1440, height: 900 });
    await gotoDesktop(page, adminTokens);
    const tokens = await page.evaluate(() => {
      // A custom property holding `calc(8px + 62px)` computes to that literal
      // token stream — `calc()` is only evaluated when it lands in a REAL
      // property. So read the token's value through `height` on a throwaway
      // probe: that forces the arithmetic and yields a px number. Reading
      // `getPropertyValue()` directly returns "calc(8px + 62px)", and
      // `parseFloat` of that is NaN, not the length.
      const probe = document.createElement('div');
      probe.style.cssText = 'position:absolute;visibility:hidden;pointer-events:none';
      document.body.appendChild(probe);
      const px = (name: string) => {
        probe.style.height = `var(${name})`;
        return parseFloat(getComputedStyle(probe).height) || -1;
      };
      const out = {
        topbarKeepout: px('--hv-chrome-topbar-keepout'),
        dockKeepout: px('--hv-chrome-dock-keepout'),
        dockOffset: px('--hv-chrome-dock-offset'),
        topbarH: px('--hv-chrome-topbar-h'),
        topbarOffsetY: px('--hv-chrome-topbar-offset-y'),
        dockH: px('--hv-chrome-dock-h'),
      };
      probe.remove();
      return out;
    });
    expect(tokens.topbarKeepout, '--hv-chrome-topbar-keepout').toBe(
      DEFAULT_SNAP_CONFIG.topbarHeight,
    );
    expect(tokens.dockKeepout, '--hv-chrome-dock-keepout').toBe(
      DEFAULT_SNAP_CONFIG.dockBottom + DEFAULT_SNAP_CONFIG.dockHeight,
    );
    expect(tokens.dockOffset, '--hv-chrome-dock-offset').toBe(DEFAULT_SNAP_CONFIG.dockBottom);
    // …and spell out where the dock's band comes from: the DOCK's own height,
    // not the topbar's. The two pills once shared one height token, which is
    // exactly what made a taller dock impossible; the assertion above would go
    // red on that regression, this one says why.
    expect(tokens.dockKeepout, 'dock keep-out = dock offset + dock height').toBe(
      tokens.dockOffset + tokens.dockH,
    );
    expect(tokens.topbarKeepout, 'topbar keep-out = topbar offset + topbar height').toBe(
      tokens.topbarOffsetY + tokens.topbarH,
    );
  });

  test('14.2 — every dock tile has a running-dot colour matching its own gradient', async ({
    page,
  }) => {
    // `--hv-app-<id>-c` is the dot under a running app; `--hv-app-<id>-grad`
    // is its tile. They are separate tokens, so a new app whose tile falls
    // back to the generic accent gradient while its dot uses its own colour
    // ships a tile/dot mismatch that no code review catches. Resolve both and
    // require the dot to be the gradient's darker stop.
    await gotoDesktop(page, adminTokens);
    const pairs = await page.evaluate(() => {
      const root = getComputedStyle(document.documentElement);
      const out: { id: string; dot: string; darkStop: string }[] = [];
      for (const el of Array.from(document.querySelectorAll('.hv-dock-item'))) {
        const id = (el.getAttribute('data-testid') || '').replace(/^dock-/, '');
        if (!id) continue;
        const dot = root.getPropertyValue(`--hv-app-${id}-c`).trim().toLowerCase();
        const grad = root.getPropertyValue(`--hv-app-${id}-grad`).trim();
        const stops = grad.match(/#[0-9a-f]{3,8}/gi);
        if (!dot || !stops || stops.length < 2) continue;
        out.push({ id, dot, darkStop: stops[stops.length - 1].toLowerCase() });
      }
      return out;
    });
    expect(pairs.length, 'dock tiles carrying both tokens').toBeGreaterThan(3);
    for (const p of pairs) {
      // The Launchpad launcher is deliberately off this ladder: it is not an
      // app, its tile is pale and its dot stays brand indigo.
      if (p.id === 'launchpad') continue;
      expect(p.dot, `${p.id}: dot vs dark gradient stop`).toBe(p.darkStop);
    }
  });

  test('14.3 — the floating pills are opaque paper (no blur, no transparency)', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const surfaces = await page.evaluate(() => {
      const read = (sel: string) => {
        const el = document.querySelector(sel);
        if (!el) return null;
        const s = getComputedStyle(el);
        return {
          backdrop: s.backdropFilter || s.getPropertyValue('-webkit-backdrop-filter'),
          bg: s.backgroundColor,
        };
      };
      return { pill: read('.hv-menubar'), dock: read('.hv-dock') };
    });
    for (const [name, s] of Object.entries(surfaces)) {
      expect(s, `${name} exists`).not.toBeNull();
      expect(s!.backdrop, `${name} backdrop-filter`).toBe('none');
      // rgb(...) with no 4th (alpha) component = fully opaque.
      expect(s!.bg, `${name} background is opaque`).toMatch(/^rgb\(/);
    }
  });

  test('14.4 — hovering a dock tile reveals its tooltip on the chrome fill', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await gotoDesktop(page, adminTokens);
    const tile = page.locator('.hv-dock-item').first();
    await expect(tile).toBeVisible();
    const label = await tile.getAttribute('aria-label');
    expect(label, 'the tile has an accessible label to show').toBeTruthy();
    await tile.hover();

    const tip = await page.evaluate(() => {
      const el = document.querySelector('.hv-dock-item');
      if (!el) return null;
      const s = getComputedStyle(el, '::before');
      const root = getComputedStyle(document.documentElement);
      return {
        content: s.content,
        background: s.backgroundColor,
        tokenBg: root.getPropertyValue('--hv-tooltip-bg').trim(),
        fontSize: s.fontSize,
        radius: s.borderTopLeftRadius,
      };
    });
    expect(tip).not.toBeNull();
    // It renders the tile's aria-label, so a tile without one shows nothing.
    expect(tip!.content).toContain(label!);
    // It waits --hv-chrome-tooltip-delay before fading in — give it the beat.
    await expect
      .poll(() =>
        page.evaluate(() => {
          const el = document.querySelector('.hv-dock-item');
          return el ? getComputedStyle(el, '::before').opacity : '0';
        }),
      )
      .toBe('1');

    const rgb = (hex: string) => {
      const h = hex.replace('#', '');
      const n = parseInt(
        h.length === 3
          ? h
              .split('')
              .map((c) => c + c)
              .join('')
          : h,
        16,
      );
      return `rgb(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255})`;
    };
    expect(tip!.background, 'tooltip fill is --hv-tooltip-bg').toBe(rgb(tip!.tokenBg));
    expect(parseFloat(tip!.radius), 'tooltip radius').toBeGreaterThan(0);
    expect(parseFloat(tip!.fontSize), 'tooltip font-size').toBeLessThanOrEqual(12);
  });

  // --- App window chrome ("AppWindow" mock, Figma "Sans titre" node 3:76) ---
  // Every app window is the same skeleton: an opaque paper frame in a
  // slate-300 hairline under one soft shadow, a 44px header bar carrying a
  // 28px app-gradient icon tile, a 14px title and the amber / blue / red
  // control trio inline on the right — all three on one vertical centre line,
  // and no subtitle. These tests pin the mock's exact geometry, colours and the
  // header reserve that keeps the trio off the title — all of it is CSS-only,
  // so nothing else would catch a drift.

  test('14.5 — an app window is opaque paper in a slate hairline', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'dashboard').click();
    const win = windows(page).first();
    await expect(win).toBeVisible();
    const s = await win.evaluate((el) => {
      const c = getComputedStyle(el);
      return {
        bg: c.backgroundColor,
        border: c.borderTopColor,
        borderWidth: c.borderTopWidth,
        radius: c.borderTopLeftRadius,
        shadow: c.boxShadow,
        backdrop: c.backdropFilter || c.getPropertyValue('-webkit-backdrop-filter'),
      };
    });
    // Opaque (rgb, no alpha component) — a dark wallpaper must never show
    // through the window the way the old transparent frame allowed.
    expect(s.bg, 'window surface is opaque paper').toBe('rgb(255, 255, 255)');
    expect(s.border, 'window hairline is --hv-window-border (#CBD5E1)').toBe('rgb(203, 213, 225)');
    expect(parseFloat(s.borderWidth), 'hairline width').toBe(1);
    // The one surface corner (--hv-window-radius): the same 18px a monitor
    // widget, a modal and a popover use, so the shell reads as one material.
    expect(parseFloat(s.radius), 'window radius is the surface stop').toBe(18);
    expect(s.shadow, 'mock shadow, both layers').toContain('rgba(0, 0, 0, 0.06)');
    expect(s.shadow, 'mock shadow, both layers').toContain('rgba(0, 0, 0, 0.08)');
    expect(s.backdrop, 'no blur behind a window').toBe('none');
  });

  test('14.6 — the header is the mock 44px bar with a 28px tile and a 14px title', async ({
    page,
  }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'dashboard').click();
    const win = windows(page).first();
    const hdr = win.locator('.hv-winhdr');
    await expect(hdr).toBeVisible();

    const bar = await hdr.evaluate((el) => {
      const c = getComputedStyle(el);
      return {
        height: el.getBoundingClientRect().height,
        bottom: el.getBoundingClientRect().bottom,
        minHeight: c.minHeight,
        padY: [c.paddingTop, c.paddingBottom],
        padLeft: c.paddingLeft,
        gap: c.gap,
      };
    });
    // The standardized bar is icon tile + title + the window-control trio and
    // nothing else — no subtitle — so the mock's 44px (8 + a 28px control row +
    // 8) is now the bar's REAL height, not just its floor. A header that grows
    // again means something crept back into it.
    expect(bar.minHeight, 'the mock bar height is the floor').toBe('44px');
    expect(Math.round(bar.height), 'a title-only header renders at exactly 44px').toBe(44);
    expect(bar.padY, 'header vertical padding').toEqual(['8px', '8px']);
    expect(parseFloat(bar.padLeft), 'header left padding').toBe(12);
    expect(parseFloat(bar.gap), 'header gap').toBe(12);
    // The rule lives on `.hv-winhdr-wrap`, not on the bar itself: the wrap
    // also contains the optional tab row, so the mock's single underline
    // closes the whole header block rather than the bar alone. Reading the
    // bar's own `borderBottomColor` returns the inherited `currentColor` (the
    // ink, #1F2733) and can never match — measure the wrap.
    const wrap = await win.locator('.hv-winhdr-wrap').evaluate((el) => {
      const c = getComputedStyle(el);
      return {
        color: c.borderBottomColor,
        width: c.borderBottomWidth,
        bottom: el.getBoundingClientRect().bottom,
      };
    });
    expect(wrap.color, 'header underline is --hv-winhdr-border (#E2E8F0)').toBe(
      'rgb(226, 232, 240)',
    );
    expect(parseFloat(wrap.width), 'header underline is a 1px hairline').toBe(1);
    // With no tab row the wrap *is* the bar, so the hairline must sit flush
    // against the bar's bottom edge — that flush rule is the mock's look.
    if ((await win.locator('.hv-winhdr-tabs').count()) === 0) {
      expect(
        Math.round(wrap.bottom - bar.bottom),
        'the underline closes the bar itself when there is no tab row',
      ).toBe(1);
    }

    const tile = await win.locator('.hv-winhdr-icon').evaluate((el) => {
      const c = getComputedStyle(el);
      const r = el.getBoundingClientRect();
      return { w: r.width, h: r.height, radius: c.borderTopLeftRadius, bg: c.backgroundImage };
    });
    // The tile is the SAME size as a window-control square (--hv-winctrl-size,
    // 28px) — the app's identity glyph and the min/max/close trio are one
    // control row and must read as the same size.
    expect(Math.round(tile.w), 'icon tile width matches the control trio').toBe(28);
    expect(Math.round(tile.h), 'icon tile height matches the control trio').toBe(28);
    // A PERFECT CIRCLE since 2026-10-18 — the dock tiles' and topbar buttons'
    // shape language. Computed `50%`, never a px stop: a px radius would
    // square the tile back the moment --hv-winhdr-icon-size moves.
    expect(tile.radius, 'icon tile is a perfect circle').toBe('50%');
    // The tile is never flat — it carries the app's own gradient.
    expect(tile.bg, 'icon tile is a gradient, not a flat fill').toContain('gradient');

    const title = await win.locator('.hv-winhdr-title').evaluate((el) => {
      const c = getComputedStyle(el);
      return {
        size: c.fontSize,
        weight: c.fontWeight,
        color: c.color,
        lineHeight: c.lineHeight,
      };
    });
    expect(parseFloat(title.size), 'title font-size').toBe(15);
    expect(title.weight, 'title weight').toBe('600');
    expect(title.color, 'title ink is --hv-winhdr-title (#314158)').toBe('rgb(49, 65, 88)');
    expect(parseFloat(title.lineHeight), 'title line-height').toBe(20);

    // The mock draws a title-only bar, so its 44px must actually be the
    // title's own floor: 8px padding + the 20px title line + 8px padding = 36,
    // inside 44. (If the title ever outgrew the floor, the mock's bar height
    // would no longer be honest.)
    expect(
      parseFloat(bar.minHeight),
      'the title-only bar (8 + 20 + 8) fits inside the 44px floor',
    ).toBeGreaterThanOrEqual(8 + parseFloat(title.lineHeight) + 8);
  });

  test('14.7 — the control trio is 28px, inline in the header, on the mock colours', async ({
    page,
  }) => {
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'dashboard').click();
    const win = windows(page).first();
    const hdr = win.locator('.hv-winhdr');
    await expect(hdr).toBeVisible();
    const controls = win.locator('.hv-window-controls');
    await expect(controls).toBeVisible();

    const geometry = await win.evaluate((el) => {
      const h = el.querySelector('.hv-winhdr') as HTMLElement;
      const c = el.querySelector('.hv-window-controls') as HTMLElement;
      const tile = el.querySelector('.hv-winhdr-icon') as HTMLElement;
      const title = el.querySelector('.hv-winhdr-title') as HTMLElement;
      const btns = Array.from(c.querySelectorAll('.hv-cl-btn')) as HTMLElement[];
      const wb = el.getBoundingClientRect();
      const hb = h.getBoundingClientRect();
      const cb = c.getBoundingClientRect();
      const tb = tile.getBoundingClientRect();
      const ttb = title.getBoundingClientRect();
      const mid = (r: DOMRect) => r.y + r.height / 2;
      const boxes = btns.map((b) => {
        const r = b.getBoundingClientRect();
        return { w: r.width, h: r.height, x: r.x, y: r.y };
      });
      const s = btns.map((b) => {
        const cs = getComputedStyle(b);
        return { bg: cs.backgroundColor, radius: cs.borderTopLeftRadius, shadow: cs.boxShadow };
      });
      return {
        winRight: wb.x + wb.width,
        headerTop: hb.y,
        headerMid: mid(hb),
        controlsTop: cb.y,
        controlsMid: mid(cb),
        controlsRight: cb.x + cb.width,
        tileMid: mid(tb),
        tileSize: { w: tb.width, h: tb.height },
        titleMid: mid(ttb),
        gap: getComputedStyle(c).gap,
        boxes,
        styles: s,
      };
    });

    // All three are 28px PERFECT CIRCLES (2026-10-18; they were the mock's
    // 8px rounded squares), with a 6px gap. `50%` rather than a px stop: a px
    // radius would square them back the moment --hv-winctrl-size moves.
    for (const [i, b] of geometry.boxes.entries()) {
      expect(Math.round(b.w), `button ${i} width`).toBe(28);
      expect(Math.round(b.h), `button ${i} height`).toBe(28);
      expect(geometry.styles[i].radius, `button ${i} is a perfect circle`).toBe('50%');
      // Flat colour, no drop shadow (the mock draws none).
      expect(geometry.styles[i].shadow, `button ${i} has no shadow`).toBe('none');
    }
    expect(parseFloat(geometry.gap), 'gap between buttons').toBe(6);
    const gapBtw =
      geometry.boxes[1].x - (geometry.boxes[0].x + geometry.boxes[0].w);
    expect(Math.round(gapBtw), 'measured gap between buttons').toBe(6);

    // Amber minimize / blue maximize / red close — the mock's exact stops.
    expect(geometry.styles[0].bg, 'minimize is amber').toBe('rgb(255, 185, 0)');
    expect(geometry.styles[1].bg, 'maximize is blue').toBe('rgb(43, 127, 255)');
    expect(geometry.styles[2].bg, 'close is red').toBe('rgb(255, 100, 103)');

    // In the header bar on the mock's 8px / 12px insets — not floating over it.
    expect(
      Math.round(geometry.controlsTop - geometry.headerTop),
      'trio top is the header padding-top (8px)',
    ).toBe(8);
    expect(
      Math.round((geometry.winRight - 1) - geometry.controlsRight),
      'trio right inset (12px from the frame inner edge)',
    ).toBe(12);

    // Standardized header: the app's icon tile, the title and the trio all sit
    // on ONE vertical centre line, and the tile is the same size as a control
    // square. Asserted on measured boxes (not tokens) so a padding or an
    // align-items drift on any of the three shows up here.
    expect(geometry.tileSize.w, 'icon tile is the control size').toBe(28);
    expect(geometry.tileSize.h, 'icon tile is the control size').toBe(28);
    expect(geometry.tileMid, 'icon tile is centred on the header bar').toBeCloseTo(
      geometry.headerMid,
      0,
    );
    expect(geometry.controlsMid, 'control trio is centred on the header bar').toBeCloseTo(
      geometry.headerMid,
      0,
    );
    expect(geometry.titleMid, 'title is centred on the header bar').toBeCloseTo(
      geometry.headerMid,
      0,
    );
  });

  test('14.8 — the header reserve keeps the title clear of the control trio', async ({ page }) => {
    // The trio is absolutely positioned, so the bar's own right padding is the
    // ONLY thing stopping a long title from running underneath it. Measured,
    // not token-read: the reserve and the trio's real width must agree.
    for (const width of [1440, 768, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await gotoDesktop(page, adminTokens);
      await dockApp(page, 'dashboard').click();
      const win = windows(page).first();
      await expect(win).toBeVisible();

      const m = await win.evaluate((el) => {
        const text = el.querySelector('.hv-winhdr-text') as HTMLElement;
        const c = el.querySelector('.hv-window-controls') as HTMLElement;
        const wb = el.getBoundingClientRect();
        const tb = text.getBoundingClientRect();
        const cb = c.getBoundingClientRect();
        return {
          textLeft: tb.x,
          textRight: tb.x + tb.width,
          controlsLeft: cb.x,
          winRight: wb.x + wb.width,
        };
      });
      // The title block ends at or before the trio's left edge, at every width.
      expect(m.textRight, `title block clears the trio @${width}px`).toBeLessThanOrEqual(
        m.controlsLeft + 0.5,
      );
      // …and the reserve still leaves it room (a reserve that ate the whole bar
      // would satisfy the assertion above vacuously).
      expect(m.textRight, `the reserve leaves the title room @${width}px`).toBeGreaterThan(
        m.textLeft,
      );
      // The inset stays the mock's 12px (+ the 1px frame hairline) at every
      // width — the bar wrapping at 480px must not drag the trio inward.
      expect(
        Math.round(m.winRight - m.controlsLeft),
        `trio right inset @${width}px (12px inset + 1px border + 96px trio)`,
      ).toBe(109);
      // No explicit close here: the next iteration's `gotoDesktop` reloads the
      // page (clearing the window manager), and at 400px the frame is clamped
      // so the trio can sit outside the viewport, where a click can't reach it.
    }
  });

  test('14.9 — every app window renders exactly one header bar', async ({ page }) => {
    // `WindowFrame` keeps a `OWN_HEADER_APPS` set of the apps that render their
    // own <WindowHeader> and skips its fallback for them. An app left out of
    // that set gets BOTH: its own bar plus the frame's blank duplicate (the
    // fallback's sprite name has no glyph). `quotas` shipped that way on
    // 2026-09-14 — this guard walks a sample of the registry so the next app
    // can't repeat it silently.
    //
    // Opened via the `hv-open-app` window event rather than the dock: only 7
    // apps are dock buttons (dashboard/terminal/docker/health/chat/launchpad/
    // settings), and the launcher tiles are addressed by their *translated*
    // name. The event is the documented app-id handoff (Explorer → Shares,
    // Users → Quotas) and is locale-free.
    await gotoDesktop(page, adminTokens);
    for (const appId of ['dashboard', 'users', 'quotas', 'explorer', 'storage', 'docker']) {
      await page.evaluate((id) => {
        window.dispatchEvent(new CustomEvent('hv-open-app', { detail: { id } }));
      }, appId);
      // Exactly one window, so a failed close on the previous turn can't leave
      // a stale window behind for `first()` to measure instead.
      await expect(windows(page), `${appId} opened exactly one window`).toHaveCount(1);
      const win = windows(page).first();
      await expect(win, `${appId} window is visible`).toBeVisible();
      await expect(
        win.locator('.hv-winhdr'),
        `${appId} renders exactly one header bar`,
      ).toHaveCount(1);
      await win.locator('[data-testid="win-close"]').click();
      await expect(windows(page), `${appId} window closes`).toHaveCount(0);
    }
  });

  test('14.10 — the dock gap clears the hover-lift, so neighbours never touch', async ({ page }) => {
    // A hovered tile grows by `.hv-dock-item:hover { scale(1.18) }`, i.e. ~5.8px
    // on EACH side of the 56px icon (--hv-dock-item-size). With the old 4px gap
    // two neighbours overlapped mid-hover; the gap has to stay above that growth
    // or the tiles visibly collide. Measured from the live boxes at the hover
    // peak, not token-read.
    await page.setViewportSize({ width: 1440, height: 900 });
    await gotoDesktop(page, adminTokens);

    const rest = await page.evaluate(() => {
      const items = Array.from(document.querySelectorAll('.hv-dock-item')) as HTMLElement[];
      const boxes = items.map((el) => el.getBoundingClientRect());
      const gaps: number[] = [];
      for (let i = 1; i < boxes.length; i++)
        gaps.push(boxes[i].x - (boxes[i - 1].x + boxes[i - 1].width));
      return {
        count: items.length,
        minGap: Math.min(...gaps),
        containerGap: parseFloat(
          getComputedStyle(document.querySelector('.hv-dock-container') as HTMLElement).gap,
        ),
        iconW: (items[0].querySelector('.hv-dock-icon') as HTMLElement).getBoundingClientRect()
          .width,
      };
    });
    expect(rest.count, 'dock items rendered').toBeGreaterThan(3);
    expect(rest.containerGap, 'the row gap is the dock gap token (10px)').toBe(10);
    expect(rest.minGap, 'no two tiles are closer than the gap').toBeGreaterThanOrEqual(10);

    // The live check: hover a middle tile and measure the real clearance to both
    // neighbours once the lift has settled (--hv-chrome-dur-med is 260ms), then
    // require that same clearance from the growth the hover actually produced.
    const items = page.locator('.hv-dock-item');
    await items.nth(2).hover();
    await page.waitForTimeout(400);
    const hovered = await page.evaluate((idx) => {
      const els = Array.from(document.querySelectorAll('.hv-dock-item')) as HTMLElement[];
      const box = (el: HTMLElement) => el.querySelector('.hv-dock-icon')!.getBoundingClientRect();
      const prev = box(els[idx - 1]);
      const cur = box(els[idx]);
      const next = box(els[idx + 1]);
      return {
        leftClear: cur.x - (prev.x + prev.width),
        rightClear: next.x - (cur.x + cur.width),
        grew: cur.width,
      };
    }, 2);
    expect(hovered.grew, 'the hovered tile actually grew').toBeGreaterThan(rest.iconW);
    // Growth per side of the hovered tile, from the measured end state.
    const growthPerSide = (hovered.grew - rest.iconW) / 2;
    expect(growthPerSide, 'the lift is real').toBeGreaterThan(0);
    expect(
      hovered.leftClear,
      `clearance to the left neighbour must clear the ${growthPerSide.toFixed(2)}px growth`,
    ).toBeGreaterThan(0);
    expect(
      hovered.rightClear,
      `clearance to the right neighbour must clear the ${growthPerSide.toFixed(2)}px growth`,
    ).toBeGreaterThan(0);
  });
  test('14.11 — every app header bar carries the icon + title only', async ({ page }) => {
    // 24 apps, each opened + asserted + closed: ~4-5s apiece, well past the
    // suite's default 60s budget (14.9 walks only 6).
    test.setTimeout(300_000);
    // The single standardized `<WindowHeader>` bar is [APP ICON] [WINDOW TITLE]
    // and nothing else — the min / max / close trio is a sibling of the bar
    // (owned by `WindowFrame`) and the tab row lives in `.hv-winhdr-wrap`,
    // outside `.hv-winhdr`. Page-level actions (buttons, status pills,
    // checkboxes, filters) belong in a `WindowToolbar` row *under* the header,
    // or in the page body — never inside the bar.
    //
    // This walks the WHOLE `OWN_HEADER_APPS` registry, so an app that re-adds a
    // `center` / `extra` / `actions` / `actionLabel` prop (or hand-rolls markup
    // into the bar) fails here rather than silently drifting from the spec.
    const APPS = [
      'terminal', 'chat', 'docker', 'dashboard', 'users', 'groups', 'shares', 'account',
      'settings', 'explorer', 'backup', 'health', 'network', 'vpn', 'rescue', 'downloader',
      'rclone', 'storage', 'raid', 'journal', 'syncthing', 'widgets', 'virtual-machines',
      'quotas',
    ];
    // `hv-open-app` resolves against the `apps` registry, which does NOT hold
    // the two dock-only tools — `terminalApp` and `settingsApp` are appended
    // separately in Desktop.svelte, so the handoff can't reach them. Open those
    // two from the dock, whose buttons carry locale-free `dock-<id>` hooks.
    const DOCK_ONLY = ['terminal', 'settings'];
    await gotoDesktop(page, adminTokens);
    for (const appId of APPS) {
      if (appId === 'account') {
        // `account` has no launcher tile either: it opens from the user pill's
        // menu (the one non-logout item).
        await page.locator('.hv-user-pill').click();
        await page.locator('.hv-user-menu-item:not(.hv-logout)').first().click();
      } else if (DOCK_ONLY.includes(appId)) {
        await clickDock(page, appId);
      } else {
        await page.evaluate((id) => {
          window.dispatchEvent(new CustomEvent('hv-open-app', { detail: { id } }));
        }, appId);
      }
      await expect(windows(page), `${appId} opened exactly one window`).toHaveCount(1);
      const win = windows(page).first();
      await expect(win, `${appId} window is visible`).toBeVisible();
      const bar = win.locator('.hv-winhdr');
      // Every assertion below is SOFT on purpose: this guard walks the whole
      // registry, and a hard failure on app #1 would hide the other 23. Soft
      // assertions keep the loop going and report every offending app at once.
      await expect.soft(bar, `${appId} renders one header bar`).toHaveCount(1);

      // 1. Exactly one icon tile and one title.
      await expect.soft(bar.locator('.hv-winhdr-icon'), `${appId} icon tile`).toHaveCount(1);
      await expect.soft(bar.locator('.hv-winhdr-title'), `${appId} title`).toHaveCount(1);

      // 2. Neither injectable slot of the bar may be filled.
      await expect
        .soft(
          bar.locator('.hv-winhdr-actions'),
          `${appId} must not fill the header bar's action slot`,
        )
        .toHaveCount(0);
      await expect
        .soft(
          bar.locator('.hv-winhdr-center'),
          `${appId} must not fill the header bar's center slot`,
        )
        .toHaveCount(0);

      // 3. Belt and braces: nothing interactive inside the bar at all (a
      //    hand-rolled element would dodge the slot classes above).
      await expect
        .soft(
          bar.locator('button, input, select, textarea, a, label, [role="button"]'),
          `${appId} header bar must hold no interactive control`,
        )
        .toHaveCount(0);

      // 4. …and nothing *non*-interactive either. Checks 2-3 only look for the
      //    injectable slots and for controls, so a plain `<span>` badge (or a
      //    status dot, or a stray wrapper) added to the bar slipped past both.
      //    The bar has exactly two pieces by construction — the icon tile and
      //    the title block — so everything *outside* the tile must be exactly
      //    `.hv-winhdr-text` then `.hv-winhdr-title`, in document order.
      if ((await bar.count()) === 1) {
        const shape = await bar.evaluate((el) => {
          const icon = el.querySelector('.hv-winhdr-icon');
          return Array.from(el.querySelectorAll('*'))
            .filter((n) => n !== icon && !(icon && icon.contains(n)))
            // Strip Svelte's scope-hash token (`svelte-1abc2de`): the compiler
            // appends it to every element targeted by the component's own
            // `<style>`, and `WindowHeader.svelte` styles both of these classes.
            // Only the design-system classes are ours to assert on.
            .map((n) =>
              (n.getAttribute('class') ?? '')
                .split(/\s+/)
                .filter((c) => c && !c.startsWith('svelte-'))
                .join(' '),
            );
        });
        expect.soft(shape, `${appId} header bar holds only the icon tile + the title`).toEqual([
          'hv-winhdr-text',
          'hv-winhdr-title',
        ]);

        // 5. The icon tile is the one place an app's own markup is free-form,
        //    so it is the one place check 4 cannot see into: pin it to the
        //    single glyph the design system mandates (an extra sibling element
        //    — the badge case — makes this 2).
        await expect
          .soft(
            bar.locator('.hv-winhdr-icon > *'),
            `${appId} icon tile holds exactly one glyph`,
          )
          .toHaveCount(1);
      }

      await win.locator('[data-testid="win-close"]').click();
      await expect(windows(page), `${appId} window closes`).toHaveCount(0);
    }
  });

  test('14.12 — every button on every screen carries the one control corner', async ({ page }) => {
    // One control corner, everywhere (2026-10-18). The radius scale used to
    // carry three control stops (6 / 8 / 10px) plus hand-written `rounded-xl`
    // overrides, so the same button read differently depending on which screen
    // it lived on — and a button next to the input it submits did not line up.
    // Every `<button>` / `[role="button"]` now resolves to
    // `--hv-radius-control` (12px since the 2026-10-19 Apple-reference
    // retune), with four deliberate exceptions this test encodes:
    //   * `50%`   — the circular shape language (window trio, dock tiles,
    //               topbar buttons, avatars).
    //   * `9999px`— capsule chips/pills (launcher category chips, filter and
    //               severity chips, the user pill).
    //   * `0px`   — table/list rows whose hairline separators define them
    //               (Explorer's file list, the Journal log) and the children of
    //               a segmented control, which the group clips.
    // Anything else (12px on a button, an 8px leftover, a px literal) fails
    // here, for every app in the registry, in one pass.
    test.setTimeout(300_000);
    const APPS = [
      'terminal', 'chat', 'docker', 'dashboard', 'users', 'groups', 'shares',
      'settings', 'explorer', 'backup', 'health', 'network', 'vpn', 'rescue',
      'downloader', 'rclone', 'storage', 'raid', 'journal', 'syncthing', 'widgets',
      'virtual-machines', 'quotas',
    ];
    const DOCK_ONLY = ['terminal', 'settings'];
    const ALLOWED = new Set(['12px', '50%', '9999px', '0px']);
    await gotoDesktop(page, adminTokens);
    for (const appId of APPS) {
      if (DOCK_ONLY.includes(appId)) await clickDock(page, appId);
      else {
        await page.evaluate((id) => {
          window.dispatchEvent(new CustomEvent('hv-open-app', { detail: { id } }));
        }, appId);
      }
      await expect(windows(page), `${appId} opened exactly one window`).toHaveCount(1);
      const win = windows(page).first();
      await expect(win, `${appId} window is visible`).toBeVisible();

      const corners = await win.evaluate(() => {
        const out: Record<string, string[]> = {};
        for (const el of Array.from(
          document.querySelectorAll<HTMLElement>('button, [role="button"]'),
        )) {
          const r = getComputedStyle(el).borderTopLeftRadius;
          out[r] ??= [];
          if (out[r].length < 3) {
            out[r].push(
              (el.getAttribute('class') ?? '')
                .split(/\s+/)
                .filter((c) => c && !c.startsWith('svelte-'))
                .slice(0, 4)
                .join(' '),
            );
          }
        }
        return out;
      });
      for (const [radius, examples] of Object.entries(corners)) {
        expect
          .soft(ALLOWED.has(radius), `${appId}: unexpected button corner ${radius} on ${examples.join(' | ')}`)
          .toBe(true);
      }
      // The control corner must actually be the 12px token, not a coincidence.
      const token = await page.evaluate(() =>
        getComputedStyle(document.documentElement).getPropertyValue('--hv-radius-control').trim(),
      );
      expect.soft(token, `${appId}: --hv-radius-control`).toBe('12px');

      await win.locator('[data-testid="win-close"]').click();
      await expect(windows(page), `${appId} window closes`).toHaveCount(0);
    }
  });

  test('14.13 — the frame footer glyphs take the header trio’s 14px step', async ({ page }) => {
    // The two rows of window chrome — the header's min/max/close trio and the
    // frame footer's help / resize grip — must read as ONE icon size. The
    // footer shipped with 16px glyphs next to the trio's 14px ones, and since
    // its help / grip assets are full-bleed circles (18.5/24 of their box)
    // while the trio's are inset, its 13.75px of ink sat above the trio's
    // 8–12px: the strip looked a size larger than the bar above it. Both are
    // now clamped to `--hv-winctrl-glyph` (14px), the same "CSS owns the glyph
    // box" rule the header's app icon tile already follows (its SVG carries no
    // size attribute either), so the two rows cannot drift apart again.
    await gotoDesktop(page, adminTokens);
    await dockApp(page, 'dashboard').click();
    const win = windows(page).first();
    await expect(win.locator('.hv-winframe-footer')).toBeVisible();

    const m = await win.evaluate((el) => {
      const box = (n: Element) => {
        const r = n.getBoundingClientRect();
        return { w: Math.round(r.width), h: Math.round(r.height) };
      };
      const css = (n: Element) => {
        const c = getComputedStyle(n);
        return { w: c.width, h: c.height };
      };
      const trio = Array.from(el.querySelectorAll('.hv-cl-btn svg')).map((s) => css(s));
      const footer = Array.from(el.querySelectorAll('.hv-winframe-fbtn')).map((b) => {
        const g = b.querySelector('svg') as Element;
        return {
          id: b.getAttribute('data-testid'),
          btn: box(b),
          glyph: css(g),
          rotated: getComputedStyle(g).transform !== 'none',
        };
      });
      return {
        trio,
        footer,
        strip: box(el.querySelector('.hv-winframe-footer') as Element),
        token: getComputedStyle(el.querySelector('.hv-winframe-fbtn') as Element)
          .getPropertyValue('--hv-winctrl-glyph')
          .trim(),
      };
    });

    // One token, and it is the trio's own step.
    expect(m.token, '--hv-winctrl-glyph').toBe('14px');
    expect(m.trio, 'the header trio still renders three glyphs').toHaveLength(3);
    for (const [i, g] of m.trio.entries()) {
      expect(g.w, `trio glyph ${i} width`).toBe('14px');
      expect(g.h, `trio glyph ${i} height`).toBe('14px');
    }

    // Same 28px button, same 14px glyph box as the trio — measured, not read
    // from the tokens the two rules happen to share.
    expect(m.footer.map((f) => f.id)).toEqual(['win-help', 'win-resize-grip']);
    for (const f of m.footer) {
      expect(f.btn.w, `${f.id} button width`).toBe(28);
      expect(f.btn.h, `${f.id} button height`).toBe(28);
      expect(f.glyph.w, `${f.id} glyph width matches the trio`).toBe(m.trio[0].w);
      expect(f.glyph.h, `${f.id} glyph height matches the trio`).toBe(m.trio[0].h);
    }
    // Only the grip is turned 45°; its rendered diagonal (14 × √2 ≈ 19.8px)
    // must stay inside the 28px button, never spill out of the strip.
    expect(m.footer[0].rotated, 'the help glyph is not rotated').toBe(false);
    expect(m.footer[1].rotated, 'the grip glyph is the rotated one').toBe(true);

    // The strip itself is unchanged chrome: a thin 36px band.
    expect(m.strip.h, 'frame footer height').toBeGreaterThanOrEqual(36);
  });
});

// ===========================================================================
// 15. Topbar circular icon buttons
// ===========================================================================

// The pill's three buttons have been through both shapes: icon-only squares,
// then icon + label chips from 901px up, and since 2026-10-18 icon-only
// CIRCLES — the same shape language as the dock's circular app tiles, inside
// the capsule pill. Since the dock proportions pass of the same day the
// diameter is `--hv-chrome-topbar-btn-size`: **0.775 × the pill height**, the
// dock tile's own ratio (56px in the 72px desktop pill, 44px in the phone's
// 56px one). There is no label regime left, so this section pins the circle
// geometry (uniform size, `50%` radius), its stability across widths, the
// resulting DOM order, the name that replaced the visible label (aria-label +
// the shared CSS tooltip), and the notification badge that now hangs off a
// circular button.
test.describe('15 — Topbar circular icon buttons', () => {
  test('15.1 — at 1440px each topbar button is a 56px perfect circle, icon-only', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await gotoDesktop(page, adminTokens);
    const sizes: number[] = [];
    for (const id of TOPBAR_IDS) {
      const btn = topbarBtn(page, id);
      await expect(btn, `${id} exists`).toBeVisible();
      const box = (await btn.boundingBox())!;
      const radius = await btn.evaluate((el) => getComputedStyle(el).borderRadius);

      // `50%` — not a px radius: that is the only value that keeps the control
      // a circle whatever the tile size is (the phone renders the same button
      // at 44px inside a 56px pill).
      expect(radius, `${id} is a circle`).toBe('50%');
      // 0.775 × the 72px pill — the dock tile's proportion, so both pills carry
      // the same icon-to-bar weight.
      expect(Math.round(box.height), `${id} height`).toBe(56);
      expect(Math.round(box.width), `${id} width`).toBe(56);
      sizes.push(Math.round(box.width));

      // Icon-only: the glyph is there, the label markup is gone for good (it
      // was a `display:none` span from 901px down before this redesign).
      await expect(btn.locator('svg').first(), `${id} shows its glyph`).toBeVisible();
      await expect(btn.locator('.hv-topbar-btn-label'), `${id} carries no label`).toHaveCount(0);
    }
    // Uniform size across the row — the "one tile size" rule of the dock.
    expect(new Set(sizes).size, 'every button is the same size').toBe(1);
  });

  test('15.2 — the circles are identical at every width (no breakpoint regime left)', async ({ page }) => {
    // One shape, one token: 56px in the 72px desktop pill, and only the phone
    // drops to 44px — because its pill is 56px and the ratio (0.775) is what
    // must survive, not a pixel count. Both stay above the 44px touch floor.
    for (const w of [1440, 900, 768, 400]) {
      const expected = w > 480 ? 56 : 44;
      await page.setViewportSize({ width: w, height: 900 });
      await gotoDesktop(page, adminTokens);
      for (const id of TOPBAR_IDS) {
        const btn = topbarBtn(page, id);
        const box = (await btn.boundingBox())!;
        const radius = await btn.evaluate((el) => getComputedStyle(el).borderRadius);
        expect(radius, `${id} radius @${w}px`).toBe('50%');
        expect(Math.round(box.width), `${id} width @${w}px`).toBe(expected);
        expect(Math.round(box.height), `${id} height @${w}px`).toBe(expected);
        await expect(btn.locator('svg').first(), `${id} glyph @${w}px`).toBeVisible();
      }
    }
  });

  test('15.3 — the buttons keep their order and Power is still last', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await gotoDesktop(page, adminTokens);
    const order = await page
      .locator('button[data-testid^="topbar-"]')
      .evaluateAll((els) =>
        els.map((e) => (e.getAttribute('data-testid') || '').replace('topbar-', '')),
      );
    expect(order).toEqual([...TOPBAR_IDS]);

    // Sections 7.x used to reach the power button with
    // `.hv-topbar-btn').last()`; they now use its testid. This keeps the
    // positional fact they relied on asserted, so a future reorder still
    // trips something instead of silently retargeting them.
    const lastTestId = await page
      .locator('.hv-topbar-btn')
      .evaluateAll((els) => (els.length ? els[els.length - 1].getAttribute('data-testid') : null));
    expect(lastTestId).toBe('topbar-power');
  });

  test('15.4 — the notification badge is a 16px corner marker on the circular button', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await gotoDesktop(page, adminTokens);

    const m = await page.evaluate(() => {
      const btn = document.querySelector('[data-testid="topbar-notifications"]');
      const glyph = btn?.querySelector('.hv-notif-bell-icon');
      const pill = document.querySelector('.hv-menubar');
      if (!btn || !glyph || !pill) return null;

      // The probe must be a CLONE of a template element: Svelte scopes this
      // component's CSS with a hash class that `document.createElement` would
      // miss, so an injected span would render unstyled and measure nothing.
      // The bell span replaces the (now removed) label span as the template —
      // any element of this component carries the same hash.
      const hash = [...glyph.classList].filter((c) => c.startsWith('svelte-')).join(' ');
      const probe = glyph.cloneNode(false) as HTMLElement;
      btn.appendChild(probe);

      // The badge classes carry `animation: hvChromePop … backwards`, and that
      // keyframe's `from` is `transform: scale(0.4)`. This probe is measured
      // synchronously with the class swap, i.e. while the animation is still in
      // its BEFORE phase — `backwards` applies the from-keyframe, so
      // getBoundingClientRect would report layout × 0.4 (16px → 6.4px) and this
      // test would fail on a correct stylesheet. It only ever affected the
      // probe: the real badge's animation finished long ago. `animation: none`
      // inline beats the class rule, so the rect below is the true layout box.
      // (getComputedStyle is transform-independent and would have been immune,
      // but the positional assertions genuinely need the painted box.)
      probe.style.animation = 'none';

      const rect = (el: Element) => {
        const r = el.getBoundingClientRect();
        return {
          top: r.top,
          right: r.right,
          bottom: r.bottom,
          left: r.left,
          width: r.width,
          height: r.height,
        };
      };
      probe.className = `${hash} hv-notification-badge`;
      const badge = { ...rect(probe), height: getComputedStyle(probe).height };
      // Same probe, the dock's class: proves the two badges are decoupled
      // again rather than sharing one rule with one size.
      probe.className = `${hash} hv-dock-badge`;
      const dockHeight = getComputedStyle(probe).height;
      const dockWidth = rect(probe).width;

      const out = { badge, dockHeight, dockWidth, pill: rect(pill), btn: rect(btn) };
      probe.remove();
      return out;
    });

    expect(m, 'the notifications button, its glyph and the pill all resolve').not.toBeNull();
    const p = m!;
    // 16px is the mockup's value and what a 44px circle can carry.
    // `min-width: 16px` is what squares a single-digit count off; the rect
    // agrees because the probe's entry animation is disabled (see above).
    expect(p.badge.height, 'notification badge height').toBe('16px');
    expect(Math.round(p.badge.width), 'a single-digit badge is 16px wide').toBe(16);
    // The dock badge follows its tile: 22px on the 56px dock tile (40% of it,
    // the proportion the 26px badge held on the 64px tile before the
    // 2026-10-18 tightening). It is still deliberately NOT the notification
    // badge's 16px — the two are decoupled, not one rule with one size.
    expect(p.dockHeight, 'dock badge follows the 56px tile at 22px').toBe('22px');
    // Both badges measured square, so no scale transform survived on the probe —
    // without this, a future 0.4-scaled rect would sail through the height check
    // (computed style) and only the width assertion below would catch it.
    expect(Math.round(p.dockWidth), 'the probe measures untransformed').toBe(22);

    // Anchored to the BUTTON's top-right corner (3px out), still on the pill:
    // the button is a 56px circle now, so "the corner" is the corner of its
    // square box — the badge pokes just past the arc, as in the mockup.
    expect(p.badge.right, 'badge hangs off the button corner').toBeCloseTo(p.btn.right + 3, 0);
    expect(p.badge.top, 'badge hangs off the button corner').toBeCloseTo(p.btn.top - 3, 0);
    expect(p.badge.right, 'badge stays on the pill').toBeLessThanOrEqual(p.pill.right);
    expect(p.badge.top, 'badge stays under the pill top').toBeGreaterThanOrEqual(p.pill.top);
    // A corner marker: wholly inside the button's TOP-RIGHT QUADRANT. On the
    // old icon + label chip the badge sat above the label's line box; a 26px
    // glyph centred in a circle leaves no such band (their boxes overlap,
    // exactly as in the mockup), so the invariant is stated on the quadrants —
    // a badge that drifted onto the bell's centre, or back over the pill's
    // text, fails here.
    expect(p.badge.bottom, 'badge stays in the top half').toBeLessThanOrEqual(
      p.btn.top + p.btn.height / 2,
    );
    expect(p.badge.left, 'badge stays in the right half').toBeGreaterThanOrEqual(
      p.btn.left + p.btn.width / 2,
    );

    // …and it hangs off the button, never off the 24px bell inside it.
    await expect(page.locator('.hv-notif-bell-icon .hv-notification-badge')).toHaveCount(0);
  });

  test('15.5 — the accessible name never leaks a key and the tooltip carries it', async ({ page }) => {
    for (const w of [1440, 900]) {
      await page.setViewportSize({ width: w, height: 900 });
      await gotoDesktop(page, adminTokens);
      for (const id of TOPBAR_IDS) {
        const btn = topbarBtn(page, id);
        const aria = (await btn.getAttribute('aria-label')) || '';
        expect(aria, `${id} has an aria-label @${w}px`).not.toBe('');
        // A raw key means the translation is missing and t() fell through.
        expect(aria, `${id} aria-label is translated @${w}px`).not.toMatch(/\bdesktop\.[a-z_]+/i);
        // Icon-only control: `title` would stack a second, native tooltip on
        // top of the CSS one — the dock made the same call, so it is asserted
        // rather than left to drift back in.
        expect(await btn.getAttribute('title'), `${id} has no duplicate title`).toBeNull();
      }
    }

    // The visible label is gone, so WCAG 2.5.3 has no pair left to validate;
    // what replaces it is the dock's pattern — the name the pointer reads on
    // hover/focus is literally `attr(aria-label)` (see `.hv-topbar-btn::before`).
    await page.setViewportSize({ width: 1440, height: 900 });
    await gotoDesktop(page, adminTokens);
    const btn = topbarBtn(page, 'power');
    await btn.hover();
    // The label chip waits out --hv-chrome-tooltip-delay (120ms) before it
    // fades in, so a synchronous read right after `hover()` still sees the
    // `opacity: 0` start state — poll instead of guessing a timeout.
    await expect
      .poll(
        () => btn.evaluate((el) => getComputedStyle(el, '::before').opacity),
        { message: 'the tooltip fades in on hover', timeout: 2_000 },
      )
      .toBe('1');
    const tooltip = await btn.evaluate((el) => {
      const s = getComputedStyle(el, '::before');
      return {
        content: s.content.replace(/^"|"$/g, ''),
        background: s.backgroundColor,
      };
    });
    const aria = (await btn.getAttribute('aria-label')) || '';
    expect(tooltip.content, 'the tooltip reads the aria-label').toBe(aria);
    // Same dark inverted label chip as the dock's, not a bare pill-coloured one.
    const tooltipBg = await page.evaluate(() =>
      getComputedStyle(document.documentElement).getPropertyValue('--hv-tooltip-bg').trim(),
    );
    expect(tooltipBg).not.toBe('');
    expect(tooltip.background).not.toBe('rgba(0, 0, 0, 0)');
  });

  test('15.6 — the pill still fits at the 901px breakpoint', async ({ page }) => {
    // 8.2 only samples 360/768/1920, so 901 — the width that used to open the
    // label regime and is still the shell's "full desktop chrome" threshold —
    // stays covered. The pill no longer widens with the language, so this is
    // now a pure fit check.
    for (const w of [901, 1440]) {
      await page.setViewportSize({ width: w, height: 900 });
      await gotoDesktop(page, adminTokens);
      const m = await page.evaluate(() => {
        const pill = document.querySelector('.hv-menubar')!.getBoundingClientRect();
        const doc = document.documentElement;
        return {
          pillLeft: Math.round(pill.left),
          pillRight: Math.round(pill.right),
          scroll: doc.scrollWidth - doc.clientWidth,
        };
      });
      // The pill is anchored to the RIGHT edge, so an overflow reads as a
      // negative `left`, never as an over-large `right`.
      expect(m.pillLeft, `pill left @${w}px`).toBeGreaterThanOrEqual(0);
      expect(m.pillRight, `pill right @${w}px`).toBeLessThanOrEqual(w + 1);
      expect(m.scroll, `horizontal scroll @${w}px`).toBeLessThanOrEqual(2);
    }
  });

  test('15.7 — every locale translates the names, and the German pill fits at 901px', async ({ page }) => {
    for (const lang of ['en', 'fr', 'de', 'es']) {
      await page.setViewportSize({ width: 1440, height: 900 });
      await gotoDesktop(page, adminTokens, lang);
      const names = await page
        .locator('.hv-topbar-btn')
        .evaluateAll((els) => els.map((e) => (e.getAttribute('aria-label') || '').trim()));
      expect(names, `${lang}: three names`).toHaveLength(3);
      for (const n of names) {
        expect(n, `${lang}: a name is missing`).not.toBe('');
        expect(/\bdesktop\.[a-z_]+/i.test(n), `${lang}: "${n}" is a raw key`).toBe(false);
      }
    }

    // German is the longest of the four, so it stays the worst case — measured
    // right at the breakpoint, not at 1440.
    await page.setViewportSize({ width: 901, height: 900 });
    await gotoDesktop(page, adminTokens, 'de');
    const de = await page.evaluate(() => {
      const pill = document.querySelector('.hv-menubar')!.getBoundingClientRect();
      return {
        left: Math.round(pill.left),
        names: [...document.querySelectorAll('.hv-topbar-btn')].map((e) =>
          (e.getAttribute('aria-label') || '').trim(),
        ),
      };
    });
    expect(de.names, 'de really did apply').toContain('Benachrichtigungen');
    expect(de.left, 'the German pill still starts on-screen @901px').toBeGreaterThanOrEqual(0);
  });
});

// ===========================================================================
// 16. Notification center & What's New
// ===========================================================================

test.describe('16 — Notification center & What\'s New', () => {
  test('16.1 — the notification panel exposes the persistent-center filters', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('.hv-notif-btn').click();
    const panel = page.locator('.hv-topbar-panel');
    await expect(panel).toBeVisible();

    // Severity chips + search + unread toggle are the "center" affordances
    // (not just a toast list). They must be real, labelled controls.
    await expect(panel.locator('.severity-chip')).toHaveCount(4);
    await expect(panel.locator('.center-search')).toBeVisible();
    await expect(panel.locator('.unread-toggle input[type="checkbox"]')).toBeVisible();

    // The severity group carries an accessible name.
    await expect(panel.locator('[role="group"]')).toHaveAttribute('aria-label', /.+/);
  });

  test('16.2 — severity chips filter and are keyboard-operable', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('.hv-notif-btn').click();
    const panel = page.locator('.hv-topbar-panel');
    await expect(panel).toBeVisible();

    const errorChip = panel.locator('.severity-chip').nth(3);
    await errorChip.focus();
    await page.keyboard.press('Enter');
    await expect(errorChip).toHaveAttribute('aria-pressed', 'true');

    // With an empty notification list the filtered view shows the no-match
    // state OR the plain empty state — never a crash or a stale list.
    const empty = panel.locator('.empty-state');
    await expect(empty).toBeVisible();
  });

  test('16.3 — What\'s New modal state matches the server acknowledgement', async ({
    page,
    request,
  }) => {
    // Deterministic: read the server's per-user `seen` flag for this version,
    // then assert the modal is shown exactly when the server says unseen.
    const meta = await request.get('/api/whats-new', {
      headers: { 'x-session-token': adminTokens.token },
    });
    const body = await meta.json();
    const seen = body?.data?.seen === true;

    await gotoDesktop(page, adminTokens);
    const modal = page.locator('[data-testid="whats-new-modal"]');
    if (seen) {
      await expect(modal).toHaveCount(0);
    } else {
      await expect(modal).toBeVisible();
      // Dismissing acknowledges it server-side and closes the modal.
      await page.locator('[data-testid="whats-new-dismiss"]').click();
      await expect(modal).toHaveCount(0);
      const after = await request.get('/api/whats-new', {
        headers: { 'x-session-token': adminTokens.token },
      });
      const afterBody = await after.json();
      expect(afterBody?.data?.seen).toBe(true);
    }
  });

  test('16.4 — a stale contract header surfaces the reload banner', async ({ page }) => {
    // A bogus contract on an API call must make the shell show the reload
    // banner instead of silently failing. Drive it through the real client.
    await seedBrowser(page, adminTokens);
    // `**/api/health*`, not `**/api/health/**`: the desktop calls `/api/health`
    // with no trailing segment, so the deeper glob never matched and the 426
    // never fired — the banner assertion could not pass (fixed 2026-10-19).
    await page.route('**/api/health*', async (route) => {
      const res = await route.fetch();
      if (res.status() === 426) return route.fulfill({ response: res });
      return route.fulfill({
        status: 426,
        contentType: 'application/json',
        body: JSON.stringify({ success: false, error: 'contract mismatch' }),
      });
    });
    await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
    await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
    await expect(page.locator('[data-testid="contract-mismatch-banner"]')).toBeVisible();
  });
});
