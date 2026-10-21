/* =============================================================================
 * File          : group-access.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 20/10/2026
 * Last modified : 20/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Group-level resource access end-to-end tests (Playwright): an admin grants
 *   a HomeVault group access to an app (migration 051,
 *   `resource_group_shares`), the Users detail pane lists it under "Inherited
 *   from groups", and revoking it from the card removes it — per TESTING.md.
 *
 *   Two layers on purpose:
 *   - the grant CRUD + the admin-only rule are pure database work, so they run
 *     everywhere;
 *   - the *inheritance* (a member actually seeing the resource) needs a real
 *     OS group and therefore `groupadd` over sudo. That half is skipped with a
 *     reason when the host has no NOPASSWD sudo — the same documented
 *     limitation as `users.spec.ts`'s `beforeAll`. `tests/
 *     resource_shares_integration.rs` covers the union rule itself without
 *     touching the host.
 *
 * Dependencies:
 *   - sudo NOPASSWD for `groupadd`/`groupdel` on the inheritance tests only
 *
 * Usage:
 *   npx playwright test tests/e2e/users/group-access.spec.ts --project=firefox
 * ============================================================================= */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const MEMBER_PASS = 'GrpAccess0Key';

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
let group: string;
/** `false` when the host has no NOPASSWD sudo — the inheritance half is skipped. */
let groupAvailable = false;
const NO_SUDO = 'requires NOPASSWD sudo for groupadd (see TESTING.md)';

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await playwright.request.newContext({ baseURL: BASE });
  const login = await sharedCtx.post('/api/auth/login', {
    data: { username: ADMIN_USER, password: ADMIN_PASS },
  });
  const body = await login.json().catch(() => ({}));
  expect(body?.data?.token, 'seeded admin login must succeed').toBeTruthy();
  adminHeaders = { 'x-session-token': body.data.token, 'x-csrf-token': body.data.csrf_token };

  member = uniq('grp_');
  const created = await sharedCtx.post('/api/users', {
    headers: adminHeaders,
    data: {
      username: member,
      password: MEMBER_PASS,
      fullname: 'Group Access Target',
      email: '',
      role_id: 1,
    },
  });
  const createdBody = await created.json().catch(() => ({}));
  expect(createdBody?.success, `create must succeed: ${JSON.stringify(createdBody)}`).toBe(true);

  // A throwaway HomeVault group with the member in it — best effort: the OS
  // call needs sudo, which the test host may not grant.
  group = uniq('grpacc');
  const groupRes = await sharedCtx.post('/api/groups', {
    headers: adminHeaders,
    data: { name: group, members: [member] },
  });
  const groupBody = await groupRes.json().catch(() => ({}));
  groupAvailable = groupBody?.success === true;
});

test.afterAll(async () => {
  if (!sharedCtx) return;
  if (groupAvailable) {
    await sharedCtx
      .delete(`/api/groups/${encodeURIComponent(group)}`, { headers: adminHeaders })
      .catch(() => {});
  }
  await sharedCtx.delete(`/api/users/${member}`, { headers: adminHeaders }).catch(() => {});
  await sharedCtx.dispose().catch(() => {});
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
  await expect(scope.locator('[data-testid="users-access-groups"]')).toBeVisible();
}

test.describe('Group-level resource access', () => {
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

  test('1.1 group grants are admin-only and CRUD works on the server', async () => {
    // A grant row needs no OS group — the table keys on the name only, so the
    // CRUD contract is testable on every host.
    const created = await sharedCtx.post('/api/shares/access/groups', {
      headers: adminHeaders,
      data: { group: 'e2e_ghost_group', kind: 'app', resource: 'jellyfin', permission: 'write' },
    });
    expect(created.ok(), 'admin grant must succeed').toBe(true);

    const listed = await sharedCtx.get(
      '/api/shares/access/groups?group=e2e_ghost_group',
      { headers: adminHeaders },
    );
    const listedBody = await listed.json().catch(() => ({}));
    expect((listedBody?.data ?? []).length).toBe(1);
    expect(listedBody.data[0].permission).toBe('write');

    // A member cannot grant to a group, nor read the group listing.
    const login = await sharedCtx.post('/api/auth/login', {
      data: { username: member, password: MEMBER_PASS },
    });
    const body = await login.json().catch(() => ({}));
    const memberHeaders = {
      'x-session-token': body?.data?.token || '',
      'x-csrf-token': body?.data?.csrf_token || '',
    };
    const denied = await sharedCtx.post('/api/shares/access/groups', {
      headers: memberHeaders,
      data: { group: 'e2e_ghost_group', kind: 'app', resource: 'jellyfin', permission: 'full' },
    });
    expect(denied.ok(), 'group grants are admin-only').toBe(false);
    const deniedList = await sharedCtx.get('/api/shares/access/groups', {
      headers: memberHeaders,
    });
    expect(deniedList.ok(), 'the group listing is admin-only').toBe(false);

    // Clean up the ghost grant.
    const revoked = await sharedCtx.delete(
      '/api/shares/access/groups?group=e2e_ghost_group&kind=app&resource=jellyfin',
      { headers: adminHeaders },
    );
    expect(revoked.ok(), 'revoke must succeed').toBe(true);
  });

  test('1.2 the card shows the "no groups" state for a member without groups', async ({ page }) => {
    adminTokens = await apiLogin();
    await gotoUsers(page, adminTokens);
    await openMemberDetail(page, member);

    // This member is in no HomeVault group unless the host allowed 1.3's
    // group — in which case the list is empty but the section still renders.
    const empty = page.locator(
      '[data-testid="users-access-group-empty"], [data-testid="users-access-group-list"]',
    );
    await expect(empty.first()).toBeVisible();
    if (!groupAvailable) {
      await expect(page.locator('[data-testid="users-access-group-empty"]')).toBeVisible();
    }
  });

  test('1.3 a group grant reaches the member through the union', async () => {
    test.skip(!groupAvailable, NO_SUDO);

    const granted = await sharedCtx.post('/api/shares/access/groups', {
      headers: adminHeaders,
      data: { group, kind: 'app', resource: 'jellyfin', permission: 'write' },
    });
    expect(granted.ok(), 'group grant must succeed').toBe(true);

    const login = await sharedCtx.post('/api/auth/login', {
      data: { username: member, password: MEMBER_PASS },
    });
    const body = await login.json().catch(() => ({}));
    const mine = await sharedCtx.get('/api/shares/access/mine', {
      headers: { 'x-session-token': body?.data?.token || '' },
    });
    const mineBody = await mine.json().catch(() => ({}));
    const inherited = mineBody?.data?.inherited ?? [];
    expect(
      inherited.some(
        (g: any) => g.group_name === group && g.resource === 'jellyfin' && g.permission === 'write',
      ),
      `the member must inherit the group grant: ${JSON.stringify(mineBody?.data)}`,
    ).toBe(true);
    // ... and it is NOT a direct row.
    expect((mineBody?.data?.apps ?? []).length).toBe(0);
  });

  test('1.4 the Users card lists the inherited grant and revokes it', async ({ page }) => {
    test.skip(!groupAvailable, NO_SUDO);

    adminTokens = await apiLogin();
    await gotoUsers(page, adminTokens);
    await openMemberDetail(page, member);

    const rows = page.locator(
      '[data-testid="users-access-group-list"] [data-testid="users-access-group-row"]',
    );
    await expect(rows).toHaveCount(1);
    await expect(rows.first()).toContainText(group);
    await expect(rows.first()).toContainText('jellyfin');

    await rows.first().locator('button').click();
    await expect(page.locator('[data-testid="users-access-group-empty"]')).toBeVisible();

    // Server-side truth: the member no longer inherits it.
    const login = await sharedCtx.post('/api/auth/login', {
      data: { username: member, password: MEMBER_PASS },
    });
    const body = await login.json().catch(() => ({}));
    const mine = await sharedCtx.get('/api/shares/access/mine', {
      headers: { 'x-session-token': body?.data?.token || '' },
    });
    const mineBody = await mine.json().catch(() => ({}));
    expect((mineBody?.data?.inherited ?? []).length).toBe(0);
  });
});
