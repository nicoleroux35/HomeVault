/* =============================================================================
 * File          : explorer.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Explorer screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * File Explorer app — End-to-End Test Suite
 *
 * The Explorer is not a route: it is a window opened from the desktop's
 * Applications launcher (id `explorer`, "Explorer" tile). It browses the
 * server's filesystem — list / search / mkdir / rename / copy / move /
 * delete-to-trash / restore / permanent purge, plus upload, download,
 * preview, chmod, compress and extract — backend `src/handlers/explorer.rs`
 * + `src/browser.rs`.
 *
 * **Safety model:** unlike Shares / Storage / RAID, every Explorer
 * operation is a plain filesystem call (no `sudo`, no system service, no
 * write under `/etc`), so the real success paths *are* exercised — but
 * only inside a single throwaway working directory created under `/tmp`
 * (`/tmp/hv-fx-e2e-<ts>/`) in `beforeAll` and removed in `afterAll`.
 * Nothing outside that directory is created, renamed or deleted. The
 * suite **never** calls `DELETE /api/explorer/trash` (empty-all-trash),
 * which would wipe a real deployment's trash — trashed test items are
 * always restored or purged by their own id.
 *
 * Access-control tests use one throwaway **non-admin** user (deleted in
 * `afterAll`) to prove a non-privileged session cannot escape its allowed
 * roots.
 *
 * All mutating requests go through the CSRF middleware, so the in-page
 * `fetch` helpers send both `x-session-token` and `x-csrf-token`.
 *
 * Run (all):     npx playwright test tests/e2e/explorer/explorer.spec.ts --project=firefox
 * Run (single):  npx playwright test tests/e2e/explorer/explorer.spec.ts -g "1.1"
 * Run (category):npx playwright test tests/e2e/explorer/explorer.spec.ts -g "Security"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/explorer/explorer.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const USER_ROLE_ID = 1;

interface Tokens {
  username: string;
  password: string;
  token: string;
  csrf: string;
}

function uniq(prefix: string): string {
  return `${prefix}${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
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

/** Open the Explorer window and wait for its first directory load to settle. */
async function gotoExplorer(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openLaunchpadApp(page, lang === 'fr' ? 'Explorateur' : 'Explorer');
  await page.waitForSelector('[data-testid="explorer-page"]', { timeout: 10_000 });
  // The initial loading spinner clears (list or an error message appears).
  await expect
    .poll(
      async () =>
        (await win(page).locator('[data-testid="explorer-file-table"], [data-testid="explorer-file-grid"], [data-testid="explorer-state-error"]').count()) >
        0,
      { timeout: 20_000 },
    )
    .toBe(true);
}

/** The Explorer window as a scoping root. */
function win(page: Page) {
  return page.locator('[data-testid="explorer-page"]');
}

/** Walk the file list from `/` down to `absPath`, opening each dir by
 *  selecting its row and pressing Enter (the row's keydown handler calls
 *  `navigateToEntry`). A native double-click is unreliable here: the first
 *  click toggles selection, which inserts the selection toolbar above the
 *  list and shifts every row down between the two clicks. */
async function navigateTo(page: Page, absPath: string) {
  const rowFor = (seg: string) =>
    win(page)
      .locator('[data-testid="explorer-file-row"]:not([data-testid="explorer-file-header"])')
      .filter({ has: page.locator('span.truncate', { hasText: new RegExp(`^${escapeRe(seg)}$`) }) })
      .first();

  await win(page).locator('[data-testid="explorer-breadcrumb"] > button').first().click();
  await expect.poll(() => currentPath(page), { timeout: 10_000 }).toBe('/');

  let acc = '';
  for (const seg of absPath.split('/').filter(Boolean)) {
    acc += `/${seg}`;
    const row = rowFor(seg);
    await row.waitFor({ state: 'visible', timeout: 10_000 });
    for (let attempt = 0; attempt < 3; attempt++) {
      await row.scrollIntoViewIfNeeded().catch(() => {});
      await row.click();
      await page.keyboard.press('Enter');
      try {
        await expect.poll(() => currentPath(page), { timeout: 5_000 }).toBe(acc);
        break;
      } catch {
        if (attempt === 2) throw new Error(`navigateTo: could not open ${acc}`);
      }
    }
  }
}

function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Open the "New folder" modal from the file-toolbar action. */
async function openNewFolder(page: Page) {
  await page.getByRole('button', { name: /new folder|nouveau dossier/i }).first().click();
  await expect(page.locator('[data-slot="dialog-content"] [data-testid="explorer-new-folder-input"]')).toBeVisible();
}

/** Create a folder in the current directory through the modal. */
async function createFolderViaModal(page: Page, name: string) {
  await openNewFolder(page);
  const dlg = page.locator('[data-slot="dialog-content"]');
  await dlg.locator('[data-testid="explorer-new-folder-input"]').fill(name);
  await dlg.getByRole('button', { name: /^create$|^créer$/i }).click();
}

async function currentPath(page: Page): Promise<string> {
  // Reconstruct from the breadcrumb (root button + one button per path segment).
  return page.evaluate(() => {
    const items = Array.from(document.querySelectorAll('[data-testid="explorer-breadcrumb"] > button'));
    if (items.length <= 1) return '/';
    const parts = items.slice(1).map((el) => (el.textContent || '').trim());
    return '/' + parts.join('/');
  });
}

// --- in-page authed fetch helpers (CSRF-aware) ----------------------------

type Api = { http: number; success?: boolean; data?: any; message?: string };

async function apiGet(page: Page, t: Tokens, uri: string): Promise<Api> {
  return page.evaluate(
    async ({ token, uri }) => {
      const r = await fetch(uri, { headers: { 'x-session-token': token } });
      const b = await r.json().catch(() => ({}));
      return { http: r.status, success: b?.success, data: b?.data, message: b?.message };
    },
    { token: t.token, uri },
  );
}

async function apiPost(page: Page, t: Tokens, uri: string, body?: unknown): Promise<Api> {
  return page.evaluate(
    async ({ token, csrf, uri, body }) => {
      const r = await fetch(uri, {
        method: 'POST',
        headers: {
          'x-session-token': token,
          'x-csrf-token': csrf,
          ...(body ? { 'Content-Type': 'application/json' } : {}),
        },
        body: body ? JSON.stringify(body) : undefined,
      });
      const b = await r.json().catch(() => ({}));
      return { http: r.status, success: b?.success, data: b?.data, message: b?.message };
    },
    { token: t.token, csrf: t.csrf, uri, body },
  );
}

async function apiPut(page: Page, t: Tokens, uri: string, body?: unknown): Promise<Api> {
  return page.evaluate(
    async ({ token, csrf, uri, body }) => {
      const r = await fetch(uri, {
        method: 'PUT',
        headers: {
          'x-session-token': token,
          'x-csrf-token': csrf,
          ...(body ? { 'Content-Type': 'application/json' } : {}),
        },
        body: body ? JSON.stringify(body) : undefined,
      });
      const b = await r.json().catch(() => ({}));
      return { http: r.status, success: b?.success, data: b?.data, message: b?.message };
    },
    { token: t.token, csrf: t.csrf, uri, body },
  );
}

async function apiDelete(page: Page, t: Tokens, uri: string, body?: unknown): Promise<Api> {
  return page.evaluate(
    async ({ token, csrf, uri, body }) => {
      const r = await fetch(uri, {
        method: 'DELETE',
        headers: { 'x-session-token': token, 'x-csrf-token': csrf, ...(body ? { 'Content-Type': 'application/json' } : {}) },
        body: body ? JSON.stringify(body) : undefined,
      });
      const b = await r.json().catch(() => ({}));
      return { http: r.status, success: b?.success, data: b?.data, message: b?.message };
    },
    { token: t.token, csrf: t.csrf, uri, body },
  );
}

/** Raw byte-length of a download (or the HTTP status on failure). */
async function apiDownloadLen(page: Page, t: Tokens, path: string): Promise<{ http: number; len: number }> {
  return page.evaluate(
    async ({ token, path }) => {
      const r = await fetch(`/api/explorer/download?path=${encodeURIComponent(path)}`, {
        headers: { 'x-session-token': token },
        cache: 'no-store',
      });
      if (!r.ok) return { http: r.status, len: 0 };
      const buf = await r.arrayBuffer();
      return { http: r.status, len: buf.byteLength };
    },
    { token: t.token, path },
  );
}

async function apiUpload(
  page: Page,
  t: Tokens,
  dir: string,
  filename: string,
  contents: string,
): Promise<Api> {
  return page.evaluate(
    async ({ token, csrf, dir, filename, contents }) => {
      const fd = new FormData();
      fd.append('file', new Blob([contents], { type: 'text/plain' }), filename);
      const r = await fetch(`/api/explorer/upload?path=${encodeURIComponent(dir)}`, {
        method: 'POST',
        headers: { 'x-session-token': token, 'x-csrf-token': csrf },
        body: fd,
      });
      const b = await r.json().catch(() => ({}));
      return { http: r.status, success: b?.success, data: b?.data, message: b?.message };
    },
    { token: t.token, csrf: t.csrf, dir, filename, contents },
  );
}

const q = encodeURIComponent;

// --- shared state --------------------------------------------------------

let adminTokens: Tokens;
let plainUser: Tokens;
let sharedCtx: APIRequestContext;
let adminHeaders: Record<string, string>;
/** The one throwaway working directory everything happens inside. */
let WORK = '';

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
  adminHeaders = { 'x-session-token': token, 'x-csrf-token': csrf };

  WORK = `/tmp/hv-fx-e2e-${Date.now().toString(36)}`;
  const mk = await sharedCtx.post(`/api/explorer/mkdir?path=${q(WORK)}`, { headers: adminHeaders });
  if (!mk.ok()) throw new Error(`could not create the E2E work dir: ${mk.status()} ${await mk.text()}`);

  // A throwaway non-admin for the access-control tests.
  const username = uniq('fx_');
  const password = 'FxUserPassw0rd!';
  let res = await sharedCtx.post('/api/users', {
    headers: adminHeaders,
    data: {
      username,
      password,
      fullname: 'Explorer E2E User',
      email: '',
      role_id: USER_ROLE_ID,
      groups: [],
      home_enabled: false,
    },
  });
  for (let attempt = 0; !res.ok() && attempt < 3; attempt++) {
    if (!/rate limit/i.test(await res.text())) break;
    await new Promise((r) => setTimeout(r, 15_000));
    res = await sharedCtx.post('/api/users', {
      headers: adminHeaders,
      data: { username, password, fullname: 'Explorer E2E User', email: '', role_id: USER_ROLE_ID, groups: [], home_enabled: false },
    });
  }
  if (!res.ok()) throw new Error(`could not create the throwaway non-admin: ${res.status()} ${await res.text()}`);
  // The account was created with an admin-chosen password, so
  // `password_change_guard` refuses every call until the owner replaces it.
  // Clear the flag here: these tests are about explorer authorization, and a
  // 403 from the forced-change gate would prove nothing.
  const ownedPassword = `Owned${password}`;
  const flagged = await apiLogin(sharedCtx, username, password);
  if (!flagged.token) throw new Error('throwaway non-admin could not log in');
  await sharedCtx.put(`/api/users/${username}/password`, {
    headers: { 'x-session-token': flagged.token, 'x-csrf-token': flagged.csrf },
    data: { current_password: password, new_password: ownedPassword },
  });
  const lg = await apiLogin(sharedCtx, username, ownedPassword);
  if (!lg.token) throw new Error('throwaway non-admin could not log in after the password change');
  plainUser = { username, password: ownedPassword, token: lg.token, csrf: lg.csrf };

  // A freshly seeded server answers `GET /api/whats-new` with `seen: false`,
  // and the desktop then opens a bits-ui modal whose full-screen overlay
  // swallows every pointer event ("dialog-overlay intercepts pointer events"
  // on the dock, in every test that has to open an app). Acknowledging the
  // release once here — the same authenticated + CSRF'd call the modal's own
  // close button makes — takes the overlay out of every test's path.
  // `desktop.spec.ts` carries the same guard for the same reason.
  await sharedCtx.post('/api/whats-new/seen', {
    headers: { 'X-Session-Token': adminTokens.token, 'X-CSRF-Token': adminTokens.csrf },
  });
});

test.afterAll(async () => {
  try {
    // Sweep every hv-fx-e2e work dir (this run's and any left by an earlier
    // crashed run — Playwright restarts the worker on each failure).
    const root = await sharedCtx.get(`/api/explorer/list?path=${q('/tmp')}`, { headers: adminHeaders });
    const rootBody = await root.json().catch(() => ({}));
    for (const e of (rootBody?.data?.entries as any[]) || []) {
      if (String(e.name || '').startsWith('hv-fx-e2e-') || String(e.name || '').startsWith('hv-fx-probe-')) {
        await sharedCtx
          .post(`/api/explorer/delete?path=${q(e.path)}`, { headers: adminHeaders })
          .catch(() => {});
      }
    }
    if (plainUser?.username) {
      await sharedCtx.delete(`/api/users/${plainUser.username}`, { headers: adminHeaders }).catch(() => {});
    }
    // Revoke any share-by-link the suite created (links are per-user + kept
    // server-side, so they'd otherwise accumulate across runs).
    const links = await sharedCtx.get('/api/explorer/shares', { headers: adminHeaders });
    const linksBody = await links.json().catch(() => ({}));
    for (const l of (linksBody?.data as any[]) || []) {
      if (/\/hv-fx-e2e-/.test(String(l.path || '')) && l.token) {
        await sharedCtx
          .delete(`/api/explorer/shares/${encodeURIComponent(l.token)}`, { headers: adminHeaders })
          .catch(() => {});
      }
    }
    // Purge any trash entry pointing back into a work dir.
    const tr = await sharedCtx.get('/api/explorer/trash', { headers: adminHeaders });
    const body = await tr.json().catch(() => ({}));
    for (const e of (body?.data as any[]) || []) {
      if (/\/hv-fx-e2e-/.test(String(e.original_path || '')) && e.id) {
        await sharedCtx.delete(`/api/explorer/trash/${e.id}`, { headers: adminHeaders }).catch(() => {});
      }
    }
    // Favorites are per-user and server-side: the `e2ework` shortcut `openWork`
    // pins would otherwise accumulate one row per run and, once `.first()`
    // picked a stale one, break every later run. Sweep them all (not just this
    // run's, since a filtered `-g` run skips describe 15's own cleanup).
    for (const p of await staleWorkFavorites()) {
      await sharedCtx.delete('/api/explorer/favorites', { headers: adminHeaders, data: { path: p } }).catch(() => {});
    }
  } finally {
    await sharedCtx.dispose();
  }
});

test.afterEach(async ({ page }, testInfo) => {
  const heapMB = await getHeapMB(page).catch(() => 0);
  if (heapMB > 0) {
    // eslint-disable-next-line no-console
    console.log(`[MEM] ${testInfo.title}: ${heapMB.toFixed(1)} MB heap`);
  }
});

// ===========================================================================
// 1. Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — Explorer window opens from the Applications launcher', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — header bar is icon + title, and New folder + Upload sit in the file toolbar', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const bar = win(page).locator('.hv-winhdr');
    await expect(bar.locator('.hv-winhdr-title')).toHaveCount(1);
    // No page-level action may sit in the header bar itself.
    await expect(bar.locator('.hv-winhdr-actions')).toHaveCount(0);
    await expect(bar.locator('button, input, select, a, label')).toHaveCount(0);
    // Both actions render in the in-body file toolbar instead.
    const toolbar = win(page).locator('[data-testid="explorer-toolbar"]');
    await expect(toolbar.getByRole('button', { name: /new folder|nouveau dossier/i }).first()).toBeVisible();
    await expect(toolbar.getByRole('button', { name: /upload|téléverser|importer/i }).first()).toBeVisible();
  });

  test('1.3 — layout: sidebar + main pane + toolbar + a file list', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await expect(win(page).locator('[data-testid="explorer-sidebar"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="explorer-file-area"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="explorer-toolbar"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="explorer-file-table"], [data-testid="explorer-file-grid"]')).toBeVisible();
  });

  test('1.4 — breadcrumb starts at root and the footer summarises the folder', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await expect.poll(async () => currentPath(page)).toBe('/');
    await expect(win(page).locator('[data-testid="explorer-footer"]')).toBeVisible();
  });

  test('1.5 — the loading spinner does not persist', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await expect(win(page).locator('[data-testid="explorer-file-area"] .animate-spin')).toHaveCount(0);
  });

  test('1.6 — the sidebar collapses to an icon rail and expands back', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const sidebar = win(page).locator('[data-testid="explorer-sidebar"]');
    const toggle = win(page).locator('.hv-winsidebar-toggle-btn');
    const labels = sidebar.locator('.hv-winsidebar-item-label');
    await expect(toggle).toBeVisible();
    await expect(sidebar).not.toHaveClass(/hv-winsidebar--collapsed/);
    await expect(labels.first()).toBeVisible();

    // Collapse → rail keeps the icons (as tooltips), drops the labels.
    await toggle.click();
    await expect(sidebar).toHaveClass(/hv-winsidebar--collapsed/);
    await expect(labels).toHaveCount(0);
    await expect(sidebar.locator('.hv-winsidebar-item').first()).toHaveAttribute('title', /.+/);
    // The rail width animates (`transition: width …` in `WindowSidebar`), and
    // `toHaveClass` resolves on the very first frame — measuring right then
    // caught the box mid-transition (72px on the way from 256 to 64) and made
    // this test fail intermittently. Poll until the width settles instead.
    await expect
      .poll(async () => (await sidebar.boundingBox())?.width ?? Infinity, { timeout: 5_000 })
      .toBeLessThanOrEqual(70);

    // Expand back → labels and full width return.
    await toggle.click();
    await expect(sidebar).not.toHaveClass(/hv-winsidebar--collapsed/);
    await expect(labels.first()).toBeVisible();
    await expect
      .poll(async () => (await sidebar.boundingBox())?.width ?? 0, { timeout: 5_000 })
      .toBeGreaterThan(150);
  });

  test('1.7 — the status footer shows the folder summary and the selection count', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const footer = win(page).locator('[data-testid="explorer-footer"]');
    await expect(footer).toBeVisible();
    await expect(footer).toContainText(/\d+/);
    await expect(win(page).locator('[data-testid="explorer-selection-summary"]')).toHaveCount(0);

    // Selecting a row surfaces the selection count in the footer.
    const row = win(page).locator('[data-testid="explorer-file-row"]').first();
    await row.waitFor({ state: 'visible', timeout: 10_000 });
    await row.click();
    await expect(win(page).locator('[data-testid="explorer-selection-summary"]')).toBeVisible();
  });
});

// ===========================================================================
// 2. Form fields
// ===========================================================================

test.describe('2 — Form fields', () => {
  test('2.1 — New folder modal has a single text input with a placeholder', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await page.getByRole('button', { name: /new folder|nouveau dossier/i }).first().click();
    const input = page.locator('[data-testid="explorer-new-folder-input"]').first();
    await expect(input).toBeVisible();
    await expect(input).toHaveAttribute('type', 'text');
    await expect(input).toHaveAttribute('placeholder', /.+/);
  });

  test('2.2 — search input is a search field whose value can be cleared', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await navigateTo(page, WORK); // a near-empty dir so the recursive search returns fast
    const search = win(page).locator('[data-testid="explorer-search-input"]');
    await expect(search).toHaveAttribute('type', 'search');
    await search.fill('needle-nomatch-xyz');
    await expect(search).toHaveValue('needle-nomatch-xyz');
    // The clear (×) button appears once there is a query (it briefly yields
    // to a spinner while the debounced search runs, then comes back).
    const clear = win(page).locator('[data-testid="explorer-search-clear"]');
    await expect(clear).toBeVisible({ timeout: 10_000 });
    await clear.click();
    await expect(search).toHaveValue('');
  });

  test('2.3 — compress modal exposes an archive name + a format selector', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await apiUpload(page, adminTokens, WORK, 'c1.txt', 'x');
    await navigateTo(page, WORK);
    const row = win(page).locator('[data-testid="explorer-file-row"]').filter({ hasText: 'c1.txt' }).first();
    await row.click();
    // The compress action only appears once something is selected.
    await win(page).locator('[data-testid="explorer-compress"]').first().click();
    await expect(page.locator('#compressName')).toBeVisible();
    await expect(page.locator('#compressFormat')).toBeVisible();
  });
});

// ===========================================================================
// 3. Validation
// ===========================================================================

test.describe('3 — Validation', () => {
  test('3.1 — chmod rejects a non-octal mode (clean error, no 500)', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await apiUpload(page, adminTokens, WORK, 'perm.txt', 'x');
    for (const bad of ['999', 'abc', '77777']) {
      const r = await apiGet(
        page,
        adminTokens,
        `/api/explorer/permissions?path=${q(`${WORK}/perm.txt`)}&mode=${bad}`,
      );
      expect(r.http).not.toBe(500);
      expect(r.success).toBe(false);
    }
  });

  test('3.2 — deleting a path that does not exist fails cleanly', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const r = await apiPost(page, adminTokens, `/api/explorer/delete?path=${q(`${WORK}/ghost-xyz`)}`);
    expect(r.http).not.toBe(500);
    expect(r.success).toBe(false);
  });

  test('3.3 — New folder modal with a blank name creates nothing', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await navigateTo(page, WORK);
    const before = (await apiGet(page, adminTokens, `/api/explorer/list?path=${q(WORK)}`)).data
      ?.entries?.length ?? 0;
    await openNewFolder(page);
    const dlg = page.locator('[data-slot="dialog-content"]');
    await dlg.locator('[data-testid="explorer-new-folder-input"]').fill('   ');
    await dlg.getByRole('button', { name: /^create$|^créer$/i }).click();
    await page.waitForTimeout(500);
    const after = (await apiGet(page, adminTokens, `/api/explorer/list?path=${q(WORK)}`)).data
      ?.entries?.length ?? 0;
    expect(after).toBe(before);
  });
});

// ===========================================================================
// 4. All user interactions
// ===========================================================================

test.describe('4 — Interactions', () => {
  test('4.1 — list ↔ grid view toggle', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await win(page).locator('[data-testid="explorer-view-grid"]').click();
    await expect(win(page).locator('[data-testid="explorer-file-grid"]')).toBeVisible();
    await win(page).locator('[data-testid="explorer-view-list"]').click();
    await expect(win(page).locator('[data-testid="explorer-file-table"]')).toBeVisible();
  });

  test('4.2 — column sort buttons toggle the arrow', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const nameSort = win(page).locator('[data-testid="explorer-file-header"] .sort-btn').first();
    await nameSort.click();
    await expect(nameSort).toHaveText(/[▲▼]/);
  });

  test('4.3 — Trash sidebar button opens the trash view', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    // The Trash button is not necessarily the first .disk-btn — the sidebar
    // also has a "Locations" section (roots + virtual disks) above it.
    await win(page)
      .locator('[data-testid="explorer-sidebar"] .disk-btn')
      .filter({ hasText: /trash|corbeille|papierkorb|papelera/i })
      .first()
      .click();
    await expect(win(page).locator('[data-testid="explorer-trash-toolbar"]')).toBeVisible();
  });

  test('4.4 — New folder modal opens and cancels without side effects', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await openNewFolder(page);
    await page.locator('[data-slot="dialog-content"]').getByRole('button', { name: /^cancel$|^annuler$/i }).click();
    await expect(page.locator('[data-slot="dialog-content"] [data-testid="explorer-new-folder-input"]')).toHaveCount(0);
  });

  test('4.5 — the Trash view exposes the admin retention settings', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await win(page)
      .locator('[data-testid="explorer-sidebar"] .disk-btn')
      .filter({ hasText: /trash|corbeille|papierkorb|papelera/i })
      .first()
      .click();

    const panel = win(page).locator('[data-testid="explorer-trash-retention"]');
    await expect(panel).toBeVisible();
    // Collapsed by default; expanding reveals the age + size controls.
    await expect(win(page).locator('[data-testid="explorer-trash-retention-days"]')).toHaveCount(0);
    await win(page).locator('[data-testid="explorer-trash-retention-toggle"]').click();
    await expect(win(page).locator('[data-testid="explorer-trash-retention-days"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="explorer-trash-retention-size"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="explorer-trash-retention-days"] option')).toHaveCount(4);

    // The panel reads the real policy — a fresh install shows the default 30 days.
    const pol = await apiGet(page, adminTokens, '/api/explorer/trash/policy');
    expect(pol.http).toBe(200);
    expect(pol.data?.retention_days).toBeGreaterThanOrEqual(0);
  });
});

// ===========================================================================
// 5. Submission flows
// ===========================================================================

test.describe('5 — Submission flows', () => {
  test('5.1 — creating a folder from the modal makes it appear in the list', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await navigateTo(page, WORK);
    const name = uniq('ui_dir_');
    await createFolderViaModal(page, name);
    await expect(
      win(page).locator('[data-testid="explorer-file-row"]').filter({ hasText: name }),
    ).toBeVisible({ timeout: 10_000 });
  });

  test('5.2 — an upload lands on disk and shows up after refresh', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const r = await apiUpload(page, adminTokens, WORK, 'uploaded.txt', 'hello from e2e');
    expect(r.success).toBe(true);
    const list = await apiGet(page, adminTokens, `/api/explorer/list?path=${q(WORK)}`);
    expect((list.data?.entries || []).some((e: any) => e.name === 'uploaded.txt')).toBe(true);
  });

  test('5.3 — hammering Create does not create two folders', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await navigateTo(page, WORK);
    const name = uniq('once_');
    await openNewFolder(page);
    const dlg = page.locator('[data-slot="dialog-content"]');
    await dlg.locator('[data-testid="explorer-new-folder-input"]').fill(name);
    const create = dlg.getByRole('button', { name: /^create$|^créer$/i });
    await create.click();
    await create.click().catch(() => {});
    await page.waitForTimeout(1000);
    const list = await apiGet(page, adminTokens, `/api/explorer/list?path=${q(WORK)}`);
    const hits = (list.data?.entries || []).filter((e: any) => e.name === name).length;
    expect(hits).toBe(1);
  });
});

// ===========================================================================
// 6. Navigation
// ===========================================================================

test.describe('6 — Navigation', () => {
  test('6.1 — descend into a folder then climb back with the parent button', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await navigateTo(page, WORK);
    await expect.poll(async () => currentPath(page)).toBe(WORK);
    await win(page).locator('[data-testid="explorer-back"]').click(); // back / parent
    await expect.poll(async () => currentPath(page)).toBe('/tmp');
  });

  test('6.2 — breadcrumb root jumps back to "/"', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await navigateTo(page, WORK);
    await win(page).locator('[data-testid="explorer-breadcrumb"] > button').first().click();
    await expect.poll(async () => currentPath(page)).toBe('/');
  });

  test('6.3 — a breadcrumb segment jumps straight to that ancestor', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await navigateTo(page, WORK);
    // Click the "tmp" segment (second breadcrumb button — index 1, after the root).
    await win(page).locator('[data-testid="explorer-breadcrumb"] > button').nth(1).click();
    await expect.poll(() => currentPath(page)).toBe('/tmp');
  });
});

// ===========================================================================
// 7. Responsive
// ===========================================================================

test.describe('7 — Responsive', () => {
  for (const w of [360, 768, 1920]) {
    test(`7.x — no horizontal overflow at ${w}px`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: 800 });
      await gotoExplorer(page, adminTokens);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(overflow).toBeLessThanOrEqual(2);
      await expect(win(page).locator('[data-testid="explorer-toolbar"]')).toBeVisible();
    });
  }
});

// ===========================================================================
// 8. Accessibility
// ===========================================================================

test.describe('8 — Accessibility', () => {
  test('8.1 — file rows are keyboard-focusable buttons', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await navigateTo(page, '/tmp');
    const row = win(page).locator('[data-testid="explorer-file-row"]:not([data-testid="explorer-file-header"])').first();
    await expect(row).toHaveAttribute('role', 'button');
    await expect(row).toHaveAttribute('tabindex', '0');
  });

  test('8.2 — the drop zone has an accessible label', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await expect(win(page).locator('[data-testid="explorer-file-area"]')).toHaveAttribute('aria-label', /.+/);
  });

  test('8.3 — modals are marked up as dialogs', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await apiUpload(page, adminTokens, WORK, 'a11y.txt', 'x');
    await navigateTo(page, WORK);
    await win(page).locator('[data-testid="explorer-file-row"]').filter({ hasText: 'a11y.txt' }).first().click();
    await win(page).locator('[data-testid="explorer-row-permissions"]').first().click();
    await expect(page.locator('[role="dialog"][aria-modal="true"]').first()).toBeVisible();
  });

  test('8.4 — no duplicate element IDs on the page', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const dupes = await page.evaluate(() => {
      const seen = new Set<string>();
      const dup: string[] = [];
      for (const el of Array.from(document.querySelectorAll('[id]'))) {
        const id = el.id;
        if (seen.has(id)) dup.push(id);
        seen.add(id);
      }
      return dup;
    });
    expect(dupes).toEqual([]);
  });

  // W18: a role="button" row must activate on Space as well as Enter
  // (WAI-ARIA button pattern) — Space also needs preventDefault, otherwise
  // the file list scrolls instead of the row opening.
  test('8.5 — Space activates a focused folder row, not just Enter', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const sub = `${WORK}/space-nav`;
    expect((await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(sub)}`)).success).toBe(true);
    await navigateTo(page, WORK);
    const row = win(page)
      .locator('[data-testid="explorer-file-row"]:not([data-testid="explorer-file-header"])')
      .filter({ has: page.locator('span.truncate', { hasText: /^space-nav$/ }) })
      .first();
    await row.waitFor({ state: 'visible', timeout: 10_000 });
    await row.focus();
    await page.keyboard.press('Space');
    await expect.poll(() => currentPath(page), { timeout: 10_000 }).toBe(sub);
  });
});

// ===========================================================================
// 9. Performance
// ===========================================================================

test.describe('9 — Performance', () => {
  test('9.1 — Explorer window is interactive quickly', async ({ page }) => {
    const start = Date.now();
    await gotoExplorer(page, adminTokens);
    expect(Date.now() - start).toBeLessThan(15_000);
  });

  test('9.2 — the Explorer window DOM stays reasonably small for one directory', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await navigateTo(page, WORK); // a small directory we control
    const nodes = await win(page).evaluate((el) => el.querySelectorAll('*').length);
    expect(nodes).toBeLessThan(2500);
  });

  test('9.3 — repeated open/close does not leak large amounts of heap', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const before = await getHeapMB(page);
    for (let i = 0; i < 4; i++) {
      await navigateTo(page, '/tmp');
      await win(page).locator('[data-testid="explorer-breadcrumb"] > button').first().click();
      await expect.poll(async () => currentPath(page)).toBe('/');
    }
    const after = await getHeapMB(page);
    if (before > 0 && after > 0) expect(after - before).toBeLessThan(20);
  });
});

// ===========================================================================
// 10. Security
// ===========================================================================

test.describe('10 — Security', () => {
  test('10.1 — every JSON endpoint rejects a request with no session token', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const noAuth = (uri: string, method = 'GET') =>
      page.evaluate(
        async ({ uri, method }) => {
          const r = await fetch(uri, { method });
          const b = await r.json().catch(() => ({}));
          return { http: r.status, success: b?.success };
        },
        { uri, method },
      );
    for (const [uri, method] of [
      [`/api/explorer/list?path=${q(WORK)}`, 'GET'],
      [`/api/explorer/mkdir?path=${q(`${WORK}/x`)}`, 'POST'],
      [`/api/explorer/delete?path=${q(`${WORK}/x`)}`, 'POST'],
      [`/api/explorer/trash?path=${q(`${WORK}/x`)}`, 'POST'],
      [`/api/explorer/trash`, 'GET'],
      [`/api/explorer/trash/policy`, 'GET'],
    ] as const) {
      const r = await noAuth(uri, method);
      expect(r.success, `${method} ${uri}`).toBe(false);
      expect(r.http, `${method} ${uri}`).not.toBe(200);
    }
  });

  test('10.2 — a non-admin session cannot list a directory outside its allowed roots', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const r = await apiGet(page, plainUser, '/api/explorer/list?path=/etc');
    expect(r.success).toBe(false);
  });

  test('10.3 — a non-admin session cannot download /etc/passwd', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const r = await apiDownloadLen(page, plainUser, '/etc/passwd');
    expect(r.http).toBe(403);
    expect(r.len).toBe(0);
  });

  test('10.4 — a non-admin cannot traverse out with ".."', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const r = await apiGet(
      page,
      plainUser,
      `/api/explorer/preview?path=${q(`${WORK}/../../../../etc/passwd`)}`,
    );
    expect(r.success).toBe(false);
  });

  test('10.5 — the session token never appears in the URL', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    expect(page.url()).not.toContain(adminTokens.token);
  });

  test('10.6 — a filename that looks like markup is rendered as text, not HTML', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const xss = 'x<img src=x onerror=alert(1)>.txt';
    await apiUpload(page, adminTokens, WORK, xss, 'x');
    await navigateTo(page, WORK); // its final load picks up the file just uploaded
    const cell = win(page).locator('span.truncate', { hasText: 'onerror=alert(1)' }).first();
    await expect(cell).toBeVisible();
    // The filename must not have injected a real <img> element.
    expect(await page.locator('img[src="x"]').count()).toBe(0);
  });

  test('10.7 — a non-admin can read the trash policy but cannot change it', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    // Reading the policy is fine for any signed-in user (the panel is
    // admin-only in the UI, but the value itself is not a secret).
    const read = await apiGet(page, plainUser, '/api/explorer/trash/policy');
    expect(read.http).toBe(200);
    expect(read.success).toBe(true);

    // Changing when deleted files are purged is an admin decision.
    const write = await apiPut(page, plainUser, '/api/explorer/trash/policy', {
      retention_days: 7,
      max_bytes: 0,
    });
    expect(write.http).not.toBe(200);
    expect(write.success).toBe(false);
  });
});

// ===========================================================================
// 11. Keyboard
// ===========================================================================

test.describe('11 — Keyboard', () => {
  test('11.1 — Enter in the New folder input submits', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await navigateTo(page, WORK);
    const name = uniq('kbd_');
    await openNewFolder(page);
    const input = page.locator('[data-slot="dialog-content"] [data-testid="explorer-new-folder-input"]');
    await input.fill(name);
    await input.press('Enter');
    await expect(
      win(page).locator('[data-testid="explorer-file-row"]').filter({ hasText: name }),
    ).toBeVisible({ timeout: 10_000 });
  });

  test('11.2 — Escape closes the New folder modal', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await openNewFolder(page);
    await page.keyboard.press('Escape');
    await expect(page.locator('[data-slot="dialog-content"] [data-testid="explorer-new-folder-input"]')).toHaveCount(0);
  });

  test('11.3 — a directory row opens on Enter after being focused', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    await navigateTo(page, WORK);
    const name = uniq('kbdnav_');
    await createFolderViaModal(page, name);
    const row = win(page).locator('[data-testid="explorer-file-row"]').filter({ hasText: name }).first();
    await row.click();
    await page.keyboard.press('Enter');
    await expect.poll(() => currentPath(page), { timeout: 10_000 }).toBe(`${WORK}/${name}`);
  });
});

// ===========================================================================
// 12. Console & network
// ===========================================================================

test.describe('12 — Console & network', () => {
  test('12.1 — no console errors while loading and browsing', async ({ page }) => {
    const errors: string[] = [];
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(m.text());
    });
    await gotoExplorer(page, adminTokens);
    await navigateTo(page, '/tmp');
    const real = errors.filter(
      (e) => !/favicon|Failed to load resource.*(404|401)|ResizeObserver/i.test(e),
    );
    expect(real).toEqual([]);
  });

  test('12.2 — opening the app issues a bounded number of explorer API calls', async ({ page }) => {
    const calls: string[] = [];
    page.on('request', (r) => {
      if (r.url().includes('/api/explorer/')) calls.push(r.url());
    });
    await gotoExplorer(page, adminTokens);
    // The budget is "one bounded batch per open", not "as few as possible":
    // 16 leaves exactly the historic one-request margin over the measured
    // baseline. The batch grew by one when the sidebar gained its File tags
    // list (`GET /api/explorer/tags`, one request, no per-directory refetch).
    expect(calls.length).toBeLessThan(16);
  });
});

// ===========================================================================
// 13. Real lifecycle (all verified via the API)
// ===========================================================================

test.describe('13 — Real lifecycle', () => {
  test('13.1 — mkdir → upload → download bytes → copy → move → rename → delete', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const dir = `${WORK}/${uniq('life_')}`;
    expect((await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(dir)}`)).success).toBe(true);

    const payload = 'lifecycle-payload-0123456789';
    expect((await apiUpload(page, adminTokens, dir, 'src.txt', payload)).success).toBe(true);

    const dl = await apiDownloadLen(page, adminTokens, `${dir}/src.txt`);
    expect(dl.http).toBe(200);
    expect(dl.len).toBe(payload.length);

    expect(
      (await apiPost(
        page,
        adminTokens,
        `/api/explorer/copy?source=${q(`${dir}/src.txt`)}&destination=${q(`${dir}/copy.txt`)}`,
      )).success,
    ).toBe(true);

    expect(
      (await apiPost(
        page,
        adminTokens,
        `/api/explorer/move?source=${q(`${dir}/copy.txt`)}&destination=${q(`${dir}/moved.txt`)}`,
      )).success,
    ).toBe(true);

    expect(
      (await apiPost(
        page,
        adminTokens,
        `/api/explorer/rename?path=${q(`${dir}/moved.txt`)}&new_name=${q(`${dir}/final.txt`)}`,
      )).success,
    ).toBe(true);

    const list = await apiGet(page, adminTokens, `/api/explorer/list?path=${q(dir)}`);
    const names = (list.data?.entries || []).map((e: any) => e.name).sort();
    expect(names).toEqual(['final.txt', 'src.txt']);

    expect((await apiPost(page, adminTokens, `/api/explorer/delete?path=${q(dir)}`)).success).toBe(true);
  });

  test('13.2 — trash, restore and purge-by-id round-trip through the trash', async ({ page }) => {
    // Full lifecycle against the live server: the 2026-09-06 fix to
    // `FileBrowser::find_trash_entry` (a missing trash root is now `NotFound`
    // + try the next root, instead of propagating its `read_dir` error) is
    // what makes restore/purge-by-id work when the root the file came from
    // has no `.homevault-trash` yet. Before it this test had to stop at the
    // trash move. Purge stays per-id — never `DELETE /api/explorer/trash`
    // (empty-all), which would wipe a real deployment's bin.
    await gotoExplorer(page, adminTokens);
    const file = `${WORK}/trash-me-${Date.now().toString(36)}.txt`;
    await apiUpload(page, adminTokens, WORK, file.split('/').pop()!, 'bin me');

    const inTrash = async () => {
      const tr = await apiGet(page, adminTokens, '/api/explorer/trash');
      return ((tr.data as any[]) || []).find((e) => e.original_path === file);
    };
    const inWork = async () => {
      const dir = await apiGet(page, adminTokens, `/api/explorer/list?path=${q(WORK)}`);
      return ((dir.data?.entries || []) as any[]).some((e) => e.path === file);
    };

    // 1. move to trash — gone from the folder, listed in the trash.
    expect((await apiPost(page, adminTokens, `/api/explorer/trash?path=${q(file)}`)).success).toBe(true);
    expect(await inWork()).toBe(false);
    const entry = await inTrash();
    expect(entry, 'the trashed file is listed in the trash with its original path').toBeTruthy();

    // 2. restore by id — back at its original path, gone from the trash.
    const restored = await apiPost(page, adminTokens, `/api/explorer/trash/restore?id=${q(entry.id)}`);
    expect(restored.success, restored.message).toBe(true);
    expect(await inWork()).toBe(true);
    expect(await inTrash()).toBeFalsy();

    // 3. trash again, then purge that one entry by id.
    expect((await apiPost(page, adminTokens, `/api/explorer/trash?path=${q(file)}`)).success).toBe(true);
    const again = await inTrash();
    expect(again, 'the re-trashed file is listed again').toBeTruthy();
    const purged = await apiDelete(page, adminTokens, `/api/explorer/trash/${q(again.id)}`);
    expect(purged.success, purged.message).toBe(true);
    expect(await inTrash(), 'the purged entry is gone from the trash listing').toBeFalsy();
    expect(await inWork(), 'a purge must not resurrect the file').toBe(false);
  });

  test('13.3 — compress a selection then extract it back', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const dir = `${WORK}/${uniq('zip_')}`;
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(dir)}`);
    await apiUpload(page, adminTokens, dir, 'doc-a.txt', 'aaa');
    await apiUpload(page, adminTokens, dir, 'doc-b.txt', 'bbb');

    const archive = `${dir}/bundle.tar.gz`;
    const c = await apiPost(page, adminTokens, '/api/explorer/compress', {
      paths: [`${dir}/doc-a.txt`, `${dir}/doc-b.txt`],
      destination: archive,
      format: 'tar.gz',
    });
    expect(c.success).toBe(true);

    const out = `${dir}/out`;
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(out)}`);
    const x = await apiPost(
      page,
      adminTokens,
      `/api/explorer/extract?path=${q(archive)}&destination=${q(out)}`,
    );
    expect(x.success).toBe(true);
  });
});

// ===========================================================================
// 14. Edge cases & memory regression
// ===========================================================================

test.describe('14 — Edge cases', () => {
  test('14.1 — a unicode filename round-trips through list', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const name = 'rapporté-café-日本語-😀.txt';
    expect((await apiUpload(page, adminTokens, WORK, name, 'u')).success).toBe(true);
    const list = await apiGet(page, adminTokens, `/api/explorer/list?path=${q(WORK)}`);
    expect((list.data?.entries || []).some((e: any) => e.name === name)).toBe(true);
  });

  test('14.2 — a very long filename is handled without a crash', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const name = `${'a'.repeat(200)}.txt`;
    const r = await apiUpload(page, adminTokens, WORK, name, 'x');
    expect(r.http).not.toBe(500);
  });

  test('14.3 — no manual refresh button; rapid visibility events do not break the list', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    // The toolbar refresh button was removed — the directory re-lists on the
    // live watcher + on visibilitychange.
    await expect(
      win(page).locator('button[title*="efresh" i], button[title*="ctualiser" i]'),
    ).toHaveCount(0);
    for (let i = 0; i < 6; i++) {
      await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
    }
    await expect(win(page).locator('[data-testid="explorer-file-table"], [data-testid="explorer-file-grid"]')).toBeVisible();
  });

  test('14.4 — the current directory being deleted underneath surfaces a clean error', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const dir = `${WORK}/${uniq('vanish_')}`;
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(dir)}`);
    await navigateTo(page, dir);
    // Pull the rug: delete the directory we are viewing, then force a re-list
    // the way the app now does it (visibilitychange -> loadDirectory).
    await apiPost(page, adminTokens, `/api/explorer/delete?path=${q(dir)}`);
    await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
    await expect(
      win(page).locator('[data-testid="explorer-state-error"], [data-testid="explorer-state-message"]'),
    ).toBeVisible({ timeout: 10_000 });
    // The window itself must still be alive (no crash / white screen).
    await expect(win(page)).toBeVisible();
  });
});

// ===========================================================================
// 15. Explorer extras — inline editor / favorites / collision strategy /
//     ZFS snapshot mode / Explorer→Shares hand-off
//
// These endpoints and UI paths landed after the 2026-09-06 audit and were
// previously E2E-uncovered (the CLAUDE.md "Explorer" frozen section and
// TESTING.md both list them as "à étendre"). Integration coverage for the
// same endpoints is in tests/explorer_integration.rs; this section drives
// them through the real window. Everything stays inside WORK; favorites
// created here are per-user and server-side, so each test removes its own.
// ===========================================================================

/** DELETE with a JSON body (the favorites endpoint wants `{ path }`). */
async function favRemove(page: Page, t: Tokens, path: string): Promise<Api> {
  return page.evaluate(
    async ({ token, csrf, path }) => {
      const r = await fetch('/api/explorer/favorites', {
        method: 'DELETE',
        headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
        body: JSON.stringify({ path }),
      });
      const b = await r.json().catch(() => ({}));
      return { http: r.status, success: b?.success, data: b?.data, message: b?.message };
    },
    { token: t.token, csrf: t.csrf, path },
  );
}

/** The current user's server-side favorites (array of `{ path, label, … }`). */
async function favList(page: Page, t: Tokens): Promise<any[]> {
  const r = await apiGet(page, t, '/api/explorer/favorites');
  return Array.isArray(r.data) ? (r.data as any[]) : [];
}

async function fileTags(page: Page, t: Tokens): Promise<any[]> {
  const r = await apiGet(page, t, '/api/explorer/tags');
  return Array.isArray(r.data) ? (r.data as any[]) : [];
}

/** Every favorite that points into an `hv-fx-e2e-_` work dir, wherever it
 *  points. `DELETE /api/explorer/favorites` is per-path, so a run that only
 *  removes *its own* favorite leaves the ones an earlier run (or a filtered
 *  `-g "17 …"` run, which skips describe 15's `afterAll`) created behind. */
async function staleWorkFavorites(): Promise<string[]> {
  const paths: string[] = [];
  try {
    const r = await sharedCtx.get('/api/explorer/favorites', { headers: adminHeaders });
    const body = await r.json().catch(() => ({}));
    for (const f of (body?.data as any[]) || []) {
      const p = String(f?.path || '');
      if (p && (f?.label === 'e2ework' || /\/hv-fx-e2e-/.test(p))) paths.push(p);
    }
  } catch {
    /* best effort */
  }
  return paths;
}

// Reaching WORK by walking the file list means clicking through a very
// large /tmp, which is slow and flaky (rows reflow as the list renders and
// on the 8 s auto-refresh). Instead each test opens WORK in one click via a
// sidebar favorite; `openWork` re-asserts it every time so 15.3 (which
// toggles it) can't break the tests after it. Declared at file scope: the
// describe 16 tests use it too (nested in describe 15 they hit a
// ReferenceError — `openWork is not defined` — on every run).
async function openWork(page: Page) {
  // Re-create the favorite with a known label every time (15.3 toggles it,
  // and the UI re-pin path would relabel it to the raw folder name). Every
  // stale work favorite goes first: otherwise the sidebar holds several
  // `e2ework` entries and `.first()` clicks an old one, whose path then
  // never matches `WORK` in the `currentPath` poll below.
  for (const p of await staleWorkFavorites()) {
    await sharedCtx.delete('/api/explorer/favorites', { headers: adminHeaders, data: { path: p } }).catch(() => {});
  }
  await sharedCtx
    .post('/api/explorer/favorites', { headers: adminHeaders, data: { path: WORK, label: 'e2ework' } })
    .catch(() => {});
  await gotoExplorer(page, adminTokens);
  await win(page)
    .locator('[data-testid="explorer-sidebar"] .disk-btn')
    .filter({ hasText: 'e2ework' })
    .first()
    .click();
  await expect.poll(() => currentPath(page), { timeout: 15_000 }).toBe(WORK);
}

test.describe('15 — Explorer extras', () => {
  // Favorites are per-user + server-side; the file-level afterAll only sweeps
  // /tmp work dirs, so clean up any favorite pointing back into one here.
  test.afterAll(async () => {
    try {
      const tr = await sharedCtx.get('/api/explorer/favorites', { headers: adminHeaders });
      const body = await tr.json().catch(() => ({}));
      for (const f of (body?.data as any[]) || []) {
        if (/\/hv-fx-e2e-|\/sharehost/.test(String(f.path || ''))) {
          await sharedCtx
            .delete('/api/explorer/favorites', { headers: adminHeaders, data: { path: f.path } })
            .catch(() => {});
        }
      }
    } catch {
      /* best effort */
    }
    // File tags are per-user + server-side too, and a filtered `-g "15.5"`
    // run skips the in-test cleanup of earlier attempts: sweep every tag this
    // suite may have left, so `15.5` starts from a known-empty list.
    try {
      const tr = await sharedCtx.get('/api/explorer/tags', { headers: adminHeaders });
      const body = await tr.json().catch(() => ({}));
      for (const row of (body?.data as any[]) || []) {
        await sharedCtx
          .delete('/api/explorer/tags', { headers: adminHeaders, data: { path: row.path, tag: row.tag } })
          .catch(() => {});
      }
    } catch {
      /* best effort */
    }
  });

  test('15.1 — inline editor: the preview modal exposes Edit, which opens a textarea with the file body', async ({ page }) => {
    const fname = `${uniq('edit_')}.txt`;
    const body = 'the original line — inline-editor probe';
    await openWork(page);
    expect((await apiUpload(page, adminTokens, WORK, fname, body)).success).toBe(true);
    // (No manual Refresh: the list re-lists via the live watcher.)

    const row = win(page).locator('[data-testid="explorer-file-row"]').filter({ hasText: fname }).first();
    await expect(row).toBeVisible({ timeout: 10_000 });
    await row.locator('[data-testid="explorer-row-preview"]').dispatchEvent('click');

    const modal = page.locator('[data-slot="dialog-content"]');
    await expect(modal).toBeVisible({ timeout: 10_000 });
    await expect(modal.locator('[data-testid="explorer-preview-text"]')).toContainText('inline-editor probe');

    // Edit is offered for a non-truncated text preview and reveals the editor
    // pre-filled with the file body. (The save round-trip through this
    // `position:fixed`-in-a-transformed-window modal is Playwright-flaky, so
    // the actual write is covered by 15.2 against the endpoint directly.)
    const editBtn = modal.getByRole('button', { name: /^edit$|^modifier$/i });
    await expect(editBtn).toBeVisible();
    await editBtn.dispatchEvent('click');
    const editor = modal.locator('[data-testid="explorer-preview-editor"]');
    await expect(editor).toBeVisible({ timeout: 10_000 });
    await expect(editor).toHaveValue(body);
    await expect(modal.getByRole('button', { name: /^save$|^enregistrer$/i })).toBeVisible();
  });

  test('15.2 — POST /api/explorer/save writes the file and rejects an oversized payload', async ({ page }) => {
    await openWork(page);
    const f = `${WORK}/${uniq('save_')}.txt`;
    expect((await apiPost(page, adminTokens, '/api/explorer/save', { path: f, content: 'v1' })).success).toBe(true);
    expect((await apiDownloadLen(page, adminTokens, f)).len).toBe(2);

    // Overwrite in place.
    expect(
      (await apiPost(page, adminTokens, '/api/explorer/save', { path: f, content: 'a longer version' })).success,
    ).toBe(true);
    expect((await apiDownloadLen(page, adminTokens, f)).len).toBe('a longer version'.length);

    // Over the 5 MB editor cap → rejected (never written, never a 500). Build
    // the payload in-page so the 6 MB string isn't marshalled as an eval arg.
    const big = await page.evaluate(
      async ({ token, csrf, path }) => {
        try {
          const r = await fetch('/api/explorer/save', {
            method: 'POST',
            headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
            body: JSON.stringify({ path, content: 'x'.repeat(6 * 1024 * 1024) }),
          });
          const b = await r.json().catch(() => ({}));
          return { http: r.status, success: b?.success === true };
        } catch {
          return { http: 0, success: false };
        }
      },
      { token: adminTokens.token, csrf: adminTokens.csrf, path: f },
    );
    expect(big.http).not.toBe(500);
    expect(big.success).toBe(false);
    // The original small content is still what's on disk.
    expect((await apiDownloadLen(page, adminTokens, f)).len).toBe('a longer version'.length);
  });

  test('15.3 — favorites: the toolbar star toggles the current folder in the sidebar and on the server', async ({ page }) => {
    await openWork(page); // at WORK, which openWork guarantees is favorited ("e2ework")

    const star = win(page).locator('[data-testid="explorer-pin"]');
    await expect(star).toBeVisible();
    const inSidebar = () => win(page).locator('[data-testid="explorer-sidebar"] .disk-name', { hasText: 'e2ework' });
    const onServer = async () => (await favList(page, adminTokens)).some((f) => f.path === WORK);
    // The window's CSS transform makes Playwright's "receives pointer events"
    // check flaky on this toolbar button; it is genuinely visible + unobscured.
    const toggle = () => star.click({ force: true });

    await expect.poll(onServer, { timeout: 10_000 }).toBe(true);
    await expect(inSidebar()).toBeVisible();

    // Un-pin via the toolbar star — gone from the sidebar and the server.
    await toggle();
    await expect(inSidebar()).toHaveCount(0, { timeout: 10_000 });
    await expect.poll(onServer, { timeout: 10_000 }).toBe(false);

    // Re-pin via the same star — the server picks it back up (the sidebar row
    // comes back too, but relabelled to the folder name by the UI re-pin path).
    await toggle();
    await expect.poll(onServer, { timeout: 15_000 }).toBe(true);
  });

  test('15.4 — favorites persist to a freshly opened Explorer window', async ({ page }) => {
    const sub = `${WORK}/${uniq('favp_')}`;
    await gotoExplorer(page, adminTokens);
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(sub)}`);
    const base = sub.split('/').pop()!;
    try {
      expect(
        (await apiPost(page, adminTokens, '/api/explorer/favorites', { path: sub, label: base })).success,
      ).toBe(true);

      // Full desktop reload + a brand-new Explorer window.
      await gotoExplorer(page, adminTokens);
      await expect(
        win(page).locator('[data-testid="explorer-sidebar"] .disk-name', { hasText: base }),
      ).toBeVisible({ timeout: 10_000 });
      // Clicking it jumps straight there.
      await win(page).locator('[data-testid="explorer-sidebar"] .disk-btn').filter({ hasText: base }).first().click();
      await expect.poll(() => currentPath(page), { timeout: 10_000 }).toBe(sub);
    } finally {
      await favRemove(page, adminTokens, sub);
    }
  });

  // File tags have two independent requirements that pull in opposite
  // directions: the tag endpoints accept any path the caller can browse (so the
  // fixture can live in the throwaway `/tmp` workdir), while the **tag-filtered
  // global search** only fans out over the sidebar roots — `/tmp` is not one of
  // them by default (`src/config/explorer_config.rs`), so a file there can never
  // come back from `/api/search/files`. The search half therefore runs in a
  // dedicated folder inside a root the sidebar actually offers, discovered from
  // `GET /api/explorer/roots` (`HV_E2E_TAG_ROOT` overrides it) — the same idea
  // as `security.spec.ts`'s `HV_E2E_WORKROOT`.
  const TAG_SEARCH_SUBDIR = 'hv-e2e-tags';

  test('15.5 — file tags are private, visible in the sidebar and searchable', async ({ page }) => {
    await openWork(page);
    const folder = `${WORK}/${uniq('tag_')}`;
    const file = `${folder}/tagged-manual.txt`;
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(folder)}`);
    await apiUpload(page, adminTokens, folder, 'tagged-manual.txt', 'tag search fixture');
    // Declared before `try` so `finally` can clean it up even on an early throw.
    let tagRoot = '';
    try {
      // --- the sidebar half: add, click through, remove — all through the UI.
      // `openWork` left the Explorer on WORK, so the current folder is a real
      // tag target for the form (a submit at `/` is refused by design).
      const sidebar = win(page).locator('[data-testid="explorer-sidebar"]');
      await sidebar.locator('#explorer-tag-input').fill('Family');
      await sidebar.getByRole('button', { name: /add tag|ajouter une étiquette/i }).click();
      await expect(sidebar).toContainText('Family', { timeout: 10_000 });
      // Polled, not read once: the row is optimistically rendered, the server
      // row lands a moment later.
      await expect
        .poll(
          async () => (await fileTags(page, adminTokens)).map((tag) => `${tag.path}\u0000${tag.tag}`),
          { timeout: 10_000 },
        )
        .toContain(`${WORK}\u0000Family`);
      // A tag left on a deleted file is exactly what the sidebar × must clear,
      // so removal never requires the path to still exist.
      const goneTagPath = `${WORK}/${uniq('gone_')}.txt`;
      expect((await apiPost(page, adminTokens, '/api/explorer/tags', { path: goneTagPath, tag: 'Stale' })).success).toBe(false);
      await apiUpload(page, adminTokens, WORK, goneTagPath.split('/').pop()!, 'transient');
      expect((await apiPost(page, adminTokens, '/api/explorer/tags', { path: goneTagPath, tag: 'Stale' })).success).toBe(true);
      await apiPost(page, adminTokens, `/api/explorer/delete?path=${q(goneTagPath)}`);
      expect((await apiDelete(page, adminTokens, '/api/explorer/tags', { path: goneTagPath, tag: 'Stale' })).success).toBe(true);

      // The account's tags are listed by default; "Only this folder" narrows
      // them to the folder being browsed (and its own entries).
      const tagRow = sidebar.locator('.disk-btn').filter({ hasText: 'Family' }).first();
      await expect(tagRow).toBeVisible();
      const onlyThisFolder = sidebar.getByRole('checkbox', { name: /only this folder|ce dossier uniquement/i });
      await onlyThisFolder.check();
      await expect(tagRow).toBeVisible();
      await onlyThisFolder.uncheck();
      // The × is a sibling of the row button (WindowSidebarItem never nests
      // buttons), so it is addressed by its own hook.
      await sidebar.locator('[data-testid="explorer-tag-remove"]').first().click();
      // Scoped to this suite's own row: an earlier failed run (or a filtered
      // `-g` run that skipped the cleanup) may have left unrelated tags.
      await expect
        .poll(
          async () => (await fileTags(page, adminTokens)).filter((tag) => tag.path === WORK).length,
          { timeout: 10_000 },
        )
        .toBe(0);

      // --- the searchable half, inside a real sidebar root ------------------
      const rootsRes = await apiGet(page, adminTokens, '/api/explorer/roots');
      const sidebarRoots: string[] = (Array.isArray(rootsRes.data) ? rootsRes.data : [])
        .map((root: any) => String(root.path || ''))
        .filter(Boolean);
      expect(sidebarRoots.length).toBeGreaterThan(0);
      // The first root is not necessarily writable by the service account
      // (`/home` and `/media` are root-owned on a stock host), so the fixture
      // goes into the first one that accepts a directory — and if none does,
      // `HV_E2E_TAG_ROOT` is the operator's way to point it somewhere usable.
      if (process.env.HV_E2E_TAG_ROOT) {
        tagRoot = process.env.HV_E2E_TAG_ROOT;
        await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(tagRoot)}`);
      } else {
        for (const root of sidebarRoots) {
          const candidate = `${root}/${TAG_SEARCH_SUBDIR}`;
          const made = await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(candidate)}`);
          if (made.success) {
            tagRoot = candidate;
            break;
          }
        }
        // A scratch server whose data dir lives outside every sidebar root
        // has no writable root: that is an environment limitation, not a
        // product failure (set HV_E2E_TAG_ROOT to point it somewhere usable).
        if (!tagRoot) {
          test.skip(true, `no writable sidebar root among ${sidebarRoots.join(', ')}`);
        }
      }
      const searchName = `tagged-search-${Date.now().toString(36)}.txt`;
      const searchPath = `${tagRoot}/${searchName}`;
      expect((await apiUpload(page, adminTokens, tagRoot, searchName, 'tag search fixture')).success).toBe(true);
      expect((await apiPost(page, adminTokens, '/api/explorer/tags', { path: searchPath, tag: 'Family' })).success).toBe(true);
      const search = await apiGet(page, adminTokens, `/api/search/files?q=${searchName.slice(0, -4)}&tag=Family`);
      expect(search.data.tag).toBe('Family');
      expect(search.data.hits.some((hit: any) => hit.path === searchPath)).toBe(true);
      // Unfiltered, the same query still finds it: the tag narrows, never widens.
      const unfiltered = await apiGet(page, adminTokens, `/api/search/files?q=${searchName.slice(0, -4)}`);
      expect(unfiltered.data.hits.some((hit: any) => hit.path === searchPath)).toBe(true);
      // A tag the caller has on nothing answers no hits, and never an error.
      expect((await apiGet(page, adminTokens, `/api/search/files?q=${searchName.slice(0, -4)}&tag=NobodyUsesThis`)).data.hits.length).toBe(0);

      await page.keyboard.press('Control+k');
      const overlay = page.locator('.search-overlay');
      const filter = overlay.locator('select[aria-label]');
      await expect(filter).toBeVisible();
      await filter.selectOption('Family');
      await overlay.locator('.search-input').fill(searchName.slice(0, -4));
      await expect(overlay.locator('.search-item', { hasText: searchName })).toBeVisible({ timeout: 10_000 });
      await page.keyboard.press('Escape');

      // An admin resolves any existing path, so the refusal that matters is a
      // path that does not exist: a tag must never create a phantom entry.
      expect((await apiPost(page, adminTokens, '/api/explorer/tags', { path: `${WORK}/does-not-exist.txt`, tag: 'Secret' })).success).toBe(false);
      const outsiderTags = await apiGet(page, plainUser, '/api/explorer/tags');
      expect(outsiderTags.data.some((tag: any) => tag.path === file)).toBe(false);
      expect((await apiPost(page, plainUser, '/api/explorer/tags', { path: file, tag: 'Private' })).success).toBe(false);
      // A tag with no visibly invalid character is still refused when empty.
      expect((await apiPost(page, adminTokens, '/api/explorer/tags', { path: file, tag: '   ' })).success).toBe(false);
      await apiDelete(page, adminTokens, '/api/explorer/tags', { path: searchPath, tag: 'Family' }).catch(() => {});
    } finally {
      await apiDelete(page, adminTokens, '/api/explorer/tags', { path: file, tag: 'Family' }).catch(() => {});
      await apiPost(page, adminTokens, `/api/explorer/delete?path=${q(folder)}`).catch(() => {});
      if (tagRoot) await apiPost(page, adminTokens, `/api/explorer/delete?path=${q(tagRoot)}`).catch(() => {});
    }
  });

  test('15.7 — copy collision strategy: skip / keep_both / overwrite', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const dir = `${WORK}/${uniq('col_')}`;
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(dir)}`);
    await apiUpload(page, adminTokens, dir, 'a.txt', 'AAAA'); // 4 bytes
    await apiUpload(page, adminTokens, dir, 'b.txt', 'BB'); // 2 bytes

    const copy = (strategy?: string) =>
      apiPost(
        page,
        adminTokens,
        `/api/explorer/copy?source=${q(`${dir}/a.txt`)}&destination=${q(`${dir}/b.txt`)}` +
          (strategy ? `&strategy=${strategy}` : ''),
      );

    // skip — b.txt stays 2 bytes.
    expect((await copy('skip')).success).toBe(true);
    expect((await apiDownloadLen(page, adminTokens, `${dir}/b.txt`)).len).toBe(2);

    // keep_both — a new "b (1).txt" (4 bytes) appears, b.txt still 2.
    expect((await copy('keep_both')).success).toBe(true);
    const list = await apiGet(page, adminTokens, `/api/explorer/list?path=${q(dir)}`);
    const names = (list.data?.entries || []).map((e: any) => e.name);
    expect(names).toContain('b (1).txt');
    expect((await apiDownloadLen(page, adminTokens, `${dir}/b (1).txt`)).len).toBe(4);
    expect((await apiDownloadLen(page, adminTokens, `${dir}/b.txt`)).len).toBe(2);

    // overwrite (the default) — b.txt becomes 4 bytes.
    expect((await copy()).success).toBe(true);
    expect((await apiDownloadLen(page, adminTokens, `${dir}/b.txt`)).len).toBe(4);
  });

  test('15.8 — move collision strategy: keep_both renames instead of clobbering', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const dir = `${WORK}/${uniq('colm_')}`;
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(dir)}`);
    await apiUpload(page, adminTokens, dir, 'src.txt', 'SOURCE-8B'); // 9 bytes
    await apiUpload(page, adminTokens, dir, 'dst.txt', 'DD'); // 2 bytes

    const r = await apiPost(
      page,
      adminTokens,
      `/api/explorer/move?source=${q(`${dir}/src.txt`)}&destination=${q(`${dir}/dst.txt`)}&strategy=keep_both`,
    );
    expect(r.success).toBe(true);

    const list = await apiGet(page, adminTokens, `/api/explorer/list?path=${q(dir)}`);
    const names = (list.data?.entries || []).map((e: any) => e.name).sort();
    // src.txt is gone (moved), dst.txt untouched, dst (1).txt is the moved file.
    expect(names).toEqual(['dst (1).txt', 'dst.txt']);
    expect((await apiDownloadLen(page, adminTokens, `${dir}/dst.txt`)).len).toBe(2);
    expect((await apiDownloadLen(page, adminTokens, `${dir}/dst (1).txt`)).len).toBe(9);
  });

  test('15.7 — snapshots: the History menu shows "no snapshots" on a non-ZFS folder', async ({ page }) => {
    await openWork(page);

    await win(page).locator('[data-testid="explorer-history-btn"]').click();
    await expect(
      win(page).locator('[data-testid="explorer-history-menu"] [data-testid="explorer-history-empty"]'),
    ).toBeVisible({ timeout: 10_000 });

    // The endpoint itself answers cleanly with an empty list (WORK is /tmp).
    const snap = await apiGet(page, adminTokens, `/api/explorer/snapshots?path=${q(WORK)}`);
    expect(snap.success).toBe(true);
    expect(Array.isArray(snap.data)).toBe(true);
    expect((snap.data as any[]).length).toBe(0);
  });

  test('15.8 — eject is admin-only and rejects a path that is not a removable mount', async ({ page }) => {
    await gotoExplorer(page, adminTokens);

    // A non-admin session is refused outright.
    const denied = await apiPost(page, plainUser, `/api/explorer/eject?path=${q('/tmp')}`);
    expect(denied.http).toBe(403);

    // An admin hitting a plain directory (not a removable block device's
    // mount point) gets a clean error, never a 500.
    const bad = await apiPost(page, adminTokens, `/api/explorer/eject?path=${q(WORK)}`);
    expect(bad.http).not.toBe(500);
    expect(bad.success).toBe(false);
  });

  test('15.9 — "Share this folder" opens the Shares app with the create form pre-filled', async ({ page }) => {
    // A folder name that survives the sharename sanitiser unchanged.
    const dirName = `sharehost${Date.now().toString(36)}`;
    await openWork(page);
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(`${WORK}/${dirName}`)}`);
    // (No manual Refresh: the list re-lists via the live watcher.)

    const row = win(page)
      .locator('[data-testid="explorer-file-row"]:not([data-testid="explorer-file-header"])')
      .filter({ has: page.locator('span.truncate', { hasText: new RegExp(`^${escapeRe(dirName)}$`) }) })
      .first();
    await expect(row).toBeVisible({ timeout: 10_000 });
    await row.locator('[data-testid="explorer-row-share"]').click();

    // The Shares window opens and its create-share modal is pre-filled.
    await expect(page.locator('[data-testid="shares-page"]')).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('[data-testid="shares-name-input"]')).toHaveValue(dirName);
  });
});

// ===========================================================================
// 16. Share by link & unsaved-editor close guard
// ===========================================================================

test.describe('16 — Share by link & editor close guard', () => {
  /** Row scoped to one file, matched by name via the current data-testid DOM. */
  function fileRow(page: Page, fname: string) {
    return win(page)
      .locator('[data-testid="explorer-file-row"]:not([data-testid="explorer-file-header"])')
      .filter({ has: page.locator('span.truncate', { hasText: new RegExp(`^${escapeRe(fname)}$`) }) })
      .first();
  }

  test('16.1 — share by link: create from a file row, public GET serves it, revoke cuts it', async ({ page }) => {
    await openWork(page);
    const fname = `${uniq('share_')}.txt`;
    const body = 'SHARED-PAYLOAD-7';
    expect((await apiUpload(page, adminTokens, WORK, fname, body)).success).toBe(true);
    // (No manual Refresh: the list re-lists via the live watcher.)

    const row = fileRow(page, fname);
    await expect(row).toBeVisible({ timeout: 10_000 });

    // Files get a "Share by link" action (folders keep the Shares-app hand-off).
    await row.locator('[data-testid="explorer-row-share-link"]').click();

    const modal = page.locator('[data-slot="dialog-content"]');
    await expect(modal).toBeVisible({ timeout: 10_000 });

    // Default form: anyone + 30-day expiry → Create.
    await modal.getByRole('button', { name: /create link|créer le lien/i }).click();
    await expect(modal.locator('[data-testid="share-link-active"]')).toBeVisible({ timeout: 10_000 });
    await expect(modal.locator('[data-testid="share-link-qr"]')).toBeVisible();

    // The absolute share URL is displayed; a no-session GET serves the file.
    const urlText = (await modal.locator('.select-all').first().textContent()) || '';
    const m = urlText.match(/(\/share\/[A-Za-z0-9]+)/);
    expect(m).not.toBeNull();
    const sharePath = m![1];
    const anon = await page.request.get(`${BASE}${sharePath}`);
    expect(anon.status()).toBe(200);
    expect(await anon.text()).toBe(body);

    // Revoke it from the modal → the link stops working.
    await modal.getByRole('button', { name: /^revoke$/i }).first().click();
    await expect(modal.locator('[data-testid="share-link-active"]')).not.toBeVisible({ timeout: 10_000 });
    const gone = await page.request.get(`${BASE}${sharePath}`);
    expect(gone.status()).toBe(400);
  });

  test('16.2 — signed-in share link refuses an anonymous download', async ({ page }) => {
    await openWork(page);
    const fname = `${uniq('priv_')}.txt`;
    expect((await apiUpload(page, adminTokens, WORK, fname, 'private')).success).toBe(true);
    // (No manual Refresh: the list re-lists via the live watcher.)

    const row = fileRow(page, fname);
    await row.locator('[data-testid="explorer-row-share-link"]').click();
    const modal = page.locator('[data-slot="dialog-content"]');
    await expect(modal).toBeVisible({ timeout: 10_000 });

    await modal.locator('select#share-mode').selectOption('signed_in');
    await modal.getByRole('button', { name: /create link|créer le lien/i }).click();
    await expect(modal.locator('[data-testid="share-link-active"]')).toBeVisible({ timeout: 10_000 });

    const urlText = (await modal.locator('.select-all').first().textContent()) || '';
    const m = urlText.match(/(\/share\/[A-Za-z0-9]+)/);
    expect(m).not.toBeNull();
    // Anonymous → 401; with the session header → 200.
    const anon = await page.request.get(`${BASE}${m![1]}`);
    expect(anon.status()).toBe(401);
    const authed = await page.request.get(`${BASE}${m![1]}`, {
      headers: { 'x-session-token': adminTokens.token },
    });
    expect(authed.status()).toBe(200);
  });

  test('16.3 — unsaved editor edits are never dropped silently', async ({ page }) => {
    await openWork(page);
    const fname = `${uniq('guard_')}.txt`;
    expect((await apiUpload(page, adminTokens, WORK, fname, 'original body')).success).toBe(true);
    // (No manual Refresh: the list re-lists via the live watcher.)

    const row = fileRow(page, fname);
    await row.locator('[data-testid="explorer-row-preview"]').dispatchEvent('click');

    const modal = page.locator('[data-slot="dialog-content"]');
    await expect(modal).toBeVisible({ timeout: 10_000 });
    await modal.getByRole('button', { name: /^edit$|^modifier$/i }).click();
    const editor = modal.locator('[data-testid="explorer-preview-editor"]');
    await expect(editor).toBeVisible({ timeout: 10_000 });

    // Type something, then close the preview — the guard must intercept.
    await editor.fill('original body + unsaved typing');
    await modal.getByRole('button', { name: /^close$|^fermer$/i }).first().click();
    const guard = page.locator('[data-slot="dialog-content"]', { hasText: /unsaved changes|modifications non enregistrées/i });
    await expect(guard).toBeVisible({ timeout: 10_000 });

    // Discard → preview closes, nothing else lingers.
    await guard.getByRole('button', { name: /discard edits|ignorer les modifications/i }).click();
    await expect(guard).not.toBeVisible({ timeout: 10_000 });
    await expect(page.locator('[data-slot="dialog-content"]')).not.toBeVisible({ timeout: 10_000 });
  });

  test('16.4 — a password one-time link prompts on the public page, downloads once, then dies', async ({ page, browser }) => {
    await openWork(page);
    const fname = `${uniq('onetime_')}.txt`;
    const body = 'ONE-TIME-PAYLOAD';
    expect((await apiUpload(page, adminTokens, WORK, fname, body)).success).toBe(true);

    const row = fileRow(page, fname);
    await expect(row).toBeVisible({ timeout: 10_000 });
    await row.locator('[data-testid="explorer-row-share-link"]').click();
    const modal = page.locator('[data-slot="dialog-content"]');
    await expect(modal).toBeVisible({ timeout: 10_000 });

    // The form asks for exactly what this link needs: a password, one download.
    await modal.locator('#share-password').fill('neighbour-pass');
    await modal.locator('#share-limit').selectOption('one');
    await modal.getByRole('button', { name: /create link|créer le lien/i }).click();
    await expect(modal.locator('[data-testid="share-link-active"]')).toBeVisible({ timeout: 10_000 });

    const urlText = (await modal.locator('.select-all').first().textContent()) || '';
    const m = urlText.match(/(\/share\/[A-Za-z0-9]+)/);
    expect(m).not.toBeNull();
    const sharePath = m![1];

    // A neighbour with no HomeVault account: a separate context, so no session,
    // no CSRF token and no cookies from the admin's browser.
    const visitor = await browser.newContext();
    try {
      const vpage = await visitor.newPage();
      await vpage.goto(`${BASE}${sharePath}`);
      const passwordField = vpage.locator('#public-share-password');
      await expect(passwordField).toBeVisible({ timeout: 10_000 });

      // A wrong password is refused with a visible alert, never a URL hint.
      await passwordField.fill('not-the-password');
      await vpage.getByRole('button', { name: /download file|télécharger le fichier/i }).click();
      await expect(vpage.getByRole('alert')).toContainText(
        /incorrect|indisponible|faux|nicht verfügbar|incorrecta/i,
      );
      expect(vpage.url()).not.toContain('not-the-password');

      // The right one is exchanged for a cookie and the browser downloads.
      await passwordField.fill('neighbour-pass');
      const [download] = await Promise.all([
        vpage.waitForEvent('download', { timeout: 20_000 }),
        vpage.getByRole('button', { name: /download file|télécharger le fichier/i }).click(),
      ]);
      expect(download.suggestedFilename()).toBe(fname);

      // The one-time link is dead after the completed download. `HEAD` spends
      // no slot, so polling it can never race the counter.
      await expect
        .poll(async () => (await vpage.request.head(`${BASE}${sharePath}`)).status(), {
          timeout: 20_000,
        })
        .toBe(400);

      // …and a fresh visit says so instead of asking for the password again.
      // (A new page in the same context, not `reload()`: after an attachment
      // response Firefox reloads that URL as a download.)
      const after = await visitor.newPage();
      await after.goto(`${BASE}${sharePath}`);
      await expect(after.getByRole('alert')).toBeVisible({ timeout: 10_000 });
      await expect(after.locator('#public-share-password')).toHaveCount(0);
    } finally {
      await visitor.close();
    }

    await modal.getByRole('button', { name: /^revoke$/i }).first().click();
  });

  test('16.5 — an open link downloads straight away, with no password prompt', async ({ page, browser }) => {
    await openWork(page);
    const fname = `${uniq('open_')}.txt`;
    expect((await apiUpload(page, adminTokens, WORK, fname, 'OPEN-PAYLOAD')).success).toBe(true);
    const file = `${WORK}/${fname}`;

    const created = await page.evaluate(
      async ({ token, csrf, file }) => {
        const r = await fetch('/api/explorer/shares', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ path: file }),
        });
        const b = await r.json().catch(() => ({}));
        return { status: r.status, data: b?.data };
      },
      { token: adminTokens.token, csrf: adminTokens.csrf, file },
    );
    expect(created.status).toBe(200);
    const sharePath = (created.data as { share_path: string }).share_path;

    const visitor = await browser.newContext();
    try {
      const vpage = await visitor.newPage();
      // A link that needs nothing must not show a page at all: a plain
      // navigation is the download (that is the whole point of the feature).
      const [download] = await Promise.all([
        vpage.waitForEvent('download', { timeout: 20_000 }),
        vpage.goto(`${BASE}${sharePath}`).catch(() => null),
      ]);
      expect(download.suggestedFilename()).toBe(fname);
      await expect(vpage.locator('#public-share-password')).toHaveCount(0);
    } finally {
      await visitor.close();
      await sharedCtx
        .delete(`/api/explorer/shares/${encodeURIComponent(sharePath.split('/').pop() || '')}`, {
          headers: adminHeaders,
        })
        .catch(() => {});
    }
  });
});

// ===========================================================================
// 17. Instant copy (reflink) — QW-03
// ===========================================================================

test.describe('17 — Instant copy (reflink)', () => {
  test('17.1 — a copy reports the strategy it used, and the bytes are exact', async ({ page }) => {
    await openWork(page);
    const dir = `${WORK}/${uniq('rl_')}`;
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(dir)}`);
    const body = 'REF-LINK-PROBE-' + 'x'.repeat(200);
    expect((await apiUpload(page, adminTokens, dir, 'src.txt', body)).success).toBe(true);

    const r = await apiPost(
      page,
      adminTokens,
      `/api/explorer/copy?source=${q(`${dir}/src.txt`)}&destination=${q(`${dir}/dst.txt`)}`,
    );
    expect(r.success).toBe(true);
    // The reply names the strategy, and `instant` must agree with it. Whether
    // a clone is possible depends on the host (`/tmp` is ext4 here), so the
    // expectation is derived from the mounts the server reports instead of
    // being hardcoded — on a Btrfs box this test still has to be true.
    const method = r.data?.method;
    expect(['clone', 'kernel_clone', 'kernel_copy', 'stream']).toContain(method);
    expect(r.data?.instant).toBe(method === 'clone' || method === 'kernel_clone');
    const settings = await apiGet(page, adminTokens, '/api/explorer/instant-copy');
    const mount = (settings.data?.mounts ?? [])
      .filter(
        (m: { mount_point: string }) =>
          dir === m.mount_point || dir.startsWith(m.mount_point.replace(/\/$/, '') + '/'),
      )
      .sort((a: { mount_point: string }, b: { mount_point: string }) => b.mount_point.length - a.mount_point.length)[0];
    if (mount) {
      // A clone on a filesystem the server itself reports as unable to share
      // extents would be a lie; the converse is what the feature is for.
      expect(r.data?.instant).toBe(mount.reflink);
    }
    // The copy is byte-identical either way.
    expect((await apiDownloadLen(page, adminTokens, `${dir}/dst.txt`)).len).toBe(body.length);
  });

  test('17.2 — mode=off still copies correctly and never claims a clone', async ({ page }) => {
    await openWork(page);
    const original = await apiGet(page, adminTokens, '/api/explorer/instant-copy');
    const restore = original.data?.mode === 'off' ? 'off' : 'auto';
    try {
      expect((await apiPut(page, adminTokens, '/api/explorer/instant-copy', { mode: 'off' })).success).toBe(true);

      const dir = `${WORK}/${uniq('rloff_')}`;
      await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(dir)}`);
      expect((await apiUpload(page, adminTokens, dir, 'a.txt', 'STREAM-PROBE')).success).toBe(true);

      const r = await apiPost(
        page,
        adminTokens,
        `/api/explorer/copy?source=${q(`${dir}/a.txt`)}&destination=${q(`${dir}/b.txt`)}`,
      );
      expect(r.success).toBe(true);
      expect(r.data?.method).toBe('stream');
      expect(r.data?.instant).toBe(false);
      expect((await apiDownloadLen(page, adminTokens, `${dir}/b.txt`)).len).toBe('STREAM-PROBE'.length);
    } finally {
      await apiPut(page, adminTokens, '/api/explorer/instant-copy', { mode: restore });
    }
  });

  test('17.3 — the admin panel reads and writes the policy, and shows the mounts', async ({ page }) => {
    await gotoExplorer(page, adminTokens);
    const original = await apiGet(page, adminTokens, '/api/explorer/instant-copy');
    const restore = original.data?.mode === 'off' ? 'off' : 'auto';
    try {
      // Open the panel from the toolbar (admin only).
      await win(page).locator('[data-testid="explorer-instant-copy-btn"]').click({ force: true });
      const modal = page.locator('[data-slot="dialog-content"]');
      await expect(modal).toBeVisible({ timeout: 10_000 });

      // Both options are offered, and the evidence list is rendered.
      await expect(modal.locator('[data-testid="explorer-instant-copy-auto"]')).toBeVisible();
      await expect(modal.locator('[data-testid="explorer-instant-copy-off"]')).toBeVisible();
      await expect(modal.locator('[data-testid="explorer-instant-copy-mounts"]')).toBeVisible();

      // Choosing "always make a full copy" persists it on the server.
      await modal.locator('[data-testid="explorer-instant-copy-off"]').check({ force: true });
      await expect
        .poll(async () => (await apiGet(page, adminTokens, '/api/explorer/instant-copy')).data?.mode, {
          timeout: 10_000,
        })
        .toBe('off');

      // And switching back to auto persists that too.
      await modal.locator('[data-testid="explorer-instant-copy-auto"]').check({ force: true });
      await expect
        .poll(async () => (await apiGet(page, adminTokens, '/api/explorer/instant-copy')).data?.mode, {
          timeout: 10_000,
        })
        .toBe('auto');
    } finally {
      await apiPut(page, adminTokens, '/api/explorer/instant-copy', { mode: restore });
    }
  });

  test('17.4 — the policy refuses a non-admin writer and an unknown mode', async ({ page }) => {
    // A regular user may read it… (openWork logs the admin in; the plain user
    // is exercised through its own context below.)
    await gotoExplorer(page, adminTokens);

    // …but an unknown mode is refused rather than silently coerced.
    const bad = await apiPut(page, adminTokens, '/api/explorer/instant-copy', { mode: 'instant-please' });
    expect(bad.http).not.toBe(500);
    expect(bad.success).toBe(false);

    // The stored policy was not changed by the rejected write.
    const after = await apiGet(page, adminTokens, '/api/explorer/instant-copy');
    expect(['auto', 'off']).toContain(after.data?.mode);

    // A non-admin session cannot write it.
    const denied = await apiPut(page, plainUser, '/api/explorer/instant-copy', { mode: 'off' });
    expect(denied.http).toBe(403);
  });

  test('17.5 — a same-filesystem move reports no copy method at all', async ({ page }) => {
    await openWork(page);
    const dir = `${WORK}/${uniq('rlmv_')}`;
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(dir)}`);
    expect((await apiUpload(page, adminTokens, dir, 'm1.txt', 'MOVE-ME')).success).toBe(true);

    const r = await apiPost(
      page,
      adminTokens,
      `/api/explorer/move?source=${q(`${dir}/m1.txt`)}&destination=${q(`${dir}/m2.txt`)}`,
    );
    expect(r.success).toBe(true);
    // An atomic rename copied nothing: reporting a method would be invented.
    expect(r.data?.method ?? null).toBeNull();
    expect((await apiDownloadLen(page, adminTokens, `${dir}/m2.txt`)).len).toBe('MOVE-ME'.length);
  });
});

// ===========================================================================
// 18. Transfer tray, transfer history and on-demand checksums
//    (HV-UX-01 / HV-UX-02 / HV-ADV-02 / HV-INT-01)
// ===========================================================================

test.describe('18 — Transfers', () => {
  test('18.1 — the tray opens from the toolbar and shows the three zones', async ({ page }) => {
    await openWork(page);

    await win(page).locator('[data-testid="explorer-transfers-btn"]').click();
    const modal = page.locator('[data-slot="dialog-content"]');
    await expect(modal).toBeVisible({ timeout: 10_000 });
    await expect(modal.locator('[data-testid="transfer-tray"]')).toBeVisible();

    // Three zones, each an accessible toggle button with a badge.
    for (const zone of ['upload', 'download', 'copy']) {
      const tab = modal.locator(`[data-testid="transfer-tab-${zone}"]`);
      await expect(tab).toBeVisible();
      await expect(tab).toHaveAttribute('aria-pressed', zone === 'upload' ? 'true' : 'false');
      await expect(modal.locator(`[data-testid="transfer-count-${zone}"]`)).toBeVisible();
    }
    // The panel always says something: either rows, or the empty state — never
    // a blank box. (A rerun can legitimately still hold rows from an earlier
    // crashed run, whose sessions live for 30 minutes, so the assertion is
    // "one of the two", not "empty".)
    if ((await modal.locator('[data-testid="transfer-row"]').count()) === 0) {
      await expect(modal.locator('[data-testid="transfer-empty"]')).toBeVisible();
    }

    // Switching zones keeps the panel usable and updates the pressed state.
    await modal.locator('[data-testid="transfer-tab-copy"]').click();
    await expect(modal.locator('[data-testid="transfer-tab-copy"]')).toHaveAttribute('aria-pressed', 'true');
    await modal.locator('[data-testid="transfer-close"]').click();
    await expect(modal).toBeHidden();
  });

  test('18.2 — an open upload is listed with its bytes and can be cancelled from the tray', async ({ page }) => {
    await openWork(page);
    const dir = `${WORK}/${uniq('tray_')}`;
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(dir)}`);

    // Open a chunked session and send the first chunk, then leave it open —
    // this is what a browser does between two chunks of a large file.
    const start = await apiPost(
      page,
      adminTokens,
      `/api/explorer/upload/session?path=${q(dir)}&filename=partial.bin&size=2048`,
    );
    expect(start.success).toBe(true);
    const sessionId = start.data?.session_id;
    expect(typeof sessionId).toBe('string');
    const chunk = await page.evaluate(
      async ({ token, csrf, sessionId }) => {
        const r = await fetch(`/api/explorer/upload/chunk?session_id=${sessionId}&offset=0`, {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf },
          body: new Uint8Array(1024),
        });
        return r.json();
      },
      { token: adminTokens.token, csrf: adminTokens.csrf, sessionId },
    );
    expect(chunk?.success).toBe(true);
    expect(chunk?.data?.offset).toBe(1024);

    await win(page).locator('[data-testid="explorer-transfers-btn"]').click();
    const modal = page.locator('[data-slot="dialog-content"]');
    await expect(modal).toBeVisible({ timeout: 10_000 });

    // The upload zone lists it with the byte detail the enriched progress adds.
    const row = modal.locator('[data-testid="transfer-row"]').filter({ hasText: 'partial.bin' });
    await expect(row).toHaveCount(1);
    await expect(row.locator('[data-testid="transfer-detail"]')).toContainText('1 KB / 2 KB');
    await expect(modal.locator('[data-testid="transfer-count-upload"]')).not.toHaveText('0');

    // Cancelling removes it — and the server agrees, not just the UI.
    await row.locator('[data-testid="transfer-cancel"]').click();
    await expect(row).toHaveCount(0, { timeout: 10_000 });
    const after = await apiGet(page, adminTokens, '/api/explorer/transfers/active');
    expect(after.success).toBe(true);
    expect(
      (after.data?.uploads ?? []).filter((u: { session_id: string }) => u.session_id === sessionId),
    ).toHaveLength(0);
    // No partial file was left behind under its final name.
    const listing = await apiGet(page, adminTokens, `/api/explorer/list?path=${q(dir)}`);
    expect((listing.data?.entries ?? []).map((e: { name: string }) => e.name)).not.toContain('partial.bin');
  });

  test('18.3 — a finished copy is recorded in the transfer history', async ({ page }) => {
    await openWork(page);
    const dir = `${WORK}/${uniq('hist_')}`;
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(dir)}`);
    const payload = 'HISTORY-PROBE-' + 'y'.repeat(64);
    expect((await apiUpload(page, adminTokens, dir, 'h1.txt', payload)).success).toBe(true);
    const copied = await apiPost(
      page,
      adminTokens,
      `/api/explorer/copy?source=${q(`${dir}/h1.txt`)}&destination=${q(`${dir}/h2.txt`)}`,
    );
    expect(copied.success).toBe(true);

    // The API row carries the byte count, the method and the status.
    const history = await apiGet(page, adminTokens, '/api/explorer/transfers/history?operation=copy&limit=5');
    expect(history.success).toBe(true);
    const entry = (history.data?.entries ?? []).find(
      (e: { dest_path: string | null }) => e.dest_path === `${dir}/h2.txt`,
    );
    expect(entry).toBeTruthy();
    expect(entry.status).toBe('complete');
    expect(entry.bytes_total).toBe(payload.length);
    expect(entry.method).toBeTruthy();

    // And the tray renders it (the audit trail is visible, not API-only).
    await win(page).locator('[data-testid="explorer-transfers-btn"]').click();
    const modal = page.locator('[data-slot="dialog-content"]');
    await modal.locator('[data-testid="transfer-history"] summary').click();
    await expect(modal.locator('[data-testid="transfer-history-row"]').first()).toBeVisible({
      timeout: 10_000,
    });
    await expect(modal.locator('[data-testid="transfer-history-row"]').first()).toContainText('h2.txt');
  });

  test('18.4 — on-demand checksum returns the real digest and compares two files', async ({ page }) => {
    await openWork(page);
    const dir = `${WORK}/${uniq('sum_')}`;
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(dir)}`);
    expect((await apiUpload(page, adminTokens, dir, 'abc.txt', 'abc')).success).toBe(true);
    expect((await apiUpload(page, adminTokens, dir, 'same.txt', 'abc')).success).toBe(true);
    expect((await apiUpload(page, adminTokens, dir, 'other.txt', 'abd')).success).toBe(true);

    // `abc` is the classic vector: SHA-256("abc").
    const digest = await apiPost(page, adminTokens, '/api/explorer/checksum', {
      path: `${dir}/abc.txt`,
    });
    expect(digest.success).toBe(true);
    expect(digest.data?.algorithm).toBe('sha256');
    expect(digest.data?.checksum).toBe(
      'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
    );

    const same = await apiPost(page, adminTokens, '/api/explorer/checksum', {
      path: `${dir}/abc.txt`,
      compare_with: `${dir}/same.txt`,
    });
    expect(same.data?.match).toBe(true);

    const different = await apiPost(page, adminTokens, '/api/explorer/checksum', {
      path: `${dir}/abc.txt`,
      algorithm: 'blake3',
      compare_with: `${dir}/other.txt`,
    });
    expect(different.success).toBe(true);
    expect(different.data?.algorithm).toBe('blake3');
    expect(different.data?.match).toBe(false);

    // MD5 is what DSM offers and what this refuses; the refusal must not be a
    // silent SHA-256.
    const refused = await apiPost(page, adminTokens, '/api/explorer/checksum', {
      path: `${dir}/abc.txt`,
      algorithm: 'md5',
    });
    expect(refused.success).toBe(false);
    expect(refused.http).toBeGreaterThanOrEqual(400);
    expect(refused.http).toBeLessThan(500);
  });

  test('18.5 — another user cannot read the digest of a private file', async ({ page }) => {
    await openWork(page);
    const dir = `${WORK}/${uniq('priv_')}`;
    await apiPost(page, adminTokens, `/api/explorer/mkdir?path=${q(dir)}`);
    expect((await apiUpload(page, adminTokens, dir, 'secret.txt', 'do-not-leak')).success).toBe(true);

    // The throwaway member cannot reach the admin's work directory at all.
    const denied = await apiPost(page, plainUser, '/api/explorer/checksum', {
      path: `${dir}/secret.txt`,
    });
    expect(denied.success).toBe(false);
    expect(denied.http).toBeGreaterThanOrEqual(400);
    expect(denied.http).toBeLessThan(500);

    // An anonymous call is unauthorized, never a 200.
    const anonymous = await page.evaluate(async ({ dir }) => {
      const r = await fetch('/api/explorer/checksum', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ path: `${dir}/secret.txt` }),
      });
      return r.status;
    }, { dir });
    expect(anonymous).toBe(401);
  });

  test('18.6 — the tray is keyboard reachable and closes on Escape', async ({ page }) => {
    await openWork(page);

    const button = win(page).locator('[data-testid="explorer-transfers-btn"]');
    await expect(button).toHaveAttribute('aria-label', /transfers|transferts|übertragungen|transferencias/i);
    await button.focus();
    await expect(button).toBeFocused();
    await page.keyboard.press('Enter');

    const modal = page.locator('[data-slot="dialog-content"]');
    await expect(modal).toBeVisible({ timeout: 10_000 });
    // The zone toggles are buttons: Tab reaches them, Space activates them.
    const copyTab = modal.locator('[data-testid="transfer-tab-copy"]');
    await copyTab.focus();
    await expect(copyTab).toBeFocused();
    await page.keyboard.press(' ');
    await expect(copyTab).toHaveAttribute('aria-pressed', 'true');

    await page.keyboard.press('Escape');
    await expect(modal).toBeHidden();
    // Focus goes back to the control that opened it.
    await expect(button).toBeFocused();
  });

  test('18.7 — the tray stays usable at 360, 768 and 1920 px', async ({ page }) => {
    await openWork(page);
    for (const width of [360, 768, 1920]) {
      await page.setViewportSize({ width, height: 900 });
      await win(page).locator('[data-testid="explorer-transfers-btn"]').click();
      const modal = page.locator('[data-slot="dialog-content"]');
      await expect(modal).toBeVisible({ timeout: 10_000 });
      const box = await modal.boundingBox();
      expect(box).not.toBeNull();
      // No horizontal overflow: the dialog never exceeds the viewport.
      expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(width + 1);
      await page.keyboard.press('Escape');
      await expect(modal).toBeHidden();
    }
  });
});
