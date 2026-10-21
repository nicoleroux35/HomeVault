/* =============================================================================
 * File          : account-expiry.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 20/10/2026
 * Last modified : 20/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Account expiry end-to-end tests (Playwright): the Users detail pane's
 *   "Account expiry" card reads "No expiry" by default, accepts a
 *   datetime-local horizon that the server stores as UTC, and clears back to
 *   "never" — plus the API-level refusal of a past horizon and of a member
 *   call, per TESTING.md (migration 050, users.expires_at).
 *
 * Dependencies:
 *   - none (Playwright against a live HomeVault server)
 *
 * Usage:
 *   npx playwright test tests/e2e/users/account-expiry.spec.ts --project=firefox
 * ============================================================================= */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const MEMBER_PASS = 'ExpiryTest0Key';

interface Tokens {
  username: string;
  password: string;
  token: string;
  csrf: string;
}

function uniq(prefix: string): string {
  return `${prefix}${Date.now().toString(36)}${Math.floor(Math.random() * 1e4)}`;
}

let sharedCtx: APIRequestContext;
let adminHeaders: Record<string, string>;
let member: string;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await playwright.request.newContext({ baseURL: BASE });
  const login = await sharedCtx.post('/api/auth/login', {
    data: { username: ADMIN_USER, password: ADMIN_PASS },
  });
  const body = await login.json().catch(() => ({}));
  expect(body?.data?.token, 'seeded admin login must succeed').toBeTruthy();
  adminHeaders = { 'x-session-token': body.data.token, 'x-csrf-token': body.data.csrf_token };

  member = uniq('exp_');
  const created = await sharedCtx.post('/api/users', {
    headers: adminHeaders,
    data: {
      username: member,
      password: MEMBER_PASS,
      fullname: 'Expiry Target',
      email: '',
      role_id: 1,
    },
  });
  const createdBody = await created.json().catch(() => ({}));
  expect(createdBody?.success, `create must succeed: ${JSON.stringify(createdBody)}`).toBe(true);
});

test.afterAll(async () => {
  if (sharedCtx) {
    await sharedCtx.delete(`/api/users/${member}`, { headers: adminHeaders }).catch(() => {});
    await sharedCtx.dispose().catch(() => {});
  }
});

/** Land on the desktop with an ADMIN session and open the Users app. */
async function gotoUsers(page: Page, tokens: Tokens) {
  await page.addInitScript(
    ({ t }) => {
      try {
        if (t) {
          localStorage.setItem('session-token', t.token);
          localStorage.setItem('csrf-token', t.csrf);
        }
      } catch {
        /* storage unavailable — the test will surface it */
      }
    },
    { t: tokens },
  );
  await page.goto('/desktop');
  await page.locator('.hv-user-pill').waitFor({ timeout: 20000 });
  // Fresh installs show the once-per-version "What's New" modal — close it
  // before driving the dock (its overlay otherwise intercepts the click).
  const modal = page.locator('[data-testid="whats-new-modal"]');
  if (await modal.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await modal.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[title="Users"]');
  await card.waitFor({ state: 'visible', timeout: 10000 });
  await card.click();
  await page.waitForSelector('[data-testid="users-page"]', { timeout: 10000 });
}

/** Open a member's detail pane (search by username, click the row). */
async function openMemberDetail(page: Page, username: string) {
  const scope = page.locator('[data-testid="users-page"]');
  await scope.locator(`label:has-text("${username}"), span:has-text("${username}")`).first().click();
  await expect(scope.locator('[data-testid="users-expiry-edit"]')).toBeVisible();
}

test.describe('Account expiry', () => {
  let adminTokens: Tokens;

  /** Fresh admin tokens (the browser is seeded per test, never shared). */
  async function apiLogin(): Promise<Tokens> {
    const res = await sharedCtx.post('/api/auth/login', {
      data: { username: ADMIN_USER, password: ADMIN_PASS },
    });
    const body = await res.json().catch(() => ({}));
    return {
      username: ADMIN_USER,
      password: ADMIN_PASS,
      token: body?.data?.token || '',
      csrf: body?.data?.csrf_token || '',
    };
  }

  test('1.1 a fresh account reads "No expiry" and has no badge', async ({ page }) => {
    adminTokens = await apiLogin();
    await gotoUsers(page, adminTokens);
    await openMemberDetail(page, member);

    await expect(page.locator('[data-testid="users-expiry-badge"]')).toHaveCount(0);    // The "never" sentence is the account's default state.
    await expect(page.getByText(/No expiry/i).first()).toBeVisible();
  });

  test('1.2 setting a horizon persists it and the server reports it back', async ({ page }) => {
    adminTokens = await apiLogin();
    await gotoUsers(page, adminTokens);
    await openMemberDetail(page, member);

    await page.locator('[data-testid="users-expiry-edit"]').click();
    const input = page.locator('[data-testid="users-expiry-input"]');
    await expect(input).toBeVisible();
    // A fixed future local datetime: 2030-06-15 12:00.
    await input.fill('2030-06-15T12:00');
    await page.locator('[data-testid="users-expiry-save"]').click();

    await expect(page.locator('[data-testid="users-expiry-ok"]')).toBeVisible();
    await expect(page.locator('[data-testid="users-expiry-input"]')).toHaveCount(0);

    // Server-side truth: the stored horizon is the UTC instant of that local time.
    const res = await sharedCtx.get('/api/users', { headers: adminHeaders });
    const body = await res.json().catch(() => ({}));
    const row = (body?.data ?? []).find((u: any) => u.username === member);
    expect(row?.expires_at, 'expiry must be persisted').toBeTruthy();
    expect(new Date(row.expires_at).getTime()).toBe(new Date('2030-06-15T12:00').getTime());
  });

  test('1.3 removing the horizon clears it server-side', async ({ page }) => {
    adminTokens = await apiLogin();
    await gotoUsers(page, adminTokens);
    await openMemberDetail(page, member);

    await page.locator('[data-testid="users-expiry-edit"]').click();
    await page.locator('[data-testid="users-expiry-clear"]').click();
    await expect(page.locator('[data-testid="users-expiry-ok"]')).toBeVisible();

    const res = await sharedCtx.get('/api/users', { headers: adminHeaders });
    const body = await res.json().catch(() => ({}));
    const row = (body?.data ?? []).find((u: any) => u.username === member);
    expect(row?.expires_at ?? null).toBeNull();
  });

  test('1.4 the API refuses a past horizon and a member caller', async () => {
    // Past horizon: would lock the account out silently — 400.
    const past = new Date(Date.now() - 86_400_000).toISOString();
    const bad = await sharedCtx.put(`/api/users/${member}/expiry`, {
      headers: adminHeaders,
      data: { expires_at: past },
    });
    expect(bad.status(), 'past horizon must be refused').toBeGreaterThanOrEqual(400);
    expect(bad.status()).toBeLessThan(500);

    // Member caller: 403/401, never a silent success.
    const memberLogin = await sharedCtx.post('/api/auth/login', {
      data: { username: member, password: MEMBER_PASS },
    });
    const memberBody = await memberLogin.json().catch(() => ({}));
    const denied = await sharedCtx.put(`/api/users/${member}/expiry`, {
      headers: {
        'x-session-token': memberBody?.data?.token || '',
        'x-csrf-token': memberBody?.data?.csrf_token || '',
      },
      data: { expires_at: null },
    });
    expect(denied.ok(), 'a member must not move an expiry horizon').toBe(false);
  });
});
