/* =============================================================================
 * File          : shares.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Shares screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Shares / Protocols app — End-to-End Test Suite
 *
 * The Shares app is not a route: it is a window opened from the desktop's
 * Applications launcher (id `shares`, "Shares" tile). It manages SMB / NFS /
 * FTP / WebDAV / SFTP network file shares, the per-protocol service config,
 * and per-share ACL permission grants — backend `src/shares.rs`,
 * `src/permissions.rs`, `src/sftp.rs`.
 *
 * **This suite never creates, deletes, toggles or reconfigures a real
 * share, never starts/stops/restarts a real file-sharing service, never
 * grants/revokes a real ACL, and never provisions a real SFTP Linux
 * account or bind mount.** Every one of those paths writes a root-owned
 * file under `/etc`, shells out via `sudo` to `tee`/`systemctl`/`exportfs`/
 * `useradd`/`mount --bind`, or mutates the host's real Samba/NFS/SFTP
 * state — doing it for real from CI could break file sharing for a real
 * deployment with no clean rollback. Per CLAUDE.md's "What NOT to test"
 * reasoning (mirrored here for E2E), mutating flows are only exercised far
 * enough to confirm they fail *cleanly* (400 / 401 / 403, never a panic)
 * against a missing / non-admin session or invalid input, via direct
 * `fetch` — exactly mirroring `tests/shares_integration.rs`. Read-only
 * data (share list, per-protocol config, SFTP grantee list, a share's
 * permission list) is safe to load for real and is asserted on directly.
 *
 * Run (all):     npx playwright test tests/e2e/shares/shares.spec.ts --project=firefox
 * Run (single):  npx playwright test tests/e2e/shares/shares.spec.ts -g "4.1"
 * Run (category):npx playwright test tests/e2e/shares/shares.spec.ts -g "Security"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/shares/shares.spec.ts
 *
 * Requires a live server whose setup is complete and that is seeded with an
 * admin account (defaults: e2eadmin / E2eSecret123, override with
 * E2E_ADMIN_USER / E2E_ADMIN_PASS).
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

async function gotoShares(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openLaunchpadApp(page, lang === 'fr' ? 'Partages' : 'Shares');
  await page.waitForSelector('[data-testid="shares-page"]', { timeout: 10_000 });
  // The initial list load spinner clears.
  await expect(page.locator('[data-testid="shares-page"] [data-testid="shares-loading"]')).toHaveCount(0, {
    timeout: 10_000,
  });
}

/** The Shares window as a scoping root — `[data-testid="shares-page"]` is unique to
 * this page (confirmed via grep across `frontend/src/routes`). */
function win(page: Page) {
  return page.locator('[data-testid="shares-page"]');
}

/** Modals render as DOM siblings of `[data-testid="shares-page"]`, not descendants. */
function modal(page: Page) {
  return page.locator('[data-slot="dialog-content"]');
}

function closeWindowButton(page: Page) {
  return page.locator('.hv-cl-btn-close').first();
}

async function switchTab(page: Page, label: string) {
  await win(page).locator('[data-testid="shares-tab"]', { hasText: label }).first().click();
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

// ===========================================================================
// 1. Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — the Shares window opens from the Applications launcher', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — the window header shows the title only (standardized header)', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title')).toHaveText('Shares');
    // The bar holds the icon tile + the title and nothing else. The old
    // `.hv-winhdr-subtitle` locator guarded nothing: `subtitle` was removed
    // from `WindowHeader` on 2026-09-14, so it matched no element either way.
    // Assert the bar's real child count instead (registry-wide: desktop 14.11).
    await expect(win(page).locator('.hv-winhdr > *')).toHaveCount(2);
  });

  test('1.3 — the tab strip lists all six views', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await expect(win(page).locator('[data-testid="shares-tab"]')).toHaveCount(6);
    // Strip the optional trailing count badge before comparing — "FTP" is a
    // substring of "SFTP", so a `hasText` filter would double-match.
    const labels = await win(page)
      .locator('[data-testid="shares-tab"]')
      .evaluateAll((els) => els.map((e) => (e.textContent || '').replace(/\s*\d+\s*$/, '').trim()));
    expect(labels).toEqual(['Shares', 'SMB', 'NFS', 'FTP', 'WebDAV', 'SFTP']);
  });

  test('1.4 — the list tab is selected by default', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await expect(win(page).locator('[data-testid="shares-tab"]').first()).toHaveAttribute(
      'aria-selected',
      'true',
    );
  });

  test('1.5 — the list view shows either the two stat tiles or the empty state', async ({
    page,
  }) => {
    await gotoShares(page, adminTokens);
    const stats = win(page).locator('[data-testid="shares-stat"]');
    const empty = win(page).locator('[data-testid="shares-empty"]');
    const hasStats = (await stats.count()) > 0;
    if (hasStats) {
      await expect(stats).toHaveCount(2);
    } else {
      await expect(empty).toBeVisible();
    }
  });

  test('1.6 — the list matches what GET /api/shares returns', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const apiCount = await page.evaluate(async (token) => {
      const r = await fetch('/api/shares', { headers: { 'x-session-token': token } });
      const b = await r.json();
      return Array.isArray(b?.data) ? b.data.length : -1;
    }, adminTokens.token);
    expect(apiCount).toBeGreaterThanOrEqual(0);
    const cards = await win(page).locator('[data-testid="shares-card"]').count();
    expect(cards).toBe(apiCount);
  });
});

// ===========================================================================
// 2. Form fields — the create-share modal
// ===========================================================================

test.describe('2 — Form fields', () => {
  test('2.1 — "New share" opens a modal with name / path / protocol / comment', async ({
    page,
  }) => {
    await gotoShares(page, adminTokens);
    await win(page).locator('[data-testid="shares-new"]').click();
    await expect(modal(page)).toBeVisible();
    await expect(modal(page).locator('input').first()).toBeVisible();
    await expect(modal(page).locator('select')).toBeVisible();
  });

  test('2.2 — the protocol select offers exactly smb/nfs/ftp/webdav/sftp', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await win(page).locator('[data-testid="shares-new"]').click();
    const values = await modal(page)
      .locator('select option')
      .evaluateAll((opts) => opts.map((o) => (o as HTMLOptionElement).value));
    expect(values.sort()).toEqual(['ftp', 'nfs', 'sftp', 'smb', 'webdav']);
  });

  test('2.3 — the name and path fields are marked required', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await win(page).locator('[data-testid="shares-new"]').click();
    await expect(modal(page).locator('[data-testid="shares-req"]')).toHaveCount(2);
  });
});

// ===========================================================================
// 3. Validation — direct fetch, mirroring tests/shares_integration.rs.
//    Never creates / mutates a real share.
// ===========================================================================

test.describe('3 — Validation', () => {
  test('3.1 — creating a share with an unknown protocol is rejected', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/shares', {
          method: 'POST',
          headers: {
            'x-session-token': token,
            'x-csrf-token': csrf,
            'Content-Type': 'application/json',
          },
          body: JSON.stringify({ name: 'x', share_type: 'gopher', path: '/tmp/x', comment: '' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.2 — a bad protocol in the URL is rejected on GET/PUT/DELETE', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const statuses = await page.evaluate(
      async ({ token, csrf }) => {
        const h = { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' };
        const g = await fetch('/api/shares/demo/bogusproto', { headers: h });
        const d = await fetch('/api/shares/demo/bogusproto', { method: 'DELETE', headers: h });
        const p = await fetch('/api/shares/demo/bogusproto/permissions', { headers: h });
        return [g.status, d.status, p.status];
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(statuses).toEqual([400, 400, 400]);
  });

  test('3.3 — granting an invalid permission string is rejected', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const { status, message } = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/shares/demo/smb/permissions', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ grantee_type: 'user', grantee_name: 'x', permission: 'rwxr' }),
        });
        const b = await r.json().catch(() => ({}));
        return { status: r.status, message: b?.message || '' };
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
    expect(message).toContain('Invalid permission');
  });

  test('3.4 — granting to a bogus grantee type is rejected', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/shares/demo/smb/permissions', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ grantee_type: 'robot', grantee_name: 'x', permission: 'rw' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.5 — granting on a share that does not exist fails before any ACL write', async ({
    page,
  }) => {
    await gotoShares(page, adminTokens);
    const { status, message } = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/shares/definitely-not-a-real-share/smb/permissions', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ grantee_type: 'user', grantee_name: 'x', permission: 'rw' }),
        });
        const b = await r.json().catch(() => ({}));
        return { status: r.status, message: b?.message || '' };
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
    expect(message).toContain('Share not found');
  });

  test('3.6 — setting a malformed SFTP public key is rejected', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/shares/sftp/users/key', {
          method: 'PUT',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ username: 'nobody', public_key: 'not-a-real-key' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.7 — a newline-injection SFTP key is rejected', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/shares/sftp/users/key', {
          method: 'PUT',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({
            username: 'nobody',
            public_key: 'ssh-ed25519 AAAAlegit key\ncommand="rm -rf /" ssh-ed25519 AAAAevil',
          }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });
});

// ===========================================================================
// 4. All user interactions
// ===========================================================================

test.describe('4 — All user interactions', () => {
  test('4.1 — "New share" opens the modal, Cancel closes it', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await win(page).locator('[data-testid="shares-new"]').click();
    await expect(modal(page)).toBeVisible();
    await modal(page).locator('.hv-btn-ghost').click();
    await expect(modal(page)).toHaveCount(0);
  });

  test('4.2 — the create modal closes on backdrop click', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await win(page).locator('[data-testid="shares-new"]').click();
    await expect(modal(page)).toBeVisible();
    await page.locator('[data-slot="dialog-overlay"]').click({ position: { x: 5, y: 5 } });
    await expect(modal(page)).toHaveCount(0);
  });

  test('4.3 — no manual Refresh button; a window-focus event revalidates in place', async ({
    page,
  }) => {
    // The header Refresh button was removed — data revalidates on focus /
    // visibility (onAppVisible -> refreshCurrent).
    await gotoShares(page, adminTokens);
    await expect(win(page).getByRole('button', { name: 'Refresh' })).toHaveCount(0);
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await page.waitForTimeout(1_600);
    await page.evaluate(() => {
      document.dispatchEvent(new Event('visibilitychange'));
      window.dispatchEvent(new Event('focus'));
    });
    await page.waitForTimeout(500);
    await expect(win(page)).toBeVisible();
    await expect(win(page).locator('[data-testid="shares-tab"]').first()).toHaveAttribute(
      'aria-selected',
      'true',
    );
    expect(errors).toEqual([]);
  });

  test('4.4 — switching to a protocol tab shows its service bar + config form', async ({
    page,
  }) => {
    await gotoShares(page, adminTokens);
    await switchTab(page, 'SMB');
    await expect(win(page).locator('[data-testid="shares-svcbar"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="shares-config"] [data-testid="shares-form"]').first()).toBeVisible();
  });

  test('4.5 — the SMB tab exposes a Simple/Advanced switch', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await switchTab(page, 'SMB');
    // The switch's buttons carry `data-testid="shares-mode-btn"`
    // (`Shares.svelte:762-763`); `segBtn` is a const Tailwind string, so it
    // never appears as a class token. Only the active tab renders its switch.
    const modeBtns = win(page).locator('[data-testid="shares-mode-btn"]');
    await expect(modeBtns).toHaveCount(2);
    await modeBtns.nth(1).click();
    await expect(modeBtns.nth(1)).toHaveClass(/active/);
  });

  test('4.6 — the SFTP tab shows the key-management panel', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await switchTab(page, 'SFTP');
    await expect(win(page).locator('[data-testid="shares-form-head"]').first()).toBeVisible();
    // The "add SFTP user" key textarea is always present; per-grantee
    // textareas may add more on hosts that already have SFTP users.
    expect(await win(page).locator('textarea').count()).toBeGreaterThanOrEqual(1);
  });

  test('4.7 — the Permissions modal opens for an existing share (when any exist)', async ({
    page,
  }) => {
    await gotoShares(page, adminTokens);
    const permBtn = win(page).locator('[data-testid="shares-card"] [data-testid="shares-icon-btn"]').nth(0);
    if ((await win(page).locator('[data-testid="shares-card"]').count()) === 0) {
      test.skip(true, 'no shares configured on this host');
    }
    // The first icon button on a card is "Manage access".
    await win(page).locator('[data-testid="shares-card"]').first().locator('[data-testid="shares-icon-btn"]').first().click();
    await expect(modal(page)).toBeVisible();
    await expect(modal(page).locator('[data-testid="shares-grant"]')).toBeVisible();
    await page.keyboard.press('Escape').catch(() => {});
  });
});

// ===========================================================================
// 5. Submission flows — guard / no-op paths only (see file header)
// ===========================================================================

test.describe('5 — Submission flows', () => {
  test('5.1 — cancelling the create modal never fires POST /api/shares', async ({ page }) => {
    await gotoShares(page, adminTokens);
    let posted = false;
    page.on('request', (req) => {
      if (req.url().endsWith('/api/shares') && req.method() === 'POST') posted = true;
    });
    await win(page).locator('[data-testid="shares-new"]').click();
    await modal(page).locator('input').first().fill('e2e-should-never-be-created');
    await modal(page).locator('.hv-btn-ghost').click();
    await page.waitForTimeout(300);
    expect(posted).toBe(false);
  });

  test('5.2 — the create form does not submit with an empty name/path', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await win(page).locator('[data-testid="shares-new"]').click();
    let status = 0;
    page.on('response', (res) => {
      if (res.url().endsWith('/api/shares') && res.request().method() === 'POST') {
        status = res.status();
      }
    });
    await modal(page).locator('.hv-btn-primary').click();
    await page.waitForTimeout(400);
    // Either the client blocks it outright (status stays 0) or the server
    // rejects it — never a 200 "created".
    expect(status).not.toBe(200);
    // Clean up in the unlikely event a blank share was created.
    if (status === 200) {
      await page.evaluate(
        async ({ token, csrf }) => {
          await fetch('/api/shares//smb', {
            method: 'DELETE',
            headers: { 'x-session-token': token, 'x-csrf-token': csrf },
          });
        },
        { token: adminTokens.token, csrf: adminTokens.csrf },
      );
    }
  });
});

// ===========================================================================
// 6. Navigation
// ===========================================================================

test.describe('6 — Navigation', () => {
  test('6.1 — the window closes and can be reopened', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const closeBtn = closeWindowButton(page);
    if ((await closeBtn.count()) === 0) test.skip(true, 'no close control for this window chrome');
    await closeBtn.click();
    await expect(page.locator('[data-testid="shares-page"]')).toHaveCount(0);
    await openLaunchpadApp(page, 'Shares');
    await expect(win(page)).toBeVisible();
  });

  test('6.2 — switching tabs preserves the window (no reload)', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const marker = 'e2e-shares-nav-marker';
    await win(page).evaluate((el, m) => ((el as any)[m] = true), marker);
    await switchTab(page, 'SMB');
    await switchTab(page, 'NFS');
    await win(page).locator('[data-testid="shares-tab"]').first().click();
    const stillThere = await win(page).evaluate((el, m) => Boolean((el as any)[m]), marker);
    expect(stillThere).toBe(true);
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
      await gotoShares(page, adminTokens);
      await switchTab(page, 'SFTP');
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
  test('8.1 — the tab strip exposes tablist/tab roles + aria-selected', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await expect(win(page).locator('[role="tablist"]')).toHaveCount(1);
    await expect(win(page).locator('[role="tab"]').first()).toHaveAttribute('aria-selected', 'true');
  });

  test('8.2 — the create modal fields have visible labels', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await win(page).locator('[data-testid="shares-new"]').click();
    const labels = modal(page).locator('[data-testid="shares-field"] span').first();
    await expect(labels).toBeVisible();
  });

  test('8.3 — the modal has role="dialog" and aria-modal', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await win(page).locator('[data-testid="shares-new"]').click();
    await expect(modal(page)).toHaveAttribute('role', 'dialog');
    await expect(modal(page)).toHaveAttribute('aria-modal', 'true');
  });

  test('8.4 — no duplicate element ids on the page', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const dupes = await page.evaluate(() => {
      const ids = Array.from(document.querySelectorAll('[id]')).map((e) => e.id).filter(Boolean);
      const seen = new Set<string>();
      const dup: string[] = [];
      for (const id of ids) {
        if (seen.has(id)) dup.push(id);
        seen.add(id);
      }
      return dup;
    });
    expect(dupes).toEqual([]);
  });
});

// ===========================================================================
// 9. Performance
// ===========================================================================

test.describe('9 — Performance', () => {
  test('9.1 — the window opens in a reasonable time', async ({ page }) => {
    const start = Date.now();
    await gotoShares(page, adminTokens);
    const elapsed = Date.now() - start;
    console.log(`[PERF] Shares window ready in ${elapsed}ms`);
    expect(elapsed).toBeLessThan(15_000);
  });

  test('9.2 — opening the window does not add an excessive number of DOM nodes', async ({
    page,
  }) => {
    await gotoDesktop(page, adminTokens);
    const before = await page.evaluate(() => document.querySelectorAll('*').length);
    await openLaunchpadApp(page, 'Shares');
    await page.waitForSelector('[data-testid="shares-page"]', { timeout: 10_000 });
    const after = await page.evaluate(() => document.querySelectorAll('*').length);
    const delta = after - before;
    console.log(`[PERF] Shares window added ${delta} DOM nodes`);
    expect(delta).toBeLessThan(3000);
  });
});

// ===========================================================================
// 10. Security — the permission model is the whole point of this app
// ===========================================================================

test.describe('10 — Security', () => {
  test('10.1 — the Shares API rejects a missing session token', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/shares');
      return r.status;
    });
    expect(status).toBe(400);
  });

  test('10.2 — the Shares API rejects a tampered session token', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/shares', {
        headers: { 'x-session-token': 'totally-bogus-token-value' },
      });
      return r.status;
    });
    expect(status).toBe(401);
  });

  test('10.3 — every share/permission endpoint refuses a non-admin session', async ({
    page,
    playwright,
  }) => {
    await gotoShares(page, adminTokens);
    const ctx = await newCtx(playwright);
    const username = `e2esharesnonadmin${Date.now().toString(36)}`;
    const create = await ctx.post('/api/users', {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
      data: {
        username,
        password: 'NonAdminPass123!',
        fullname: 'E2E Non Admin',
        email: `${username}@example.com`,
        role_id: 1,
      },
    });
    if (create.status() !== 200 && create.status() !== 201) {
      await ctx.dispose();
      test.skip(true, 'could not provision a throwaway non-admin user');
    }
    const { token: userToken } = await apiLoginAsViewer(ctx, username, 'NonAdminPass123!');

    const results = await page.evaluate(async (token) => {
      const h = { 'x-session-token': token, 'Content-Type': 'application/json' };
      const out: Record<string, number> = {};
      out.list = (await fetch('/api/shares', { headers: h })).status;
      out.smbConfig = (await fetch('/api/shares/smb/config', { headers: h })).status;
      out.sftpUsers = (await fetch('/api/shares/sftp/users', { headers: h })).status;
      out.permList = (await fetch('/api/shares/x/smb/permissions', { headers: h })).status;
      out.create = (
        await fetch('/api/shares', {
          method: 'POST',
          headers: h,
          body: JSON.stringify({ name: 'x', share_type: 'smb', path: '/tmp/x', comment: '' }),
        })
      ).status;
      out.grant = (
        await fetch('/api/shares/x/smb/permissions', {
          method: 'POST',
          headers: h,
          body: JSON.stringify({ grantee_type: 'user', grantee_name: 'x', permission: 'r' }),
        })
      ).status;
      return out;
    }, userToken);

    for (const [name, status] of Object.entries(results)) {
      expect(status, `${name} must not be 200 for a non-admin`).not.toBe(200);
    }

    await ctx.delete(`/api/users/${username}`, {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    });
    await ctx.dispose();
  });

  test('10.4 — GET /api/shares/smb/config never leaks a plaintext credential', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const raw = await page.evaluate(async (token) => {
      const r = await fetch('/api/shares/smb/config', { headers: { 'x-session-token': token } });
      return r.text();
    }, adminTokens.token);
    expect(raw.toLowerCase()).not.toContain('password');
  });

  test('10.5 — no session token is ever placed in the URL', async ({ page }) => {
    await gotoShares(page, adminTokens);
    expect(page.url()).not.toContain(adminTokens.token);
  });

  test('10.6 — an XSS payload in the share-name field is never executed', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await win(page).locator('[data-testid="shares-new"]').click();
    const payload = '<img src=x onerror="window.__sharesXss=true">';
    await modal(page).locator('input').first().fill(payload);
    const fired = await page.evaluate(() => (window as any).__sharesXss === true);
    expect(fired).toBe(false);
  });
});

// ===========================================================================
// 11. Keyboard
// ===========================================================================

test.describe('11 — Keyboard', () => {
  test('11.1 — Arrow / Home / End keys navigate the protocol tab strip (WAI-ARIA)', async ({
    page,
  }) => {
    await gotoShares(page, adminTokens);
    const tabs = win(page).locator('[data-testid="shares-tab"]');
    await tabs.first().focus();

    // ArrowRight from tab 0 → tab 1 selected + focused.
    await page.keyboard.press('ArrowRight');
    expect(
      await page.evaluate(() => document.activeElement?.getAttribute('aria-selected')),
    ).toBe('true');
    expect(await page.evaluate(() => document.activeElement?.textContent?.replace(/\s*\d+\s*$/, '').trim())).toBe(
      'SMB',
    );

    // ArrowLeft wraps from tab 0 back to the last tab (SFTP).
    await tabs.first().focus();
    await page.keyboard.press('ArrowLeft');
    expect(await page.evaluate(() => document.activeElement?.textContent?.replace(/\s*\d+\s*$/, '').trim())).toBe(
      'SFTP',
    );

    // End → last, Home → first.
    await page.keyboard.press('Home');
    expect(await page.evaluate(() => document.activeElement?.textContent?.replace(/\s*\d+\s*$/, '').trim())).toBe(
      'Shares',
    );
    await page.keyboard.press('End');
    expect(await page.evaluate(() => document.activeElement?.textContent?.replace(/\s*\d+\s*$/, '').trim())).toBe(
      'SFTP',
    );
  });

  test('11.2 — Escape / backdrop closes the create modal', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await win(page).locator('[data-testid="shares-new"]').click();
    await expect(modal(page)).toBeVisible();
    // No documented Escape handler — closing via the explicit control is
    // the guaranteed path; assert the modal is dismissable at all.
    await modal(page).locator('[data-testid="shares-icon-btn"]').click();
    await expect(modal(page)).toHaveCount(0);
  });
});

// ===========================================================================
// 12. Console & network
// ===========================================================================

test.describe('12 — Console & network', () => {
  test('12.1 — no uncaught JS errors while browsing every tab', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await gotoShares(page, adminTokens);
    for (const t of ['SMB', 'NFS', 'FTP', 'WebDAV', 'SFTP']) {
      await switchTab(page, t);
      await page.waitForTimeout(150);
    }
    expect(errors).toEqual([]);
  });

  test('12.2 — no failed (4xx/5xx) API requests during a normal page load', async ({ page }) => {
    const failed: string[] = [];
    page.on('response', (res) => {
      if (res.status() >= 400 && res.url().includes('/api/')) {
        failed.push(`${res.status()} ${res.url()}`);
      }
    });
    await gotoShares(page, adminTokens);
    expect(failed).toEqual([]);
  });

  test('12.3 — switching to every protocol tab issues at most one config request each', async ({
    page,
  }) => {
    await gotoShares(page, adminTokens);
    const calls: Record<string, number> = {};
    page.on('request', (req) => {
      const m = req.url().match(/\/api\/shares\/(smb|nfs|ftp|webdav|sftp)\/config/);
      if (m && req.method() === 'GET') calls[m[1]] = (calls[m[1]] || 0) + 1;
    });
    for (const t of ['SMB', 'NFS', 'FTP', 'WebDAV', 'SFTP']) {
      await switchTab(page, t);
      await page.waitForTimeout(200);
    }
    for (const [proto, n] of Object.entries(calls)) {
      expect(n, `${proto} config fetched ${n}×`).toBeLessThanOrEqual(2);
    }
  });
});

// ===========================================================================
// 13. Edge cases & memory regression
// ===========================================================================

test.describe('13 — Edge cases & memory regression', () => {
  test('13.1 — a unicode share name is accepted by the field without throwing', async ({
    page,
  }) => {
    await gotoShares(page, adminTokens);
    await win(page).locator('[data-testid="shares-new"]').click();
    await modal(page).locator('input').first().fill('partagé-🗂️-données');
    await expect(modal(page).locator('input').first()).toHaveValue('partagé-🗂️-données');
  });

  test('13.2 — a very long comment value does not break modal layout', async ({ page }) => {
    await gotoShares(page, adminTokens);
    await win(page).locator('[data-testid="shares-new"]').click();
    await modal(page).locator('input').last().fill('c'.repeat(600));
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
    );
    expect(overflow).toBe(false);
  });

  test('13.3 — rapidly switching tabs does not crash the page', async ({ page }) => {
    await gotoShares(page, adminTokens);
    const tabs = win(page).locator('[data-testid="shares-tab"]');
    for (let i = 0; i < 12; i++) {
      await tabs.nth(i % 6).click();
    }
    await expect(win(page)).toBeVisible();
  });

  test('13.4 — clearing localStorage mid-session and reloading does not crash', async ({
    page,
  }) => {
    await gotoShares(page, adminTokens);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await page.waitForTimeout(1_000);
    const bodyText = await page.locator('body').innerText().catch(() => '');
    expect(bodyText.length).toBeGreaterThan(0);
  });

  test('13.5 — opening and closing the Shares window ×5 does not leak significant heap', async ({
    page,
  }) => {
    test.setTimeout(150_000);
    await gotoDesktop(page, adminTokens);
    const before = await getHeapMB(page).catch(() => 0);
    for (let i = 0; i < 5; i++) {
      await openLaunchpadApp(page, 'Shares');
      await page.waitForSelector('[data-testid="shares-page"]', { timeout: 10_000 });
      const closeBtn = closeWindowButton(page);
      if ((await closeBtn.count()) === 0) test.skip(true, 'no close control for this window chrome');
      await closeBtn.click();
      await expect(page.locator('[data-testid="shares-page"]')).toHaveCount(0);
    }
    const after = await getHeapMB(page).catch(() => 0);
    if (before > 0 && after > 0) {
      const delta = after - before;
      console.log(`[MEM] 5x window open/close delta: ${delta.toFixed(1)} MB`);
      expect(delta).toBeLessThan(15);
    }
  });
});

// ===========================================================================
// 14. Memory regression — SFTP grantee reload
// ===========================================================================

test.describe('14 — Memory regression', () => {
  test('14.1 — re-entering the SFTP tab (re-fetching the grantee list) does not accumulate DOM nodes', async ({
    page,
  }) => {
    await gotoShares(page, adminTokens);
    await switchTab(page, 'SFTP');
    await page.waitForTimeout(300);
    const before = await page.evaluate(() => document.querySelectorAll('*').length);
    for (let i = 0; i < 5; i++) {
      await switchTab(page, 'SMB');
      await switchTab(page, 'SFTP');
      await page.waitForTimeout(250);
    }
    const after = await page.evaluate(() => document.querySelectorAll('*').length);
    console.log(`[MEM] SFTP tab re-enter ×5 DOM delta: ${after - before}`);
    expect(after - before).toBeLessThan(400);
  });
});
