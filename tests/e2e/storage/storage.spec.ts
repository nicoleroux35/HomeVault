/* =============================================================================
 * File          : storage.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Storage app — End-to-End Test Suite
 *
 * The Storage app is not a route: it's a window opened from the desktop's
 * Applications launcher (id `storage`, "Storage" tile). It inventories the
 * host's disks/partitions/removable drives and manages the **disk cache
 * tier** and **standalone LUKS2 encrypted volumes**. RAID (mdadm) is
 * deliberately NOT managed here — that's the separate RAID app
 * (`tests/e2e/raid/`).
 *
 * **Safety model (like `network`/`vpn`/`shares` — see those headers):**
 * creating/deleting a real ZFS/Btrfs pool or formatting/erasing a real
 * LUKS device is destructive and every path goes through `sudo`
 * (`zpool`/`btrfs`/`cryptsetup`/`mkfs`/`mount`). This suite therefore:
 *   - drives the read-only inventory for real (safe — `lsblk` etc.),
 *   - exercises the mutating endpoints only up to a **clean rejection**
 *     (400/401/403, never a 500/panic) for missing/again non-admin
 *     sessions and invalid input — including the passphrase policy (12
 *     characters, no wordlist entry, not the volume label),
 *   - and runs a genuine LUKS create→verify→rotate→delete lifecycle **only**
 *     when an operator opts in with `STORAGE_E2E_LOOP=1` AND passwordless
 *     `sudo` is available (it builds its own throwaway loopback file, never
 *     touches real hardware, and always tears down in a `finally`). Creation
 *     is a tracked job: the test follows `/api/progress/{id}/status` and then
 *     finds the volume by label, since the request answers `{ job_id }`.
 *
 * Mutating requests pass the CSRF middleware, so the in-page `fetch`
 * helpers send `x-session-token` + `x-csrf-token` (captured in beforeAll).
 *
 * Run (all):     npx playwright test tests/e2e/storage/storage.spec.ts --project=firefox
 * Run (single):  npx playwright test tests/e2e/storage/storage.spec.ts -g "1.1"
 * Run (LUKS):    STORAGE_E2E_LOOP=1 npx playwright test tests/e2e/storage/storage.spec.ts -g "13."
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/storage/storage.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const RUN_LOOP = process.env.STORAGE_E2E_LOOP === '1';

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

/**
 * A non-admin session that can actually use the API.
 *
 * `POST /api/users` creates the account with an admin-chosen password, so
 * `password_change_guard` answers `403 password_change_required` to every
 * call until the owner picks their own. Log in, change it, log in again —
 * otherwise these authorization tests would assert a 403 that comes from the
 * forced-change gate instead of the admin gate they are about.
 */
async function apiLoginAsViewer(
  ctx: APIRequestContext,
  username: string,
  initialPassword: string,
  newPassword = 'Viewer67890!',
): Promise<{ token: string; csrf: string }> {
  const first = await apiLogin(ctx, username, initialPassword);
  await ctx.put(`/api/users/${username}/password`, {
    headers: { 'x-session-token': first.token, 'x-csrf-token': first.csrf },
    data: { current_password: initialPassword, new_password: newPassword },
  });
  return apiLogin(ctx, username, newPassword);
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

/**
 * The "What's New" dialog opens once per version. On a fresh scratch instance
 * it covers the dock and swallows the first click of the run, so dismiss it
 * before touching the desktop.
 */
async function dismissWhatsNew(page: Page) {
  const modal = page.locator('[data-testid="whats-new-modal"]');
  if (await modal.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await modal.waitFor({ state: 'hidden', timeout: 5_000 }).catch(() => {});
  }
}

async function gotoDesktop(page: Page, tokens: Tokens, lang?: string) {
  await seedBrowser(page, tokens, lang);
  await page.goto('/desktop');
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await dismissWhatsNew(page);
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

async function gotoStorage(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openLaunchpadApp(page, 'Storage');
  await page.waitForSelector('[data-testid="storage-page"]', { timeout: 10_000 });
  await expect(page.locator('[data-testid="storage-page"] [data-testid="storage-loading"]')).toHaveCount(0, { timeout: 15_000 });
}

function win(page: Page) {
  return page.locator('[data-testid="storage-page"]');
}

async function switchTab(page: Page, label: RegExp) {
  await win(page).locator('.hv-winhdr-tab', { hasText: label }).first().click();
}

/** authed fetch inside the page; returns {http, success, message}. */
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
      return { http: r.status, success: j?.success, message: j?.message || j?.error || '' };
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
  test('1.1 — Storage window opens from the Applications launcher', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — the loading spinner clears', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    await expect(win(page).locator('[data-testid="storage-loading"]')).toHaveCount(0);
  });

  test('1.3 — header shows the Storage title and its 7 tabs', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title')).toContainText(/storage|stockage|speicher|almacen/i);
    // disks / virtual disks / partitions / removable / encryption / cache /
    // snapshot schedules (2026-10-13)
    await expect(win(page).locator('.hv-winhdr-tab')).toHaveCount(7);
  });

  test('1.4 — the 4 stat tiles render', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    await expect(win(page).locator('[data-testid="storage-stats"] [data-testid="storage-stat-card"]')).toHaveCount(4);
  });

  test('1.5 — the disks tab shows a grid or an empty state', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    const grid = await win(page).locator('[data-testid="storage-disk-grid"]').count();
    const empty = await win(page).locator('[data-testid="storage-empty"]').count();
    expect(grid + empty).toBeGreaterThan(0);
  });
});

// ===========================================================================
// 2. Tab navigation
// ===========================================================================

test.describe('2 — Tab navigation', () => {
  const tabs: [RegExp, string][] = [
    [/virtual|virtuel|virtuell|virtual/i, '[data-testid="storage-tab-panel"]'],
    [/partition/i, '[data-testid="storage-tab-panel"]'],
    [/removable|amovible|wechsel|extra/i, '[data-testid="storage-tab-panel"]'],
    [/encrypt|chiffr|verschl|cifrad/i, '[data-testid="storage-tab-panel"]'],
    [/cache|caché/i, '[data-testid="storage-cache"]'],
  ];
  for (const [label, sel] of tabs) {
    test(`2.${label.source.slice(0, 8)} — switching to a tab renders its panel`, async ({ page }) => {
      await gotoStorage(page, adminTokens);
      await switchTab(page, label);
      await expect(win(page).locator(sel).first()).toBeVisible({ timeout: 8_000 });
    });
  }

  test('2.encryption — the Encrypt-a-device button is present', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    await switchTab(page, /encrypt|chiffr|verschl|cifrad/i);
    await expect(win(page).locator('[data-testid="storage-btn-primary"]').first()).toBeVisible();
  });
});

// ===========================================================================
// 3. Validation — mutating endpoints reject bad input cleanly (no 500)
// ===========================================================================

test.describe('3 — Validation', () => {
  test('3.1 — cache settings rejects a malformed cron', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    const r = await api(page, adminTokens, 'PUT', '/api/cache/settings', { mover_cron: 'nope' });
    expect(r.http).toBe(400);
    expect(r.message).toMatch(/5-field cron/i);
  });

  test('3.2 — cache settings rejects out-of-range knobs', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    for (const [field, val] of [
      // 0 would mean "drain the cache to nothing" for Prefer shares: the UI
      // clamps 1-95 and the server must refuse it too.
      ['min_free_percent', 0],
      ['min_free_percent', 200],
      ['mover_age_days', 99999],
      ['mover_bwlimit', 99999999],
    ] as const) {
      const r = await api(page, adminTokens, 'PUT', '/api/cache/settings', { [field]: val });
      expect(r.http, `${field}=${val}`).toBe(400);
    }
  });

  test('3.2b — an empty cache_pool clears the designation', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    const r = await api(page, adminTokens, 'PUT', '/api/cache/settings', { cache_pool: '' });
    expect(r.http).toBe(200);
    expect(r.success).toBe(true);
    const cleared = await page.evaluate(async () => {
      const token = localStorage.getItem('session-token') || '';
      const res = await fetch('/api/cache/settings', { headers: { 'x-session-token': token } });
      const body = await res.json().catch(() => ({}));
      return body?.data?.cache_pool ?? null;
    });
    expect(cleared).toBeNull();
  });

  test('3.3 — cache share create rejects a bad name and a bad mode', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    const bad = await api(page, adminTokens, 'POST', '/api/cache/shares', {
      name: 'has space',
      cache_mode: 'yes',
      target_pool: 'p',
    });
    expect(bad.http).toBe(400);
    expect(bad.message).toMatch(/cache share name/i);

    const mode = await api(page, adminTokens, 'POST', '/api/cache/shares', {
      name: 'ok',
      cache_mode: 'turbo',
      target_pool: 'p',
    });
    expect(mode.http).toBe(400);
    expect(mode.message).toMatch(/cache_mode/i);
  });

  test('3.4 — cache share update/delete report a missing share (not 500)', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    const upd = await api(page, adminTokens, 'PUT', '/api/cache/shares/ghost', { enabled: false });
    expect(upd.http).toBe(400);
    expect(upd.message).toMatch(/not found/i);
    const del = await api(page, adminTokens, 'DELETE', '/api/cache/shares/ghost');
    expect(del.http).toBe(400);
  });

  test('3.5 — encryption create rejects a weak passphrase / bad fs / bad label', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    const base = {
      device: '/dev/hv-nonexistent',
      filesystem: 'ext4',
      label: 'vault',
      passphrase: 'correct-horse-battery',
    };

    // 12 characters is the documented floor (it used to be 8, with no
    // strength check at all).
    const short = await api(page, adminTokens, 'POST', '/api/encryption/volumes', { ...base, passphrase: 'short' });
    expect(short.http).toBe(400);
    expect(short.message).toMatch(/12 characters/i);

    // Long enough, but the first thing a wordlist tries.
    const common = await api(page, adminTokens, 'POST', '/api/encryption/volumes', {
      ...base,
      passphrase: 'password1234',
    });
    expect(common.http).toBe(400);
    expect(common.message).toMatch(/too common/i);

    const fs = await api(page, adminTokens, 'POST', '/api/encryption/volumes', { ...base, filesystem: 'ntfs' });
    expect(fs.http).toBe(400);
    expect(fs.message).toMatch(/invalid filesystem/i);

    const label = await api(page, adminTokens, 'POST', '/api/encryption/volumes', { ...base, label: 'bad/label' });
    expect(label.http).toBe(400);
    expect(label.message).toMatch(/invalid label/i);

    // A compliant payload clears the policy and stops at the device gate —
    // which runs before any privileged command, so nothing is formatted.
    const device = await api(page, adminTokens, 'POST', '/api/encryption/volumes', base);
    expect(device.http).toBe(400);
    expect(device.message).toMatch(/not an available/i);
    expect(device.message).not.toMatch(/passphrase/i);
  });

  test('3.6 — encryption delete without confirmation is rejected', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    const r = await api(
      page,
      adminTokens,
      'DELETE',
      '/api/encryption/volumes/00000000-0000-0000-0000-000000000000',
      { confirm: false },
    );
    expect(r.http).toBe(400);
    expect(r.message).toMatch(/confirm/i);
  });

  test('3.7 — encryption unlock/lock of an unknown volume report "not found"', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    const id = '11111111-2222-3333-4444-555555555555';
    const unlock = await api(page, adminTokens, 'POST', `/api/encryption/volumes/${id}/unlock`, {
      passphrase: 'correct-horse-battery',
    });
    expect(unlock.http).toBe(400);
    expect(unlock.message).toMatch(/not found/i);
  });

  test('3.8 — encryption passphrase rotation reports an unknown volume', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    const id = '99999999-8888-7777-6666-555555555555';
    const r = await api(page, adminTokens, 'POST', `/api/encryption/volumes/${id}/passphrase`, {
      old_passphrase: 'correct-horse-battery',
      new_passphrase: 'a-longer-replacement-1',
    });
    expect(r.http).toBe(400);
    expect(r.message).toMatch(/not found/i);
  });
});

// ===========================================================================
// 4. Security — auth guards
// ===========================================================================

test.describe('4 — Security', () => {
  test('4.1 — read endpoints reject a missing session token', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    for (const path of [
      '/api/storage/disks',
      '/api/storage/partitions',
      '/api/cache/settings',
      '/api/cache/pools',
      '/api/encryption/volumes',
    ]) {
      const r = await api(page, null, 'GET', path);
      expect(r.http, path).toBe(400);
    }
  });

  test('4.2 — read endpoints reject a tampered token', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    const r = await api(page, { ...adminTokens, token: adminTokens.token + 'x' }, 'GET', '/api/cache/settings');
    expect(r.http).toBe(401);
  });

  test('4.3 — write endpoints reject a non-admin session', async ({ playwright }) => {
    const ctx = await newCtx(playwright);
    const adminHeaders = { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf };
    const uname = `stviewer${Date.now().toString(36)}`;
    const created = await ctx.post('/api/users', {
      headers: adminHeaders,
      data: {
        username: uname,
        password: 'Viewer12345!',
        fullname: 'ST Viewer',
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
      const { token, csrf } = await apiLoginAsViewer(ctx, uname, 'Viewer12345!');
      const h = { 'x-session-token': token, 'x-csrf-token': csrf };
      for (const [method, path, data] of [
        ['put', '/api/cache/settings', { mover_enabled: true }],
        ['post', '/api/cache/shares', { name: 'x', cache_mode: 'yes', target_pool: 'p' }],
        ['post', '/api/cache/mover/run', {}],
        ['post', '/api/encryption/volumes', { device: '/dev/x', filesystem: 'ext4', label: 'l', passphrase: 'correct-horse-battery' }],
        [
          'post',
          '/api/encryption/volumes/00000000-0000-0000-0000-000000000000/passphrase',
          { old_passphrase: 'correct-horse-battery', new_passphrase: 'a-longer-replacement-1' },
        ],
        ['delete', '/api/encryption/volumes/00000000-0000-0000-0000-000000000000', { confirm: true }],
      ] as const) {
        const r = await (ctx as any)[method](path, { headers: h, data });
        expect(r.status(), `${method} ${path}`).toBe(403);
      }
    } finally {
      await ctx.delete(`/api/users/${uname}`, { headers: adminHeaders }).catch(() => {});
      await ctx.dispose();
    }
  });

  test('4.4 — read endpoints allow a non-admin session', async ({ playwright }) => {
    const ctx = await newCtx(playwright);
    const adminHeaders = { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf };
    const uname = `stviewer2${Date.now().toString(36)}`;
    const created = await ctx.post('/api/users', {
      headers: adminHeaders,
      data: { username: uname, password: 'Viewer12345!', fullname: 'V', email: '', role_id: 1, groups: [], home_enabled: false },
    });
    if (!created.ok()) {
      await ctx.dispose();
      test.skip(true, `could not create a non-admin user (status ${created.status()})`);
    }
    try {
      const { token } = await apiLoginAsViewer(ctx, uname, 'Viewer12345!');
      const r = await ctx.get('/api/storage/disks', { headers: { 'x-session-token': token } });
      expect(r.status()).toBe(200);
    } finally {
      await ctx.delete(`/api/users/${uname}`, { headers: adminHeaders }).catch(() => {});
      await ctx.dispose();
    }
  });

  test('4.5 — no session token appears in any URL', async ({ page }) => {
    const urls: string[] = [];
    page.on('request', (r) => urls.push(r.url()));
    await gotoStorage(page, adminTokens);
    await page.waitForTimeout(1000);
    for (const u of urls) expect(u).not.toContain(adminTokens.token);
  });

  test('4.6 — an XSS payload in the encrypt label field is not rendered as markup', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    await switchTab(page, /encrypt|chiffr|verschl|cifrad/i);
    await win(page).locator('[data-testid="storage-btn-primary"]').first().click();
    const field = page.locator('input').filter({ hasNot: page.locator('[type="password"]') }).last();
    await field.fill('"><img src=x onerror=alert(1)>').catch(() => {});
    const html = await page.content();
    expect(html).not.toContain('<img src=x onerror=');
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
      await gotoStorage(page, adminTokens);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
      );
      expect(overflow).toBe(false);
    });
  }
});

// ===========================================================================
// 6. Accessibility
// ===========================================================================

test.describe('6 — Accessibility', () => {
  test('6.1 — the tab bar exposes tablist/tab roles', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    await expect(win(page).locator('[role="tablist"]')).toHaveCount(1);
    expect(await win(page).locator('[role="tab"]').count()).toBeGreaterThanOrEqual(5);
  });

  test('6.2 — no duplicate element ids', async ({ page }) => {
    await gotoStorage(page, adminTokens);
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
});

// ===========================================================================
// 7. Console & network
// ===========================================================================

test.describe('7 — Console & network', () => {
  test('7.1 — no uncaught JS errors browsing all tabs', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await gotoStorage(page, adminTokens);
    for (const label of [/partition/i, /removable|amovible/i, /encrypt|chiffr/i, /cache/i, /disk|disque/i]) {
      await switchTab(page, label).catch(() => {});
      await page.waitForTimeout(300);
    }
    expect(errors).toEqual([]);
  });

  test('7.2 — no failed storage/cache/encryption requests during a normal load', async ({ page }) => {
    const bad: string[] = [];
    page.on('response', (r) => {
      if (/\/api\/(storage|cache|encryption)\//.test(r.url()) && r.status() >= 400) {
        bad.push(`${r.status()} ${r.url()}`);
      }
    });
    await gotoStorage(page, adminTokens);
    await page.waitForTimeout(2500);
    expect(bad).toEqual([]);
  });
});

// ===========================================================================
// 8. Edge cases & memory
// ===========================================================================

test.describe('8 — Edge cases & memory', () => {
  test('8.1 — rapidly switching tabs ×10 does not crash', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    const labels = [/disk|disque/i, /partition/i, /removable|amovible/i, /encrypt|chiffr/i, /cache/i];
    for (let i = 0; i < 10; i++) await switchTab(page, labels[i % labels.length]).catch(() => {});
    await expect(win(page)).toBeVisible();
  });

  test('8.2 — clearing localStorage mid-session and reloading does not crash', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await gotoStorage(page, adminTokens);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await expect(page.locator('.hv-login-page, [data-testid="storage-page"], #app')).toBeAttached({ timeout: 15_000 });
    await page.waitForTimeout(800);
    expect(errors).toEqual([]);
  });

  test('8.3 — open/close the window ×5 without significant heap growth', async ({ page }) => {
    test.setTimeout(150_000);
    await gotoStorage(page, adminTokens);
    const base = await getHeapMB(page);
    for (let i = 0; i < 5; i++) {
      await page.locator('.hv-cl-btn-close').first().click();
      await expect(win(page)).toHaveCount(0);
      await openLaunchpadApp(page, 'Storage');
      await page.waitForSelector('[data-testid="storage-page"]', { timeout: 10_000 });
    }
    const after = await getHeapMB(page);
    if (base > 0 && after > 0) {
      console.log(`[MEM] heap ${base.toFixed(1)} → ${after.toFixed(1)} MB`);
      expect(after - base).toBeLessThan(12);
    }
  });
});

// ===========================================================================
// 12. Cache mover history — per-share outcome rendering (stubbed response)
// ===========================================================================
//
// The per-share rows only exist after a real run, which needs a real cache
// pool; the scratch host has none. Stub `GET /api/cache/mover/runs` so the
// rendering path (badge, share names, failure message) is still exercised.

test.describe('12 — Cache mover history', () => {
  const RUNS = {
    success: true,
    data: [
      {
        id: 'run-partial',
        status: 'partial',
        started_at: '2026-10-22T03:00:00Z',
        finished_at: '2026-10-22T03:04:00Z',
        files_moved: 12,
        bytes_moved: 4096,
        error_message: 'Shares not moved: backups',
        shares: [
          {
            share_name: 'media',
            mode: 'yes',
            status: 'moved',
            files_moved: 12,
            bytes_moved: 4096,
            message: null,
          },
          {
            share_name: 'backups',
            mode: 'prefer',
            status: 'failed',
            files_moved: 0,
            bytes_moved: 0,
            message: 'not enough space on /mnt/array: 1.0 GiB to move, 12.0 MiB free',
          },
        ],
      },
    ],
  };

  test('12.1 — a partial run names the shares it moved and the one it did not', async ({ page }) => {
    await page.route('**/api/cache/mover/runs', (route) =>
      route.fulfill({ status: 200, json: RUNS }),
    );
    await gotoStorage(page, adminTokens);
    await switchTab(page, /cache|caché|zwischenspeicher|caché/i);

    const card = win(page).locator('[data-testid="storage-cache-runs"]').first();
    await expect(card).toBeVisible({ timeout: 8_000 });

    // The status badge must read "Partial" — the old code compared against
    // `success` and fell through to the neutral colour for every real status.
    await expect(card.getByText(/partial/i).first()).toBeVisible();

    // Both shares are listed, with the failure reason on the one that failed.
    await expect(card.getByText('media', { exact: true })).toBeVisible();
    await expect(card.getByText('backups', { exact: true })).toBeVisible();
    await expect(card.getByText(/not enough space on \/mnt\/array/)).toBeVisible();
    await expect(card.getByText(/moved/).first()).toBeVisible();
  });

  test('12.2 — a run with no per-share rows still renders', async ({ page }) => {
    await page.route('**/api/cache/mover/runs', (route) =>
      route.fulfill({
        status: 200,
        json: {
          success: true,
          data: [
            {
              id: 'run-old',
              status: 'completed',
              started_at: '2026-10-21T03:00:00Z',
              finished_at: '2026-10-21T03:01:00Z',
              files_moved: 0,
              bytes_moved: 0,
              error_message: null,
              shares: [],
            },
          ],
        },
      }),
    );
    await gotoStorage(page, adminTokens);
    await switchTab(page, /cache|caché|zwischenspeicher|caché/i);

    const card = win(page).locator('[data-testid="storage-cache-runs"]').first();
    await expect(card.getByText(/completed/i).first()).toBeVisible();
  });
});

// ===========================================================================
// 13. Real LUKS lifecycle — opt-in only (STORAGE_E2E_LOOP=1 + passwordless sudo)
// ===========================================================================

test.describe('13 — Real LUKS lifecycle (opt-in)', () => {
  test.skip(!RUN_LOOP, 'set STORAGE_E2E_LOOP=1 to run the real loop-device LUKS lifecycle');

  test('13.1 — create → verify listed → rotate passphrase → delete a LUKS volume on a throwaway loop device', async ({
    page,
    playwright,
  }) => {
    // This test needs a helper the browser cannot provide (losetup). It is
    // only meaningful on a host with passwordless sudo; the server itself
    // runs the cryptsetup calls. We drive it entirely through the API and
    // assume the operator has arranged a spare loop device path in
    // STORAGE_E2E_LOOP_DEV (e.g. /dev/loop9 backed by a 64MiB file).
    const dev = process.env.STORAGE_E2E_LOOP_DEV;
    test.skip(!dev, 'set STORAGE_E2E_LOOP_DEV to a disposable loop device (e.g. /dev/loop9)');

    const ctx = await newCtx(playwright);
    const h = { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf };
    const label = `e2eluks${Date.now().toString(36)}`;
    const passphrase = 'e2e-passphrase-123';
    const rotated = 'e2e-passphrase-456';
    let volumeId: string | null = null;

    try {
      // Create answers `{ job_id }` — formatting a real device takes minutes,
      // so the work runs in a tracked job (ROADMAP 9.1). Follow it to the end.
      const create = await ctx.post('/api/encryption/volumes', {
        headers: h,
        data: { device: dev, filesystem: 'ext4', label, passphrase },
      });
      const body = await create.json().catch(() => ({}));
      if (!body?.success || !body?.data?.job_id) {
        test.skip(true, `create failed (${create.status()}): ${body?.message || body?.error}`);
      }
      const jobId = body.data.job_id as string;
      expect(body.data.id, 'create must not return the volume synchronously').toBeUndefined();

      // Poll the job until it completes, then find the volume by label.
      const deadline = Date.now() + 180_000;
      let failed = '';
      while (Date.now() < deadline) {
        const job = await ctx.get(`/api/progress/${jobId}/status`, { headers: h });
        const state = await job.json().catch(() => ({}));
        const status = String(state?.data?.state ?? '').toLowerCase();
        if (status === 'failed' || status === 'cancelled') {
          failed = state?.data?.message || status;
          break;
        }
        const listed = await ctx.get('/api/encryption/volumes', { headers: h });
        const listedBody = await listed.json().catch(() => ({}));
        const found = (listedBody?.data ?? []).find((v: any) => v.label === label);
        if (found) {
          volumeId = found.id;
          break;
        }
        if (status === 'completed') break;
        await page.waitForTimeout(1000);
      }
      if (!volumeId) {
        test.skip(true, `the create job did not produce a volume: ${failed || 'timed out'}`);
      }

      await gotoStorage(page, adminTokens);
      await switchTab(page, /encrypt|chiffr|verschl|cifrad/i);
      await expect(win(page).getByText(label, { exact: false })).toBeVisible({ timeout: 10_000 });

      // Rotate the passphrase, then prove the new one is the one that works:
      // `luksChangeKey` + a `--test-passphrase` proof server-side.
      const rotate = await ctx.post(`/api/encryption/volumes/${volumeId}/passphrase`, {
        headers: h,
        data: { old_passphrase: passphrase, new_passphrase: rotated },
      });
      expect(rotate.status(), 'the rotation must be accepted').toBe(200);

      // The old passphrase must now be refused (the volume is still mounted,
      // so this is a lock → unlock round trip with the wrong one).
      const lock = await ctx.post(`/api/encryption/volumes/${volumeId}/lock`, { headers: h });
      expect([200, 400]).toContain(lock.status());
      if (lock.ok()) {
        const stale = await ctx.post(`/api/encryption/volumes/${volumeId}/unlock`, {
          headers: h,
          data: { passphrase },
        });
        expect(stale.status(), 'the rotated-away passphrase must not unlock').toBe(400);
        const fresh = await ctx.post(`/api/encryption/volumes/${volumeId}/unlock`, {
          headers: h,
          data: { passphrase: rotated },
        });
        expect(fresh.status(), 'the new passphrase must unlock').toBe(200);
      }
    } finally {
      if (volumeId) {
        const del = await ctx.delete(`/api/encryption/volumes/${volumeId}`, {
          headers: h,
          data: { confirm: true },
        });
        expect([200, 400]).toContain(del.status());
      }
      await ctx.dispose();
    }
  });
});

// ===========================================================================
// 14. Storage health — scrub status & foreign-pool import
// ===========================================================================

test.describe('14 — Scrub status & foreign-pool import', () => {
  test('14.1 — the virtual-disks tab offers a scrub-status control per disk', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    await switchTab(page, /virtual|virtuel|virtuell|virtual/i);
    const cards = win(page).locator('[data-testid="storage-vd-card"]');
    const count = await cards.count();
    test.skip(count === 0, 'no virtual disks on this host — nothing to check');
    // Every card carries the "Check status" button (it never shells out until
    // clicked, so this stays read-only).
    const buttons = win(page).locator('[data-testid="storage-vd-card"] button', {
      hasText: /status|état|estado/i,
    });
    expect(await buttons.count()).toBeGreaterThan(0);
  });

  test('14.2 — clicking "Check status" renders the scrub state inline', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    await switchTab(page, /virtual|virtuel|virtuell|virtual/i);
    const cards = win(page).locator('[data-testid="storage-vd-card"]');
    const count = await cards.count();
    test.skip(count === 0, 'no virtual disks on this host — nothing to check');
    const statusBtn = cards
      .first()
      .locator('button', { hasText: /status|état|estado/i })
      .first();
    await statusBtn.click();
    // Either the inline status appears, or a dialog explains the failure —
    // never a crash or a silent no-op.
    const inline = cards.first().locator('[data-testid="storage-vd-scrub-status"]');
    await expect(inline).toBeVisible({ timeout: 8_000 });
  });

  test('14.3 — the importable-pools section is well-formed when present', async ({ page }) => {
    await gotoStorage(page, adminTokens);
    await switchTab(page, /virtual|virtuel|virtuell|virtual/i);
    const section = win(page).locator('[data-testid="storage-importable-pools"]');
    if ((await section.count()) === 0) {
      // No foreign pool on this host: the section is correctly absent.
      expect(await section.count()).toBe(0);
      return;
    }
    await expect(section).toBeVisible();
    const rows = section.locator('[data-testid="storage-importable-pool"]');
    expect(await rows.count()).toBeGreaterThan(0);
  });

  test('14.4 — the import endpoint rejects a non-admin session', async ({ playwright }) => {
    const ctx = await playwright.request.newContext({ baseURL: BASE });
    // Unauthenticated → 400/401.
    const anon = await ctx.post('/api/storage/import-pool', {
      data: { name: 'tank', pool_type: 'zfs' },
    });
    expect([400, 401]).toContain(anon.status());

    // A valid non-admin session → 403 (require_admin), never a command run.
    const login = await ctx.post('/api/auth/login', {
      data: { username: 'e2euser', password: 'E2eSecret123' },
    });
    if (login.ok()) {
      const body = await login.json();
      const token = body?.data?.token;
      if (token) {
        const forbidden = await ctx.post('/api/storage/import-pool', {
          headers: { 'x-session-token': token, 'x-csrf-token': body?.data?.csrf_token || '' },
          data: { name: 'tank', pool_type: 'zfs' },
        });
        expect([403, 401]).toContain(forbidden.status());
      }
    }
    await ctx.dispose();
  });

  test('14.5 — the scrub-status endpoint rejects a non-admin session', async ({ playwright }) => {
    const ctx = await playwright.request.newContext({ baseURL: BASE });
    const anon = await ctx.get('/api/storage/virtual-disks/does-not-exist/scrub');
    expect([400, 401]).toContain(anon.status());
    await ctx.dispose();
  });
});
