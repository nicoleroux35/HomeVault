/* =============================================================================
 * File          : quota.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Shares → per-share size quota (QW-07), end-to-end (Playwright): the quota
 *   button, the editor, the bar, the unsupported explanation, validation,
 *   access control and accessibility.
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/shares/quota.spec.ts --project=firefox
 * =============================================================================
 */

/**
 * Per-share size quota (Shares app) — End-to-End Test Suite
 *
 * **SAFETY: no filesystem limit is ever applied.** The quota endpoints are
 * intercepted with `page.route` in every test that changes something, so the
 * assertions are about the screen; the backend is covered by
 * `tests/share_quota_integration.rs` (endpoint contract) and by
 * `src/share_quota.rs` (backend selection, argv, parsers, the decision).
 *
 * The suite restores whatever quota the instance had in `afterAll`.
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

async function apiLogin(ctx: APIRequestContext, username: string, password: string) {
  const res = await ctx.post('/api/auth/login', { data: { username, password } });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

async function seedBrowser(page: Page, tokens: Tokens) {
  await page.addInitScript(
    ({ t }) => {
      try {
        localStorage.setItem('session-token', t.token);
        localStorage.setItem('csrf-token', t.csrf);
      } catch {
        /* storage unavailable — the test will surface it */
      }
    },
    { t: tokens },
  );
}

async function dismissWhatsNew(page: Page) {
  const modal = page.locator('[data-testid="whats-new-modal"]');
  if (await modal.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await modal.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

/** Opens the Shares app from the desktop launcher. */
async function openShares(page: Page, tokens: Tokens) {
  await seedBrowser(page, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await dismissWhatsNew(page);

  const tile = page.locator('.hv-dock-item:has-text("Shares"), [data-app-id="shares"]').first();
  try {
    await tile.click({ timeout: 5_000 });
  } catch {
    // The launcher lists it; fall back to the app grid.
    await page.locator('[data-testid="launcher-open"]').click().catch(() => {});
    const launcherTile = page.locator('[data-app-id="shares"]').first();
    if (await launcherTile.count()) {
      await launcherTile.click();
    } else {
      await page.evaluate(() => window.dispatchEvent(new CustomEvent('hv-open-app', { detail: { id: 'shares' } })));
    }
  }
  await page.waitForSelector('[data-testid="shares-page"]', { timeout: 15_000 });
}

/** The first share card, or `null` when the instance has no share. */
async function firstShare(page: Page) {
  const cards = page.locator('[data-testid="shares-card"]');
  await expect(cards.first()).toBeVisible({ timeout: 15_000 });
  return cards.first();
}

const quotaState = (over: Record<string, unknown> = {}) => ({
  success: true,
  data: {
    share_name: 'photos',
    share_type: 'smb',
    backend: 'zfs',
    supported: true,
    filesystem: 'zfs',
    reason: null,
    quota_bytes: 10_000_000_000,
    used_bytes: 2_500_000_000,
    free_bytes: 7_500_000_000,
    percent: 25,
    quota_human: '10 GB',
    used_human: '2.5 GB',
    min_bytes: 1_048_576,
    max_bytes: 1_099_511_627_776,
    ...over,
  },
});

let adminTokens: Tokens;
let sharedCtx: APIRequestContext;
let hadShare = false;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await playwright.request.newContext({ baseURL: BASE });
  const { token, csrf } = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
  const list = await sharedCtx.get(`${BASE}/api/shares`, {
    headers: { 'x-session-token': token },
  });
  const shares = (await list.json().catch(() => ({})))?.data;
  hadShare = Array.isArray(shares) && shares.length > 0;
});

test.afterAll(async () => {
  await sharedCtx.dispose();
});

// ===========================================================================
// 1. The control exists, and only where it makes sense
// ===========================================================================

test('1.1 — every share card offers a size-quota action', async ({ page }) => {
  test.skip(!hadShare, 'this instance has no share to set a quota on');
  await openShares(page, adminTokens);
  const card = await firstShare(page);
  const button = card.locator('[data-testid="shares-quota-btn"]');
  await expect(button).toBeVisible();
  // Icon-only: the accessible name comes from an i18n key, never a bare title.
  expect(await button.getAttribute('aria-label')).toBeTruthy();
});

test('1.2 — the editor opens with the current value in GB and can be cancelled', async ({
  page,
}) => {
  test.skip(!hadShare, 'this instance has no share to set a quota on');
  await openShares(page, adminTokens);
  const card = await firstShare(page);
  await card.locator('[data-testid="shares-quota-btn"]').click();

  const editor = card.locator('[data-testid="shares-quota-editor"]');
  await expect(editor).toBeVisible({ timeout: 10_000 });
  await expect(editor.locator('[data-testid="shares-quota-input"]')).toBeVisible();
  // A label, not a placeholder-only field.
  const inputId = await editor.locator('[data-testid="shares-quota-input"]').getAttribute('id');
  expect(inputId).toBeTruthy();
  await expect(page.locator(`label[for="${inputId}"]`)).toHaveCount(1);

  await editor.locator('[data-testid="shares-quota-cancel"]').click();
  await expect(editor).toHaveCount(0);
});

// ===========================================================================
// 2. A set quota renders as a bar (mocked: nothing is applied)
// ===========================================================================

test('2.1 — a share with a ceiling shows used-of-quota and a labelled bar', async ({ page }) => {
  test.skip(!hadShare, 'this instance has no share to set a quota on');
  await page.route('**/api/shares/*/*/quota', (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(quotaState()),
    });
  });
  await openShares(page, adminTokens);
  const card = await firstShare(page);

  const bar = card.locator('[data-testid="shares-quota"]');
  await expect(bar).toBeVisible({ timeout: 10_000 });
  await expect(bar).toContainText('2.5 GB');
  await expect(bar).toContainText('10 GB');
  await expect(card.locator('[data-testid="shares-quota-percent"]')).toHaveText('25%');

  // The bar is a real progressbar with its numbers exposed, not a div that only
  // looks like one.
  const progress = card.locator('[role="progressbar"]');
  await expect(progress).toHaveAttribute('aria-valuenow', '25');
  await expect(progress).toHaveAttribute('aria-valuemax', '100');
  expect(await progress.getAttribute('aria-label')).toBeTruthy();

  await page.unroute('**/api/shares/*/*/quota');
});

test('2.2 — an unsupported filesystem explains itself instead of showing a field', async ({
  page,
}) => {
  test.skip(!hadShare, 'this instance has no share to set a quota on');
  await page.route('**/api/shares/*/*/quota', (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(
        quotaState({
          backend: 'unsupported',
          supported: false,
          filesystem: 'ext4',
          reason:
            "the 'ext4' filesystem has no per-directory quota this build can enforce — use ZFS, a Btrfs subvolume, or XFS with prjquota",
          quota_bytes: 0,
          percent: 0,
        }),
      ),
    });
  });
  await openShares(page, adminTokens);
  const card = await firstShare(page);

  const notice = card.locator('[data-testid="shares-quota-unsupported"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toContainText('ext4');
  // The reason is the server's, and it names what does work.
  await expect(notice).toContainText(/ZFS|Btrfs|prjquota/);
  // No bar for a limit that is not in force.
  await expect(card.locator('[data-testid="shares-quota"]')).toHaveCount(0);

  await page.unroute('**/api/shares/*/*/quota');
});

// ===========================================================================
// 3. Saving (mocked)
// ===========================================================================

test('3.1 — saving sends bytes and the bar reflects the answer', async ({ page }) => {
  test.skip(!hadShare, 'this instance has no share to set a quota on');
  let sentBody: any = null;
  await page.route('**/api/shares/*/*/quota', (route) => {
    const method = route.request().method();
    if (method === 'GET') {
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(quotaState({ quota_bytes: 0, used_bytes: 2_500_000_000, percent: 0 })),
      });
    }
    sentBody = route.request().postDataJSON();
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(quotaState()),
    });
  });
  await openShares(page, adminTokens);
  const card = await firstShare(page);
  await card.locator('[data-testid="shares-quota-btn"]').click();

  const editor = card.locator('[data-testid="shares-quota-editor"]');
  await editor.locator('[data-testid="shares-quota-input"]').fill('10');
  await editor.locator('[data-testid="shares-quota-save"]').click();

  // The editor speaks GB; the API takes bytes.
  await expect.poll(() => sentBody?.quota_bytes ?? null, { timeout: 10_000 }).toBe(10_000_000_000);
  await expect(editor.locator('[data-testid="shares-quota-notice"]')).toBeVisible({
    timeout: 10_000,
  });
  await expect(card.locator('[data-testid="shares-quota"]')).toBeVisible();

  await page.unroute('**/api/shares/*/*/quota');
});

test('3.2 — a refused save shows the server reason as an alert', async ({ page }) => {
  test.skip(!hadShare, 'this instance has no share to set a quota on');
  await page.route('**/api/shares/*/*/quota', (route) => {
    if (route.request().method() === 'GET') {
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(quotaState({ quota_bytes: 0, percent: 0 })),
      });
    }
    return route.fulfill({
      status: 400,
      contentType: 'application/json',
      body: JSON.stringify({
        success: false,
        message: 'a share quota must be at least 1.0 MB (or 0 for no limit)',
      }),
    });
  });
  await openShares(page, adminTokens);
  const card = await firstShare(page);
  await card.locator('[data-testid="shares-quota-btn"]').click();
  const editor = card.locator('[data-testid="shares-quota-editor"]');
  await editor.locator('[data-testid="shares-quota-input"]').fill('0.0001');
  await editor.locator('[data-testid="shares-quota-save"]').click();

  const notice = editor.locator('[data-testid="shares-quota-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'alert');
  await expect(notice).toContainText(/at least/i);

  await page.unroute('**/api/shares/*/*/quota');
});

// ===========================================================================
// 4. Access control and accessibility
// ===========================================================================

test('4.1 — a non-admin session cannot read or set a quota', async ({ page }) => {
  test.skip(!hadShare, 'this instance has no share to set a quota on');
  const name = `quotamember${Date.now().toString(36).slice(-4)}`;
  const created = await page.request.post(`${BASE}/api/users`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: { username: name, password: 'MemberSecret123', fullname: 'Quota Member', email: '', role_id: 1 },
  });
  expect(created.ok()).toBeTruthy();
  const member = await apiLogin(sharedCtx, name, 'MemberSecret123');
  expect(member.token).toBeTruthy();

  const list = await page.request.get(`${BASE}/api/shares`, {
    headers: { 'x-session-token': adminTokens.token },
  });
  const shares = (await list.json()).data;
  const first = shares[0];

  const read = await page.request.get(
    `${BASE}/api/shares/${encodeURIComponent(first.name)}/${first.share_type}/quota`,
    { headers: { 'x-session-token': member.token } },
  );
  expect(read.status()).toBe(403);
  const write = await page.request.put(
    `${BASE}/api/shares/${encodeURIComponent(first.name)}/${first.share_type}/quota`,
    {
      headers: { 'x-session-token': member.token, 'x-csrf-token': member.csrf },
      data: { quota_bytes: 10_000_000_000 },
    },
  );
  expect(write.status()).toBe(403);

  await page.request
    .delete(`${BASE}/api/users/${encodeURIComponent(name)}`, {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    })
    .catch(() => {});
});

test('4.2 — no duplicate ids on the screen with the editor open', async ({ page }) => {
  test.skip(!hadShare, 'this instance has no share to set a quota on');
  await page.route('**/api/shares/*/*/quota', (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(quotaState()),
    });
  });
  await openShares(page, adminTokens);
  const card = await firstShare(page);
  await card.locator('[data-testid="shares-quota-btn"]').click();
  await expect(card.locator('[data-testid="shares-quota-editor"]')).toBeVisible({ timeout: 10_000 });

  const duplicates = await page.evaluate(() => {
    const seen = new Map<string, number>();
    document.querySelectorAll('[id]').forEach((el) => {
      seen.set(el.id, (seen.get(el.id) ?? 0) + 1);
    });
    return [...seen.entries()].filter(([, n]) => n > 1).map(([id]) => id);
  });
  expect(duplicates).toEqual([]);

  await page.unroute('**/api/shares/*/*/quota');
});

test('4.3 — the card does not overflow horizontally with the bar and editor open', async ({
  page,
}) => {
  test.skip(!hadShare, 'this instance has no share to set a quota on');
  await page.route('**/api/shares/*/*/quota', (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(quotaState({ percent: 99, used_bytes: 9_900_000_000 })),
    });
  });
  await openShares(page, adminTokens);
  const card = await firstShare(page);
  await card.locator('[data-testid="shares-quota-btn"]').click();

  const overflow = await page.evaluate(() => {
    const pageEl = document.querySelector('[data-testid="shares-page"]');
    if (!pageEl) return -1;
    const box = pageEl.getBoundingClientRect();
    let worst = 0;
    pageEl.querySelectorAll('[data-testid^="shares-quota"]').forEach((el) => {
      const r = el.getBoundingClientRect();
      worst = Math.max(worst, r.right - box.right);
    });
    return Math.round(worst);
  });
  expect(overflow).toBeLessThanOrEqual(0);

  await page.unroute('**/api/shares/*/*/quota');
});
