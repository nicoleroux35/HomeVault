/* =============================================================================
 * File          : group-identity.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 20/10/2026
 * Last modified : 20/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Group identity end-to-end tests (Playwright): the Groups detail pane's
 *   "Identity" card renames a group and stores a description
 *   (`PUT /api/groups/{name}`, 2026-10-20), and a rename carries every
 *   display-name reference with it. Per TESTING.md.
 *
 *   Two layers on purpose:
 *   - the API contract (admin-only, admin-group refusal, a clean 4xx for an
 *     unknown group) and the create form's description field run everywhere;
 *   - selecting a group, saving a description and renaming need a real
 *     HomeVault group, i.e. `groupadd` over NOPASSWD sudo. Those are skipped
 *     with that reason when the host has none — the same documented
 *     limitation as `groups.spec.ts`'s `beforeAll`. The migration of the
 *     dependent rows (mirror, group grants) is covered without a host group
 *     by `tests/groups_integration.rs`.
 *
 * Dependencies:
 *   - none for the API/UI-contract tests; NOPASSWD sudo for the rest
 *
 * Usage:
 *   npx playwright test tests/e2e/groups/group-identity.spec.ts --project=firefox
 * ============================================================================= */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const MEMBER_PASS = 'GrpIdentity0Key';

interface Tokens {
  username: string;
  password: string;
  token: string;
  csrf: string;
}

interface GroupRow {
  name: string;
  display_name: string;
  gid: number;
  member_count: number;
  description?: string | null;
}

function uniq(prefix: string): string {
  return `${prefix}${Date.now().toString(36)}${Math.floor(Math.random() * 1e4)}`;
}

let sharedCtx: APIRequestContext;
let adminHeaders: Record<string, string>;
let member: string;
/** A real HomeVault group to edit, when the host could create one. */
let targetGroup: GroupRow | null = null;
const NO_HOST_GROUP = 'requires NOPASSWD sudo for groupadd (see TESTING.md)';

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await playwright.request.newContext({ baseURL: BASE });
  const login = await sharedCtx.post('/api/auth/login', {
    data: { username: ADMIN_USER, password: ADMIN_PASS },
  });
  const body = await login.json().catch(() => ({}));
  expect(body?.data?.token, 'seeded admin login must succeed').toBeTruthy();
  adminHeaders = { 'x-session-token': body.data.token, 'x-csrf-token': body.data.csrf_token };

  member = uniq('gid_');
  const created = await sharedCtx.post('/api/users', {
    headers: adminHeaders,
    data: {
      username: member,
      password: MEMBER_PASS,
      fullname: 'Group Identity Target',
      email: '',
      role_id: 1,
    },
  });
  const createdBody = await created.json().catch(() => ({}));
  expect(createdBody?.success, `create must succeed: ${JSON.stringify(createdBody)}`).toBe(true);

  // Try to provision a throwaway group of our own; best effort (needs sudo).
  const owned = uniq('gidteam_');
  const groupRes = await sharedCtx.post('/api/groups', {
    headers: adminHeaders,
    data: { name: owned, members: [member], description: 'Created by the identity spec' },
  });
  const groupBody = await groupRes.json().catch(() => ({}));

  // Whatever group is editable: ours when the host allowed it, otherwise the
  // first non-protected group already on the server (if any).
  const listed = await sharedCtx.get('/api/groups', { headers: adminHeaders });
  const listedBody = await listed.json().catch(() => ({}));
  const rows = (listedBody?.data ?? []) as GroupRow[];
  targetGroup =
    (groupBody?.success ? rows.find((g) => g.display_name === owned) : undefined) ??
    rows.find((g) => g.display_name !== 'admin') ??
    null;
});

test.afterAll(async () => {
  if (!sharedCtx) return;
  // Only delete what we created (a group found on the server is not ours).
  if (targetGroup?.display_name.startsWith('gidteam_')) {
    await sharedCtx
      .delete(`/api/groups/${encodeURIComponent(targetGroup.display_name)}`, {
        headers: adminHeaders,
      })
      .catch(() => {});
  }
  await sharedCtx.delete(`/api/users/${member}`, { headers: adminHeaders }).catch(() => {});
  await sharedCtx.dispose().catch(() => {});
});

/** Land on the desktop with an ADMIN session and open the Groups app. */
async function gotoGroups(page: Page, tokens: Tokens) {
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
  const card = page.locator('.lp-card[title="Groups"]');
  await card.waitFor({ state: 'visible', timeout: 10000 });
  await card.click();
  await page.waitForSelector('[data-testid="groups-page"]', { timeout: 10000 });
}

/** Select a group by its display name in the list rail. */
async function selectGroup(page: Page, displayName: string) {
  const scope = page.locator('[data-testid="groups-page"]');
  await scope.locator(`[data-testid="groups-row"]:has-text("${displayName}")`).first().click();
  await expect(scope.locator('[data-testid="groups-meta-card"]')).toBeVisible();
}

test.describe('Group identity (rename + description)', () => {
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

  test('1.1 the update endpoint is admin-only', async () => {
    // A member cannot rename or describe a group.
    const login = await sharedCtx.post('/api/auth/login', {
      data: { username: member, password: MEMBER_PASS },
    });
    const body = await login.json().catch(() => ({}));
    const memberHeaders = {
      'x-session-token': body?.data?.token || '',
      'x-csrf-token': body?.data?.csrf_token || '',
    };
    const denied = await sharedCtx.put('/api/groups/whatever', {
      headers: memberHeaders,
      data: { description: 'nope' },
    });
    expect(denied.ok(), 'the identity update is admin-only').toBe(false);
  });

  test('1.2 an unknown group is a clean client error, never a 500', async () => {
    const unknown = await sharedCtx.put(`/api/groups/${uniq('ghost_')}`, {
      headers: adminHeaders,
      data: { name: uniq('ghost_') },
    });
    expect(unknown.status(), 'unknown group must be a clean client error').toBeGreaterThanOrEqual(
      400,
    );
    expect(unknown.status()).toBeLessThan(500);
  });

  test('1.3 the admin group cannot be renamed', async () => {
    // The admin group carries the sudo grant: renaming it would leave the
    // sudoers rule dangling. Skipped when the host exposes no such group.
    const listed = await sharedCtx.get('/api/groups', { headers: adminHeaders });
    const listedBody = await listed.json().catch(() => ({}));
    const adminGroup = ((listedBody?.data ?? []) as GroupRow[]).find(
      (g) => g.display_name === 'admin',
    );
    test.skip(!adminGroup, 'no admin system group visible on this server');

    const refused = await sharedCtx.put('/api/groups/admin', {
      headers: adminHeaders,
      data: { name: uniq('admins_') },
    });
    expect(refused.ok(), 'the admin group cannot be renamed').toBe(false);
    const refusedBody = await refused.json().catch(() => ({}));
    expect(JSON.stringify(refusedBody)).toMatch(/admin group/i);
  });

  test('1.4 the create form exposes the description field', async ({ page }) => {
    adminTokens = await apiLogin();
    await gotoGroups(page, adminTokens);
    await page.locator('[data-testid="groups-new"]').click();

    const input = page.locator('[data-testid="groups-create-description"]');
    await expect(input).toBeVisible();
    await input.fill('Household members');
    await expect(input).toHaveValue('Household members');
  });

  test('1.5 a description is saved from the Identity card and read back', async ({ page }) => {
    test.skip(!targetGroup, NO_HOST_GROUP);
    adminTokens = await apiLogin();
    await gotoGroups(page, adminTokens);
    await selectGroup(page, targetGroup!.display_name);

    const description = `Household ${Date.now().toString(36)}`;
    const input = page.locator('[data-testid="groups-description-input"]');
    await input.fill(description);
    await page.locator('[data-testid="groups-meta-card"] button:has-text("Save")').click();

    await expect(page.locator('[data-testid="groups-meta-ok"]')).toBeVisible();

    // Server-side truth.
    const listed = await sharedCtx.get('/api/groups', { headers: adminHeaders });
    const listedBody = await listed.json().catch(() => ({}));
    const row = ((listedBody?.data ?? []) as GroupRow[]).find(
      (g) => g.display_name === targetGroup!.display_name,
    );
    expect(row?.description, 'the description must be persisted').toBe(description);

    // Clearing it is not the same as leaving it alone.
    await input.fill('');
    await page.locator('[data-testid="groups-meta-card"] button:has-text("Save")').click();
    await expect(page.locator('[data-testid="groups-meta-ok"]')).toBeVisible();
    const again = await sharedCtx.get('/api/groups', { headers: adminHeaders });
    const againBody = await again.json().catch(() => ({}));
    const cleared = ((againBody?.data ?? []) as GroupRow[]).find(
      (g) => g.display_name === targetGroup!.display_name,
    );
    expect(cleared?.description ?? null).toBeNull();
  });

  test('1.6 renaming the group carries its description and updates the list', async ({ page }) => {
    test.skip(!targetGroup, NO_HOST_GROUP);
    adminTokens = await apiLogin();

    const from = targetGroup!.display_name;
    const to = uniq('renamed_');
    const res = await sharedCtx.put(`/api/groups/${encodeURIComponent(from)}`, {
      headers: adminHeaders,
      data: { name: to, description: 'Renamed by the identity spec' },
    });
    if (!res.ok()) {
      const body = await res.text();
      // No NOPASSWD sudo on this host: the OS rename cannot run, and that is
      // a documented environment limit, not a product failure.
      test.skip(/sudo|password|mot de passe/i.test(body), NO_HOST_GROUP);
      throw new Error(`rename failed: ${res.status()} ${body}`);
    }

    try {
      const listed = await sharedCtx.get('/api/groups', { headers: adminHeaders });
      const listedBody = await listed.json().catch(() => ({}));
      const rows = (listedBody?.data ?? []) as GroupRow[];
      const moved = rows.find((g) => g.display_name === to);
      expect(moved, 'the renamed group must be listed under its new name').toBeTruthy();
      expect(moved!.description, 'the description follows the rename').toBe(
        'Renamed by the identity spec',
      );
      expect(rows.some((g) => g.display_name === from)).toBe(false);

      // The UI shows the new name and the description.
      await gotoGroups(page, adminTokens);
      await selectGroup(page, to);
      await expect(
        page.locator('[data-testid="groups-detail-h1"]:has-text("' + to + '")'),
      ).toBeVisible();
      await expect(page.locator('[data-testid="groups-detail-description"]')).toContainText(
        'Renamed by the identity spec',
      );
    } finally {
      // Always put the host back the way we found it.
      await sharedCtx
        .put(`/api/groups/${encodeURIComponent(to)}`, {
          headers: adminHeaders,
          data: { name: from },
        })
        .catch(() => {});
    }
  });
});
