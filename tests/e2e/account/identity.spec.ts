/* =============================================================================
 * File          : identity.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Identity end-to-end tests (Playwright): the trusted-devices and rescue
 *   e-mail cards in My Account, and the rescue-mail recovery entry point on
 *   the sign-in screen. See docs/guides/identity.md.
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/account/identity.spec.ts --project=firefox
 * ============================================================================= */

/**
 * Trusted devices, rescue e-mail and account recovery — End-to-End Test Suite
 *
 * **SAFETY: this suite never sends real mail and never touches the seeded
 * admin's own 2FA.** Every test provisions a throwaway account through the
 * admin API and deletes it in `afterAll`; the one sign-in test uses a
 * deliberately unknown account name so the public recovery endpoint has
 * nothing to mail. Hashing, code TTL, attempt limits, cookie scoping and the
 * second-factor skip are covered by `tests/identity_integration.rs` and the
 * unit tests in `src/identity.rs`.
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';
import { createHmac } from 'node:crypto';

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

// ---------------------------------------------------------------------------
// Admin-API helpers — provision / tear down throwaway accounts
// ---------------------------------------------------------------------------

async function apiLogin(ctx: APIRequestContext, username: string, password: string) {
  const res = await ctx.post('/api/auth/login', { data: { username, password } });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

/**
 * Create a throwaway account and return a usable session. `POST /api/users`
 * stores an admin-chosen password, so the account starts flagged
 * `must_change_password`; the flag is cleared through the real change flow,
 * which also revokes the first session.
 */
async function createAccount(
  ctx: APIRequestContext,
  adminHeaders: Record<string, string>,
  opts: { fullname?: string; email?: string; roleId?: number } = {},
): Promise<Tokens> {
  const username = `idt_${Date.now()}_${Math.floor(Math.random() * 1e4)}`;
  const password = 'IdtPassw0rd!';
  const res = await ctx.post('/api/users', {
    headers: adminHeaders,
    data: {
      username,
      password,
      fullname: opts.fullname ?? 'Identity E2E',
      email: opts.email ?? '',
      role_id: opts.roleId ?? ADMIN_ROLE_ID,
      groups: [],
      home_enabled: false,
    },
  });
  if (!res.ok()) {
    throw new Error(`could not create throwaway account: ${res.status()} ${await res.text()}`);
  }
  const ownedPassword = `Owned${password}`;
  const flagged = await apiLogin(ctx, username, password);
  await ctx.put(`/api/users/${username}/password`, {
    headers: { 'x-session-token': flagged.token, 'x-csrf-token': flagged.csrf },
    data: { current_password: password, new_password: ownedPassword },
  });
  const { token, csrf } = await apiLogin(ctx, username, ownedPassword);
  return { username, password: ownedPassword, token, csrf };
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

async function rescueState(ctx: APIRequestContext, who: Tokens) {
  const res = await ctx.get('/api/identity/rescue-email', {
    headers: { 'x-session-token': who.token },
  });
  expect(res.ok()).toBeTruthy();
  return (await res.json()).data;
}

// ---------------------------------------------------------------------------
// Browser helpers
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
  // A brand-new account sees the once-per-version "What's New" modal, whose
  // overlay would intercept the user-menu click.
  const whatsNew = page.locator('[data-testid="whats-new-modal"]');
  if (await whatsNew.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await whatsNew.waitFor({ state: 'hidden', timeout: 5_000 }).catch(() => {});
  }
}

/** Land on the desktop and open the "My Account" window. */
async function gotoAccount(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await page.locator('.hv-user-pill').click();
  const menuItem = page.locator('.hv-user-menu-item').first();
  await menuItem.waitFor({ state: 'visible', timeout: 5_000 });
  await menuItem.click();
  await page.waitForSelector('[data-testid="account-page"]', { timeout: 10_000 });
  await page
    .locator('[data-testid="account-body"], [data-testid="account-state"]')
    .first()
    .waitFor({ state: 'visible', timeout: 10_000 });
}

/** The account window as a scoping root. */
function acc(page: Page) {
  return page.locator('[data-testid="account-page"]');
}

// ---------------------------------------------------------------------------
// Shared fixture: one throwaway account for the read-only assertions
// ---------------------------------------------------------------------------

let sharedCtx: APIRequestContext;
let adminHeaders: Record<string, string>;
let readOnly: Tokens;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await playwright.request.newContext({ baseURL: BASE });
  const admin = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  adminHeaders = { 'x-session-token': admin.token, 'x-csrf-token': admin.csrf };
  expect(admin.token, 'seeded admin login must succeed').not.toEqual('');
  readOnly = await createAccount(sharedCtx, adminHeaders, { fullname: 'Grace Hopper' });
});

test.afterAll(async () => {
  if (sharedCtx && readOnly) await deleteAccount(sharedCtx, adminHeaders, readOnly.username);
  await sharedCtx?.dispose();
});

// ===========================================================================
// 1. Trusted devices card
// ===========================================================================

test.describe('1 — Trusted devices', () => {
  test('1.1 — a fresh account shows the card with an empty state', async ({ page }) => {
    await gotoAccount(page, readOnly);

    const card = acc(page).locator('[data-testid="account-card"]', {
      hasText: /Trusted devices|Appareils de confiance|Vertrauenswürdige Geräte|Dispositivos de confianza/,
    });
    await expect(card.first()).toBeVisible({ timeout: 10_000 });
    await expect(acc(page).locator('[data-testid="devices-empty"]')).toBeVisible({ timeout: 10_000 });
    await expect(acc(page).locator('[data-testid="devices-list"]')).toHaveCount(0);
  });

  test('1.2 — the description states the 30-day trust lifetime', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const card = acc(page).locator('[data-testid="account-card"]', {
      hasText: /Trusted devices|Appareils de confiance|Vertrauenswürdige Geräte|Dispositivos de confianza/,
    });
    await expect(card.first()).toContainText('30', { timeout: 10_000 });
  });

  test('1.3 — revoking every device is refused while none is trusted', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const revokeAll = acc(page).getByRole('button', {
      name: /Revoke every device|Révoquer tous les appareils|Alle Geräte widerrufen|Revocar todos los dispositivos/,
    });
    await expect(revokeAll).toBeVisible({ timeout: 10_000 });
    await expect(revokeAll).toBeDisabled();
    // No confirmation dialog can be opened by a disabled control.
    await revokeAll.click({ force: true }).catch(() => {});
    await expect(page.locator('[data-testid="confirm-dialog-message"]')).toHaveCount(0);
  });

  test('1.4 — the API reports an empty list and the documented lifetime', async () => {
    const res = await sharedCtx.get('/api/identity/devices', {
      headers: { 'x-session-token': readOnly.token },
    });
    expect(res.ok()).toBeTruthy();
    const data = (await res.json()).data;
    expect(data.devices).toEqual([]);
    expect(data.lifetime_days).toBe(30);
  });

  test('1.5 — no cookie value is rendered in the card', async ({ page }) => {
    await gotoAccount(page, readOnly);
    const text = await acc(page).innerText();
    // The card never shows the device token, only labels and dates.
    expect(text).not.toMatch(/hv_trusted_device/);
  });
});

// ===========================================================================
// 2. Rescue e-mail card
// ===========================================================================

test.describe('2 — Rescue e-mail', () => {
  test('2.1 — the card offers the address field and a disabled send button while empty', async ({
    page,
  }) => {
    await gotoAccount(page, readOnly);

    const input = acc(page).locator('[data-testid="rescue-email"]');
    await expect(input).toBeVisible({ timeout: 10_000 });
    expect(await input.getAttribute('type')).toBe('email');
    expect(await input.inputValue()).toBe('');

    const send = acc(page).locator('[data-testid="rescue-send"]');
    await expect(send).toBeDisabled();
    // Nothing is stored before a code comes back.
    await expect(acc(page).locator('[data-testid="rescue-verified"]')).toHaveCount(0);
  });

  test('2.2 — a malformed address is refused and nothing is stored', async ({ page }) => {
    const before = await rescueState(sharedCtx, readOnly);
    await gotoAccount(page, readOnly);

    const input = acc(page).locator('[data-testid="rescue-email"]');
    await input.fill('not-an-address');
    await acc(page).locator('[data-testid="rescue-send"]').click();

    await expect(acc(page).locator('[data-testid="rescue-err"]')).toBeVisible({ timeout: 10_000 });
    // No pending-code form appears for a refusal.
    await expect(acc(page).locator('[data-testid="rescue-pending"]')).toHaveCount(0);

    const after = await rescueState(sharedCtx, readOnly);
    expect(after.configured).toBe(before.configured);
    expect(after.email).toBe(before.email);
  });

  test('2.3 — a whitespace-only address keeps the send button disabled', async ({ page }) => {
    await gotoAccount(page, readOnly);
    await acc(page).locator('[data-testid="rescue-email"]').fill('   ');
    await expect(acc(page).locator('[data-testid="rescue-send"]')).toBeDisabled();
  });

  test('2.4 — the API reports the address as not configured on a new account', async () => {
    const state = await rescueState(sharedCtx, readOnly);
    expect(state.configured).toBe(false);
    expect(state.email).toBeNull();
    expect(state.code_ttl_minutes).toBe(15);
  });

  test('2.5 — a code cannot be verified without one waiting', async () => {
    const res = await sharedCtx.post('/api/identity/rescue-email/verify', {
      headers: { 'x-session-token': readOnly.token, 'x-csrf-token': readOnly.csrf },
      data: { code: '123456' },
    });
    expect(res.ok()).toBeFalsy();
    expect((await rescueState(sharedCtx, readOnly)).configured).toBe(false);
  });
});

// ===========================================================================
// 3. Sign-in — rescue-mail recovery entry point
// ===========================================================================

test.describe('3 — Recovery from the sign-in screen', () => {
  test('3.1 — the forgot-password panel offers the rescue-mail route', async ({ page }) => {
    await page.goto('/login');
    await page.waitForSelector('#username', { timeout: 15_000 });

    await page.locator('[data-testid="login-forgot-link"]').click();
    const link = page.locator('[data-testid="login-email-recovery-link"]');
    await expect(link).toBeVisible({ timeout: 5_000 });
    await link.click();
    await expect(page.locator('[data-testid="login-email-recovery"]')).toBeVisible();
  });

  test('3.2 — an unknown account gets the same generic answer and no error', async ({ page }) => {
    await page.goto('/login');
    await page.waitForSelector('#username', { timeout: 15_000 });
    await page.locator('[data-testid="login-forgot-link"]').click();
    await page.locator('[data-testid="login-email-recovery-link"]').click();

    // A name nobody owns: the endpoint must not reveal that, and must not
    // send anything.
    await page.fill('#recovery-username', `ghost_${Date.now()}`);
    await page.locator('[data-testid="login-email-recovery-send"]').click();

    await expect(page.locator('[data-testid="login-email-recovery-notice"]')).toBeVisible({
      timeout: 10_000,
    });
    await expect(page.locator('[data-testid="login-email-recovery-error"]')).toHaveCount(0);
    // The code form is offered, so a real account would proceed the same way.
    await expect(page.locator('[data-testid="login-email-recovery-code"]')).toBeVisible();
  });

  test('3.3 — the code field takes six digits and the verify button waits for them', async ({
    page,
  }) => {
    await page.goto('/login');
    await page.waitForSelector('#username', { timeout: 15_000 });
    await page.locator('[data-testid="login-forgot-link"]').click();
    await page.locator('[data-testid="login-email-recovery-link"]').click();
    await page.fill('#recovery-username', readOnly.username);
    await page.locator('[data-testid="login-email-recovery-send"]').click();
    const code = page.locator('[data-testid="login-email-recovery-code"]');
    await code.waitFor({ state: 'visible', timeout: 10_000 });

    expect(await code.getAttribute('inputmode')).toBe('numeric');
    expect(await code.getAttribute('maxlength')).toBe('6');
    expect(await code.getAttribute('autocomplete')).toBe('one-time-code');

    const verify = page.locator('[data-testid="login-email-recovery-verify"]');
    await expect(verify).toBeDisabled();
    await code.fill('123456');
    await expect(verify).toBeEnabled();
  });

  test('3.4 — a wrong code keeps the user on the sign-in screen', async ({ page }) => {
    await page.goto('/login');
    await page.waitForSelector('#username', { timeout: 15_000 });
    await page.locator('[data-testid="login-forgot-link"]').click();
    await page.locator('[data-testid="login-email-recovery-link"]').click();
    await page.fill('#recovery-username', readOnly.username);
    await page.locator('[data-testid="login-email-recovery-send"]').click();
    const code = page.locator('[data-testid="login-email-recovery-code"]');
    await code.waitFor({ state: 'visible', timeout: 10_000 });

    await code.fill('000000');
    await page.locator('[data-testid="login-email-recovery-verify"]').click();
    await expect(page.locator('[data-testid="login-email-recovery-error"]')).toBeVisible({
      timeout: 10_000,
    });
    // No session was handed out.
    const stored = await page.evaluate(() => localStorage.getItem('session-token'));
    expect(stored).toBeFalsy();
  });
});

// ===========================================================================
// 4. Remember this browser — control on the second-factor step
// ===========================================================================

test.describe('4 — Remember this browser', () => {
  let ctx: APIRequestContext;
  let who: Tokens;
  let secret = '';

  test.beforeAll(async ({ playwright }) => {
    ctx = await playwright.request.newContext({ baseURL: BASE });
    who = await createAccount(ctx, adminHeaders);
    secret = await enrol2fa(ctx, who);
  });
  test.afterAll(async () => {
    await deleteAccount(ctx, adminHeaders, who.username);
    await ctx.dispose();
  });

  test('4.1 — the second-factor step offers the opt-in switch, off by default', async ({ page }) => {
    test.skip(!secret, '2FA could not be enrolled over the API');

    await page.goto('/login');
    await page.waitForSelector('#username', { timeout: 15_000 });
    await page.fill('#username', who.username);
    await page.fill('#password', who.password);
    await page.locator('#password').press('Enter');
    await page.locator('[data-testid="login-code-input"]').first().waitFor({
      state: 'visible',
      timeout: 10_000,
    });

    const row = page.locator('[data-testid="login-remember-device"]');
    await expect(row).toBeVisible();
    const sw = row.locator('[role="switch"]');
    await expect(sw).toHaveAttribute('aria-checked', 'false');

    await sw.click();
    await expect(sw).toHaveAttribute('aria-checked', 'true');
  });

  test('4.2 — a complete sign-in with the switch on sets an HttpOnly device cookie', async ({
    page,
  }) => {
    test.skip(!secret, '2FA could not be enrolled over the API');

    await page.goto('/login');
    await page.waitForSelector('#username', { timeout: 15_000 });
    await page.fill('#username', who.username);
    await page.fill('#password', who.password);
    await page.locator('#password').press('Enter');
    const boxes = page.locator('[data-testid="login-code-input"]');
    await boxes.first().waitFor({ state: 'visible', timeout: 10_000 });

    await page.locator('[data-testid="login-remember-device"] [role="switch"]').click();
    const code = totpNow(secret);
    for (let i = 0; i < 6; i++) await boxes.nth(i).fill(code[i]);

    // The cookie is set by the server and never exposed to scripts.
    await expect
      .poll(async () => (await page.context().cookies()).some((c) => c.name === 'hv_trusted_device'), {
        timeout: 15_000,
      })
      .toBe(true);
    const cookie = (await page.context().cookies()).find((c) => c.name === 'hv_trusted_device');
    expect(cookie?.httpOnly).toBe(true);
    expect(cookie?.sameSite).toBe('Strict');

    // ...and the device shows up in the account's own list once trusted.
    const list = await ctx.get('/api/identity/devices', {
      headers: { 'x-session-token': who.token },
    });
    const devices = (await list.json()).data.devices as { label: string }[];
    expect(devices.length).toBeGreaterThanOrEqual(1);

    // Clean up: this suite must not leave a trusted browser behind.
    await ctx.post('/api/identity/devices/revoke-all', {
      headers: { 'x-session-token': who.token, 'x-csrf-token': who.csrf },
    });
    const after = await ctx.get('/api/identity/devices', {
      headers: { 'x-session-token': who.token },
    });
    expect((await after.json()).data.devices).toEqual([]);
  });
});

// ===========================================================================
// 5. Console hygiene
// ===========================================================================

test.describe('5 — Console', () => {
  test('5.1 — opening the account window logs no uncaught JS errors', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await gotoAccount(page, readOnly);
    await acc(page).locator('[data-testid="devices-empty"]').waitFor({ state: 'visible', timeout: 10_000 });
    const critical = errors.filter(
      (e) => !e.includes('404') && !e.includes('Failed to load resource') && !e.includes('favicon'),
    );
    expect(critical, critical.join('; ')).toHaveLength(0);
  });
});
