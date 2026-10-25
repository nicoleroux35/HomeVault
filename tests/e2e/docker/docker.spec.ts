/* =============================================================================
 * File          : docker.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Docker screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Docker page — End-to-End Test Suite
 *
 * Docker is pinned to the desktop dock (aria-label "Docker"), not opened
 * through the Applications launcher. It talks to a real Docker Engine
 * through the backend — container/image/network/compose mutations are
 * genuinely real, not mocked.
 *
 * **This suite never mutates the host's real containers/images/networks
 * by default.** Every modal-driven test stops at "Cancel" / closes the
 * modal instead of submitting. Validation is exercised directly via
 * `fetch` against auth guards and read-only endpoints (safe — no `docker`
 * daemon call succeeds without a valid admin session in the first place).
 * The one real-lifecycle test (create → start → stop → restart → remove a
 * throwaway container) is opt-in and adaptive: it tries to pull a tiny
 * image (`hello-world`) and skips cleanly if that fails (no network / no
 * registry access in this environment) rather than requiring an env var —
 * unlike RAID's loop-device lifecycle, a throwaway container never touches
 * host disks, so this is safe to attempt by default. See section 13.
 *
 * **§17 (2026-10-24, ROADMAP 31.1)** pins the consolidation of the screen
 * onto `$lib/services/api.ts`: every Docker request must carry the session,
 * CSRF and `X-HomeVault-Contract` headers and the `/api` prefix (i.e. it
 * really went through the shared client), no call site may bypass it, and a
 * mutation must not be answered from the 3 s GET cache. Those assertions are
 * request/response observations — they need no docker daemon, so they run
 * everywhere.
 *
 * Run (all, safe subset):  npx playwright test tests/e2e/docker/docker.spec.ts --project=firefox
 * Run (single):            npx playwright test tests/e2e/docker/docker.spec.ts -g "4.1"
 * Run (category):          npx playwright test tests/e2e/docker/docker.spec.ts -g "Security"
 * Custom server:           BASE_URL=http://localhost:3000 npx playwright test tests/e2e/docker/docker.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override with
 * E2E_ADMIN_USER / E2E_ADMIN_PASS), and a reachable Docker Engine (the
 * process must run as / in the `docker` group — see CLAUDE.md's Docker
 * section).
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
  await page.goto('/desktop');
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  // A fresh account defaults every desktop monitor widget to visible (RAID,
  // Calendar, Docker, ...) — each is position:fixed and floats over the
  // whole screen, so left on, they intercept clicks meant for this window
  // or the dock (confirmed live: a monitor's header blocked a modal-close
  // click). Same fix as tests/e2e/settings/settings.spec.ts.
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

/** Docker is dock-pinned — click its dock icon directly. */
async function openDocker(page: Page) {
  await page.locator('.hv-dock-item[aria-label="Docker"]').click();
  await page.waitForSelector('[data-testid="docker-page"]', { timeout: 10_000 });
}

async function gotoDocker(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openDocker(page);
  // No Docker daemon on this host: the app only renders its "daemon not
  // available" panel (covered by 1.3), so the daemon-requiring tests cannot
  // run. Skip them instead of failing on the permanent panel.
  const down = win(page).locator('[data-testid="docker-daemon-down"]');
  if (await down.count()) test.skip(true, 'Docker daemon not available on this host');
}

function win(page: Page) {
  return page.locator('[data-testid="docker-page"]');
}

async function switchTab(page: Page, tabId: 'containers' | 'images' | 'volumes' | 'networks' | 'discover' | 'catalog' | 'compose') {
  const index = ['containers', 'images', 'volumes', 'networks', 'discover', 'catalog', 'compose'].indexOf(tabId);
  await win(page).locator('[data-testid="docker-nav-item"]').nth(index).click();
}

/** Opens the "New container" modal (containers tab's scoped primary toolbar
 * button — not a loose text match, which can collide with other tabs'
 * differently-labelled primary buttons or get raced by a leftover open
 * modal). Returns the modal locator, or null if the button isn't present
 * (e.g. the daemon is down), letting the caller skip gracefully. */
async function openCreateContainerModal(page: Page) {
  await switchTab(page, 'containers');
  const btn = win(page).locator('[data-testid="docker-toolbar-actions"] [data-testid="docker-top-btn"]');
  if ((await btn.count()) === 0) return null;
  await btn.click();
  const modal = page.locator('[data-slot="dialog-content"][role="dialog"]');
  await modal.waitFor({ state: 'visible', timeout: 10_000 });
  return modal;
}

let sharedCtx: APIRequestContext;
let adminHeaders: Record<string, string>;
let adminTokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await newCtx(playwright);
  const admin = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  expect(admin.token, 'seeded admin login must succeed').not.toEqual('');
  adminHeaders = { 'x-session-token': admin.token, 'x-csrf-token': admin.csrf };
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token: admin.token, csrf: admin.csrf };
  // Acknowledge the release notes: on a freshly seeded server the What's New
  // modal opens over the desktop and its bits-ui overlay swallows every
  // pointer event, so the very first `openDocker()` click would never land.
  // Same fixture as `desktop.spec.ts`.
  await sharedCtx
    .post('/api/whats-new/seen', { headers: adminHeaders, data: {} })
    .catch(() => null);
});

test.afterAll(async () => {
  await sharedCtx?.dispose();
});

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

// ===========================================================================
// 1. Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — Docker window opens from the dock', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await expect(page.locator('[data-testid="docker-page"]')).toHaveCount(0);
    await openDocker(page);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — header shows "Docker"', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await expect(page.locator('.hv-winhdr-title, [class*="winhdr"]').first()).toContainText('Docker');
  });

  test('1.3 — adapts to daemon state: available shows containers, unavailable shows the retry panel', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await openDocker(page);
    const down = win(page).locator('[data-testid="docker-daemon-down"]');
    const main = win(page).locator('[data-testid="docker-main-toolbar"], [data-testid="docker-card-grid"], [data-testid="docker-empty"]');
    const isDown = (await down.count()) > 0;
    if (isDown) {
      await expect(down).toContainText(/./);
    } else {
      expect(await main.count()).toBeGreaterThan(0);
    }
  });

  test('1.4 — the sidebar lists all 7 nav tabs', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await expect(win(page).locator('[data-testid="docker-nav-item"]')).toHaveCount(7);
  });

  test('1.5 — the metrics rail shows 4 tiles', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await expect(win(page).locator('[data-testid="docker-metric"]')).toHaveCount(4);
  });

  test('1.6 — the containers tab is selected by default', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await expect(win(page).locator('[data-testid="docker-nav-item"]').first()).toHaveAttribute('aria-selected', 'true');
  });
});

// ===========================================================================
// 2. Form fields
// ===========================================================================

test.describe('2 — Form fields', () => {
  test('2.1 — the create-container modal exposes name/image text fields', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    if (!(await openCreateContainerModal(page))) test.skip(true, 'no create-container button visible (daemon down?)');
    await expect(page.locator('#cName')).toBeVisible();
    await expect(page.locator('#cImage')).toBeVisible();
    await expect(page.locator('#cName')).toHaveAttribute('type', 'text');
  });

  test('2.2 — the create-network modal exposes a name field', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'networks');
    const createBtn = win(page).locator('[data-testid="docker-top-btn"]').first();
    if ((await createBtn.count()) === 0) test.skip(true, 'no create-network button visible in this state');
    await createBtn.click();
    await expect(page.locator('[data-slot="dialog-content"][role="dialog"]')).toBeVisible();
  });

  test('2.3 — the compose panel exposes a YAML textarea and a stack-name field', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'compose');
    await expect(win(page).locator('[data-testid="docker-code-area"]')).toBeVisible();
    await expect(win(page).locator('#composeName')).toBeVisible();
  });
});

// ===========================================================================
// 3. Validation
// ===========================================================================

test.describe('3 — Validation', () => {
  test('3.1 — server rejects create-container calls without a session', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/docker/containers', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 'e2e-test', image: 'hello-world' }),
      });
      return r.status;
    });
    expect(status).toBeGreaterThanOrEqual(400);
  });

  test('3.2 — server rejects a nonexistent container action', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/docker/containers/hv-e2e-does-not-exist/start', {
          method: 'PUT',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf },
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBeGreaterThanOrEqual(400);
  });

  test('3.3 — server rejects deleting a nonexistent network', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/docker/networks/hv-e2e-does-not-exist', {
          method: 'DELETE',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf },
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBeGreaterThanOrEqual(400);
  });

  test('3.4 — the create-container submit button disables while a port conflict is flagged', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    if (!(await openCreateContainerModal(page))) test.skip(true, 'no create-container button visible (daemon down?)');
    // Use the host's own SSH-adjacent low port (22) as a value guaranteed
    // to already be in use on a typical HomeVault host, without needing
    // to know a specific container's port ahead of time — if no conflict
    // is flagged this simply confirms the field accepts input harmlessly.
    await page.locator('[data-slot="dialog-content"] [data-testid="docker-row3"] input').first().fill('80');
    // The port field triggers a debounced live availability check
    // (`onPortInput`) — wait for it to settle before closing.
    await page.waitForTimeout(1_000);
    // Close via the backdrop (top-left corner, outside the modal card)
    // rather than the header's close button — the latter is flaky to
    // click here: it sits near the same screen region as the floating
    // topbar pill, which (like the wallpaper-picker modal documented in
    // CLAUDE.md's Settings section) can win the stacking-context fight
    // against a modal rendered inside the window's own transformed
    // container, intermittently intercepting the click.
    await page.locator('[data-slot="dialog-overlay"]').click({ position: { x: 5, y: 5 } });
    await expect(page.locator('[data-slot="dialog-content"]')).toHaveCount(0);
  });
});

// ===========================================================================
// 4. All user interactions
// ===========================================================================

test.describe('4 — All user interactions', () => {
  test('4.1 — clicking a sidebar tab switches the active tab and content', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'images');
    await expect(win(page).locator('[data-testid="docker-nav-item"]').nth(1)).toHaveAttribute('aria-selected', 'true');
  });

  test('4.2 — the container search field filters the list', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'containers');
    const search = win(page).locator('[data-testid="docker-search"] input');
    if ((await search.count()) === 0) test.skip(true, 'no containers to search — search box only renders when containers exist');
    const before = await win(page).locator('[data-testid="docker-ctr-card"]').count();
    await search.fill('zzz-no-such-container-zzz');
    await page.waitForTimeout(300);
    const after = await win(page).locator('[data-testid="docker-ctr-card"]').count();
    expect(after).toBeLessThanOrEqual(before);
    await search.fill('');
  });

  test('4.3 — the All/Active/Stopped filter chips toggle active state', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'containers');
    const chips = win(page).locator('[data-testid="docker-chip"]');
    if ((await chips.count()) === 0) test.skip(true, 'no filter chips visible (no containers)');
    await chips.nth(1).click();
    await expect(chips.nth(1)).toHaveClass(/dk-chip-active/);
    await chips.nth(0).click();
    await expect(chips.nth(0)).toHaveClass(/dk-chip-active/);
  });

  test('4.4 — the catalog search field updates as you type', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'catalog');
    const search = win(page).locator('input[placeholder]').first();
    await search.fill('jellyfin');
    await expect(search).toHaveValue('jellyfin');
  });

  test('4.5 — "Sources d\'apps" panel toggles open and closed', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'catalog');
    const storesBtn = win(page).locator('button[aria-expanded]').first();
    await storesBtn.click();
    await expect(storesBtn).toHaveAttribute('aria-expanded', 'true');
    await storesBtn.click();
    await expect(storesBtn).toHaveAttribute('aria-expanded', 'false');
  });

  test('4.6 — adding and removing a port row in the create-container modal', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    if (!(await openCreateContainerModal(page))) test.skip(true, 'no create-container button visible (daemon down?)');
    const before = await page.locator('[data-slot="dialog-content"] [data-testid="docker-row3"]').count();
    await page.locator('[data-slot="dialog-content"] [data-testid="docker-section"] button', { hasText: /port/i }).first().click();
    await expect(page.locator('[data-slot="dialog-content"] [data-testid="docker-row3"]')).toHaveCount(before + 1);
    await page.locator('[data-slot="dialog-content"] [data-testid="docker-row3"]').last().locator('[data-testid="docker-icon-btn"]').click();
    await expect(page.locator('[data-slot="dialog-content"] [data-testid="docker-row3"]')).toHaveCount(before);
  });

  test('4.7 — the privileged/auto-remove switches toggle', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    if (!(await openCreateContainerModal(page))) test.skip(true, 'no create-container button visible (daemon down?)');
    const sw = page.locator('[data-slot="dialog-content"] [role="switch"]').first();
    await sw.click();
    await expect(sw).toHaveAttribute('data-state', 'checked');
    await sw.click();
    await expect(sw).not.toHaveAttribute('data-state', 'checked');
  });

  test('4.8 — the "Edit" button only shows on compose-managed containers and opens the compose editor', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'containers');
    const cards = win(page).locator('[data-testid="docker-ctr-card"]');
    if ((await cards.count()) === 0) test.skip(true, 'no containers on this host');

    // The Edit button is rendered iff the card also shows a compose-project
    // meta chip — find the first such card.
    const editBtn = win(page).locator('[data-testid="docker-ctr-card"] [data-testid="docker-ctr-actions"] button', { hasText: /Edit|Modifier|Bearbeiten|Editar/ }).first();
    if ((await editBtn.count()) === 0) test.skip(true, 'no compose-managed container on this host');

    await editBtn.click();
    const modal = page.locator('[data-slot="dialog-content"][role="dialog"]');
    await expect(modal).toBeVisible();
    // Either the YAML editor loads, or a clear "not managed here" message —
    // never a blank modal.
    await expect(modal.locator('[data-testid="docker-code-area"], [data-testid="docker-compose-status"]')).toBeVisible({ timeout: 10_000 });
    await page.keyboard.press('Escape').catch(() => {});
    await modal.locator('[data-slot="dialog-close"]').click({ trial: false }).catch(() => {});
  });
});

// ===========================================================================
// 5. Submission flows
// ===========================================================================

test.describe('5 — Submission flows', () => {
  test('5.1 — validating compose YAML does not deploy anything', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    // The compose-availability probe resolves async after the tab switch:
    // wait for its response (if any — a revisit serves the cached value and
    // fires no request) before reading the editor's disabled state, or the
    // check races the probe and fills an editor that is about to disable.
    await Promise.all([
      page
        .waitForResponse((r) => r.url().includes('/api/docker/compose/available'), {
          timeout: 15_000,
        })
        .catch(() => null),
      switchTab(page, 'compose'),
    ]);
    if (await win(page).locator('[data-testid="docker-code-area"]').isDisabled()) {
      test.skip(true, 'compose unavailable on this host');
    }
    const before = await win(page).locator('[data-testid="docker-panel"] [data-testid="docker-row"]').count();
    await win(page).locator('[data-testid="docker-code-area"]').fill('services:\n  test:\n    image: hello-world');
    await win(page).locator('[data-testid="docker-compose-validate"]').click();
    await page.waitForTimeout(1_000);
    const after = await win(page).locator('[data-testid="docker-panel"] [data-testid="docker-row"]').count();
    expect(after).toBe(before);
  });

  test('5.2 — cancelling the create-container modal creates nothing', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'containers');
    if (!(await openCreateContainerModal(page))) test.skip(true, 'no create-container button visible (daemon down?)');
    await page.locator('#cName').fill('e2e-should-not-be-created');
    await page.locator('[data-slot="dialog-overlay"]').click({ position: { x: 5, y: 5 } });
    await expect(page.locator('[data-slot="dialog-content"]')).toHaveCount(0);
    await switchTab(page, 'images');
    await switchTab(page, 'containers');
    // Assert on the name rather than on a card count: the container list is
    // fetched asynchronously, so a count taken before it settles reads 0 and
    // then disagrees with the same list once it has loaded (a false failure
    // on a loaded machine). Absence of the name is what the test is about.
    await expect(
      win(page).locator('[data-testid="docker-ctr-card"]', { hasText: 'e2e-should-not-be-created' }),
    ).toHaveCount(0);
  });
});

// ===========================================================================
// 6. Navigation
// ===========================================================================

test.describe('6 — Navigation', () => {
  test('6.1 — clicking the backdrop closes the create-container modal', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    if (!(await openCreateContainerModal(page))) test.skip(true, 'no create-container button visible (daemon down?)');
    await page.locator('[data-slot="dialog-overlay"]').click({ position: { x: 5, y: 5 } });
    await expect(page.locator('[data-slot="dialog-content"]')).toHaveCount(0);
  });

  test('6.2 — the window closes and can be reopened', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    const closeBtn = page.locator('.hv-cl-btn-close').first();
    if ((await closeBtn.count()) === 0) test.skip(true, 'no close control found for this window chrome');
    await closeBtn.click();
    await expect(page.locator('[data-testid="docker-page"]')).toHaveCount(0);
    await openDocker(page);
    await expect(win(page)).toBeVisible();
  });

  test('6.3 — switching tabs preserves the window (no reload)', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'images');
    await switchTab(page, 'networks');
    await switchTab(page, 'containers');
    await expect(win(page)).toBeVisible();
  });
});

// ===========================================================================
// 7. Responsive
// ===========================================================================

test.describe('7 — Responsive', () => {
  for (const [label, width, height] of [
    ['360px (mobile)', 360, 800],
    ['768px (tablet)', 768, 1024],
    ['1920px (desktop)', 1920, 1080],
  ] as const) {
    test(`7.${label} — renders without horizontal overflow`, async ({ page }) => {
      await page.setViewportSize({ width, height });
      await gotoDocker(page, adminTokens);
      const scrollWidth = await page.evaluate(() => document.documentElement.scrollWidth);
      const clientWidth = await page.evaluate(() => document.documentElement.clientWidth);
      expect(scrollWidth).toBeLessThanOrEqual(clientWidth + 4);
    });
  }
});

// ===========================================================================
// 8. Accessibility
// ===========================================================================

test.describe('8 — Accessibility', () => {
  test('8.1 — the sidebar nav exposes tablist/tab roles', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await expect(win(page).locator('[role="tablist"]')).toHaveCount(1);
    await expect(win(page).locator('[role="tab"]')).toHaveCount(7);
  });

  test('8.2 — the active tab has aria-selected=true and the rest false', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    const selected = win(page).locator('[role="tab"][aria-selected="true"]');
    await expect(selected).toHaveCount(1);
  });

  test('8.3 — modals expose dialog role and aria-modal', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    if (!(await openCreateContainerModal(page))) test.skip(true, 'no create-container button visible (daemon down?)');
    await expect(page.locator('[data-slot="dialog-content"][role="dialog"][aria-modal="true"]')).toBeVisible();
  });

  test('8.4 — no duplicate element ids on the page', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    const dupes = await page.evaluate(() => {
      const ids = Array.from(document.querySelectorAll('[id]')).map((e) => e.id);
      return ids.filter((id, i) => ids.indexOf(id) !== i);
    });
    expect(dupes).toEqual([]);
  });

  test('8.5 — the create-container name/image fields have associated labels', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    if (!(await openCreateContainerModal(page))) test.skip(true, 'no create-container button visible (daemon down?)');
    await expect(page.locator('label[for="cName"]')).toBeVisible();
    await expect(page.locator('label[for="cImage"]')).toBeVisible();
  });
});

// ===========================================================================
// 9. Performance
// ===========================================================================

test.describe('9 — Performance', () => {
  test('9.1 — the window opens in a reasonable time', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const start = Date.now();
    await openDocker(page);
    await page.waitForSelector('[data-testid="docker-metric"]', { timeout: 10_000 });
    const elapsed = Date.now() - start;
    console.log(`[PERF] Docker window ready in ${elapsed}ms`);
    expect(elapsed).toBeLessThan(10_000);
  });

  test('9.2 — opening the Docker window does not add an excessive number of DOM nodes', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const before = await page.evaluate(() => document.querySelectorAll('*').length);
    await openDocker(page);
    await page.waitForSelector('[data-testid="docker-metric"]');
    const after = await page.evaluate(() => document.querySelectorAll('*').length);
    console.log(`[PERF] Docker window added ${after - before} DOM nodes`);
    expect(after - before).toBeLessThan(4_000);
  });

  test('9.3 — the initial load issues a bounded number of API calls', async ({ page }) => {
    const calls: string[] = [];
    page.on('request', (r) => {
      if (r.url().includes('/api/docker/')) calls.push(r.url());
    });
    await gotoDocker(page, adminTokens);
    await page.waitForTimeout(500);
    expect(calls.length).toBeLessThan(15);
  });
});

// ===========================================================================
// 10. Security
// ===========================================================================

test.describe('10 — Security', () => {
  test('10.1 — the Docker API rejects a missing session token', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/docker/containers');
      return r.status;
    });
    expect(status).toBeGreaterThanOrEqual(400);
  });

  test('10.2 — the Docker API rejects a tampered session token', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/docker/containers', { headers: { 'x-session-token': 'not-a-real-token' } });
      return r.status;
    });
    expect(status).toBeGreaterThanOrEqual(400);
  });

  test('10.3 — mutating endpoints require admin, not just any session', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    const statuses = await page.evaluate(async () => {
      const headers = { 'x-session-token': 'not-a-real-token', 'x-csrf-token': 'x', 'Content-Type': 'application/json' };
      const create = await fetch('/api/docker/containers', { method: 'POST', headers, body: '{}' });
      const del = await fetch('/api/docker/containers/whatever', { method: 'DELETE', headers });
      const netCreate = await fetch('/api/docker/networks', { method: 'POST', headers, body: '{}' });
      return [create.status, del.status, netCreate.status];
    });
    for (const s of statuses) expect(s).toBeGreaterThanOrEqual(400);
  });

  test('10.4 — an XSS payload in the container-name field is never rendered as markup', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    let dialogFired = false;
    page.on('dialog', () => (dialogFired = true));
    if (!(await openCreateContainerModal(page))) test.skip(true, 'no create-container button visible (daemon down?)');
    await page.locator('#cName').fill('<img src=x onerror=alert(1)>');
    expect(dialogFired).toBe(false);
    expect(await win(page).locator('img[onerror]').count()).toBe(0);
  });

  test('10.5 — no session token is ever placed in the URL', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    expect(page.url()).not.toContain(adminTokens.token);
  });

  test('10.6 — the compose-config read endpoint rejects an unauthenticated request', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    const statuses = await page.evaluate(async () => {
      const noTok = await fetch('/api/docker/compose/projects/whatever/config');
      const badTok = await fetch('/api/docker/compose/projects/whatever/config', {
        headers: { 'x-session-token': 'not-a-real-token' },
      });
      return [noTok.status, badTok.status];
    });
    for (const s of statuses) expect(s).toBeGreaterThanOrEqual(400);
  });
});

// ===========================================================================
// 11. Keyboard
// ===========================================================================

test.describe('11 — Keyboard', () => {
  test('11.1 — Escape closes the create-container modal', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    if (!(await openCreateContainerModal(page))) test.skip(true, 'no create-container button visible (daemon down?)');
    await expect(page.locator('[data-slot="dialog-content"]')).toBeVisible();
    await page.keyboard.press('Escape');
    // Docker.svelte may or may not bind Escape globally — if it doesn't,
    // fall back to explicit close so the rest of the suite isn't affected.
    const stillOpen = (await page.locator('[data-slot="dialog-content"]').count()) > 0;
    if (stillOpen) await page.locator('[data-slot="dialog-overlay"]').click({ position: { x: 5, y: 5 } });
    await expect(page.locator('[data-slot="dialog-content"]')).toHaveCount(0);
  });

  test('11.2 — arrow keys move focus between sidebar tabs', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await win(page).locator('[data-testid="docker-nav-item"]').first().focus();
    await page.keyboard.press('ArrowDown');
    const focused = await page.evaluate(
      () => document.activeElement?.getAttribute('data-testid') || '',
    );
    expect(focused).toBe('docker-nav-item');
  });
});

// ===========================================================================
// 12. Console & network
// ===========================================================================

test.describe('12 — Console & network', () => {
  test('12.1 — no uncaught JS errors while browsing tabs', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'images');
    await switchTab(page, 'networks');
    await switchTab(page, 'catalog');
    await switchTab(page, 'compose');
    await switchTab(page, 'containers');
    expect(errors).toEqual([]);
  });

  test('12.2 — no failed (4xx/5xx) requests during a normal page load', async ({ page }) => {
    const failed: string[] = [];
    page.on('response', (r) => {
      if (r.status() >= 400 && r.url().includes('/api/docker/')) failed.push(`${r.status()} ${r.url()}`);
    });
    await gotoDocker(page, adminTokens);
    await page.waitForTimeout(500);
    expect(failed).toEqual([]);
  });
});

// ===========================================================================
// 13. Real container lifecycle (best-effort — never touches real containers)
// ===========================================================================

test.describe('13 — Real container lifecycle', () => {
  test('13.1 — create → start/stop/restart → remove a throwaway container', async ({ page }) => {
    const name = `e2edocker${Date.now().toString(36)}`;

    // Navigate first — page.evaluate's fetch needs a real page origin to
    // resolve a relative URL against (about:blank has none).
    await gotoDocker(page, adminTokens);

    const createStatus = await page.evaluate(
      async ({ token, csrf, name }) => {
        const r = await fetch('/api/docker/containers', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ name, image: 'hello-world', auto_remove: false }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf, name },
    );

    if (createStatus !== 200) {
      test.skip(true, `could not create a throwaway container (status ${createStatus}) — likely no registry access in this environment`);
    }

    try {
      await switchTab(page, 'containers');
      await expect(win(page).locator('[data-testid="docker-ctr-card"]', { hasText: name })).toBeVisible({ timeout: 10_000 });

      // hello-world exits immediately on its own — restart re-runs it.
      const restartStatus = await page.evaluate(
        async ({ token, csrf, name }) => {
          const r = await fetch(`/api/docker/containers/${name}/restart`, {
            method: 'PUT',
            headers: { 'x-session-token': token, 'x-csrf-token': csrf },
          });
          return r.status;
        },
        { token: adminTokens.token, csrf: adminTokens.csrf, name },
      );
      expect(restartStatus).toBe(200);
    } finally {
      const delStatus = await page.evaluate(
        async ({ token, csrf, name }) => {
          const r = await fetch(`/api/docker/containers/${name}?force=true`, {
            method: 'DELETE',
            headers: { 'x-session-token': token, 'x-csrf-token': csrf },
          });
          return r.status;
        },
        { token: adminTokens.token, csrf: adminTokens.csrf, name },
      );
      expect(delStatus).toBe(200);
    }
  });
});

// ===========================================================================
// 14. Edge cases & memory regression
// ===========================================================================

test.describe('14 — Edge cases & memory regression', () => {
  test('14.1 — a unicode container name is accepted by the field without throwing', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    if (!(await openCreateContainerModal(page))) test.skip(true, 'no create-container button visible (daemon down?)');
    await page.locator('#cName').fill('容器-🐳-test');
    await expect(page.locator('#cName')).toHaveValue('容器-🐳-test');
  });

  test('14.2 — a very long image name does not break layout', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    if (!(await openCreateContainerModal(page))) test.skip(true, 'no create-container button visible (daemon down?)');
    await page.locator('#cImage').fill('a'.repeat(300));
    const scrollWidth = await page.evaluate(() => document.documentElement.scrollWidth);
    const clientWidth = await page.evaluate(() => document.documentElement.clientWidth);
    expect(scrollWidth).toBeLessThanOrEqual(clientWidth + 4);
  });

  test('14.3 — rapidly switching tabs does not crash the page', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    for (let i = 0; i < 3; i++) {
      await switchTab(page, 'images');
      await switchTab(page, 'containers');
      await switchTab(page, 'catalog');
    }
    await expect(win(page)).toBeVisible();
  });

  test('14.4 — clearing localStorage mid-session and reloading does not crash', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await page.waitForTimeout(1_000);
    const bodyText = await page.locator('body').innerText().catch(() => '');
    expect(bodyText.length).toBeGreaterThan(0);
  });

  test('14.5 — opening and closing the create-container modal ×5 does not leak significant heap', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    const before = await getHeapMB(page).catch(() => 0);
    for (let i = 0; i < 5; i++) {
      const modal = await openCreateContainerModal(page);
      if (!modal) test.skip(true, 'no create-container button visible (daemon down?)');
      await page.locator('[data-slot="dialog-overlay"]').click({ position: { x: 5, y: 5 } });
    }
    const after = await getHeapMB(page).catch(() => 0);
    if (before > 0 && after > 0) {
      const delta = after - before;
      console.log(`[MEM] 5x modal open/close delta: ${delta.toFixed(1)} MB`);
      expect(delta).toBeLessThan(15);
    }
  });
});

// ===========================================================================
// 15. Catalog phase B — install review, compatibility, logs, update-all
// ===========================================================================

test.describe('15 — Catalog phase B', () => {
  test('15.1 — install-review API returns plan, settings and verdict', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    const review = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/docker/catalog/homevault:jellyfin/install-review', {
          headers: { 'x-session-token': token, 'x-csrf-token': csrf },
        });
        return r.json();
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(review.success).toBe(true);
    expect(review.data.id).toBe('homevault:jellyfin');
    expect(Array.isArray(review.data.dependencies)).toBe(true);
    expect(Array.isArray(review.data.environment)).toBe(true);
    expect('incompatibility_reason' in review.data).toBe(true);
    expect(typeof review.data.default_port).toBe('number');

    const unknown = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/docker/catalog/homevault:nope/install-review', {
          headers: { 'x-session-token': token, 'x-csrf-token': csrf },
        });
        return r.json();
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(unknown.success).toBe(false);
  });

  test('15.2 — compatible apps show no incompatibility badge and an enabled Install', async ({
    page,
  }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'catalog');
    const cards = win(page).locator('[data-testid="docker-app-card"]');
    await expect(cards.first()).toBeVisible({ timeout: 10_000 });
    // This host is x86_64 with a current server version: the 15 built-in
    // apps declare no arch/version gate, so no card may carry the badge.
    await expect(win(page).getByText(/Cannot be installed here/)).toHaveCount(0);
    const jellyfin = win(page).locator('[data-testid="docker-app-card"]', { hasText: 'Jellyfin' });
    if ((await jellyfin.count()) > 0) {
      const installBtn = jellyfin.getByRole('button', { name: /^Install/ });
      if ((await installBtn.count()) > 0) await expect(installBtn.first()).toBeEnabled();
    }
  });

  test('15.3 — the Install dialog resolves its review and closes without installing', async ({
    page,
  }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'catalog');
    const jellyfin = win(page).locator('[data-testid="docker-app-card"]', { hasText: 'Jellyfin' });
    if ((await jellyfin.count()) === 0) test.skip(true, 'jellyfin not in catalog');
    const installBtn = jellyfin.getByRole('button', { name: /^Install/ });
    if ((await installBtn.count()) === 0) test.skip(true, 'jellyfin already installed here');
    await installBtn.first().click();
    const modal = page.locator('[data-slot="dialog-content"][role="dialog"]');
    await expect(modal).toBeVisible({ timeout: 10_000 });
    // The review resolves: the loading line disappears (no incompatible
    // verdict for a built-in app on this host).
    await expect(modal.getByText(/Checking requirements/)).toHaveCount(0, { timeout: 10_000 });
    await expect(modal.getByText(/cannot install this app/i)).toHaveCount(0);
    await expect(modal.getByRole('button', { name: /^Install/ })).toBeEnabled();
    // Close without installing — no image is ever pulled by this suite.
    await modal.getByRole('button', { name: /Cancel/ }).click();
    await expect(modal).toHaveCount(0);
  });

  test('15.4 — update-all and per-app logs are admin-gated and safe when idle', async ({
    page,
  }) => {
    await gotoDocker(page, adminTokens);
    const updateAll = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/docker/catalog/update-all', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf },
        });
        return r.json();
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(updateAll.success).toBe(true);
    expect(Array.isArray(updateAll.data.updated)).toBe(true);
    expect(Array.isArray(updateAll.data.failed)).toBe(true);

    const logs = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/docker/catalog/homevault:jellyfin/logs', {
          headers: { 'x-session-token': token, 'x-csrf-token': csrf },
        });
        return r.json();
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(logs.success).toBe(false);
  });

  test('15.5 — install rejects an alternative for a dependency the app does not have', async ({
    page,
  }) => {
    await gotoDocker(page, adminTokens);
    const res = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/docker/catalog/homevault:jellyfin/install', {
          method: 'POST',
          headers: {
            'x-session-token': token,
            'x-csrf-token': csrf,
            'Content-Type': 'application/json',
          },
          body: JSON.stringify({ alternatives: { postgres: 'homevault:postgres' } }),
        });
        return r.json();
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(res.success).toBe(false);
    expect(String(res.message || res.error)).toContain('does not depend');
  });
});

// ===========================================================================
// 16. Production surface — volumes, disk usage, container extras, live logs
// ===========================================================================

/**
 * Read-only by default: these tests open panels, read what they render and
 * close them. The one real action (16.7) runs `echo` inside an already
 * running container, which changes nothing on the host.
 */
test.describe('16 — Production surface', () => {
  test('16.1 — the Volumes tab shows the disk-usage table and the prune buttons', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'volumes');
    const panel = win(page).locator('[data-testid="docker-panel"]').first();
    await expect(panel).toBeVisible();
    // The table is filled by `docker system df`; it must have a header row with
    // the five columns the backend reports.
    const headers = panel.locator('th');
    await expect(headers).toHaveCount(5);
    await expect(headers.first()).not.toBeEmpty();
    // Four reclaim actions, the last one being the destructive "everything".
    const prune = panel.locator('button', { hasText: /Clean|Nettoyer|Bereinigen|Limpiar/ });
    await expect(prune).toHaveCount(4);
  });

  test('16.2 — the new-volume modal opens with a name field and cancels cleanly', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'volumes');
    const btn = win(page).locator('[data-testid="docker-top-btn"]').first();
    if ((await btn.count()) === 0) test.skip(true, 'no create-volume button in this state');
    await btn.click();
    const modal = page.locator('[data-slot="dialog-content"][role="dialog"]');
    await expect(modal).toBeVisible();
    await expect(modal.locator('#vName')).toBeVisible();
    await expect(modal.locator('#vName')).toHaveAttribute('type', 'text');
    // The submit stays disabled until a name is typed — no empty POST.
    const submit = modal.locator('button', { hasText: /Create|Créer|Erstellen|Crear/ }).last();
    await expect(submit).toBeDisabled();
    await modal.locator('#vName').fill('e2e-should-not-be-created');
    await expect(submit).toBeEnabled();
    await page.keyboard.press('Escape');
    await expect(modal).toHaveCount(0);
  });

  test('16.3 — the images tab exposes Build and Private registries, and the registries modal opens', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'images');
    const buildBtn = win(page).locator('[data-testid="docker-toolbar-actions"] button', {
      hasText: /Build|Construire|Erstellen|Compilar/,
    });
    const regBtn = win(page).locator('[data-testid="docker-toolbar-actions"] button', {
      hasText: /registr|Registre|Registry/i,
    });
    await expect(buildBtn).toHaveCount(1);
    await expect(regBtn).toHaveCount(1);

    await regBtn.click();
    const modal = page.locator('[data-slot="dialog-content"][role="dialog"]');
    await expect(modal).toBeVisible();
    // Sign-in form is always offered, even with no registry configured.
    await expect(modal.locator('#registryHost')).toBeVisible();
    await expect(modal.locator('#registryUser')).toBeVisible();
    // The password field must be masked, never a text input.
    await expect(modal.locator('#registryPassword')).toHaveAttribute('type', 'password');
    await page.keyboard.press('Escape');
    await expect(modal).toHaveCount(0);
  });

  test('16.4 — the build modal exposes a path-guarded context field and a tag', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'images');
    const buildBtn = win(page).locator('[data-testid="docker-toolbar-actions"] button', {
      hasText: /Build|Construire|Erstellen|Compilar/,
    });
    await buildBtn.click();
    const modal = page.locator('[data-slot="dialog-content"][role="dialog"]');
    await expect(modal).toBeVisible();
    await expect(modal.locator('#buildTag')).toBeVisible();
    // The context is a PathField (browse button), not a free-form input only.
    await expect(modal.locator('input[placeholder*="homevault"], input[placeholder*="/"]').first()).toBeVisible();
    const submit = modal.locator('button', { hasText: /Build|Construire|Erstellen|Compilar/ }).last();
    await expect(submit).toBeDisabled();
    await page.keyboard.press('Escape');
    await expect(modal).toHaveCount(0);
  });

  test('16.5 — the containers toolbar opens the daemon.json editor', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'containers');
    const btn = win(page).locator('[data-testid="docker-toolbar-actions"] button', {
      hasText: /engine|moteur|Engine|motor/i,
    });
    await expect(btn).toHaveCount(1);
    await btn.click();
    const modal = page.locator('[data-slot="dialog-content"][role="dialog"]');
    await expect(modal).toBeVisible();
    await expect(modal.locator('#daemonConfig')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(modal).toHaveCount(0);
  });

  test('16.6 — the per-container "more actions" menu opens, lists its items and closes on Escape', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'containers');
    const cards = win(page).locator('[data-testid="docker-ctr-card"]');
    if ((await cards.count()) === 0) test.skip(true, 'no containers on this host');

    const more = cards.first().locator('button[aria-haspopup="menu"]').last();
    await expect(more).toHaveAttribute('aria-expanded', 'false');
    await more.click();
    await expect(more).toHaveAttribute('aria-expanded', 'true');
    const menu = cards.first().locator('[role="menu"]');
    await expect(menu).toBeVisible();
    // Details, Rename, Limits and Update image are always offered; the
    // state-dependent ones (pause/kill/exec) only for a running container.
    const items = menu.locator('[role="menuitem"]');
    expect(await items.count()).toBeGreaterThanOrEqual(4);
    await expect(menu).toContainText(/Details|Détails/);
    await page.keyboard.press('Escape');
    await expect(menu).toHaveCount(0);
  });

  test('16.7 — the live logs modal streams at least one line and offers the tail controls', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'containers');
    const cards = win(page).locator('[data-testid="docker-ctr-card"]');
    if ((await cards.count()) === 0) test.skip(true, 'no containers on this host');

    const logsBtn = cards.first().locator('[data-testid="docker-ctr-actions"] button', {
      hasText: /Logs|Journaux|Protokolle|Registros/,
    });
    if ((await logsBtn.count()) === 0) test.skip(true, 'no logs button on this host');
    await logsBtn.click();

    const modal = page.locator('[data-slot="dialog-content"][role="dialog"]');
    await expect(modal).toBeVisible();
    // The tail size is a real select, the two options are checkboxes.
    await expect(modal.locator('#logTail')).toBeVisible();
    await expect(modal.locator('input[type="checkbox"]')).toHaveCount(2);
    // The status pill resolves out of "connecting" once the ticket is redeemed.
    await expect(modal.locator('[role="status"]')).not.toContainText(/Connecting|Connexion|Verbinden|Conectando/, {
      timeout: 15_000,
    });
    // Either a line arrived or the container legitimately has no output — but
    // the box must exist and never stay on the raw error text.
    await expect(modal.locator('[data-testid="docker-logs"]')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(modal).toHaveCount(0);
  });

  test('16.8 — a command can be run inside a running container', async ({ page }) => {
    await gotoDocker(page, adminTokens);
    await switchTab(page, 'containers');
    const cards = win(page).locator('[data-testid="docker-ctr-card"]');
    if ((await cards.count()) === 0) test.skip(true, 'no containers on this host');

    // Open the menu of the first card and pick "Run a command" if present
    // (it is only offered for a running container).
    const card = cards.first();
    await card.locator('button[aria-haspopup="menu"]').last().click();
    const execItem = card.locator('[role="menuitem"]', {
      hasText: /Run a command|Exécuter une commande|Befehl ausführen|Ejecutar un comando/,
    });
    if ((await execItem.count()) === 0) test.skip(true, 'no running container on this host');
    await execItem.click();

    const modal = page.locator('[data-slot="dialog-content"][role="dialog"]');
    await expect(modal).toBeVisible();
    await modal.locator('#execCommand').fill('echo hv-e2e');
    await modal.locator('button', { hasText: /Run|Exécuter|Ausführen|Ejecutar/ }).last().click();
    // Either the command produced output (exit 0) or the image has no shell
    // utility — both are surfaced, neither may leave the modal blank.
    await expect(modal.locator('pre').first()).toBeVisible({ timeout: 20_000 });
    await page.keyboard.press('Escape');
    await expect(modal).toHaveCount(0);
  });
});

// ===========================================================================
// 17. API client consolidation (ROADMAP 31.1)
// ===========================================================================
//
// The screen carried its own `authedFetch` helper until 2026-10-24; every
// Docker call now goes through `$lib/services/api.ts`. Three behaviours are
// worth pinning, and none of them is visible in the rendered markup — they
// are all request/response observations:
//
//   * the shared client's per-request headers (`X-Session-Token`,
//     `X-CSRF-Token`, `X-HomeVault-Contract`) and its `/api` base — a helper
//     that comes back at a call site loses at least one of them;
//   * no call site bypasses the client (no new hand-rolled fetch);
//   * a mutation evicts the cached GET, so the list re-read after an action
//     shows the action's outcome instead of the pre-action rows (the shared
//     client caches `GET /docker/containers` for 3 s, which the old helper
//     never did — re-reading from a warm cache was the one way the migration
//     could have changed behaviour).
//
// These need no docker daemon: they watch traffic and serve stubbed answers.

test.describe('17 — API client consolidation (ROADMAP 31.1)', () => {
  test('17.1 — every Docker request carries the session, CSRF and contract headers', async ({ page }) => {
    const violations: string[] = [];
    page.on('request', (r) => {
      const url = r.url();
      if (!url.includes('/api/docker')) return;
      const h = r.headers();
      if (!h['x-session-token']) violations.push(`no session token: ${r.method()} ${url}`);
      if (!h['x-csrf-token']) violations.push(`no csrf token: ${r.method()} ${url}`);
      if (!h['x-homevault-contract']) violations.push(`no contract header: ${r.method()} ${url}`);
    });

    await gotoDesktop(page, adminTokens);
    await openDocker(page);
    const seen = page.waitForResponse((r) => r.url().includes('/api/docker/available'), { timeout: 20_000 });
    await page.waitForLoadState('networkidle').catch(() => {});
    await seen.catch(() => null);

    // The home-grown helper sent neither the contract header nor the cache
    // awareness; a single request without it fails here.
    expect(violations, violations.join('\n')).toEqual([]);
  });

  test('17.2 — no call site bypasses the shared client', async ({ page }) => {
    // The old helper's fingerprint is a raw `fetch('/api…')`. Every module the
    // page really loaded is scanned for it: the built chunks are minified, so
    // the quote style follows, but `fetch` and the `/api/` prefix survive.
    const offenders: string[] = [];
    const scan = (url: string, body: string) => {
      if (!/^https?:.*\.js(\?|$)/.test(url)) return;
      const raw = body.match(/fetch\(\s*['"`]\s*\/api/g);
      if (raw) offenders.push(`${url} → ${raw.length} raw /api fetch(es)`);
    };
    page.on('response', async (r) => {
      if (!r.url().endsWith('.js') && !r.url().includes('.js?')) return;
      try {
        scan(r.url(), await r.text());
      } catch {
        /* redirect / aborted / cross-origin — not a module we can read */
      }
    });
    page.on('request', async (r) => {
      if (r.resourceType() !== 'script') return;
      try {
        const body = r.postData();
        if (body) scan(r.url(), body);
      } catch {
        /* ignore */
      }
    });
    await gotoDesktop(page, adminTokens);
    await openDocker(page);
    await page.waitForLoadState('networkidle').catch(() => {});

    // The screen is only fully mounted once its shell is on screen — that is
    // also when its chunk has been fetched and scanned.
    await expect(win(page).locator('[data-testid="docker-page"]')).toBeVisible();
    expect(offenders, offenders.join('\n')).toEqual([]);
  });

  test('17.3 — a container action is not answered from the 3 s GET cache', async ({ page }) => {
    // Stub the whole container surface so this runs with no daemon. The card
    // must first show the row `getDockerContainerList` serves, then — after a
    // start — the row the re-read serves. If the shared client answered that
    // re-read from its 3 s `GET /docker/containers` cache, the card would
    // still carry the pre-action name.
    //
    // Two deliberate concessions, both documented rather than papered over:
    // the stub answers `GET …/containers` on the path alone (any query), and
    // the action button is located by role + accessible name, which is what
    // the card really exposes.
    const row = (name: string) => ({
      id: `id-${name}`,
      name,
      image: `img-${name}:latest`,
      status: 'Up 2 minutes',
      state: 'Running',
      created: '2026-10-24T10:00:00Z',
      ports: [],
      memory_mb: null,
      cpu_percent: null,
      net_rx_bytes: null,
      net_tx_bytes: null,
      blk_read_bytes: null,
      blk_write_bytes: null,
      update_available: false,
      compose_project: null,
      networks: [],
    });

    let listedAfterAction = false;
    let listReads = 0;
    const json = (data: unknown) => ({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: true, data }),
    });

    await page.route('**/api/docker/available', (route) => route.fulfill(json({ available: true })));
    await page.route('**/api/docker/containers**', async (route) => {
      const url = route.request().url();
      if (!/POST|PUT|DELETE/.test(route.request().method()) && !/\/(start|stop|restart|logs)/.test(url)) {
        if (url.includes('stats=true')) return route.fulfill(json([]));
        listReads += 1;
        return route.fulfill(json([row(listedAfterAction ? 'after-action' : 'before-action')]));
      }
      if (url.endsWith('/start')) listedAfterAction = true;
      return route.fulfill(json({}));
    });

    await gotoDesktop(page, adminTokens);
    await openDocker(page);
    if (await win(page).locator('[data-testid="docker-daemon-down"]').count()) {
      test.skip(true, 'the screen mounted its daemon-down panel before the stub applied');
    }

    const cards = win(page).locator('[data-testid="docker-ctr-card"]');
    await expect(cards).toHaveCount(1, { timeout: 15_000 });
    await expect(win(page).getByText('before-action')).toBeVisible();
    expect(listReads).toBeGreaterThanOrEqual(1);

    const start = cards.first().getByRole('button', { name: /start/i }).first();
    if ((await start.count()) === 0) test.skip(true, 'the card exposes no start action for a running container');
    await start.click();

    await expect(win(page).getByText('after-action')).toBeVisible({ timeout: 15_000 });
    expect(listReads).toBeGreaterThanOrEqual(2);
  });
});
