/* =============================================================================
 * File          : password-change.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 20/10/2026
 * Last modified : 20/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Forced first-sign-in password change end-to-end tests (Playwright):
 *   login routes a flagged (`must_change_password`) account to the change
 *   screen, the change flow validates, completes, the flag drops, and the
 *   security/keyboard/responsive contracts hold — per TESTING.md.
 *
 * Dependencies:
 *   - none (Playwright against a live HomeVault server)
 *
 * Usage:
 *   npx playwright test tests/e2e/password-change/password-change.spec.ts --project=firefox
 *   Requires a live server seeded with an admin account
 *   (defaults: e2eadmin / E2eSecret123 — E2E_ADMIN_USER / E2E_ADMIN_PASS).
 * =============================================================================
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

const TEMP_PASS = 'PwChangeTmp7X'; // admin-chosen temporary password (policy-compliant)
const NEW_PASS = 'PwChangeOwn9K'; // the account's own, post-change password
const MEMBER_PASS = 'PwChangeKeep8Z'; // the pool member's untouched password

interface Fixture {
  adminHeaders: Record<string, string>;
  ctx?: APIRequestContext;
  /** Member whose flag is never changed by this suite (UI-only tests). */
  member: string;
  /** Dedicated member for the flow that SETS a new password — creation and
   *  deletion are its own business, so test order cannot break the others. */
  selfChangeMember: string;
  selfChangePass: string;
}

let fx: Fixture;

test.describe('Forced password change', () => {
  /** Create a flagged member through the admin API (never fails silently). */
  async function createMember(ctx: APIRequestContext, username: string, password: string) {
    const created = await ctx.post('/api/users', {
      headers: fx.adminHeaders,
      data: {
        username,
        password,
        fullname: 'Pw Change Test',
        email: '',
        role_id: 1,
      },
    });
    const body = await created.json().catch(() => ({}));
    expect(body?.success, `create must succeed: ${JSON.stringify(body)}`).toBe(true);
  }

  test.beforeAll(async ({ playwright }) => {
    const ctx = await playwright.request.newContext({ baseURL: BASE });
    const login = await ctx.post('/api/auth/login', {
      data: { username: ADMIN_USER, password: ADMIN_PASS },
    });
    const body = await login.json().catch(() => ({}));
    const adminToken = body?.data?.token || '';
    expect(adminToken, 'the seeded admin must be available').toBeTruthy();
    // Mutating admin calls need the session-tied CSRF token (middleware::csrf).
    fx = {
      adminHeaders: { 'x-session-token': adminToken, 'x-csrf-token': body?.data?.csrf_token || '' },
      ctx,
      // Unique names survive parallel suite reruns within the same minute.
      member: `pwchg${Date.now().toString(36)}`,
      selfChangeMember: `pwchgc${Date.now().toString(36)}`,
      selfChangePass: NEW_PASS,
    };
    await createMember(ctx, fx.member, MEMBER_PASS);
  });

  test.afterAll(async () => {
    if (!fx?.ctx) return;
    // Best-effort: the flagged members (and their admin-chosen passwords)
    // must not survive the suite. The admin session itself is never touched.
    await fx.ctx.delete(`/api/users/${fx.member}`, { headers: fx.adminHeaders }).catch(() => {});
    await fx.ctx
      .delete(`/api/users/${fx.selfChangeMember}`, { headers: fx.adminHeaders })
      .catch(() => {});
    await fx.ctx.dispose().catch(() => {});
  });

  /** Sign in through the real form and land on the change screen. */
  async function loginAsFlaggedMember(page: Page, username = fx.member, password = MEMBER_PASS) {
    await page.goto('/login');
    await page.getByLabel(new RegExp('Username', 'i')).first().fill(username);
    await page.locator('#password').fill(password);
    await page.getByRole('button', { name: /sign in/i }).click();
    // The change form replaces the credential form on the same card.
    await expect(page.getByTestId('login-pwchange-notice')).toBeVisible();
    await expect(page).toHaveURL(/\/login$/);
  }

  async function fillChangeForm(page: Page, current: string, next: string, confirm?: string) {
    await page.locator('#pw-current').fill(current);
    await page.locator('#pw-new').fill(next);
    await page.locator('#pw-confirm').fill(confirm ?? next);
  }

  // -----------------------------------------------------------------------
  // 1 — Page load (+ 5 refresh mid-flow)
  // -----------------------------------------------------------------------

  test('1.1 routes a flagged login to the change screen, not the desktop', async ({ page }) => {
    await loginAsFlaggedMember(page);
    await expect(page.getByTestId('login-pwchange-submit')).toBeVisible();
    await expect(page).not.toHaveURL(/desktop/);
  });

  test('1.2 the server flags exactly what the admin created (API sanity)', async () => {
    const res = await fx.ctx!.post('/api/auth/login', {
      data: { username: fx.member, password: MEMBER_PASS },
    });
    const body = await res.json().catch(() => ({}));
    expect(body?.data?.must_change_password).toBe(true);
  });

  test('5.1 a reload mid-flow keeps the change screen (no desktop shortcut)', async ({ page }) => {
    await loginAsFlaggedMember(page);
    await page.reload();
    await expect(page.getByTestId('login-pwchange-notice')).toBeVisible();
    await expect(page).not.toHaveURL(/desktop/);
  });

  // -----------------------------------------------------------------------
  // 2 — Form fields
  // -----------------------------------------------------------------------

  test('2.1 the change form shows current, new and confirm fields', async ({ page }) => {
    await loginAsFlaggedMember(page);
    await expect(page.locator('#pw-current')).toBeVisible();
    await expect(page.locator('#pw-new')).toBeVisible();
    await expect(page.locator('#pw-confirm')).toBeVisible();
  });

  // -----------------------------------------------------------------------
  // 3 — Validation (client mirror + handler errors; nothing mutates)
  // -----------------------------------------------------------------------

  test('3.1 an empty new password is refused', async ({ page }) => {
    await loginAsFlaggedMember(page);
    await page.locator('#pw-current').fill(MEMBER_PASS);
    await page.getByTestId('login-pwchange-submit').click();
    await expect(page.getByTestId('login-pwchange-error')).toBeVisible();
  });

  test('3.2 mismatched confirmation is refused client-side', async ({ page }) => {
    await loginAsFlaggedMember(page);
    await fillChangeForm(page, MEMBER_PASS, NEW_PASS, 'Different99X');
    await page.getByTestId('login-pwchange-submit').click();
    await expect(page.getByTestId('login-pwchange-error')).toContainText(/match/i);
  });

  test('3.3 a weak password is refused by the client mirror of the server policy', async ({ page }) => {
    await loginAsFlaggedMember(page);
    await fillChangeForm(page, MEMBER_PASS, 'short');
    await page.getByTestId('login-pwchange-submit').click();
    await expect(page.getByTestId('login-pwchange-error')).toBeVisible();
  });

  test('3.4 a wrong current password surfaces the handler error, not a dead end', async ({ page }) => {
    await loginAsFlaggedMember(page);
    await fillChangeForm(page, 'WrongCurrent1', NEW_PASS);
    await page.getByTestId('login-pwchange-submit').click();
    await expect(page.getByTestId('login-pwchange-error')).toBeVisible();
    // The user stays on the change screen with the form intact.
    await expect(page.getByTestId('login-pwchange-submit')).toBeVisible();
  });

  test('3.5 the cancel button leaves the flagged session and returns to the plain login card', async ({ page }) => {
    await loginAsFlaggedMember(page);
    await page.getByTestId('login-pwchange-cancel').click();
    await expect(page.getByTestId('login-pwchange-notice')).toHaveCount(0);
    await expect(page.locator('#username')).toBeVisible();
  });

  // -----------------------------------------------------------------------
  // 7 — Keyboard
  // -----------------------------------------------------------------------

  test('7.1 Enter submits the change form', async ({ page }) => {
    await loginAsFlaggedMember(page);
    await fillChangeForm(page, MEMBER_PASS, 'short');
    await page.locator('#pw-confirm').press('Enter');
    await expect(page.getByTestId('login-pwchange-error')).toBeVisible();
  });

  // -----------------------------------------------------------------------
  // 8 — Responsive
  // -----------------------------------------------------------------------

  for (const width of [360, 768, 1920]) {
    test(`8.x the change screen stays usable at ${width}px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await loginAsFlaggedMember(page);
      await expect(page.getByTestId('login-pwchange-submit')).toBeVisible();
      await expect(page.locator('#pw-new')).toBeVisible();
    });
  }

  // -----------------------------------------------------------------------
  // 10 — Security
  // -----------------------------------------------------------------------

  test('10.1 no password or token ever appears in the URL', async ({ page }) => {
    await loginAsFlaggedMember(page);
    await expect(page.url()).not.toContain('token');
    await expect(page.url()).not.toContain(MEMBER_PASS);
  });

  test('10.2 the temporary password is not echoed into localStorage', async ({ page }) => {
    await loginAsFlaggedMember(page);
    const stored = await page.evaluate(() => JSON.stringify(localStorage));
    expect(stored).not.toContain(MEMBER_PASS);
  });

  // -----------------------------------------------------------------------
  // 4 — Submission flow (self-contained fixture: own member, own password)
  // -----------------------------------------------------------------------

  test('4.1 the change completes and the new password authenticates unflagged', async ({ page }) => {
    await createMember(fx.ctx!, fx.selfChangeMember, TEMP_PASS);
    await loginAsFlaggedMember(page, fx.selfChangeMember, TEMP_PASS);
    await fillChangeForm(page, TEMP_PASS, NEW_PASS);
    await page.getByTestId('login-pwchange-submit').click();

    // Back on the plain login card with the one-shot confirmation notice.
    await expect(page.getByTestId('login-pw-changed')).toBeVisible();

    // Server-side truth: the new password logs in with the flag CLEARED
    // (the old one no longer does).
    const oldLogin = await fx.ctx!.post('/api/auth/login', {
      data: { username: fx.selfChangeMember, password: TEMP_PASS },
    });
    expect(oldLogin.status()).toBe(401);
    const newLogin = await fx.ctx!.post('/api/auth/login', {
      data: { username: fx.selfChangeMember, password: NEW_PASS },
    });
    const body = await newLogin.json().catch(() => ({}));
    expect(body?.data?.must_change_password).toBe(false);
    expect(body?.data?.token).toBeTruthy();
  });
});
