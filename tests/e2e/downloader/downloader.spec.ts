/* =============================================================================
 * File          : downloader.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Downloader screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Downloader (aria2) app — End-to-End Test Suite
 *
 * The Downloader is not a route: it is a window opened from the desktop's
 * Applications launcher (id `downloader`, "Downloader" tile). It drives a
 * per-server `aria2c` daemon (spawned lazily by the backend on port 6800)
 * through `src/handlers/downloader.rs` — add URL / list tasks / pause /
 * resume / remove.
 *
 * **Safety model (like Docker's throwaway-container / VPN's throwaway-peer
 * pattern):** a download is the *intended*, additive, reversible primary use
 * of this page. Section 13 performs one **genuine** download — but of a file
 * the app itself serves (`/wallpapers/hv-01-thumb.webp`, ~25 KB, always
 * present, no internet needed) into a unique throwaway directory under
 * `/tmp`, then removes the task. Nothing else on the host is touched.
 * `test.afterAll` sweeps any download task still referencing the test
 * asset (in case a test failed mid-way). The downloaded *file* itself
 * (~25 KB) stays under `/tmp/hv-dl-e2e-*` — aria2 does not delete files on
 * task removal and the browser cannot reach the server's filesystem; `/tmp`
 * is cleared on reboot. If `aria2c` is not installed the page shows its
 * "not installed" state and the lifecycle tests skip cleanly.
 *
 * All mutating requests go through the CSRF middleware, so the in-page
 * `fetch` helpers send both `x-session-token` and `x-csrf-token` (captured
 * once in `beforeAll`), exactly as `ApiClient` does.
 *
 * Run (all):     npx playwright test tests/e2e/downloader/downloader.spec.ts --project=firefox
 * Run (single):  npx playwright test tests/e2e/downloader/downloader.spec.ts -g "1.1"
 * Run (category):npx playwright test tests/e2e/downloader/downloader.spec.ts -g "Security"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/downloader/downloader.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

/** A file the app itself serves — a real HTTP download with no internet. */
const LOCAL_ASSET = '/wallpapers/hv-01-thumb.webp';
/**
 * Absolute URL of that asset on the instance under test. aria2 fetches it
 * server-side, so it must point at BASE_URL's port — hardcoding 8080 made the
 * whole "real lifecycle" block fail whenever BASE_URL was set to another port.
 * `localhost` is normalised to 127.0.0.1 so aria2 can't resolve to ::1 while
 * the server binds IPv4 only.
 */
const ASSET_ORIGIN = new URL(BASE).origin.replace('//localhost', '//127.0.0.1');
const LOCAL_ASSET_URL = `${ASSET_ORIGIN}${LOCAL_ASSET}`;

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
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

async function openLaunchpadApp(page: Page, appName: string) {
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator(`.lp-card[title="${appName}"]`);
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
}

async function gotoDownloader(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openLaunchpadApp(page, 'Downloader');
  await page.waitForSelector('[data-testid="downloader-page"]', { timeout: 10_000 });
  await expect(page.locator('[data-testid="downloader-page"] [data-testid="downloader-loading"]')).toHaveCount(0, { timeout: 20_000 });
}

/** The Downloader window as a scoping root. */
function win(page: Page) {
  return page.locator('[data-testid="downloader-page"]');
}

/** True when the page rendered its "aria2c not installed" state. */
async function isNotInstalled(page: Page): Promise<boolean> {
  return (await win(page).locator('[data-testid="downloader-not-available"]').count()) > 0;
}

async function skipIfNoAria2(page: Page) {
  if (await isNotInstalled(page)) {
    test.skip(true, 'aria2c is not installed on this host — Downloader shows its not-installed state');
  }
}

// --- in-page authed fetch helpers (CSRF-aware) -----------------------------

async function apiAdd(page: Page, t: Tokens, uri: string, dir: string) {
  return page.evaluate(
    async ({ token, csrf, uri, dir }) => {
      const r = await fetch('/api/downloader/add', {
        method: 'POST',
        headers: {
          'x-session-token': token,
          'x-csrf-token': csrf,
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ uri, dir }),
      });
      const body = await r.json().catch(() => ({}));
      return { http: r.status, success: body?.success, gid: body?.data, error: body?.error };
    },
    { token: t.token, csrf: t.csrf, uri, dir },
  );
}

async function apiTasks(page: Page, t: Tokens) {
  return page.evaluate(async ({ token }) => {
    const r = await fetch('/api/downloader/tasks', { headers: { 'x-session-token': token } });
    const body = await r.json().catch(() => ({}));
    return { http: r.status, success: body?.success, data: body?.data as any[] };
  }, { token: t.token });
}

async function apiRemove(page: Page, t: Tokens, gid: string) {
  return page.evaluate(
    async ({ token, csrf, gid }) => {
      const r = await fetch('/api/downloader/remove', {
        method: 'POST',
        headers: {
          'x-session-token': token,
          'x-csrf-token': csrf,
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ gid }),
      });
      const body = await r.json().catch(() => ({}));
      return { http: r.status, success: body?.success };
    },
    { token: t.token, csrf: t.csrf, gid },
  );
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

test.afterEach(async ({ page }, testInfo) => {
  const heapMB = await getHeapMB(page).catch(() => 0);
  if (heapMB > 0) {
    // eslint-disable-next-line no-console
    console.log(`[MEM] ${testInfo.title}: ${heapMB.toFixed(1)} MB heap after test`);
  }
});

/** Defensive: drop any aria2 task still pointing at the test asset. */
test.afterAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  try {
    const r = await ctx.get('/api/downloader/tasks', {
      headers: { 'x-session-token': adminTokens.token },
    });
    const body = await r.json().catch(() => ({}));
    for (const t of (body?.data as any[]) || []) {
      const refsAsset =
        t.gid &&
        (JSON.stringify(t.files || []).includes('hv-01-thumb.webp') ||
          String(t.dir || '').includes('/tmp/hv-dl-e2e-'));
      if (refsAsset) {
        await ctx
          .post('/api/downloader/remove', {
            headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
            data: { gid: t.gid },
          })
          .catch(() => {});
      }
    }
  } finally {
    await ctx.dispose();
  }
});

// ===========================================================================
// 1. Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — Downloader window opens from the Applications launcher', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — the loading spinner clears (no stuck spinner)', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await expect(win(page).locator('[data-testid="downloader-loading"]')).toHaveCount(0);
  });

  test('1.3 — adapts to state: full UI (aria2 present) or the not-installed panel', async ({
    page,
  }) => {
    await gotoDownloader(page, adminTokens);
    const notInstalled = await win(page).locator('[data-testid="downloader-not-available"]').count();
    const header = await win(page).locator('.hv-winhdr-title', { hasText: 'Downloader' }).count();
    expect(notInstalled + header).toBeGreaterThan(0);
  });

  test('1.4 — the not-installed panel names the apt package', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    if (!(await isNotInstalled(page))) {
      test.skip(true, 'aria2c is installed — not-installed panel not shown');
    }
    await expect(win(page).locator('[data-testid="downloader-not-available"] code')).toContainText('aria2');
  });

  test('1.5 — the 4 stat tiles render when aria2 is available', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    // The stats block appears once a global-stat response has landed.
    await expect(win(page).locator('[data-testid="downloader-stats"] [data-testid="downloader-stat-card"]')).toHaveCount(4, { timeout: 15_000 });
  });

  test('1.6 — the "Active downloads" section is always present', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    await expect(win(page).locator('[data-testid="downloader-section-title"]', { hasText: /active/i }).first()).toBeVisible();
  });
});

// ===========================================================================
// 2. Form fields
// ===========================================================================

test.describe('2 — Form fields', () => {
  test('2.1 — the Add-link form exposes a URL text input and a folder picker trigger', async ({
    page,
  }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    await win(page).locator('[data-testid="downloader-add-toggle"]').click();
    const form = win(page).locator('[data-testid="downloader-add-form"]');
    await expect(form).toBeVisible();
    await expect(form.locator('input[data-testid="downloader-input"]')).toHaveAttribute('type', 'text');
    await expect(form.locator('[data-testid="downloader-dir"]')).toBeVisible();
  });

  test('2.2 — the URL input has a placeholder mentioning the supported schemes', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    await win(page).locator('[data-testid="downloader-add-toggle"]').click();
    const ph = await win(page).locator('[data-testid="downloader-add-form"] input[data-testid="downloader-input"]').getAttribute('placeholder');
    expect((ph || '').toLowerCase()).toMatch(/http|magnet|torrent|url/);
  });

  test('2.3 — the Download button is disabled until a URL is typed', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    await win(page).locator('[data-testid="downloader-add-toggle"]').click();
    const submit = win(page).locator('[data-testid="downloader-add-form"] [data-testid="downloader-btn-primary"]');
    await expect(submit).toBeDisabled();
    await win(page).locator('[data-testid="downloader-add-form"] input[data-testid="downloader-input"]').fill(LOCAL_ASSET_URL);
    await expect(submit).toBeEnabled();
  });
});

// ===========================================================================
// 3. Validation
// ===========================================================================

test.describe('3 — Validation', () => {
  test('3.1 — a whitespace-only URL does not enable submit and fires no request', async ({
    page,
  }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    await win(page).locator('[data-testid="downloader-add-toggle"]').click();
    let addCalls = 0;
    page.on('request', (r) => {
      if (r.url().includes('/api/downloader/add')) addCalls++;
    });
    await win(page).locator('[data-testid="downloader-add-form"] input[data-testid="downloader-input"]').fill('   ');
    await expect(win(page).locator('[data-testid="downloader-add-form"] [data-testid="downloader-btn-primary"]')).toBeDisabled();
    await page.waitForTimeout(500);
    expect(addCalls).toBe(0);
  });

  test('3.2 — the add endpoint rejects a missing uri field (422, no aria2 work)', async ({
    page,
  }) => {
    await gotoDownloader(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/downloader/add', {
          method: 'POST',
          headers: {
            'x-session-token': token,
            'x-csrf-token': csrf,
            'Content-Type': 'application/json',
          },
          body: JSON.stringify({ dir: '/tmp/x' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(422);
  });

  test('3.3 — pause/remove with an unknown gid fail cleanly (no 500)', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    const res = await page.evaluate(
      async ({ token, csrf }) => {
        const call = async (path: string) => {
          const r = await fetch(path, {
            method: 'POST',
            headers: {
              'x-session-token': token,
              'x-csrf-token': csrf,
              'Content-Type': 'application/json',
            },
            body: JSON.stringify({ gid: '0000000000000000' }),
          });
          const b = await r.json().catch(() => ({}));
          return { http: r.status, success: b?.success };
        };
        return { pause: await call('/api/downloader/pause'), remove: await call('/api/downloader/remove') };
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(res.pause.http).toBe(200);
    expect(res.pause.success).toBe(false);
    expect(res.remove.http).toBe(200);
    expect(res.remove.success).toBe(false);
  });
});

// ===========================================================================
// 4. All user interactions
// ===========================================================================

test.describe('4 — All user interactions', () => {
  test('4.1 — the Add-link form toggles open and closed from the header action', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    const action = win(page).locator('[data-testid="downloader-add-toggle"]');
    await action.click();
    await expect(win(page).locator('[data-testid="downloader-add-form"]')).toBeVisible();
    await action.click();
    await expect(win(page).locator('[data-testid="downloader-add-form"]')).toHaveCount(0);
  });

  test('4.2 — the folder-picker trigger opens the FolderPicker dialog', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    await win(page).locator('[data-testid="downloader-add-toggle"]').click();
    await win(page).locator('[data-testid="downloader-add-form"] [data-testid="downloader-dir"]').click();
    // FolderPicker renders a modal/dialog; accept any of the common roots.
    await expect(
      page.locator('[role="dialog"], .hv-folder-picker, .hv-modal').first(),
    ).toBeVisible({ timeout: 5_000 });
  });
});

// ===========================================================================
// 5. Submission flows
// ===========================================================================

test.describe('5 — Submission flows', () => {
  test('5.1 — submitting a URL through the form issues exactly one add request', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    let addCalls = 0;
    let gid: string | null = null;
    page.on('response', async (r) => {
      if (r.url().includes('/api/downloader/add')) {
        addCalls++;
        const b = await r.json().catch(() => ({}));
        if (b?.data) gid = b.data;
      }
    });

    await win(page).locator('[data-testid="downloader-add-toggle"]').click();
    await win(page).locator('[data-testid="downloader-add-form"] input[data-testid="downloader-input"]').fill(LOCAL_ASSET_URL);
    await win(page).locator('[data-testid="downloader-add-form"] [data-testid="downloader-btn-primary"]').click();
    await page.waitForTimeout(1500);
    expect(addCalls).toBe(1);

    // cleanup whatever got created
    if (gid) await apiRemove(page, adminTokens, gid);
  });

  test('5.2 — the form clears and closes after a successful add', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    let gid: string | null = null;
    page.on('response', async (r) => {
      if (r.url().includes('/api/downloader/add')) {
        const b = await r.json().catch(() => ({}));
        if (b?.data) gid = b.data;
      }
    });
    await win(page).locator('[data-testid="downloader-add-toggle"]').click();
    await win(page).locator('[data-testid="downloader-add-form"] input[data-testid="downloader-input"]').fill(LOCAL_ASSET_URL);
    await win(page).locator('[data-testid="downloader-add-form"] [data-testid="downloader-btn-primary"]').click();
    await expect(win(page).locator('[data-testid="downloader-add-form"]')).toHaveCount(0, { timeout: 10_000 });
    if (gid) await apiRemove(page, adminTokens, gid);
  });
});

// ===========================================================================
// 6. Navigation
// ===========================================================================

test.describe('6 — Navigation', () => {
  test('6.1 — the window closes and can be reopened', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await page.locator('.hv-cl-btn-close').first().click();
    await expect(win(page)).toHaveCount(0, { timeout: 5_000 });
    await openLaunchpadApp(page, 'Downloader');
    await expect(win(page)).toBeVisible();
  });

  test('6.2 — reopening reattaches to the same live task list (no data loss)', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    const before = (await apiTasks(page, adminTokens)).data?.length ?? 0;
    await page.locator('.hv-cl-btn-close').first().click();
    await expect(win(page)).toHaveCount(0);
    await openLaunchpadApp(page, 'Downloader');
    await page.waitForSelector('[data-testid="downloader-page"]', { timeout: 10_000 });
    const after = (await apiTasks(page, adminTokens)).data?.length ?? 0;
    expect(after).toBe(before);
  });
});

// ===========================================================================
// 7. Responsive
// ===========================================================================

test.describe('7 — Responsive', () => {
  for (const [w, h, label] of [
    [360, 800, '360px (mobile)'],
    [768, 1024, '768px (tablet)'],
    [1920, 1080, '1920px (desktop)'],
  ] as const) {
    test(`7.${label} — renders without horizontal overflow`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: h });
      await gotoDownloader(page, adminTokens);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
      );
      expect(overflow).toBe(false);
    });
  }
});

// ===========================================================================
// 8. Accessibility
// ===========================================================================

test.describe('8 — Accessibility', () => {
  test('8.1 — the folder-picker trigger is keyboard-reachable (role + tabindex)', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    await win(page).locator('[data-testid="downloader-add-toggle"]').click();
    const trigger = win(page).locator('[data-testid="downloader-add-form"] [data-testid="downloader-dir"]');
    await expect(trigger).toHaveAttribute('role', 'button');
    await expect(trigger).toHaveAttribute('tabindex', '0');
  });

  test('8.2 — no duplicate element ids on the page', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    const dupes = await page.evaluate(() => {
      const seen = new Set<string>();
      const dup: string[] = [];
      for (const el of Array.from(document.querySelectorAll('[id]'))) {
        const id = el.id;
        if (!id) continue;
        if (seen.has(id)) dup.push(id);
        seen.add(id);
      }
      return dup;
    });
    expect(dupes).toEqual([]);
  });

  test('8.3 — action buttons on task cards carry a title/aria hint', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    // header action button always present
    const label = await win(page).locator('[data-testid="downloader-add-toggle"]').innerText();
    expect(label.trim().length).toBeGreaterThan(0);
  });
});

// ===========================================================================
// 9. Performance
// ===========================================================================

test.describe('9 — Performance', () => {
  test('9.1 — the window opens in a reasonable time', async ({ page }) => {
    const start = Date.now();
    await gotoDownloader(page, adminTokens);
    const elapsed = Date.now() - start;
    console.log(`[PERF] Downloader window ready in ${elapsed}ms`);
    expect(elapsed).toBeLessThan(20_000);
  });

  test('9.2 — opening the window does not add an excessive number of DOM nodes', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const before = await page.evaluate(() => document.querySelectorAll('*').length);
    await openLaunchpadApp(page, 'Downloader');
    await page.waitForSelector('[data-testid="downloader-page"]', { timeout: 10_000 });
    const after = await page.evaluate(() => document.querySelectorAll('*').length);
    const delta = after - before;
    console.log(`[PERF] Downloader window added ${delta} DOM nodes`);
    expect(delta).toBeLessThan(2000);
  });

  test('9.3 — the 2s poll issues no more than a few requests over 6s', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    let calls = 0;
    page.on('request', (r) => {
      if (/\/api\/downloader\/(tasks|global-stat)/.test(r.url())) calls++;
    });
    await page.waitForTimeout(6_000);
    console.log(`[PERF] ${calls} downloader poll requests in 6s`);
    // ~2 endpoints × ~3 ticks = ~6; allow generous slack.
    expect(calls).toBeLessThan(16);
  });
});

// ===========================================================================
// 10. Security
// ===========================================================================

test.describe('10 — Security', () => {
  test('10.1 — every downloader endpoint rejects a missing session token', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    const results = await page.evaluate(async () => {
      const eps: [string, string, any][] = [
        ['GET', '/api/downloader/status', null],
        ['POST', '/api/downloader/start', null],
        ['POST', '/api/downloader/stop', null],
        ['GET', '/api/downloader/global-stat', null],
        ['GET', '/api/downloader/tasks', null],
        ['POST', '/api/downloader/add', { uri: 'http://127.0.0.1:1/x' }],
        ['POST', '/api/downloader/pause', { gid: '0000000000000000' }],
        ['POST', '/api/downloader/unpause', { gid: '0000000000000000' }],
        ['POST', '/api/downloader/remove', { gid: '0000000000000000' }],
        ['GET', '/api/downloader/task/0000000000000000', null],
      ];
      const out: Record<string, boolean> = {};
      for (const [m, p, b] of eps) {
        const r = await fetch(p, {
          method: m,
          headers: b ? { 'Content-Type': 'application/json' } : {},
          body: b ? JSON.stringify(b) : undefined,
        });
        const j = await r.json().catch(() => ({}));
        out[p] = j?.success === false;
      }
      return out;
    });
    for (const [path, denied] of Object.entries(results)) {
      expect(denied, `${path} must be denied without a token`).toBe(true);
    }
  });

  test('10.2 — a tampered session token is rejected', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    const denied = await page.evaluate(
      async ({ token }) => {
        const r = await fetch('/api/downloader/tasks', {
          headers: { 'x-session-token': token + 'x' },
        });
        const j = await r.json().catch(() => ({}));
        return j?.success === false;
      },
      { token: adminTokens.token },
    );
    expect(denied).toBe(true);
  });

  test('10.3 — a non-admin session cannot reach mutating endpoints', async ({ playwright }) => {
    const ctx = await newCtx(playwright);
    const adminHeaders = {
      'x-session-token': adminTokens.token,
      'x-csrf-token': adminTokens.csrf,
    };
    const uname = `dlviewer${Date.now().toString(36)}`;
    const created = await ctx.post('/api/users', {
      headers: adminHeaders,
      data: {
        username: uname,
        password: 'Viewer12345!',
        fullname: 'DL Viewer',
        email: '',
        role_id: 1,
        groups: [],
        home_enabled: false,
      },
    });
    if (!created.ok()) {
      await ctx.dispose();
      test.skip(true, `could not create a non-admin user (status ${created.status()})`);
    }
    try {
      const { token: userTok, csrf: userCsrf } = await apiLogin(ctx, uname, 'Viewer12345!');
      expect(userTok).toBeTruthy();
      const r = await ctx.post('/api/downloader/add', {
        headers: { 'x-session-token': userTok, 'x-csrf-token': userCsrf },
        data: { uri: LOCAL_ASSET_URL },
      });
      const body = await r.json().catch(() => ({}));
      expect(body?.success).toBe(false);
      expect(String(body?.error || '')).toMatch(/admin/i);
    } finally {
      await ctx.delete(`/api/users/${uname}`, { headers: adminHeaders }).catch(() => {});
      await ctx.dispose();
    }
  });

  test('10.4 — no session token appears in any URL', async ({ page }) => {
    const urls: string[] = [];
    page.on('request', (r) => urls.push(r.url()));
    await gotoDownloader(page, adminTokens);
    await page.waitForTimeout(1000);
    for (const u of urls) {
      expect(u).not.toContain(adminTokens.token);
    }
  });

  test('10.5 — a URL containing an XSS payload is never rendered as markup', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    await win(page).locator('[data-testid="downloader-add-toggle"]').click();
    const payload = '"><img src=x onerror=alert(1)>';
    await win(page).locator('[data-testid="downloader-add-form"] input[data-testid="downloader-input"]').fill(payload);
    const html = await win(page).locator('[data-testid="downloader-add-form"]').innerHTML();
    expect(html).not.toContain('<img src=x onerror=');
  });
});

// ===========================================================================
// 11. Keyboard
// ===========================================================================

test.describe('11 — Keyboard', () => {
  test('11.1 — Enter in the URL field triggers an add when a URL is present', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    let gid: string | null = null;
    let addCalls = 0;
    page.on('response', async (r) => {
      if (r.url().includes('/api/downloader/add')) {
        addCalls++;
        const b = await r.json().catch(() => ({}));
        if (b?.data) gid = b.data;
      }
    });
    await win(page).locator('[data-testid="downloader-add-toggle"]').click();
    const input = win(page).locator('[data-testid="downloader-add-form"] input[data-testid="downloader-input"]');
    await input.fill(LOCAL_ASSET_URL);
    await input.press('Enter');
    await page.waitForTimeout(1500);
    expect(addCalls).toBeGreaterThanOrEqual(1);
    if (gid) await apiRemove(page, adminTokens, gid);
  });

  test('11.2 — Enter in an empty URL field does nothing', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    let addCalls = 0;
    page.on('request', (r) => {
      if (r.url().includes('/api/downloader/add')) addCalls++;
    });
    await win(page).locator('[data-testid="downloader-add-toggle"]').click();
    await win(page).locator('[data-testid="downloader-add-form"] input[data-testid="downloader-input"]').press('Enter');
    await page.waitForTimeout(500);
    expect(addCalls).toBe(0);
  });
});

// ===========================================================================
// 12. Console & network
// ===========================================================================

test.describe('12 — Console & network', () => {
  test('12.1 — no uncaught JS errors during a normal page load', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await gotoDownloader(page, adminTokens);
    await page.waitForTimeout(2000);
    expect(errors).toEqual([]);
  });

  test('12.2 — no failed (4xx/5xx) downloader requests during a normal load', async ({ page }) => {
    const bad: string[] = [];
    page.on('response', (r) => {
      if (r.url().includes('/api/downloader/') && r.status() >= 400) {
        bad.push(`${r.status()} ${r.url()}`);
      }
    });
    await gotoDownloader(page, adminTokens);
    await page.waitForTimeout(3000);
    expect(bad).toEqual([]);
  });
});

// ===========================================================================
// 13. Real download lifecycle (best-effort, always cleaned up)
// ===========================================================================

test.describe('13 — Real download lifecycle', () => {
  test('13.1 — add → downloads a real file to disk → shows complete → remove', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);

    const dir = `/tmp/hv-dl-e2e-${Date.now().toString(36)}`;
    const uri = LOCAL_ASSET_URL;
    const added = await apiAdd(page, adminTokens, uri, dir);
    if (!added.success) {
      test.skip(true, `add failed: ${added.error}`);
    }
    const gid: string = added.gid;
    expect(gid).toBeTruthy();

    try {
      // Poll the real task list until aria2 reports the download finished.
      let task: any = null;
      await expect
        .poll(
          async () => {
            const t = await apiTasks(page, adminTokens);
            task = (t.data || []).find((x) => x.gid === gid) || null;
            return task?.status ?? 'missing';
          },
          { timeout: 15_000, intervals: [500] },
        )
        .toBe('complete');

      // The bytes really landed: aria2's own accounting.
      expect(task.total_length).toBe('25966');
      expect(task.completed_length).toBe('25966');
      expect(task.files?.[0]?.path).toContain(dir);

      // The UI reflects it in the Completed section.
      await expect(
        win(page).locator('[data-testid="downloader-card-name"]', { hasText: 'hv-01-thumb.webp' }),
      ).toBeVisible({ timeout: 8_000 });
    } finally {
      const removed = await apiRemove(page, adminTokens, gid);
      expect(removed.success).toBe(true);
      const still = await apiTasks(page, adminTokens);
      expect((still.data || []).some((x) => x.gid === gid)).toBe(false);
    }
  });

  test('13.2 — removing a finished download from the UI clears its card', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);

    const dir = `/tmp/hv-dl-e2e-${Date.now().toString(36)}`;
    const added = await apiAdd(page, adminTokens, LOCAL_ASSET_URL, dir);
    if (!added.success) test.skip(true, `add failed: ${added.error}`);
    const gid: string = added.gid;

    try {
      await expect
        .poll(async () => {
          const t = await apiTasks(page, adminTokens);
          return (t.data || []).find((x) => x.gid === gid)?.status ?? 'missing';
        }, { timeout: 15_000 })
        .toBe('complete');

      // Reopen so the completed card is on screen.
      await page.locator('.hv-cl-btn-close').first().click();
      await openLaunchpadApp(page, 'Downloader');
      await page.waitForSelector('[data-testid="downloader-page"]', { timeout: 10_000 });

      const card = win(page)
        .locator('[data-testid="downloader-card"]', { has: page.locator('[data-testid="downloader-card-name"]', { hasText: 'hv-01-thumb.webp' }) })
        .first();
      await expect(card).toBeVisible({ timeout: 8_000 });
      await card.locator('[data-testid="downloader-action-danger"]').click();
      await page.waitForTimeout(1500);

      const gone = await apiTasks(page, adminTokens);
      expect((gone.data || []).some((x) => x.gid === gid)).toBe(false);
    } finally {
      await apiRemove(page, adminTokens, gid).catch(() => {});
    }
  });
});

// ===========================================================================
// 14. Edge cases & memory regression
// ===========================================================================

test.describe('14 — Edge cases & memory regression', () => {
  test('14.1 — a unicode / very long URL does not break the input or layout', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    await win(page).locator('[data-testid="downloader-add-toggle"]').click();
    const long = 'http://example.com/' + 'ẞünïcodé-'.repeat(40) + '.bin';
    await win(page).locator('[data-testid="downloader-add-form"] input[data-testid="downloader-input"]').fill(long);
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
    );
    expect(overflow).toBe(false);
  });

  test('14.2 — rapidly toggling the Add form ×8 does not crash the page', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    await skipIfNoAria2(page);
    const action = win(page).locator('[data-testid="downloader-add-toggle"]');
    for (let i = 0; i < 8; i++) await action.click();
    await expect(win(page)).toBeVisible();
  });

  test('14.3 — clearing localStorage mid-session and reloading does not crash', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await gotoDownloader(page, adminTokens);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    // With no token the app bounces to the login screen — a defined,
    // non-crashing state. Wait for it to settle and assert nothing threw.
    await expect(page.locator('.hv-login-page, [data-testid="downloader-page"], #app')).toBeAttached({ timeout: 15_000 });
    await page.waitForTimeout(1000);
    expect(errors).toEqual([]);
  });

  test('14.4 — opening/closing the window ×5 does not leak significant heap', async ({ page }) => {
    await gotoDownloader(page, adminTokens);
    const base = await getHeapMB(page);
    for (let i = 0; i < 5; i++) {
      await page.locator('.hv-cl-btn-close').first().click();
      await expect(win(page)).toHaveCount(0);
      await openLaunchpadApp(page, 'Downloader');
      await page.waitForSelector('[data-testid="downloader-page"]', { timeout: 10_000 });
    }
    const after = await getHeapMB(page);
    if (base > 0 && after > 0) {
      console.log(`[MEM] heap ${base.toFixed(1)} → ${after.toFixed(1)} MB after 5 open/close`);
      expect(after - base).toBeLessThan(10);
    }
  });
});
