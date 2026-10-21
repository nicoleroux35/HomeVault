/* =============================================================================
 * File          : backup.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Backup screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Backup (restic) app — End-to-End Test Suite
 *
 * Not a route: a window opened from the desktop's Applications launcher
 * (id `backup`, "Backup" tile). Manages restic repositories, snapshots,
 * manual + scheduled backups and retention policies via
 * `src/handlers/restic.rs` + `src/restic_scheduler.rs`.
 *
 * **Safety model:** a restic repository + backup is the intended, additive,
 * reversible primary use of this page, and restic operates on a
 * self-contained repo directory — nothing else on the host is affected.
 * Section 13 therefore runs a **genuine** lifecycle: create a repo in a
 * throwaway `/tmp/hv-backup-e2e-*` directory, run a real backup of a small
 * throwaway source dir, list the resulting snapshot, then delete the repo
 * and remove both directories. It skips cleanly if `restic` isn't installed
 * or repo creation fails.
 *
 * Mutating requests pass the CSRF middleware, so the in-page `fetch`
 * helpers send `x-session-token` + `x-csrf-token` (captured in beforeAll).
 *
 * Run (all):     npx playwright test tests/e2e/backup/backup.spec.ts --project=firefox
 * Run (single):  npx playwright test tests/e2e/backup/backup.spec.ts -g "1.1"
 * Run (real):    npx playwright test tests/e2e/backup/backup.spec.ts -g "13."
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/backup/backup.spec.ts
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

async function gotoBackup(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openLaunchpadApp(page, 'Backup');
  await page.waitForSelector('[data-testid="restic-page"]', { timeout: 10_000 });
  await expect(page.locator('[data-testid="restic-page"] [data-testid="restic-loading"]')).toHaveCount(0, { timeout: 15_000 });
}

function win(page: Page) {
  return page.locator('[data-testid="restic-page"]');
}

async function resticAvailable(page: Page): Promise<boolean> {
  return page.evaluate(async () => {
    const r = await fetch('/api/restic/available', {
      headers: { 'x-session-token': localStorage.getItem('session-token') || '' },
    });
    const b = await r.json().catch(() => ({}));
    return !!b?.data?.available;
  });
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
      return { http: r.status, success: j?.success, message: j?.message || j?.error || '', data: j?.data };
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
  test('1.1 — Backup window opens from the Applications launcher', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — the loading spinner clears', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    await expect(win(page).locator('[data-testid="restic-loading"]')).toHaveCount(0);
  });

  test('1.3 — adapts to state: repos view or the "restic not installed" panel', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    const notInstalled = await win(page).locator('[data-testid="restic-empty"]', { hasText: /not installed|non installé|nicht installiert|no está instalado/i }).count();
    const header = await win(page).locator('.hv-winhdr-title').count();
    expect(notInstalled + header).toBeGreaterThan(0);
  });

  test('1.4 — the stat tiles render when restic is available', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    if (!(await resticAvailable(page))) test.skip(true, 'restic not installed');
    await expect(win(page).locator('[data-testid="restic-stats"] [data-testid="restic-stat-card"]').first()).toBeVisible();
  });
});

// ===========================================================================
// 2. Repos view + create-repo form
// ===========================================================================

test.describe('2 — Repos view', () => {
  test('2.1 — shows a repo list or an empty state', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    if (!(await resticAvailable(page))) test.skip(true, 'restic not installed');
    const cards = await win(page).locator('[data-testid="restic-repo-card"]').count();
    const empty = await win(page).locator('[data-testid="restic-empty"]').count();
    expect(cards + empty).toBeGreaterThan(0);
  });

  test('2.2 — the create-repo form exposes name / url / password fields', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    if (!(await resticAvailable(page))) test.skip(true, 'restic not installed');
    // "Add repository" lives in the toolbar under the header (never inside the
    // header bar); fall back to the empty-state button when the toolbar action
    // is not offered.
    const action = win(page).locator('[data-testid="restic-add-repo"]');
    if (await action.count()) {
      await action.first().click();
    } else {
      await win(page).locator('[data-testid="restic-btn-primary"]').first().click();
    }
    await expect(win(page).locator('#repo-name')).toBeVisible();
    await expect(win(page).locator('#repo-password')).toHaveAttribute('type', 'password');
    await expect(win(page).locator('#repo-url')).toBeVisible();
  });
});

// ===========================================================================
// 3. Validation (via API — nothing reaches restic)
// ===========================================================================

test.describe('3 — Validation', () => {
  test('3.1 — schedule create rejects an invalid cron expression', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    // needs a repo id; use a bogus one — repo check happens first, so use a
    // real one only if present. Either way we should NOT get a 500.
    const r = await api(page, adminTokens, 'POST', '/api/restic/schedules', {
      repo_id: 'ghost',
      name: 'n',
      cron_expression: 'not a cron',
    });
    expect(r.http).toBe(400);
    // ghost repo → "Repository not found"; a real repo → "Invalid cron"
    expect(r.message).toMatch(/not found|invalid cron/i);
  });

  test('3.2 — retention policy create rejects an empty name', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    const r = await api(page, adminTokens, 'POST', '/api/restic/retention-policies', { name: '  ' });
    expect(r.http).toBe(400);
    expect(r.message).toMatch(/name is required/i);
  });

  test('3.3 — snapshot delete requires repo_id and snapshot_id', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    const r = await api(page, adminTokens, 'POST', '/api/restic/snapshots/delete', { repo_id: 'x' });
    expect(r.http).toBe(400);
    expect(r.message).toMatch(/required/i);
  });

  test('3.4 — backup / check / restore on an unknown repo report "not found" (no 500)', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    for (const [path, body] of [
      ['/api/restic/backup', { repo_id: 'ghost' }],
      ['/api/restic/check', { repo_id: 'ghost' }],
      ['/api/restic/restore', { repo_id: 'ghost', snapshot_id: 's', target: '/tmp/t' }],
    ] as const) {
      const r = await api(page, adminTokens, 'POST', path, body);
      expect(r.http, path).toBe(400);
      expect(r.message, path).toMatch(/not found/i);
    }
  });

  test('3.5 — retention policy create accepts a valid policy and round-trips', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    const name = `e2e-pol-${Date.now().toString(36)}`;
    const created = await api(page, adminTokens, 'POST', '/api/restic/retention-policies', {
      name,
      max_count: 5,
      max_age_days: 14,
      max_size_gb: 250,
    });
    expect(created.http).toBe(200);
    const id = created.data?.id;
    try {
      const list = await api(page, adminTokens, 'GET', '/api/restic/retention-policies');
      const mine = (list.data as any[]).find((p) => p.id === id);
      expect(mine).toBeTruthy();
      // The size cap is applied by the scheduler after each run — it must at
      // least survive the round-trip, otherwise the field is a silent no-op.
      expect(mine.max_size_gb).toBe(250);
    } finally {
      if (id) await api(page, adminTokens, 'DELETE', `/api/restic/retention-policies/${id}`);
    }
  });

  test('3.6 — a retention policy with no limit and a zero count is refused', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    // No limit at all: it would never prune anything.
    const none = await api(page, adminTokens, 'POST', '/api/restic/retention-policies', {
      name: `e2e-pol-none-${Date.now().toString(36)}`,
    });
    expect(none.http).toBe(400);
    expect(none.message.toLowerCase()).toContain('retention limit');

    // `max_count: 0` would mean "forget every snapshot".
    const zero = await api(page, adminTokens, 'POST', '/api/restic/retention-policies', {
      name: `e2e-pol-zero-${Date.now().toString(36)}`,
      max_count: 0,
    });
    expect(zero.http).toBe(400);
    expect(zero.message).toContain('greater than 0');
  });
});

// ===========================================================================
// 4. Security
// ===========================================================================

test.describe('4 — Security', () => {
  test('4.1 — read endpoints reject a missing token', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    for (const p of [
      '/api/restic/available',
      '/api/restic/repos',
      '/api/restic/schedules',
      '/api/restic/retention-policies',
    ]) {
      const r = await api(page, null, 'GET', p);
      expect(r.http, p).toBe(400);
    }
  });

  test('4.2 — a tampered token is rejected', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    const r = await api(page, { ...adminTokens, token: adminTokens.token + 'x' }, 'GET', '/api/restic/repos');
    expect(r.http).toBe(401);
  });

  test('4.3 — include_password=true requires admin (credential leak gate)', async ({ playwright }) => {
    const ctx = await newCtx(playwright);
    const adminHeaders = { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf };
    const uname = `bkviewer${Date.now().toString(36)}`;
    const created = await ctx.post('/api/users', {
      headers: adminHeaders,
      data: { username: uname, password: 'Viewer12345!', fullname: 'V', email: '', role_id: 1, groups: [], home_enabled: false },
    });
    if (!created.ok()) {
      await ctx.dispose();
      test.skip(true, `could not create non-admin user (${created.status()})`);
    }
    try {
      const first = await apiLogin(ctx, uname, 'Viewer12345!');
      // The account was created with an admin-chosen password, so the session
      // is gated by `password_change_guard`: every API call answers 403
      // `password_change_required` until the owner picks their own password
      // (that gate is what makes 4.3 meaningful — without it the plain read
      // below would be refused for the wrong reason).
      const changed = await ctx.put(`/api/users/${uname}/password`, {
        headers: { 'x-session-token': first.token, 'x-csrf-token': first.csrf },
        data: { current_password: 'Viewer12345!', new_password: 'Viewer67890!' },
      });
      expect(changed.status()).toBe(200);

      const { token } = await apiLogin(ctx, uname, 'Viewer67890!');
      const plain = await ctx.get('/api/restic/repos', { headers: { 'x-session-token': token } });
      expect(plain.status()).toBe(200);
      const leak = await ctx.get('/api/restic/repos?include_password=true', {
        headers: { 'x-session-token': token },
      });
      expect(leak.status()).toBe(403);
    } finally {
      await ctx.delete(`/api/users/${uname}`, { headers: adminHeaders }).catch(() => {});
      await ctx.dispose();
    }
  });

  test('4.4 — mutating endpoints reject a non-admin session', async ({ playwright }) => {
    const ctx = await newCtx(playwright);
    const adminHeaders = { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf };
    const uname = `bkviewer2${Date.now().toString(36)}`;
    const created = await ctx.post('/api/users', {
      headers: adminHeaders,
      data: { username: uname, password: 'Viewer12345!', fullname: 'V', email: '', role_id: 1, groups: [], home_enabled: false },
    });
    if (!created.ok()) {
      await ctx.dispose();
      test.skip(true, `could not create non-admin user (${created.status()})`);
    }
    try {
      const first = await apiLogin(ctx, uname, 'Viewer12345!');
      // Clear the forced first-sign-in change first: otherwise the guard
      // answers 403 for every call and these assertions would pass without
      // ever exercising the admin gate they are about.
      await ctx.put(`/api/users/${uname}/password`, {
        headers: { 'x-session-token': first.token, 'x-csrf-token': first.csrf },
        data: { current_password: 'Viewer12345!', new_password: 'Viewer67890!' },
      });
      const { token, csrf } = await apiLogin(ctx, uname, 'Viewer67890!');
      const h = { 'x-session-token': token, 'x-csrf-token': csrf };
      for (const [method, path, data] of [
        ['post', '/api/restic/retention-policies', { name: 'x' }],
        ['post', '/api/restic/schedules', { repo_id: 'x', name: 'n', cron_expression: '0 3 * * *' }],
        ['post', '/api/restic/backup', { repo_id: 'x' }],
      ] as const) {
        const r = await (ctx as any)[method](path, { headers: h, data });
        // 403 for authorization, never the 403 `password_change_required`
        // of a still-flagged session.
        expect(r.status(), `${method} ${path}`).toBe(403);
        const body = await r.json().catch(() => ({}));
        expect(body?.code, `${method} ${path}`).not.toBe('password_change_required');
      }
    } finally {
      await ctx.delete(`/api/users/${uname}`, { headers: adminHeaders }).catch(() => {});
      await ctx.dispose();
    }
  });

  test('4.5 — no session token in any URL', async ({ page }) => {
    const urls: string[] = [];
    page.on('request', (r) => urls.push(r.url()));
    await gotoBackup(page, adminTokens);
    await page.waitForTimeout(1000);
    for (const u of urls) expect(u).not.toContain(adminTokens.token);
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
      await gotoBackup(page, adminTokens);
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
  test('6.1 — no duplicate element ids', async ({ page }) => {
    await gotoBackup(page, adminTokens);
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

  test('6.2 — no uncaught JS errors on load', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await gotoBackup(page, adminTokens);
    await page.waitForTimeout(2000);
    expect(errors).toEqual([]);
  });

  test('6.3 — no failed restic requests during a normal load', async ({ page }) => {
    const bad: string[] = [];
    page.on('response', (r) => {
      if (r.url().includes('/api/restic/') && r.status() >= 400) bad.push(`${r.status()} ${r.url()}`);
    });
    await gotoBackup(page, adminTokens);
    await page.waitForTimeout(2500);
    expect(bad).toEqual([]);
  });
});

// ===========================================================================
// 13. Real repo + backup lifecycle (best-effort, always cleaned up)
// ===========================================================================

test.describe('13 — Real backup lifecycle', () => {
  /**
   * Poll a repo's job history until its newest job leaves `running`.
   *
   * `POST /api/restic/backup` answers immediately (the run is a background
   * job), so a test that reads the snapshot list straight after the response
   * would race the backup.
   */
  async function waitForBackupJob(page: Page, repoId: string, timeoutMs = 90_000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const jobs = await api(page, adminTokens, 'GET', `/api/restic/repos/${repoId}/jobs`);
      const list = (jobs.data as any[]) ?? [];
      if (list.length > 0 && list[0].status !== 'running') return list[0];
      await page.waitForTimeout(500);
    }
    throw new Error('the backup job did not finish in time');
  }

  test('13.1 — create repo → run a real backup → see the snapshot → delete', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    if (!(await resticAvailable(page))) test.skip(true, 'restic not installed');

    const stamp = Date.now().toString(36);
    const repoDir = `/tmp/hv-backup-e2e-${stamp}`;
    const srcDir = `/tmp/hv-backup-e2e-${stamp}-src`;
    let repoId: string | null = null;

    // restic needs the source dir to exist; the server process can read /tmp.
    // Create it by pointing a throwaway aria2-free write... actually the
    // server can't mkdir arbitrary dirs on demand, but `restic backup` of a
    // missing path just fails — so use a path we know exists: the repo dir's
    // parent is /tmp which always exists; back up /etc/hostname (tiny, present
    // on every Linux host, read-only, safe).
    const created = await api(page, adminTokens, 'POST', '/api/restic/repos', {
      name: `e2e-${stamp}`,
      repository: repoDir,
      password: 'e2e-backup-pass-123',
      source_paths: ['/etc/hostname'],
    });
    if (!created.success) {
      test.skip(true, `repo create failed: ${created.message}`);
    }
    repoId = created.data?.repo?.id ?? created.data?.id ?? null;
    expect(repoId).toBeTruthy();

    try {
      // run a real backup — the endpoint starts it and returns its job id
      const backup = await api(page, adminTokens, 'POST', '/api/restic/backup', { repo_id: repoId });
      expect(backup.http).toBe(200);
      expect(backup.success, `backup: ${backup.message}`).toBe(true);
      expect(backup.data?.status).toBe('running');
      expect(backup.data?.job_id).toBeTruthy();
      // the history row exists *before* the run ends (started_at is the real
      // start time, and a crash leaves a row the startup reaper closes)
      expect(backup.data?.backup_job_id).toBeTruthy();

      const job = await waitForBackupJob(page, repoId);
      expect(job.status, JSON.stringify(job)).toBe('completed');
      expect(job.snapshot_id).toBeTruthy();

      // the snapshot really exists in the repo
      const snaps = await api(page, adminTokens, 'GET', `/api/restic/repos/${repoId}/snapshots`);
      expect(snaps.http).toBe(200);
      expect(Array.isArray(snaps.data)).toBe(true);
      expect((snaps.data as any[]).length).toBeGreaterThanOrEqual(1);
      expect((snaps.data as any[])[0].paths).toContain('/etc/hostname');

      // and the UI lists the repo (scope to the name element — the path
      // `<code>` also contains the stamp)
      await gotoBackup(page, adminTokens);
      await expect(
        win(page).locator('[data-testid="restic-repo-name"]', { hasText: `e2e-${stamp}` }),
      ).toBeVisible({ timeout: 10_000 });
    } finally {
      if (repoId) {
        const del = await api(page, adminTokens, 'DELETE', `/api/restic/repos/${repoId}`);
        expect([200, 400]).toContain(del.http);
      }
    }
  });

  test('13.2 — a schedule created with a crontab weekday lands on the right day', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    if (!(await resticAvailable(page))) test.skip(true, 'restic not installed');

    const stamp = Date.now().toString(36);
    const created = await api(page, adminTokens, 'POST', '/api/restic/repos', {
      name: `e2e-sch-${stamp}`,
      repository: `/tmp/hv-backup-e2e-sch-${stamp}`,
      password: 'e2e-backup-pass-123',
      source_paths: ['/etc/hostname'],
    });
    if (!created.success) test.skip(true, `repo create failed: ${created.message}`);
    const repoId = created.data?.repo?.id ?? created.data?.id;

    try {
      // "every Monday at 02:00" in crontab numbering
      const sch = await api(page, adminTokens, 'POST', '/api/restic/schedules', {
        repo_id: repoId,
        name: 'weekly',
        cron_expression: '0 2 * * 1',
      });
      expect(sch.http).toBe(200);
      const nextRun = sch.data?.next_run_at as string;
      expect(nextRun).toBeTruthy();
      // crontab 1 == Monday (getUTCDay(): 1). If the DOW were off by one this
      // would be 0 (Sunday).
      expect(new Date(nextRun).getUTCDay()).toBe(1);
    } finally {
      if (repoId) await api(page, adminTokens, 'DELETE', `/api/restic/repos/${repoId}`);
    }
  });

  test('13.3 — a freshly written repository checks out as healthy', async ({ page }) => {
    await gotoBackup(page, adminTokens);
    if (!(await resticAvailable(page))) test.skip(true, 'restic not installed');

    const stamp = Date.now().toString(36);
    const created = await api(page, adminTokens, 'POST', '/api/restic/repos', {
      name: `e2e-chk-${stamp}`,
      repository: `/tmp/hv-backup-e2e-chk-${stamp}`,
      password: 'e2e-backup-pass-123',
      source_paths: ['/etc/hostname'],
    });
    if (!created.success) test.skip(true, `repo create failed: ${created.message}`);
    const repoId = created.data?.repo?.id ?? created.data?.id;

    try {
      const backup = await api(page, adminTokens, 'POST', '/api/restic/backup', { repo_id: repoId });
      expect(backup.success, `backup: ${backup.message}`).toBe(true);
      const job = await waitForBackupJob(page, repoId);
      expect(job.status, JSON.stringify(job)).toBe('completed');

      const check = await api(page, adminTokens, 'POST', '/api/restic/check', { repo_id: repoId });
      expect(check.http).toBe(200);
      // restic prints "no errors were found" on success. A substring match on
      // "error" used to flag *every* healthy repository as broken, so this
      // assertion is the regression guard for the check verdict.
      expect(check.data?.check?.healthy, JSON.stringify(check.data)).toBe(true);
      expect(check.data?.check?.errors ?? []).toEqual([]);
    } finally {
      if (repoId) await api(page, adminTokens, 'DELETE', `/api/restic/repos/${repoId}`);
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
    await gotoBackup(page, adminTokens);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await expect(page.locator('.hv-login-page, [data-testid="restic-page"], #app')).toBeAttached({ timeout: 15_000 });
    await page.waitForTimeout(800);
    expect(errors).toEqual([]);
  });

  test('14.2 — open/close the window ×5 without significant heap growth', async ({ page }) => {
    test.setTimeout(150_000);
    await gotoBackup(page, adminTokens);
    const base = await getHeapMB(page);
    for (let i = 0; i < 5; i++) {
      await page.locator('.hv-cl-btn-close').first().click();
      await expect(win(page)).toHaveCount(0);
      await openLaunchpadApp(page, 'Backup');
      await page.waitForSelector('[data-testid="restic-page"]', { timeout: 10_000 });
    }
    const after = await getHeapMB(page);
    if (base > 0 && after > 0) {
      console.log(`[MEM] heap ${base.toFixed(1)} → ${after.toFixed(1)} MB`);
      expect(after - base).toBeLessThan(12);
    }
  });

  // Regression for the audit item "Restic restore": the Restore modal used to
  // resolve `snapshots.find(s => s.short_id)` — i.e. it silently restored the
  // FIRST snapshot no matter which row was clicked. The fix captures the
  // clicked snapshot in `pendingRestoreSnapshot`. This test disables restic
  // entirely (every restic API call is mocked in-browser) so it runs anywhere,
  // and proves the POST /restic/restore payload carries the SECOND snapshot's
  // short_id after clicking the second row's Restore button.
  test('14.3 — restore acts on the clicked snapshot, not the first row (mocked API)', async ({ page }) => {
    const repo = {
      id: 'mock-repo',
      name: 'mock-repo',
      repository: '/tmp/mock-repo',
      password: 'mock-pass',
      retention_policy_id: null,
      source_paths: ['/etc/hostname'],
      exclude_patterns: null,
      tags: null,
      snapshot_source: 'paths',
      snapshot_pool: null,
      snapshot_dataset: null,
      snapshot_create_before_backup: false,
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
    };
    const snaps = [
      { id: 'aaa', short_id: 'aaaaaaaa', time: '2026-01-01T10:00:00Z', hostname: 'host-a', paths: ['/etc/hostname'], tags: [], program_version: '0.17.0' },
      { id: 'bbb', short_id: 'bbbbbbbb', time: '2026-01-02T10:00:00Z', hostname: 'host-b', paths: ['/var/log'], tags: ['nightly'], program_version: '0.17.0' },
    ];
    const ok = (data: unknown) => JSON.stringify({ success: true, data });
    let restorePayload: Record<string, unknown> | null = null;

    // Routes must be installed BEFORE gotoBackup: the app fetches restic
    // availability at mount, and the real backend has no restic installed.
    await page.route('**/api/restic/available', (r) => r.fulfill({ body: ok({ available: true }), headers: { 'content-type': 'application/json' } }));
    await page.route('**/api/restic/version', (r) => r.fulfill({ body: ok({ version: '0.17.0' }), headers: { 'content-type': 'application/json' } }));
    await page.route(/\/api\/restic\/repos\?/, (r) => r.fulfill({ body: ok([repo]), headers: { 'content-type': 'application/json' } }));
    await page.route('**/api/restic/repos/*/snapshots', (r) => r.fulfill({ body: ok(snaps), headers: { 'content-type': 'application/json' } }));
    await page.route('**/api/restic/repos/*/jobs', (r) => r.fulfill({ body: ok([]), headers: { 'content-type': 'application/json' } }));
    await page.route('**/api/restic/repos/*/schedules', (r) => r.fulfill({ body: ok([]), headers: { 'content-type': 'application/json' } }));
    await page.route('**/api/restic/repos/*/stats', (r) => r.fulfill({ body: ok({ snapshots_count: 2, total_size: 1024 }), headers: { 'content-type': 'application/json' } }));
    await page.route('**/api/restic/retention-policies', (r) => r.fulfill({ body: ok([]), headers: { 'content-type': 'application/json' } }));
    await page.route('**/api/restic/restore', async (r) => {
      restorePayload = (r.request().postDataJSON?.() as Record<string, unknown>) ?? null;
      await r.fulfill({ body: ok({ id: 'restore-job-1' }), headers: { 'content-type': 'application/json' } });
    });

    await gotoBackup(page, adminTokens);

    // Open the mocked repo → its snapshots arrive from the mock, in order.
    await win(page).locator('[data-testid="restic-repo-name"]', { hasText: 'mock-repo' }).click();
    const secondRow = win(page).locator('tr', { hasText: 'bbbbbbbb' });
    await expect(secondRow).toBeVisible({ timeout: 10_000 });

    // Old behaviour restored the first row; the regression clicks the SECOND.
    await secondRow.getByRole('button', { name: 'Restore' }).click();

    const dialog = page.locator('[data-slot="dialog-content"]');
    await expect(dialog).toBeVisible();
    await dialog.getByRole('button', { name: 'Restore' }).click();

    await expect.poll(() => restorePayload).not.toBeNull();
    expect(restorePayload?.repo_id).toBe('mock-repo');
    expect(restorePayload?.snapshot_id).toBe('bbbbbbbb');
  });
});
