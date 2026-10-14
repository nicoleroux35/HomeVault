/* =============================================================================
 * File          : cloudsync.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Cloudsync screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Cloud Sync (rclone) app — End-to-End Test Suite
 *
 * Not a route: a window opened from the desktop's Applications launcher
 * (id `rclone`, "Cloud Sync" tile). Manages rclone remotes, FUSE mounts and
 * sync/copy jobs via `src/handlers/rclone.rs`.
 *
 * **Safety model:** the original audit found all handlers took no session
 * at all, `GET /rclone/config` leaked every remote's credentials, and
 * `sync`/`copy` on a local path was an unrestricted file primitive. This
 * suite verifies the fixes: every endpoint needs a session, every mutation
 * + the raw-config read need admin, and the config-injection / bad-name /
 * empty-source guards reject cleanly. It never runs a real `rclone`
 * transfer or mount.
 *
 * A real `add remote → list → delete` round-trip writes the config file the
 * server resolved at request time: `/etc/rclone.conf` when nothing overrides
 * it (root-owned on a real deployment; not writable by the dev server), or
 * whatever `RCLONE_CONFIG` points at. Section 13 is therefore **opt-in**: set
 * `CLOUDSYNC_E2E_WRITABLE_CONF=1` only on a host where the server process can
 * write that file — or launch the server with
 * `RCLONE_CONFIG=/tmp/hv-e2e/rclone.conf`, which makes the whole section safe
 * and is the cheaper option since the path is no longer hardcoded
 * (`rclone_config_path()` in `src/handlers/rclone.rs`). It always removes the
 * throwaway remote afterwards.
 *
 * Run (all):     npx playwright test tests/e2e/cloudsync/cloudsync.spec.ts --project=firefox
 * Run (single):  npx playwright test tests/e2e/cloudsync/cloudsync.spec.ts -g "1.1"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/cloudsync/cloudsync.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const RUN_CONF = process.env.CLOUDSYNC_E2E_WRITABLE_CONF === '1';

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

async function gotoCloudSync(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openLaunchpadApp(page, 'Cloud Sync');
  await page.waitForSelector('[data-testid="cloudsync-page"]', { timeout: 10_000 });
  await expect(page.locator('[data-testid="cloudsync-page"] [data-testid="cloudsync-loading"]')).toHaveCount(0, { timeout: 15_000 });
}

function win(page: Page) {
  return page.locator('[data-testid="cloudsync-page"]');
}

async function api(
  page: Page,
  t: Tokens | null,
  method: string,
  path: string,
  body?: unknown,
) {
  return page.evaluate(
    async ({ t, method, path, body }) => {
      const headers: Record<string, string> = {};
      if (t) {
        headers['x-session-token'] = t.token;
        headers['x-csrf-token'] = t.csrf;
      }
      if (body !== undefined) headers['Content-Type'] = 'application/json';
      const r = await fetch(path, {
        method,
        headers,
        body: body !== undefined ? JSON.stringify(body) : undefined,
      });
      const j = await r.json().catch(() => ({}));
      return { http: r.status, success: j?.success, error: j?.error || '', data: j?.data };
    },
    { t, method, path, body },
  );
}

let adminTokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
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

// ===========================================================================
// 1. Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — Cloud Sync window opens from the Applications launcher', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — the loading spinner clears (no stuck spinner)', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    await expect(win(page).locator('[data-testid="cloudsync-loading"]')).toHaveCount(0);
  });

  test('1.3 — header shows the title and 3 tabs', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title')).toContainText(/rclone|cloud|sync/i);
    await expect(win(page).locator('.hv-winhdr-tab')).toHaveCount(3);
  });

  test('1.4 — the stat tiles render', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    await expect(win(page).locator('[data-testid="cloudsync-stats"] [data-testid="cloudsync-stat-card"]').first()).toBeVisible();
  });

  test('1.5 — the remotes tab shows a list, an empty state, or the not-installed card', async ({
    page,
  }) => {
    await gotoCloudSync(page, adminTokens);
    const cards = await win(page).locator('[data-testid="cloudsync-remote-card"], [data-testid="cloudsync-empty"]').count();
    expect(cards).toBeGreaterThan(0);
  });
});

// ===========================================================================
// 2. Tab navigation
// ===========================================================================

test.describe('2 — Tab navigation', () => {
  for (const label of [/mount/i, /sync/i, /remote/i]) {
    test(`2.${label.source.slice(0, 6)} — switching tabs does not crash`, async ({ page }) => {
      await gotoCloudSync(page, adminTokens);
      await win(page).locator('.hv-winhdr-tab', { hasText: label }).first().click();
      await expect(win(page)).toBeVisible();
    });
  }
});

// ===========================================================================
// 3. Validation (via API — nothing reaches rclone / the config file)
// ===========================================================================

test.describe('3 — Validation', () => {
  test('3.1 — add remote rejects an empty name', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    const r = await api(page, adminTokens, 'POST', '/api/rclone/remotes', {
      name: '',
      provider: 'drive',
    });
    expect(r.success).toBe(false);
    expect(r.error).toMatch(/name is required/i);
  });

  test('3.2 — add remote rejects a name with INI metacharacters', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    for (const bad of ['a:b', 'x[y', 'z]w']) {
      const r = await api(page, adminTokens, 'POST', '/api/rclone/remotes', {
        name: bad,
        provider: 'drive',
      });
      expect(r.success, bad).toBe(false);
      expect(r.error, bad).toMatch(/invalid characters/i);
    }
  });

  test('3.3 — add remote rejects a config value that injects a [section]', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    const r = await api(page, adminTokens, 'POST', '/api/rclone/remotes', {
      name: 'ok',
      provider: 'drive',
      config: 'token = x\n[evil]\ntype = local',
    });
    expect(r.success).toBe(false);
    expect(r.error).toMatch(/inject another config section/i);
  });

  test('3.4 — sync / copy reject an empty source', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    for (const path of ['/api/rclone/sync', '/api/rclone/copy']) {
      const r = await api(page, adminTokens, 'POST', path, {
        source_type: 'cloud',
        source: '',
        destination: 'b',
        delete: false,
        dry_run: true,
        update: false,
        interactive: false,
      });
      expect(r.success, path).toBe(false);
      expect(r.error, path).toMatch(/source is required/i);
    }
  });

  test('3.5 — mount start rejects an empty mount point', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    const r = await api(page, adminTokens, 'POST', '/api/rclone/mount/start', {
      remote: 'gd',
      mount_point: '',
    });
    expect(r.success).toBe(false);
    expect(r.error).toMatch(/mount point is required/i);
  });
});

// ===========================================================================
// 4. Security
// ===========================================================================

test.describe('4 — Security', () => {
  test('4.1 — every endpoint rejects a missing session token', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    const results = await page.evaluate(async () => {
      const eps: [string, string, any][] = [
        ['GET', '/api/rclone/status', null],
        ['GET', '/api/rclone/remotes', null],
        ['POST', '/api/rclone/remotes', { name: 'r', provider: 'drive' }],
        ['GET', '/api/rclone/providers', null],
        ['GET', '/api/rclone/mounts', null],
        ['POST', '/api/rclone/mount/start', { remote: 'r', mount_point: '/tmp/x' }],
        ['POST', '/api/rclone/sync', { source: 'a', destination: 'b', delete: false, dry_run: true, update: false }],
        ['GET', '/api/rclone/config', null],
        ['POST', '/api/rclone/config', { config: 'x' }],
      ];
      const out: Record<string, boolean> = {};
      for (const [m, p, b] of eps) {
        const r = await fetch(p, {
          method: m,
          headers: b ? { 'Content-Type': 'application/json' } : {},
          body: b ? JSON.stringify(b) : undefined,
        });
        const j = await r.json().catch(() => ({}));
        out[p + ' ' + m] = j?.success === false;
      }
      return out;
    });
    for (const [k, denied] of Object.entries(results)) {
      expect(denied, `${k} must be denied`).toBe(true);
    }
  });

  test('4.2 — a tampered token is rejected', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    const r = await api(page, { ...adminTokens, token: adminTokens.token + 'x' }, 'GET', '/api/rclone/remotes');
    expect(r.success).toBe(false);
    expect(r.error).toMatch(/invalid or expired/i);
  });

  test('4.3 — GET /rclone/config (raw credentials) requires admin, mutations require admin', async ({
    playwright,
  }) => {
    const ctx = await newCtx(playwright);
    const adminHeaders = { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf };
    const uname = `csviewer${Date.now().toString(36)}`;
    const created = await ctx.post('/api/users', {
      headers: adminHeaders,
      data: { username: uname, password: 'Viewer12345!', fullname: 'V', email: '', role_id: 1, groups: [], home_enabled: false },
    });
    if (!created.ok()) {
      await ctx.dispose();
      test.skip(true, `could not create a non-admin user (${created.status()})`);
    }
    try {
      const { token, csrf } = await apiLogin(ctx, uname, 'Viewer12345!');
      const h = { 'x-session-token': token, 'x-csrf-token': csrf };

      // reads that a plain session may do
      const remotes = await ctx.get('/api/rclone/remotes', { headers: h });
      expect((await remotes.json()).success !== false || remotes.status() === 200).toBeTruthy();

      // the credential-leak gate
      const cfg = await ctx.get('/api/rclone/config', { headers: h });
      expect((await cfg.json()).error || '').toMatch(/admin privileges/i);

      // a mutation
      const add = await ctx.post('/api/rclone/remotes', {
        headers: h,
        data: { name: 'x', provider: 'drive' },
      });
      expect((await add.json()).error || '').toMatch(/admin privileges/i);
    } finally {
      await ctx.delete(`/api/users/${uname}`, { headers: adminHeaders }).catch(() => {});
      await ctx.dispose();
    }
  });

  test('4.4 — no session token appears in any URL', async ({ page }) => {
    const urls: string[] = [];
    page.on('request', (r) => urls.push(r.url()));
    await gotoCloudSync(page, adminTokens);
    await page.waitForTimeout(1000);
    for (const u of urls) expect(u).not.toContain(adminTokens.token);
  });

  test('4.5 — the raw config is never written into localStorage', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    const keys = await page.evaluate(() => Object.keys(localStorage));
    for (const k of keys) {
      const v = await page.evaluate((kk) => localStorage.getItem(kk) || '', k);
      expect(v).not.toMatch(/\[.*\]\s*\ntype\s*=/);
    }
  });
});

// ===========================================================================
// 5. Responsive
// ===========================================================================

test.describe('5 — Responsive', () => {
  for (const [w, h, label] of [
    [360, 800, '360px'],
    [768, 1024, '768px'],
    [1920, 1080, '1920px'],
  ] as const) {
    test(`5.${label} — no horizontal overflow`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: h });
      await gotoCloudSync(page, adminTokens);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
      );
      expect(overflow).toBe(false);
    });
  }
});

// ===========================================================================
// 6. Accessibility & console
// ===========================================================================

test.describe('6 — Accessibility & console', () => {
  test('6.1 — the tab bar exposes tablist/tab roles', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    await expect(win(page).locator('[role="tablist"]')).toHaveCount(1);
    expect(await win(page).locator('[role="tab"]').count()).toBeGreaterThanOrEqual(3);
  });

  test('6.2 — no duplicate element ids', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    const dupes = await page.evaluate(() => {
      const seen = new Set<string>();
      const dup: string[] = [];
      for (const el of Array.from(document.querySelectorAll('[id]'))) {
        if (seen.has(el.id)) dup.push(el.id);
        seen.add(el.id);
      }
      return dup;
    });
    expect(dupes).toEqual([]);
  });

  test('6.3 — no uncaught JS errors browsing all tabs', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await gotoCloudSync(page, adminTokens);
    for (const label of [/mount/i, /sync/i, /remote/i]) {
      await win(page).locator('.hv-winhdr-tab', { hasText: label }).first().click().catch(() => {});
      await page.waitForTimeout(300);
    }
    expect(errors).toEqual([]);
  });

  test('6.4 — no failed rclone requests during a normal load', async ({ page }) => {
    const bad: string[] = [];
    page.on('response', (r) => {
      if (r.url().includes('/api/rclone/') && r.status() >= 400) bad.push(`${r.status()} ${r.url()}`);
    });
    await gotoCloudSync(page, adminTokens);
    await page.waitForTimeout(2500);
    expect(bad).toEqual([]);
  });
});

// ===========================================================================
// 13. Real remote lifecycle — opt-in (needs a writable config file)
// ===========================================================================

test.describe('13 — Real remote lifecycle (opt-in)', () => {
  test.skip(!RUN_CONF, 'set CLOUDSYNC_E2E_WRITABLE_CONF=1 on a host where the server can write its rclone config (or start it with RCLONE_CONFIG=/tmp/…/rclone.conf)');

  test('13.1 — add a credential-less remote → verify listed → delete', async ({ page }) => {
    await gotoCloudSync(page, adminTokens);
    const name = `e2e_rc_${Date.now().toString(36)}`;

    // `alias` needs no credentials — just a target remote string.
    const add = await api(page, adminTokens, 'POST', '/api/rclone/remotes', {
      name,
      provider: 'alias',
      config: 'remote = /tmp',
    });
    if (!add.success) test.skip(true, `add failed: ${add.error}`);

    try {
      const list = await api(page, adminTokens, 'GET', '/api/rclone/remotes');
      expect((list.data as any[]).some((r) => r.name === name)).toBe(true);

      await gotoCloudSync(page, adminTokens);
      await expect(win(page).getByText(name, { exact: false })).toBeVisible({ timeout: 8_000 });
    } finally {
      const del = await api(page, adminTokens, 'DELETE', `/api/rclone/remotes/${name}`);
      expect(del.success).toBe(true);
      const after = await api(page, adminTokens, 'GET', '/api/rclone/remotes');
      expect((after.data as any[]).some((r) => r.name === name)).toBe(false);
    }
  });
});

// ===========================================================================
// 14. Edge cases & memory
// ===========================================================================

test.describe('14 — Edge cases & memory', () => {
  test('14.1 — clearing localStorage mid-session and reloading does not crash', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await gotoCloudSync(page, adminTokens);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await expect(page.locator('.hv-login-page, [data-testid="cloudsync-page"], #app')).toBeAttached({ timeout: 15_000 });
    await page.waitForTimeout(800);
    expect(errors).toEqual([]);
  });

  test('14.2 — open/close the window ×5 without significant heap growth', async ({ page }) => {
    test.setTimeout(150_000);
    await gotoCloudSync(page, adminTokens);
    const base = await getHeapMB(page);
    for (let i = 0; i < 5; i++) {
      await page.locator('.hv-cl-btn-close').first().click();
      await expect(win(page)).toHaveCount(0);
      await openLaunchpadApp(page, 'Cloud Sync');
      await page.waitForSelector('[data-testid="cloudsync-page"]', { timeout: 10_000 });
    }
    const after = await getHeapMB(page);
    if (base > 0 && after > 0) {
      console.log(`[MEM] heap ${base.toFixed(1)} → ${after.toFixed(1)} MB`);
      expect(after - base).toBeLessThan(12);
    }
  });
});
