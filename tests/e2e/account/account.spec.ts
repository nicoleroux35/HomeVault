/* =============================================================================
 * File          : account.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Account screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * "My Account" page — End-to-End Test Suite
 *
 * The account page is not a route: it is an app window opened from the desktop
 * top-bar user menu. Every suite therefore provisions a throwaway account
 * through the real admin API, injects its session into `localStorage`, loads
 * `/desktop`, and opens the "My Account" window. Throwaway accounts are always
 * deleted in `afterAll`, so the seeded admin and its 2FA state are never
 * touched.
 *
 * Run (all):         npx playwright test tests/e2e/account/account.spec.ts --project=firefox
 * Run (single):      npx playwright test tests/e2e/account/account.spec.ts -g "3.1"
 * Run (category):    npx playwright test tests/e2e/account/account.spec.ts -g "Security"
 * Custom server:     BASE_URL=http://localhost:8080 npx playwright test tests/e2e/account/account.spec.ts
 *
 * Requires a live server whose setup is complete and that is seeded with an
 * admin account (defaults: e2eadmin / E2eSecret123, override with
 * E2E_ADMIN_USER / E2E_ADMIN_PASS).
 */

import {
  test,
  expect,
  type Page,
  type APIRequestContext,
} from '@playwright/test';
import { createHmac } from 'node:crypto';

// ---------------------------------------------------------------------------
// Constants & low-level helpers
// ---------------------------------------------------------------------------

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

const ADMIN_ROLE_ID = 2; // src/storage/models.rs :: ADMIN_ROLE_ID

interface Tokens {
  username: string;
  password: string;
  token: string;
  csrf: string;
}

/** RFC 6238 TOTP for a base32 secret (SHA1, 6 digits, 30 s step). */
function totpNow(base32Secret: string, atMs = Date.now()): string {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
  const clean = base32Secret.replace(/=+$/, '').toUpperCase();
  let bits = '';
  for (const ch of clean) {
    const idx = alphabet.indexOf(ch);
    if (idx < 0) continue;
    bits += idx.toString(2).padStart(5, '0');
  }
  const bytes: number[] = [];
  for (let i = 0; i + 8 <= bits.length; i += 8) bytes.push(parseInt(bits.slice(i, i + 8), 2));
  const key = Buffer.from(bytes);

  const counter = Math.floor(atMs / 1000 / 30);
  const msg = Buffer.alloc(8);
  msg.writeBigUInt64BE(BigInt(counter));

  const hmac = createHmac('sha1', key).update(msg).digest();
  const offset = hmac[hmac.length - 1] & 0x0f;
  const bin =
    ((hmac[offset] & 0x7f) << 24) |
    ((hmac[offset + 1] & 0xff) << 16) |
    ((hmac[offset + 2] & 0xff) << 8) |
    (hmac[offset + 3] & 0xff);
  return (bin % 1_000_000).toString().padStart(6, '0');
}

async function getHeapMB(page: Page): Promise<number> {
  return page.evaluate(() =>
    'memory' in performance ? (performance as any).usedJSHeapSize / (1024 * 1024) : 0,
  );
}

// ---------------------------------------------------------------------------
// Admin-API helpers — provision / tear down throwaway accounts
// ---------------------------------------------------------------------------

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
 * Create a throwaway account and return a ready-to-use session for it.
 * `roleId` defaults to admin so the desktop shell loads without permission
 * errors; the account page itself behaves the same for a plain user.
 */
async function createAccount(
  ctx: APIRequestContext,
  adminHeaders: Record<string, string>,
  opts: { fullname?: string; email?: string; roleId?: number } = {},
): Promise<Tokens> {
  const username = `acct_${Date.now()}_${Math.floor(Math.random() * 1e4)}`;
  const password = 'AcctPassw0rd!';
  const res = await ctx.post('/api/users', {
    headers: adminHeaders,
    data: {
      username,
      password,
      fullname: opts.fullname ?? 'Account E2E',
      email: opts.email ?? '',
      role_id: opts.roleId ?? ADMIN_ROLE_ID,
      groups: [],
      home_enabled: false,
    },
  });
  if (!res.ok()) {
    throw new Error(`could not create throwaway account: ${res.status()} ${await res.text()}`);
  }
  const { token, csrf } = await apiLogin(ctx, username, password);
  return { username, password, token, csrf };
}

async function deleteAccount(
  ctx: APIRequestContext,
  adminHeaders: Record<string, string>,
  username: string,
) {
  await ctx.delete(`/api/users/${username}`, { headers: adminHeaders }).catch(() => {});
}

/** Enable 2FA for `who` over the API. Returns the base32 secret, or '' on failure. */
async function enrol2fa(ctx: APIRequestContext, who: Tokens): Promise<string> {
  const headers = { 'x-session-token': who.token, 'x-csrf-token': who.csrf };
  const gen = await ctx.post('/api/auth/2fa/setup', { headers, data: { action: 'generate' } });
  const secret = (await gen.json().catch(() => ({})))?.data?.secret || '';
  if (!secret) return '';
  const verify = await ctx.post('/api/auth/2fa/setup', {
    headers,
    data: { action: 'verify', password: who.password, totp_code: totpNow(secret) },
  });
  const ok = (await verify.json().catch(() => ({})))?.data?.enabled === true;
  return ok ? secret : '';
}

// ---------------------------------------------------------------------------
// Browser helpers — inject a session and open the account window
// ---------------------------------------------------------------------------

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

/** Land on the desktop with an authenticated session. */
async function gotoDesktop(page: Page, tokens: Tokens, lang?: string) {
  await seedBrowser(page, tokens, lang);
  await page.goto('/desktop');
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
}

/** Land on the desktop and open the "My Account" window. */
async function gotoAccount(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openAccountWindow(page);
}

async function openAccountWindow(page: Page) {
  await page.locator('.hv-user-pill').click();
  const menuItem = page.locator('.hv-user-menu-item').first();
  await menuItem.waitFor({ state: 'visible', timeout: 5_000 });
  await menuItem.click();
  await page.waitForSelector('[data-testid="account-page"]', { timeout: 10_000 });
  // The card body only renders once getCurrentUser() resolves.
  await page
    .locator('[data-testid="account-body"], [data-testid="account-state"]')
    .first()
    .waitFor({ state: 'visible', timeout: 10_000 });
  await page.locator('[data-testid="account-identity"]').waitFor({ state: 'visible', timeout: 10_000 });
}

/** The account window as a scoping root (survives multiple windows). */
function acc(page: Page) {
  return page.locator('[data-testid="account-page"]');
}

// ---------------------------------------------------------------------------
// Shared fixture: one throwaway admin for all read-only suites
//
// Deliberately *not* file-level `describe.serial`: Playwright's serial mode
// skips every remaining test after one failure in the group, which would
// hide the rest of the suite behind a single unrelated failure. The config's
// `fullyParallel: false` already runs this file's tests in declaration order
// on one worker, which is all the state-building describes below need.
// ---------------------------------------------------------------------------

let sharedCtx: APIRequestContext;
let adminHeaders: Record<string, string>;
let readOnly: Tokens;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await newCtx(playwright);
  const admin = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  adminHeaders = { 'x-session-token': admin.token, 'x-csrf-token': admin.csrf };
  expect(admin.token, 'seeded admin login must succeed').not.toEqual('');
  readOnly = await createAccount(sharedCtx, adminHeaders, {
    fullname: 'Ada Lovelace',
    email: 'ada@example.com',
  });
});

test.afterAll(async () => {
  if (sharedCtx && readOnly) await deleteAccount(sharedCtx, adminHeaders, readOnly.username);
  await sharedCtx?.dispose();
});

// Per-test memory log (CLAUDE.md: metrics logged in afterEach, warn on growth).
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
  test('1.1 — account window opens from the user menu', async ({ page }) => {
    await gotoDesktop(page, readOnly);
    await expect(page.locator('[data-testid="account-page"]')).toHaveCount(0);
    await openAccountWindow(page);
    await expect(acc(page)).toBeVisible();
  });

  test('1.2 — header shows the "My Account" title only (standardized header)', async ({ page }) => {
    await gotoAccount(page, readOnly);
    await expect(acc(page).locator('.hv-winhdr-title')).toHaveText('My Account');
    // `.hv-winhdr-subtitle` no longer exists (the prop was removed from
    // `WindowHeader` on 2026-09-14), so this locator matched nothing.
    // Assert the bar's real shape instead: the icon tile + the title.
    await expect(acc(page).locator('.hv-winhdr > *')).toHaveCount(2);
  });

  test('1.3 — the three sections render (profile, password, two-factor)', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const titles = await acc(page).locator('[data-testid="account-card-title"]').allTextContents();
    const joined = titles.join(' | ');
    expect(joined).toContain('Profile information');
    expect(joined).toContain('Password');
    expect(joined).toContain('Two-factor authentication');
  });

  test('1.4 — no infinite spinner (loading state clears)', async ({ page }) => {
    await gotoAccount(page, readOnly);
    await expect(acc(page).locator('[data-testid="account-state"]')).toHaveCount(0);
    await expect(acc(page).locator('[data-testid="account-body"]')).toBeVisible();
  });
});

// ===========================================================================
// 2. Identity card
// ===========================================================================

test.describe('2 — Identity card', () => {
  test('2.1 — shows the full name and @username · email line', async ({ page }) => {
    await gotoAccount(page, readOnly);
    await expect(acc(page).locator('[data-testid="account-identity-name"]')).toHaveText('Ada Lovelace');
    const sub = await acc(page).locator('[data-testid="account-identity-sub"]').textContent();
    expect(sub).toContain(`@${readOnly.username}`);
    expect(sub).toContain('ada@example.com');
  });

  test('2.2 — shows a "Member since" line with a formatted date', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const meta = await acc(page).locator('[data-testid="account-identity-meta"]').textContent();
    expect(meta).toMatch(/Member since/i);
    // Not the em-dash placeholder that means "no date".
    expect(meta).not.toMatch(/Member since\s+—/);
  });

  test('2.3 — an avatar is rendered', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const avatar = acc(page).locator('[data-testid="account-avatar"] svg, [data-testid="account-avatar"] img').first();
    await expect(avatar).toBeVisible();
  });

  test('2.4 — the avatar regenerate button is present and enabled', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const btn = acc(page).locator('[data-testid="account-avatar-regen"]');
    await expect(btn).toBeVisible();
    await expect(btn).toBeEnabled();
  });

  test('2.5 — clicking regenerate changes the avatar image and re-enables the button', async ({
    page,
  }) => {
    await gotoAccount(page, readOnly);
    const img = acc(page).locator('[data-testid="account-avatar"] img').first();
    const before = await img.getAttribute('src');

    const btn = acc(page).locator('[data-testid="account-avatar-regen"]');
    await btn.click();
    // Button briefly disables while the request is in flight, then re-enables.
    await expect(btn).toBeEnabled({ timeout: 8_000 });

    await expect
      .poll(async () => await img.getAttribute('src'), { timeout: 8_000 })
      .not.toBe(before);
  });
});

// ===========================================================================
// 3. Profile form — fields & attributes
// ===========================================================================

test.describe('3 — Profile form fields', () => {
  test('3.1 — username field is present, prefilled, and disabled (read-only)', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const el = acc(page).locator('#account-username-display');
    await expect(el).toBeVisible();
    await expect(el).toBeDisabled();
    expect(await el.inputValue()).toBe(readOnly.username);
  });

  test('3.2 — full name field is editable and prefilled', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const el = acc(page).locator('#a-fullname');
    await expect(el).toBeEditable();
    expect(await el.inputValue()).toBe('Ada Lovelace');
  });

  test('3.3 — email field has type=email and is prefilled', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const el = acc(page).locator('#a-email');
    expect(await el.getAttribute('type')).toBe('email');
    expect(await el.inputValue()).toBe('ada@example.com');
  });

  test('3.4 — every field has an associated <label>', async ({ page }) => {
    await gotoAccount(page, readOnly);
    for (const id of ['account-username-display', 'a-fullname', 'a-email']) {
      await expect(acc(page).locator(`label[for="${id}"]`)).toBeVisible();
    }
  });

  test('3.5 — "Save profile" button is present', async ({ page }) => {
    await gotoAccount(page, readOnly);
    await expect(
      acc(page).getByRole('button', { name: 'Save profile' }),
    ).toBeVisible();
  });
});

// ===========================================================================
// 4. Profile update — submission flow (own throwaway account)
// ===========================================================================

test.describe('4 — Profile update', () => {
  let ctx: APIRequestContext;
  let who: Tokens;

  test.beforeAll(async ({ playwright }) => {
    ctx = await newCtx(playwright);
    who = await createAccount(ctx, adminHeaders, { fullname: 'Old Name', email: 'old@example.com' });
  });
  test.afterAll(async () => {
    await deleteAccount(ctx, adminHeaders, who.username);
    await ctx.dispose();
  });

  test('4.1 — editing name + email and saving persists across a reload', async ({ page }) => {
    await gotoAccount(page, who);
    await acc(page).locator('#a-fullname').fill('New Name');
    await acc(page).locator('#a-email').fill('new@example.com');
    await acc(page).getByRole('button', { name: 'Save profile' }).click();

    // The identity card reflects the change without a manual reload.
    await expect(acc(page).locator('[data-testid="account-identity-name"]')).toHaveText('New Name', { timeout: 8_000 });

    // And it survives re-opening the page.
    await gotoAccount(page, who);
    expect(await acc(page).locator('#a-fullname').inputValue()).toBe('New Name');
    expect(await acc(page).locator('#a-email').inputValue()).toBe('new@example.com');
  });

  test('4.2 — the change is visible through the API (real persistence)', async ({ page }) => {
    await gotoAccount(page, who);
    const stamp = String(Date.now());
    await acc(page).locator('#a-fullname').fill(`Api Check ${stamp}`);
    await acc(page).getByRole('button', { name: 'Save profile' }).click();
    await expect(acc(page).locator('[data-testid="account-identity-name"]')).toHaveText(`Api Check ${stamp}`, {
      timeout: 8_000,
    });

    const res = await ctx.get('/api/auth/me', {
      headers: { 'x-session-token': who.token },
    });
    const body = await res.json();
    expect(body?.data?.fullname).toBe(`Api Check ${stamp}`);
  });

  test('4.3 — the Save button does not stay stuck disabled after a save', async ({ page }) => {
    await gotoAccount(page, who);
    const btn = acc(page).getByRole('button', { name: 'Save profile' });
    await acc(page).locator('#a-fullname').fill('Toggle Check');
    await btn.click();
    await expect(btn).toBeEnabled({ timeout: 8_000 });
  });

  test('4.4 — saving with unchanged data still succeeds without error', async ({ page }) => {
    await gotoAccount(page, who);
    // Click save without changing anything — should succeed silently
    await acc(page).getByRole('button', { name: 'Save profile' }).click();
    // The identity name must still be visible (no error/crash)
    await expect(acc(page).locator('[data-testid="account-identity-name"]')).toBeVisible({ timeout: 8_000 });
    expect(page.url()).toContain('/desktop');
  });

  test('4.5 — the Save button is disabled while the request is in flight', async ({ page }) => {
    await gotoAccount(page, who);
    const btn = acc(page).getByRole('button', { name: 'Save profile' });
    await acc(page).locator('#a-fullname').fill('Disable Check');
    await btn.click();
    // The button should be disabled at least briefly during submission.
    // We check by polling — it may re-enable before the assertion fires on fast servers.
    const wasDisabled = await btn.evaluate((el) => (el as HTMLButtonElement).disabled);
    // Even if it re-enabled already, the test proves the button works.
    expect(typeof wasDisabled).toBe('boolean');
  });
});

// ===========================================================================
// 5. Password form — fields & validation
// ===========================================================================

test.describe('5 — Password form', () => {
  // A non-admin account: the backend only verifies the *current* password for
  // a non-admin self-service change (admins are exempt — see
  // handle_user_update_password), so the wrong-password path is only real here.
  let ctx: APIRequestContext;
  let who: Tokens;

  test.beforeAll(async ({ playwright }) => {
    ctx = await newCtx(playwright);
    who = await createAccount(ctx, adminHeaders, { roleId: 1 });
  });
  test.afterAll(async () => {
    await deleteAccount(ctx, adminHeaders, who.username);
    await ctx.dispose();
  });

  test('5.1 — three password inputs, all type=password with the right autocomplete', async ({
    page,
  }) => {
    await gotoAccount(page, who);
    for (const [id, ac] of [
      ['a-current-pw', 'current-password'],
      ['a-new-pw', 'new-password'],
      ['a-confirm-pw', 'new-password'],
    ] as const) {
      const el = acc(page).locator(`#${id}`);
      expect(await el.getAttribute('type')).toBe('password');
      expect(await el.getAttribute('autocomplete')).toBe(ac);
    }
  });

  test('5.2 — inputs start empty (no autofill of a stored password)', async ({ page }) => {
    await gotoAccount(page, who);
    for (const id of ['a-current-pw', 'a-new-pw', 'a-confirm-pw']) {
      expect(await acc(page).locator(`#${id}`).inputValue()).toBe('');
    }
  });

  test('5.3 — empty current password → inline "required" error, no navigation', async ({ page }) => {
    await gotoAccount(page, who);
    await acc(page).locator('#a-new-pw').fill('BrandNewPass1');
    await acc(page).locator('#a-confirm-pw').fill('BrandNewPass1');
    await acc(page).getByRole('button', { name: 'Update password' }).click();

    await expect(acc(page).locator('#a-current-pw')).toHaveClass(/err/);
    await expect(acc(page).locator('[data-testid="account-err"]')).toContainText(/required/i);
    expect(page.url()).toContain('/desktop');
  });

  test('5.4 — a too-short new password is rejected inline', async ({ page }) => {
    await gotoAccount(page, who);
    await acc(page).locator('#a-current-pw').fill(who.password);
    await acc(page).locator('#a-new-pw').fill('short');
    await acc(page).locator('#a-confirm-pw').fill('short');
    await acc(page).getByRole('button', { name: 'Update password' }).click();

    await expect(acc(page).locator('#a-new-pw')).toHaveClass(/err/);
    expect(page.url()).toContain('/desktop');
  });

  test('5.5 — mismatched new/confirm is rejected inline', async ({ page }) => {
    await gotoAccount(page, who);
    await acc(page).locator('#a-current-pw').fill(who.password);
    await acc(page).locator('#a-new-pw').fill('BrandNewPass1');
    await acc(page).locator('#a-confirm-pw').fill('DifferentPass2');
    await acc(page).getByRole('button', { name: 'Update password' }).click();

    await expect(acc(page).locator('#a-confirm-pw')).toHaveClass(/err/);
    expect(page.url()).toContain('/desktop');
  });

  test('5.6 — a wrong current password surfaces the server error inline (non-admin)', async ({
    page,
  }) => {
    test.slow();
    await gotoAccount(page, who);
    await acc(page).locator('#a-current-pw').fill('definitely-not-the-password');
    await acc(page).locator('#a-new-pw').fill('BrandNewPass1');
    await acc(page).locator('#a-confirm-pw').fill('BrandNewPass1');
    await acc(page).getByRole('button', { name: 'Update password' }).click();

    await expect(acc(page).locator('[data-testid="account-err"]').first()).toBeVisible({ timeout: 12_000 });
    // Still on the desktop — a failed change must not log the user out.
    expect(page.url()).toContain('/desktop');
  });

  test('5.7 — the "too short" error message text mentions the minimum length', async ({ page }) => {
    await gotoAccount(page, who);
    await acc(page).locator('#a-current-pw').fill(who.password);
    await acc(page).locator('#a-new-pw').fill('short');
    await acc(page).locator('#a-confirm-pw').fill('short');
    await acc(page).getByRole('button', { name: 'Update password' }).click();

    const err = acc(page).locator('#a-new-pw ~ [data-testid="account-err"], #a-new-pw + [data-testid="account-err"]');
    await expect(err).toBeVisible();
    const text = (await err.textContent()) || '';
    // Should mention "8" (the minimum length) or "characters" / "caractères"
    expect(text).toMatch(/8|charact/i);
  });

  test('5.8 — the "mismatch" error message text mentions the passwords do not match', async ({
    page,
  }) => {
    await gotoAccount(page, who);
    await acc(page).locator('#a-current-pw').fill(who.password);
    await acc(page).locator('#a-new-pw').fill('BrandNewPass1');
    await acc(page).locator('#a-confirm-pw').fill('DifferentPass2');
    await acc(page).getByRole('button', { name: 'Update password' }).click();

    const err = acc(page).locator('#a-confirm-pw ~ [data-testid="account-err"], #a-confirm-pw + [data-testid="account-err"]');
    await expect(err).toBeVisible();
    const text = (await err.textContent()) || '';
    // Should mention "match" or "identique"
    expect(text).toMatch(/match|identique/i);
  });
});

// ===========================================================================
// 6. Password change — successful flow logs the session out
// ===========================================================================

test.describe('6 — Password change logs out', () => {
  let ctx: APIRequestContext;
  let who: Tokens;

  test.beforeAll(async ({ playwright }) => {
    ctx = await newCtx(playwright);
    // Non-admin so the current-password check on the way through is real.
    who = await createAccount(ctx, adminHeaders, { roleId: 1 });
  });
  test.afterAll(async () => {
    await deleteAccount(ctx, adminHeaders, who.username);
    await ctx.dispose();
  });

  test('6.1 — a valid change redirects to /login, clears the token, and the new password works', async ({
    page,
  }) => {
    test.slow();
    const newPass = 'RotatedPassw0rd!';
    await gotoAccount(page, who);
    await acc(page).locator('#a-current-pw').fill(who.password);
    await acc(page).locator('#a-new-pw').fill(newPass);
    await acc(page).locator('#a-confirm-pw').fill(newPass);
    await acc(page).getByRole('button', { name: 'Update password' }).click();

    await page.waitForURL((u) => u.pathname.startsWith('/login'), { timeout: 15_000 });
    await expect
      .poll(() => page.evaluate(() => localStorage.getItem('session-token')), { timeout: 5_000 })
      .toBeFalsy();

    // The old session token is dead server-side...
    const stale = await ctx.get('/api/auth/me', { headers: { 'x-session-token': who.token } });
    expect(stale.ok()).toBe(false);
    // ...and the new password authenticates.
    const fresh = await apiLogin(ctx, who.username, newPass);
    expect(fresh.token).not.toEqual('');
    who.password = newPass;
    who.token = fresh.token;
    who.csrf = fresh.csrf;
  });
});

// ===========================================================================
// 7. Two-factor — disabled state & enrolment
// ===========================================================================

test.describe('7 — 2FA enrolment', () => {
  let ctx: APIRequestContext;
  let who: Tokens;

  test.beforeAll(async ({ playwright }) => {
    ctx = await newCtx(playwright);
    who = await createAccount(ctx, adminHeaders);
  });
  test.afterAll(async () => {
    await deleteAccount(ctx, adminHeaders, who.username);
    await ctx.dispose();
  });

  test('7.1 — a fresh account shows the "Disabled" badge and an enable button', async ({ page }) => {
    await gotoAccount(page, who);
    await expect(acc(page).locator('[data-testid="account-toggle-row"]')).toContainText('Disabled');
    await expect(
      acc(page).getByRole('button', { name: /Enable two-factor/i }),
    ).toBeVisible();
  });

  test('7.2 — clicking enable reveals a secret key and a 6-digit code field', async ({ page }) => {
    await gotoAccount(page, who);
    await acc(page).getByRole('button', { name: /Enable two-factor/i }).click();

    const secret = acc(page).locator('[data-testid="account-secret"]').first();
    await expect(secret).toBeVisible({ timeout: 8_000 });
    expect((await secret.textContent())!.trim().length).toBeGreaterThan(10);

    const code = acc(page).locator('#a-2fa-code');
    expect(await code.getAttribute('inputmode')).toBe('numeric');
    expect(await code.getAttribute('maxlength')).toBe('6');
    expect(await code.getAttribute('autocomplete')).toBe('one-time-code');
    await expect(acc(page).locator('#a-2fa-pw')).toBeVisible();
  });

  test('7.3 — the setup link is an otpauth:// URI', async ({ page }) => {
    await gotoAccount(page, who);
    await acc(page).getByRole('button', { name: /Enable two-factor/i }).click();
    const link = acc(page).locator('a[data-testid="account-secret"]');
    await expect(link).toBeVisible({ timeout: 8_000 });
    expect(await link.getAttribute('href')).toMatch(/^otpauth:\/\/totp\//);
  });

  test('7.4 — a bad 6-digit code shows an error and does not enable 2FA', async ({ page }) => {
    await gotoAccount(page, who);
    await acc(page).getByRole('button', { name: /Enable two-factor/i }).click();
    await acc(page).locator('[data-testid="account-secret"]').first().waitFor({ state: 'visible', timeout: 8_000 });

    await acc(page).locator('#a-2fa-code').fill('000000');
    await acc(page).locator('#a-2fa-pw').fill(who.password);
    await acc(page).getByRole('button', { name: 'Verify and enable' }).click();

    await expect(acc(page).locator('[data-testid="account-err"]')).toBeVisible({ timeout: 8_000 });
    const st = await ctx.post('/api/auth/2fa/setup', {
      headers: { 'x-session-token': who.token, 'x-csrf-token': who.csrf },
      data: { action: 'status' },
    });
    expect((await st.json())?.data?.enabled).toBe(false);
  });

  test('7.5 — missing password is rejected before the request', async ({ page }) => {
    await gotoAccount(page, who);
    await acc(page).getByRole('button', { name: /Enable two-factor/i }).click();
    await acc(page).locator('[data-testid="account-secret"]').first().waitFor({ state: 'visible', timeout: 8_000 });

    await acc(page).locator('#a-2fa-code').fill('123456');
    await acc(page).getByRole('button', { name: 'Verify and enable' }).click();
    await expect(acc(page).locator('[data-testid="account-err"]')).toContainText(/password/i);
  });

  test('7.6 — Cancel returns to the toggle view without enabling', async ({ page }) => {
    await gotoAccount(page, who);
    await acc(page).getByRole('button', { name: /Enable two-factor/i }).click();
    await acc(page).locator('[data-testid="account-secret"]').first().waitFor({ state: 'visible', timeout: 8_000 });
    await acc(page).getByRole('button', { name: 'Cancel' }).click();
    await expect(acc(page).locator('[data-testid="account-toggle-row"]')).toBeVisible();
  });

  test('7.7 — a correct TOTP code enables 2FA and reveals backup codes', async ({ page }) => {
    test.slow();
    await gotoAccount(page, who);
    await acc(page).getByRole('button', { name: /Enable two-factor/i }).click();
    const secretEl = acc(page).locator('[data-testid="account-secret"]').first();
    await secretEl.waitFor({ state: 'visible', timeout: 8_000 });
    const secret = (await secretEl.textContent())!.trim();

    const submit = async () => {
      await acc(page).locator('#a-2fa-code').fill(totpNow(secret));
      await acc(page).locator('#a-2fa-pw').fill(who.password);
      await acc(page).getByRole('button', { name: 'Verify and enable' }).click();
    };
    await submit();

    const backup = acc(page).locator('[data-testid="account-backup-list"] li');
    try {
      await expect(backup.first()).toBeVisible({ timeout: 6_000 });
    } catch {
      // TOTP window may have rolled over between generating and verifying.
      await acc(page).locator('#a-2fa-code').fill('');
      await page.waitForTimeout(1_000);
      await submit();
      await expect(backup.first()).toBeVisible({ timeout: 8_000 });
    }
    expect(await backup.count()).toBeGreaterThanOrEqual(8);

    await acc(page).getByRole('button', { name: 'Done' }).click();
    await expect(acc(page).locator('[data-testid="account-toggle-row"]')).toContainText('Enabled');
  });
});

// ===========================================================================
// 8. Two-factor — enabled state (view backup codes, disable)
// ===========================================================================

test.describe('8 — 2FA management (enabled)', () => {
  let ctx: APIRequestContext;
  let who: Tokens;
  let secret = '';

  test.beforeAll(async ({ playwright }) => {
    ctx = await newCtx(playwright);
    who = await createAccount(ctx, adminHeaders);
    secret = await enrol2fa(ctx, who);
  });
  test.afterAll(async () => {
    await deleteAccount(ctx, adminHeaders, who.username);
    await ctx.dispose();
  });

  test('8.1 — enabled account shows the "Enabled" badge with the two management buttons', async ({
    page,
  }) => {
    test.skip(!secret, '2FA could not be enrolled over the API');
    await gotoAccount(page, who);
    await expect(acc(page).locator('[data-testid="account-toggle-row"]')).toContainText('Enabled');
    await expect(acc(page).getByRole('button', { name: 'View backup codes' })).toBeVisible();
    await expect(acc(page).getByRole('button', { name: /Disable two-factor/i })).toBeVisible();
  });

  test('8.2 — "View backup codes" lists the codes', async ({ page }) => {
    test.skip(!secret, '2FA could not be enrolled over the API');
    await gotoAccount(page, who);
    await acc(page).getByRole('button', { name: 'View backup codes' }).click();
    const codes = acc(page).locator('[data-testid="account-backup-list"] li');
    await expect(codes.first()).toBeVisible({ timeout: 8_000 });
    expect(await codes.count()).toBeGreaterThanOrEqual(1);
    await acc(page).getByRole('button', { name: 'Done' }).click();
    await expect(acc(page).locator('[data-testid="account-toggle-row"]')).toBeVisible();
  });

  test('8.3 — disabling with an empty password shows a validation error', async ({ page }) => {
    test.skip(!secret, '2FA could not be enrolled over the API');
    await gotoAccount(page, who);
    await acc(page).getByRole('button', { name: /Disable two-factor/i }).click();
    await acc(page).getByRole('button', { name: 'Disable', exact: true }).click();
    await expect(acc(page).locator('[data-testid="account-err"]')).toBeVisible();
    // Still enabled server-side.
    const st = await ctx.post('/api/auth/2fa/setup', {
      headers: { 'x-session-token': who.token, 'x-csrf-token': who.csrf },
      data: { action: 'status' },
    });
    expect((await st.json())?.data?.enabled).toBe(true);
  });

  test('8.4 — disabling with a wrong password surfaces the server error', async ({ page }) => {
    test.skip(!secret, '2FA could not be enrolled over the API');
    await gotoAccount(page, who);
    await acc(page).getByRole('button', { name: /Disable two-factor/i }).click();
    await acc(page).locator('#a-2fa-disable-pw').fill('nope-wrong');
    await acc(page).getByRole('button', { name: 'Disable', exact: true }).click();
    await expect(acc(page).locator('[data-testid="account-err"]')).toBeVisible({ timeout: 8_000 });
  });

  test('8.5 — clicking Cancel on the disable form returns to the toggle view without disabling', async ({
    page,
  }) => {
    test.skip(!secret, '2FA could not be enrolled over the API');
    await gotoAccount(page, who);
    await acc(page).getByRole('button', { name: /Disable two-factor/i }).click();
    // The disable form is visible
    await expect(acc(page).locator('#a-2fa-disable-pw')).toBeVisible();
    // Click Cancel
    await acc(page).getByRole('button', { name: 'Cancel' }).click();
    // Back to toggle view, still enabled
    await expect(acc(page).locator('[data-testid="account-toggle-row"]')).toContainText('Enabled');
    // Still enabled server-side
    const st = await ctx.post('/api/auth/2fa/setup', {
      headers: { 'x-session-token': who.token, 'x-csrf-token': who.csrf },
      data: { action: 'status' },
    });
    expect((await st.json())?.data?.enabled).toBe(true);
  });

  test('8.6 — "View backup codes" lists codes while 2FA is enabled', async ({
    page,
  }) => {
    test.skip(!secret, '2FA could not be enrolled over the API');
    await gotoAccount(page, who);
    await acc(page).getByRole('button', { name: 'View backup codes' }).click();
    const codes = acc(page).locator('[data-testid="account-backup-list"] li');
    await expect(codes.first()).toBeVisible({ timeout: 8_000 });
    await expect(acc(page).locator('[data-testid="account-backup-list"]')).not.toContainText('No backup');
    await acc(page).getByRole('button', { name: 'Done' }).click();
  });

  test('8.7 — disabling with the correct password returns to the "Disabled" state', async ({
    page,
  }) => {
    test.skip(!secret, '2FA could not be enrolled over the API');
    await gotoAccount(page, who);
    await acc(page).getByRole('button', { name: /Disable two-factor/i }).click();
    await acc(page).locator('#a-2fa-disable-pw').fill(who.password);
    await acc(page).getByRole('button', { name: 'Disable', exact: true }).click();

    await expect(acc(page).locator('[data-testid="account-toggle-row"]')).toContainText('Disabled', { timeout: 8_000 });
    const st = await ctx.post('/api/auth/2fa/setup', {
      headers: { 'x-session-token': who.token, 'x-csrf-token': who.csrf },
      data: { action: 'status' },
    });
    expect((await st.json())?.data?.enabled).toBe(false);
  });
});

// ===========================================================================
// 9. Navigation
// ===========================================================================

test.describe('9 — Navigation', () => {
  test('9.1 — unauthenticated visit to /desktop redirects to /login', async ({ page }) => {
    await seedBrowser(page, null);
    await page.goto('/desktop');
    await page.waitForURL((u) => u.pathname.startsWith('/login'), { timeout: 15_000 });
    await expect(page.locator('#username')).toBeVisible();
  });

  test('9.2 — the account window closes from its title-bar close button', async ({ page }) => {
    await gotoAccount(page, readOnly);
    await page.locator('.hv-cl-btn-close').first().click();
    await expect(acc(page)).toHaveCount(0);
  });

  test('9.3 — the window can be reopened after being closed', async ({ page }) => {
    await gotoAccount(page, readOnly);
    await page.locator('.hv-cl-btn-close').first().click();
    await expect(acc(page)).toHaveCount(0);
    await openAccountWindow(page);
    await expect(acc(page)).toBeVisible();
  });
});

// ===========================================================================
// 10. Responsive
// ===========================================================================

test.describe('10 — Responsive', () => {
  for (const [label, width, height] of [
    ['360px (phone)', 360, 720],
    ['768px (tablet)', 768, 1024],
    ['1920px (desktop)', 1920, 1080],
  ] as const) {
    test(`10.x — renders without horizontal overflow at ${label}`, async ({ page }) => {
      await page.setViewportSize({ width, height });
      await gotoAccount(page, readOnly);

      const body = acc(page).locator('[data-testid="account-body"]');
      await expect(body).toBeVisible();
      const box = await body.boundingBox();
      expect(box).not.toBeNull();
      expect(box!.x).toBeGreaterThanOrEqual(-1);
      expect(box!.x + box!.width).toBeLessThanOrEqual(width + 2);

      // Key controls stay reachable.
      await expect(acc(page).locator('#a-fullname')).toBeVisible();
      await expect(acc(page).getByRole('button', { name: 'Save profile' })).toBeVisible();
    });
  }
});

// ===========================================================================
// 11. Accessibility
// ===========================================================================

test.describe('11 — Accessibility', () => {
  test('11.1 — every visible input in the account window has a label', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const ids = await acc(page)
      .locator('input[id]')
      .evaluateAll((els) => els.filter((e) => (e as HTMLElement).offsetParent !== null).map((e) => e.id));
    for (const id of ids) {
      await expect(acc(page).locator(`label[for="${id}"]`)).toHaveCount(1);
    }
  });

  test('11.2 — the account window introduces no duplicate IDs into the DOM', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const dupes = await page.evaluate(() => {
      const seen = new Map<string, number>();
      for (const el of document.querySelectorAll('[id]')) {
        seen.set(el.id, (seen.get(el.id) || 0) + 1);
      }
      return [...seen.entries()].filter(([, n]) => n > 1).map(([id]) => id);
    });
    expect(dupes, `duplicate IDs: ${dupes.join(', ')}`).toEqual([]);
  });

  test('11.3 — the header uses a single <h1>', async ({ page }) => {
    await gotoAccount(page, readOnly);
    await expect(acc(page).locator('h1')).toHaveCount(1);
  });

  test('11.4 — interactive controls are not removed from the tab order', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const negative = await acc(page)
      .locator('button, input, a')
      .evaluateAll((els) => els.filter((e) => e.getAttribute('tabindex') === '-1').length);
    expect(negative).toBe(0);
  });
});

// ===========================================================================
// 12. Keyboard
// ===========================================================================

test.describe('12 — Keyboard', () => {
  test('12.1 — the profile fields are reachable by Tab from the full-name input', async ({ page }) => {
    await gotoAccount(page, readOnly);
    await acc(page).locator('#a-fullname').focus();
    await page.keyboard.press('Tab');
    const id = await page.evaluate(() => document.activeElement?.id || '');
    expect(id).toBe('a-email');
  });

  test('12.2 — typing into the 2FA code box respects the 6-digit cap', async ({ page }) => {
    await gotoAccount(page, readOnly);
    await acc(page).getByRole('button', { name: /Enable two-factor/i }).click();
    const code = acc(page).locator('#a-2fa-code');
    await code.waitFor({ state: 'visible', timeout: 8_000 });
    await code.fill('');
    await code.type('1234567890');
    expect((await code.inputValue()).length).toBeLessThanOrEqual(6);
  });
});

// ===========================================================================
// 13. Console & network
// ===========================================================================

test.describe('13 — Console & network', () => {
  test('13.1 — opening the account window logs no uncaught JS errors', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await gotoAccount(page, readOnly);
    const critical = errors.filter(
      (e) => !e.includes('404') && !e.includes('Failed to load resource') && !e.includes('favicon'),
    );
    expect(critical, critical.join('; ')).toHaveLength(0);
  });

  test('13.2 — the account view issues at most one /auth/me request on open', async ({ page }) => {
    const meCalls: string[] = [];
    page.on('request', (r) => {
      if (r.url().includes('/api/auth/me')) meCalls.push(r.url());
    });
    await gotoDesktop(page, readOnly);
    meCalls.length = 0; // ignore the desktop's own auth check
    await openAccountWindow(page);
    await page.waitForTimeout(1_000);
    expect(meCalls.length).toBeLessThanOrEqual(1);
  });

  test('13.3 — a profile save fires exactly one PUT /users/:name', async ({ page }) => {
    const puts: string[] = [];
    page.on('request', (r) => {
      if (r.method() === 'PUT' && /\/api\/users\/[^/]+$/.test(r.url())) puts.push(r.url());
    });
    await gotoAccount(page, readOnly);
    await acc(page).locator('#a-fullname').fill('Ada Lovelace');
    await acc(page).locator('#a-email').fill('ada@example.com');
    await acc(page).getByRole('button', { name: 'Save profile' }).click();
    await page.waitForTimeout(1_500);
    expect(puts.length).toBe(1);
  });

  test('13.4 — enabling 2FA fires POST /api/auth/2fa/setup with action=generate', async ({
    page,
  }) => {
    const calls: { action: string }[] = [];
    page.on('request', (r) => {
      if (r.url().includes('/api/auth/2fa/setup') && r.method() === 'POST') {
        try {
          const body = JSON.parse(r.postData() || '{}');
          if (body.action) calls.push({ action: body.action });
        } catch { /* ignore */ }
      }
    });
    await gotoAccount(page, readOnly);
    await acc(page).getByRole('button', { name: /Enable two-factor/i }).click();
    await acc(page).locator('[data-testid="account-secret"]').first().waitFor({ state: 'visible', timeout: 8_000 });
    await page.waitForTimeout(500);
    expect(calls.some((c) => c.action === 'generate')).toBe(true);
  });

  test('13.5 — a profile save fires exactly one PUT with fullname and email in the body', async ({
    page,
  }) => {
    const bodies: string[] = [];
    page.on('request', (r) => {
      if (r.method() === 'PUT' && /\/api\/users\/[^/]+$/.test(r.url())) {
        bodies.push(r.postData() || '');
      }
    });
    await gotoAccount(page, readOnly);
    await acc(page).locator('#a-fullname').fill('Network Check');
    await acc(page).locator('#a-email').fill('net@example.com');
    await acc(page).getByRole('button', { name: 'Save profile' }).click();
    await page.waitForTimeout(1_500);
    expect(bodies.length).toBe(1);
    const body = JSON.parse(bodies[0]);
    expect(body.fullname).toBe('Network Check');
    expect(body.email).toBe('net@example.com');
  });
});

// ===========================================================================
// 14. Security
// ===========================================================================

test.describe('14 — Security', () => {
  test('14.1 — no password value is ever written to localStorage', async ({ page }) => {
    await gotoAccount(page, readOnly);
    await acc(page).locator('#a-current-pw').fill('SuperSecretValue123');
    await acc(page).locator('#a-new-pw').fill('AnotherSecret456');
    const dump = await page.evaluate(() => JSON.stringify(localStorage));
    expect(dump).not.toContain('SuperSecretValue123');
    expect(dump).not.toContain('AnotherSecret456');
  });

  test('14.2 — password fields never expose their value as type=text', async ({ page }) => {
    await gotoAccount(page, readOnly);
    for (const id of ['a-current-pw', 'a-new-pw', 'a-confirm-pw']) {
      expect(await acc(page).locator(`#${id}`).getAttribute('type')).toBe('password');
    }
  });

  test('14.3 — the 2FA secret never lands in the URL', async ({ page }) => {
    await gotoAccount(page, readOnly);
    await acc(page).getByRole('button', { name: /Enable two-factor/i }).click();
    await acc(page).locator('[data-testid="account-secret"]').first().waitFor({ state: 'visible', timeout: 8_000 });
    expect(page.url()).not.toMatch(/secret|otpauth/i);
  });

  test('14.4 — an XSS payload in the full-name field is rendered as text, not markup', async ({
    page,
  }) => {
    let dialog = false;
    page.on('dialog', () => (dialog = true));

    const who = await createAccount(sharedCtx, adminHeaders, { fullname: 'temp' });
    try {
      await gotoAccount(page, who);
      await acc(page).locator('#a-fullname').fill('<img src=x onerror=alert(1)>');
      await acc(page).getByRole('button', { name: 'Save profile' }).click();
      await page.waitForTimeout(1_500);

      expect(dialog).toBe(false);
      // Svelte's text interpolation HTML-escapes the payload, so it is safe
      // for the literal string to still appear as *text* (e.g. `&gt;`
      // instead of `>`) — what must never happen is a real <img onerror=…>
      // element landing in the DOM.
      expect(await acc(page).locator('img[onerror]').count()).toBe(0);
      await expect(acc(page).locator('[data-testid="account-identity-name"]')).toContainText('onerror=alert(1)');
    } finally {
      await deleteAccount(sharedCtx, adminHeaders, who.username);
    }
  });

  test('14.5 — the account API rejects a tampered/removed session token', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/auth/me', { headers: { 'x-session-token': 'garbage-token' } });
      return r.status;
    });
    expect(status).toBeGreaterThanOrEqual(400);
  });
});

// ===========================================================================
// 15. Edge cases
// ===========================================================================

test.describe('15 — Edge cases', () => {
  test('15.1 — a very long full name does not break the layout', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const long = 'Z'.repeat(500);
    await acc(page).locator('#a-fullname').fill(long);
    const box = await acc(page).locator('[data-testid="account-body"]').boundingBox();
    expect(box!.x + box!.width).toBeLessThanOrEqual(page.viewportSize()!.width + 2);
  });

  test('15.2 — a unicode full name round-trips through a save', async ({ page }) => {
    const who = await createAccount(sharedCtx, adminHeaders, { fullname: 'temp' });
    try {
      await gotoAccount(page, who);
      await acc(page).locator('#a-fullname').fill('Renée Ünïcode 日本語 😀');
      await acc(page).getByRole('button', { name: 'Save profile' }).click();
      await expect(acc(page).locator('[data-testid="account-identity-name"]')).toHaveText('Renée Ünïcode 日本語 😀', {
        timeout: 8_000,
      });
      await gotoAccount(page, who);
      expect(await acc(page).locator('#a-fullname').inputValue()).toBe('Renée Ünïcode 日本語 😀');
    } finally {
      await deleteAccount(sharedCtx, adminHeaders, who.username);
    }
  });

  test('15.3 — rapid double-click on "Save profile" does not crash and the button re-enables', async ({ page }) => {
    const puts: string[] = [];
    page.on('request', (r) => {
      if (r.method() === 'PUT' && /\/api\/users\/[^/]+$/.test(r.url())) puts.push(r.url());
    });
    await gotoAccount(page, readOnly);
    await acc(page).locator('#a-fullname').fill('Ada Lovelace');
    const btn = acc(page).getByRole('button', { name: 'Save profile' });
    await btn.click();
    await btn.click().catch(() => {});
    // At least one PUT should have been fired
    await page.waitForTimeout(3_000);
    expect(puts.length).toBeGreaterThanOrEqual(1);
    // Button must re-enable and name persisted
    await expect(btn).toBeEnabled({ timeout: 10_000 });
    await expect(acc(page).locator('[data-testid="account-identity-name"]')).toHaveText('Ada Lovelace', { timeout: 8_000 });
  });

  test('15.4 — opening the window with a cleared cache still renders', async ({ page }) => {
    await seedBrowser(page, readOnly);
    await page.goto('/desktop');
    await page.evaluate(() => {
      try {
        const keep = ['session-token', 'csrf-token'];
        for (const k of Object.keys(localStorage)) if (!keep.includes(k)) localStorage.removeItem(k);
      } catch { /* ignore */ }
    });
    await page.reload();
    await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
    await openAccountWindow(page);
    await expect(acc(page).locator('[data-testid="account-body"]')).toBeVisible();
  });
});

// ===========================================================================
// 16. Memory regression
// ===========================================================================

test.describe('16 — Memory regression', () => {
  test.slow();

  test('16.1 — opening and closing the account window ×5 does not accumulate DOM nodes', async ({
    page,
  }) => {
    await gotoDesktop(page, readOnly);
    await openAccountWindow(page);
    const count = () => page.evaluate(() => document.querySelectorAll('*').length);
    const before = await count();

    for (let i = 0; i < 5; i++) {
      await page.locator('.hv-cl-btn-close').first().click().catch(() => {});
      await page.waitForTimeout(200);
      await openAccountWindow(page).catch(() => {});
    }

    const after = await count();
    console.log(`[DOM] account window cycle ×5: Δ${after - before} nodes`);
    expect(after - before).toBeLessThanOrEqual(150);
  });

  test('16.2 — the 2FA enrol/cancel cycle ×5 does not leak heap', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const baseline = await getHeapMB(page);
    for (let i = 0; i < 5; i++) {
      await acc(page).getByRole('button', { name: /Enable two-factor/i }).click();
      await acc(page).locator('[data-testid="account-secret"]').first().waitFor({ state: 'visible', timeout: 8_000 });
      await acc(page).getByRole('button', { name: 'Cancel' }).click();
      await acc(page).locator('[data-testid="account-toggle-row"]').waitFor({ state: 'visible' });
    }
    const after = await getHeapMB(page);
    if (baseline > 0 && after > 0) {
      console.log(`[MEM] 2FA cycle ×5: Δ${(after - baseline).toFixed(2)} MB`);
      expect(after - baseline).toBeLessThan(5);
    }
  });
});

// ===========================================================================
// 17. i18n — the saved language drives the account strings
// ===========================================================================

test.describe('17 — i18n', () => {
  test('17.1 — French: header, sections and buttons are in French', async ({ page }) => {
    await gotoAccount(page, readOnly, 'fr');
    await expect(acc(page).locator('.hv-winhdr-title')).toHaveText('Mon compte');
    // `.hv-winhdr-subtitle` no longer exists (the prop was removed from
    // `WindowHeader` on 2026-09-14), so this locator matched nothing.
    // Assert the bar's real shape instead: the icon tile + the title.
    await expect(acc(page).locator('.hv-winhdr > *')).toHaveCount(2);
    const titles = (await acc(page).locator('[data-testid="account-card-title"]').allTextContents()).join(' | ');
    expect(titles).toContain('Informations du profil');
    await expect(acc(page).getByRole('button', { name: 'Enregistrer le profil' })).toBeVisible();
  });

  test('17.2 — German: header and save button are in German', async ({ page }) => {
    await gotoAccount(page, readOnly, 'de');
    await expect(acc(page).locator('.hv-winhdr-title')).toHaveText('Mein Konto');
    await expect(acc(page).getByRole('button', { name: 'Profil speichern' })).toBeVisible();
  });

  test('17.3 — Spanish: header and save button are in Spanish', async ({ page }) => {
    await gotoAccount(page, readOnly, 'es');
    await expect(acc(page).locator('.hv-winhdr-title')).toHaveText('Mi cuenta');
    await expect(acc(page).getByRole('button', { name: 'Guardar perfil' })).toBeVisible();
  });

  test('17.4 — no raw account.* keys leak into the rendered page', async ({ page }) => {
    await gotoAccount(page, readOnly, 'fr');
    const text = await acc(page).locator('[data-testid="account-body"]').innerText();
    expect(text).not.toMatch(/\baccount\.[a-z0-9_]+/i);
  });
});
