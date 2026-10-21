/* =============================================================================
 * File          : admin-sessions.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 20/10/2026
 * Last modified : 20/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Admin per-user session management end-to-end tests (Playwright): the
 *   Users detail pane's Security card lists a member's active sessions with
 *   masked token references and a "Sign out everywhere" action, per
 *   TESTING.md.
 *
 * Dependencies:
 *   - none (Playwright against a live HomeVault server)
 *
 * Usage:
 *   npx playwright test tests/e2e/users/admin-sessions.spec.ts --project=firefox
 * ============================================================================= */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const MEMBER_PASS = 'SessTest0KeyZ';

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

  member = uniq('sess_');
  const created = await sharedCtx.post('/api/users', {
    headers: adminHeaders,
    data: {
      username: member,
      password: MEMBER_PASS,
      fullname: 'Session Target',
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
  await expect(scope.locator('#d-sessions-title')).toBeVisible();
}

test.describe('Admin per-user sessions', () => {
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

  test('1.1 the Security card lists the member sessions with masked tokens', async ({ page }) => {
    // Two live sessions: two API logins of the member.
    for (let i = 0; i < 2; i++) {
      await sharedCtx.post('/api/auth/login', {
        data: { username: member, password: MEMBER_PASS },
      });
    }
    adminTokens = await apiLogin();
    await gotoUsers(page, adminTokens);
    await openMemberDetail(page, member);

    const rows = page.locator('[data-testid="users-sessions-list"] [data-testid="users-session-row"]');
    await expect(rows).toHaveCount(2);
    const firstRef = await rows.first().locator('code').textContent();
    expect((firstRef ?? '').endsWith('…'), 'masked reference must end with the ellipsis').toBe(true);
    const listText = (await page.locator('[data-testid="users-sessions-list"]').textContent()) ?? '';
    // No API login token ever reaches the DOM: only the 9-char mask + ellipsis.
    expect(listText.length).toBeLessThan(400);
  });

  test('1.2 Sign out everywhere empties the list, server-side truth agrees', async ({ page }) => {
    adminTokens = await apiLogin();
    await gotoUsers(page, adminTokens);
    await openMemberDetail(page, member);

    await page.locator('[data-testid="users-sessions-revoke"]').click();
    await expect(page.locator('[data-testid="users-sessions-empty"]')).toBeVisible();
    // Success notice, not an error.
    await expect(page.locator('[data-testid="users-ok"]')).toBeVisible();

    // Server-side truth: zero live sessions for the member.
    const me = await sharedCtx.get(`/api/users/${member}/sessions`, { headers: adminHeaders });
    const body = await me.json().catch(() => ({}));
    expect(body?.data?.total).toBe(0);

    // ... and the member can simply sign back in (identity intact).
    const back = await sharedCtx.post('/api/auth/login', {
      data: { username: member, password: MEMBER_PASS },
    });
    expect(back.status()).toBe(200);
  });
});
