/**
 * Groups page — End-to-End Test Suite
 *
 * The Groups app is not a route: it is a window opened from the desktop's
 * Applications launcher (Users/Groups are not pinned to the dock). Every
 * suite provisions its own throwaway users/groups through the real admin
 * API, injects an ADMIN session into `localStorage`, loads `/desktop`, opens
 * the launcher, and clicks the "Groups" tile. Throwaway groups/users are
 * always deleted in `afterAll`/`afterEach`, so the seeded e2e admin and the
 * real `homevault_admin` system group are never mutated.
 *
 * Run (all):         npx playwright test tests/e2e/groups/groups.spec.ts --project=firefox
 * Run (single):      npx playwright test tests/e2e/groups/groups.spec.ts -g "4.1"
 * Run (category):    npx playwright test tests/e2e/groups/groups.spec.ts -g "Security"
 * Custom server:     BASE_URL=http://localhost:8080 npx playwright test tests/e2e/groups/groups.spec.ts
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

// ---------------------------------------------------------------------------
// Constants & low-level helpers
// ---------------------------------------------------------------------------

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const USER_ROLE_ID = 1;

interface Tokens {
  username: string;
  password: string;
  token: string;
  csrf: string;
}

function uniq(prefix: string): string {
  return `${prefix}${Date.now().toString(36)}${Math.floor(Math.random() * 1e4)}`;
}

async function getHeapMB(page: Page): Promise<number> {
  return page.evaluate(() =>
    'memory' in performance ? (performance as any).usedJSHeapSize / (1024 * 1024) : 0,
  );
}

// ---------------------------------------------------------------------------
// Admin-API helpers — provision / tear down throwaway users & groups
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

async function sleep(ms: number) {
  await new Promise((r) => setTimeout(r, ms));
}

async function createUser(
  ctx: APIRequestContext,
  adminHeaders: Record<string, string>,
  opts: { username?: string; password?: string; fullname?: string } = {},
): Promise<Tokens> {
  const username = opts.username ?? uniq('gusr_');
  const password = opts.password ?? 'UserPassw0rd!';
  const data = {
    username,
    password,
    fullname: opts.fullname ?? 'E2E Group User',
    email: '',
    role_id: USER_ROLE_ID,
    groups: [],
    home_enabled: false,
  };
  // The backend throttles account creation ("Maximum 10 user creations per
  // minute" — a real anti-abuse guard, not a bug). This suite creates many
  // throwaway users across its describes, easily enough to trip it back to
  // back (especially right after users.spec.ts's own run), so retry with
  // backoff instead of treating a 429-style 400 as fatal.
  let res = await ctx.post('/api/users', { headers: adminHeaders, data });
  for (let attempt = 0; !res.ok() && attempt < 3; attempt++) {
    const body = await res.text();
    if (!/rate limit/i.test(body)) {
      throw new Error(`could not create throwaway user: ${res.status()} ${body}`);
    }
    await sleep(15_000);
    res = await ctx.post('/api/users', { headers: adminHeaders, data });
  }
  if (!res.ok()) {
    throw new Error(`could not create throwaway user: ${res.status()} ${await res.text()}`);
  }
  return { username, password, token: '', csrf: '' };
}

async function deleteUser(
  ctx: APIRequestContext,
  adminHeaders: Record<string, string>,
  username: string,
) {
  await ctx.delete(`/api/users/${username}`, { headers: adminHeaders }).catch(() => {});
}

async function createGroup(
  ctx: APIRequestContext,
  adminHeaders: Record<string, string>,
  name?: string,
  members: string[] = [],
): Promise<string> {
  const groupName = name ?? uniq('grp_');
  const res = await ctx.post('/api/groups', {
    headers: adminHeaders,
    data: { name: groupName, members },
  });
  if (!res.ok()) {
    throw new Error(`could not create throwaway group: ${res.status()} ${await res.text()}`);
  }
  return groupName;
}

async function deleteGroup(
  ctx: APIRequestContext,
  adminHeaders: Record<string, string>,
  name: string,
) {
  await ctx.delete(`/api/groups/${encodeURIComponent(name)}`, { headers: adminHeaders }).catch(() => {});
}

// ---------------------------------------------------------------------------
// Browser helpers — seed a session, land on the desktop, open the Groups app
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

async function gotoDesktop(page: Page, tokens: Tokens, lang?: string) {
  await seedBrowser(page, tokens, lang);
  await page.goto('/desktop');
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
}

/**
 * The desktop hydrates in two waves: the shell (topbar pill, dock) paints
 * first, then `frontend/src/main.ts` fetches and injects the ~31k-node icon
 * sprite (`<svg>` + ~7.6k `<symbol>` + ~23.6k `<path>`) and the monitor
 * widgets mount. A DOM census taken inside that window reads ~500 nodes, so
 * every later reading looks like a leak — the node-count tests in sections 9
 * and 14 must wait for the second wave before taking their baseline.
 * Same guard as `login.spec.ts::waitForIconSprite` and
 * `desktop.spec.ts::waitForDomSettle`.
 */
async function waitForDomSettle(page: Page) {
  let prev = -1;
  for (let i = 0; i < 40; i++) {
    const n = await page.evaluate(() => document.querySelectorAll('*').length);
    // The >1000 guard refuses an early "stable" reading from inside the
    // pre-hydration window itself (two polls can land on the same ~500).
    if (n === prev && n > 1000) return;
    prev = n;
    await page.waitForTimeout(250);
  }
}

/** Click the Applications launcher and open one app tile by its display name. */
async function openLaunchpadApp(page: Page, appName: string) {
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator(`.lp-card[title="${appName}"]`);
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
}

/** Land on the desktop with an authenticated ADMIN session and open Groups. */
async function gotoGroups(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openLaunchpadApp(page, lang === 'fr' ? 'Groupes' : 'Groups');
  await page.waitForSelector('[data-testid="groups-page"]', { timeout: 10_000 });
  await expect(page.locator('[data-testid="groups-empty"]:has-text("Loading")')).toHaveCount(0, { timeout: 10_000 });
}

/** The Groups window as a scoping root (survives multiple windows). */
function win(page: Page) {
  return page.locator('[data-testid="groups-page"]');
}

// ---------------------------------------------------------------------------
// Shared fixtures
//
// Deliberately *not* `describe.serial` — see users.spec.ts for the rationale.
// `fullyParallel: false` already runs this file in declaration order.
// ---------------------------------------------------------------------------

let sharedCtx: APIRequestContext;
let adminHeaders: Record<string, string>;
let adminTokens: Tokens;
let sharedMember: Tokens;
let sharedGroup: string;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await newCtx(playwright);
  const admin = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  expect(admin.token, 'seeded admin login must succeed').not.toEqual('');
  adminHeaders = { 'x-session-token': admin.token, 'x-csrf-token': admin.csrf };
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token: admin.token, csrf: admin.csrf };

  sharedMember = await createUser(sharedCtx, adminHeaders, { fullname: 'Grace Hopper' });
  sharedGroup = await createGroup(sharedCtx, adminHeaders, uniq('devteam_'), [sharedMember.username]);
});

test.afterAll(async () => {
  if (sharedCtx) {
    await deleteGroup(sharedCtx, adminHeaders, sharedGroup);
    await deleteUser(sharedCtx, adminHeaders, sharedMember.username);
  }
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
  test('1.1 — Groups window opens from the Applications launcher', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await expect(page.locator('[data-testid="groups-page"]')).toHaveCount(0);
    await openLaunchpadApp(page, 'Groups');
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — header shows the "Groups" title only (standardized header)', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title')).toHaveText('Groups');
    // No subtitle: the group count lives in the page body, not the header.
    // `.hv-winhdr-subtitle` no longer exists (the prop was removed from
    // `WindowHeader` on 2026-09-14), so this locator matched nothing.
    // Assert the bar's real shape instead: the icon tile + the title.
    await expect(win(page).locator('.hv-winhdr > *')).toHaveCount(2);
  });

  test('1.3 — no infinite loading state (spinner clears)', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await expect(win(page).locator('[data-testid="groups-empty"]:has-text("Loading")')).toHaveCount(0);
  });

  test('1.4 — the seeded group is listed', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await expect(win(page).locator('[data-testid="groups-row"]', { hasText: sharedGroup })).toBeVisible();
  });

  test('1.5 — no group selected by default shows the empty-detail state', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await expect(win(page).locator('[data-testid="groups-noselect"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="groups-noselect-title"]')).toHaveText('No group selected');
  });

  test('1.6 — the real homevault_admin group carries a "System" badge', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    const sysBadge = win(page).locator('[data-testid="groups-state"].sys');
    const count = await sysBadge.count();
    test.skip(count === 0, 'no protected system group visible on this server');
    await expect(sysBadge.first()).toBeVisible();
  });
});

// ===========================================================================
// 2. Form fields — create form
// ===========================================================================

test.describe('2 — Create form fields', () => {
  test.beforeEach(async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).getByRole('button', { name: 'New Group' }).first().click();
  });

  test('2.1 — name field: required marker, mono, autocomplete off, hint text', async ({ page }) => {
    const el = win(page).locator('#c-name');
    await expect(el).toBeVisible();
    expect(await el.getAttribute('autocomplete')).toBe('off');
    await expect(win(page).locator('label[for="c-name"] span[aria-hidden="true"]')).toBeVisible();
    // The name field's help line is rendered by the shared `FormRow`
    // (`help={$t('groups.name_hint')}`), not by a `groups-hint` div — that
    // hook now only marks the member-search/quota lines further down, so the
    // old locator matched nothing and the assertion was green-at-empty.
    await expect(win(page).locator('.hv-form-row:has(#c-name) p')).toContainText(
      'Lowercase letters, numbers, hyphens only.',
    );
  });

  test('2.2 — every visible field has an associated <label>', async ({ page }) => {
    await expect(win(page).locator('label[for="c-name"]')).toBeVisible();
  });

  test('2.3 — the initial-members search box is present with a placeholder', async ({ page }) => {
    const search = win(page).locator('[data-testid="groups-search"].sub input');
    await expect(search).toBeVisible();
    expect(await search.getAttribute('placeholder')).toBe('Search a user…');
  });

  test('2.4 — the member-selected counter starts at zero', async ({ page }) => {
    await expect(win(page).locator('[data-testid="groups-member-count"]')).toHaveText('0 member(s) selected');
  });

  test('2.5 — a known user is listed as a selectable checklist row', async ({ page }) => {
    await expect(win(page).locator('[data-testid="groups-user"]', { hasText: sharedMember.username })).toBeVisible();
  });
});

// ===========================================================================
// 3. Validation
// ===========================================================================

test.describe('3 — Validation', () => {
  test.beforeEach(async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).getByRole('button', { name: 'New Group' }).first().click();
  });

  test('3.1 — empty name → "Identifier required."', async ({ page }) => {
    await win(page).getByRole('button', { name: 'Create group' }).click();
    await expect(win(page).locator('[data-testid="groups-card"] [role="alert"]')).toHaveText('Identifier required.');
  });

  test('3.2 — an invalid character set is rejected inline', async ({ page }) => {
    await win(page).locator('#c-name').fill('Not Valid!!');
    await win(page).getByRole('button', { name: 'Create group' }).click();
    await expect(win(page).locator('[data-testid="groups-card"] [role="alert"]')).toHaveText('Lowercase letters, numbers, hyphens or underscores only.');
  });

  test('3.3 — a duplicate group name is rejected inline', async ({ page }) => {
    await win(page).locator('#c-name').fill(sharedGroup);
    await win(page).getByRole('button', { name: 'Create group' }).click();
    await expect(win(page).locator('[data-testid="groups-card"] [role="alert"]')).toHaveText('This group already exists.');
  });
});

// ===========================================================================
// 4. All user interactions
// ===========================================================================

test.describe('4 — Interactions', () => {
  test('4.1 — search filters the group list', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-search"] input').first().fill(sharedGroup);
    await expect(win(page).locator('[data-testid="groups-row"]', { hasText: sharedGroup })).toBeVisible();
    await win(page).locator('[data-testid="groups-search"] input').first().fill('no-such-group-xyz');
    await expect(win(page).locator('[data-testid="groups-empty"]')).toContainText('No group matches.');
  });

  test('4.2 — toggling a member in the create checklist flips its "on" state and the counter', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).getByRole('button', { name: 'New Group' }).first().click();
    const row = win(page).locator('[data-testid="groups-user"]', { hasText: sharedMember.username });
    await expect(row).not.toHaveClass(/\bon\b/);
    await row.click();
    await expect(row).toHaveClass(/\bon\b/);
    await expect(win(page).locator('[data-testid="groups-member-count"]')).toHaveText('1 member(s) selected');
    await row.click();
    await expect(row).not.toHaveClass(/\bon\b/);
  });

  test('4.3 — selecting a row opens the detail pane hydrated with that group', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-row"]', { hasText: sharedGroup }).click();
    await expect(win(page).locator('[data-testid="groups-detail-h1"]')).toHaveText(sharedGroup);
    await expect(win(page).locator('[data-testid="groups-user"]', { hasText: sharedMember.username })).toHaveClass(/\bon\b/, { timeout: 8_000 });
  });

  test('4.4 — the member search box filters the detail checklist', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-row"]', { hasText: sharedGroup }).click();
    const memberSearch = win(page).locator('[data-testid="groups-search"].sub input');
    await memberSearch.fill('no-such-user-xyz');
    await expect(win(page).locator('[data-testid="groups-hint"].pad')).toContainText('No user matches.');
  });

  test('4.5 — the protected admin group renders its member checklist read-only', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    const sysRow = win(page).locator('[data-testid="groups-row"]').filter({ has: page.locator('[data-testid="groups-state"].sys') });
    const count = await sysRow.count();
    test.skip(count === 0, 'no protected system group visible on this server');
    await sysRow.first().click();
    await expect(win(page).locator('[data-testid="groups-warn"]')).toBeVisible();
    const firstUserRow = win(page).locator('[data-testid="groups-user"]').first();
    await expect(firstUserRow).toBeDisabled();
    // The members Save button is hidden for a protected group (the quota
    // card's own Save button is unrelated and stays present).
    const membersCard = win(page).locator('[data-testid="groups-card"]', { has: win(page).locator('[data-testid="groups-user"]') });
    await expect(membersCard.getByRole('button', { name: 'Save', exact: true })).toHaveCount(0);
  });
});

// ===========================================================================
// 5. Submission flows
// ===========================================================================

test.describe('5 — Create flow', () => {
  test('5.1 — creating a group with an initial member adds it to the list and opens its detail pane', async ({ page }) => {
    const name = uniq('mkgrp_');
    try {
      await gotoGroups(page, adminTokens);
      await win(page).getByRole('button', { name: 'New Group' }).first().click();
      await win(page).locator('#c-name').fill(name);
      await win(page).locator('[data-testid="groups-user"]', { hasText: sharedMember.username }).click();
      await win(page).getByRole('button', { name: 'Create group' }).click();

      await expect(win(page).locator('[data-testid="groups-detail-h1"]')).toHaveText(name, { timeout: 8_000 });
      await expect(win(page).locator('[data-testid="groups-row"]', { hasText: name })).toBeVisible();
      await expect(win(page).locator('[data-testid="groups-row"]', { hasText: name }).locator('[data-testid="groups-badge"]')).toContainText('1 member(s)');
    } finally {
      await deleteGroup(sharedCtx, adminHeaders, name);
    }
  });

  test('5.2 — double-clicking "Create group" only creates one group', async ({ page }) => {
    const name = uniq('dblgrp_');
    const posts: string[] = [];
    page.on('request', (r) => {
      if (r.method() === 'POST' && r.url().endsWith('/api/groups')) posts.push(r.url());
    });
    try {
      await gotoGroups(page, adminTokens);
      await win(page).getByRole('button', { name: 'New Group' }).first().click();
      await win(page).locator('#c-name').fill(name);
      const btn = win(page).getByRole('button', { name: 'Create group' });
      await btn.click();
      await btn.click().catch(() => {});
      await page.waitForTimeout(1_500);
      expect(posts.length).toBe(1);
    } finally {
      await deleteGroup(sharedCtx, adminHeaders, name);
    }
  });
});

test.describe('5 — Members / quota / delete flows', () => {
  let ctx: APIRequestContext;
  let groupName: string;
  let memberA: Tokens;
  let memberB: Tokens;

  test.beforeEach(async ({ playwright }) => {
    ctx = await newCtx(playwright);
    memberA = await createUser(ctx, adminHeaders, { fullname: 'Member A' });
    memberB = await createUser(ctx, adminHeaders, { fullname: 'Member B' });
    groupName = await createGroup(ctx, adminHeaders, uniq('mutgrp_'), [memberA.username]);
  });
  test.afterEach(async () => {
    await deleteGroup(ctx, adminHeaders, groupName);
    await deleteUser(ctx, adminHeaders, memberA.username);
    await deleteUser(ctx, adminHeaders, memberB.username);
    await ctx.dispose();
  });

  test('5.3 — adding and removing members persists to the real member list', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-row"]', { hasText: groupName }).click();
    await win(page).locator('[data-testid="groups-user"]', { hasText: memberB.username }).click(); // add
    await win(page).locator('[data-testid="groups-user"]', { hasText: memberA.username }).click(); // remove
    await win(page).getByRole('button', { name: 'Save', exact: true }).first().click();
    await expect(win(page).locator('[data-testid="groups-ok"]')).toHaveText('Members updated', { timeout: 8_000 });

    const grp = await ctx.get(`/api/groups/${encodeURIComponent(groupName)}`, { headers: adminHeaders });
    const members: string[] = (await grp.json())?.data?.members ?? [];
    expect(members).toContain(memberB.username);
    expect(members).not.toContain(memberA.username);
  });

  test('5.4 — saving with no changes still succeeds', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-row"]', { hasText: groupName }).click();
    await win(page).getByRole('button', { name: 'Save', exact: true }).first().click();
    await expect(win(page).locator('[data-testid="groups-ok"]')).toHaveText('Members updated', { timeout: 8_000 });
  });

  test('5.5 — the list member count updates after a membership change', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-row"]', { hasText: groupName }).click();
    await win(page).locator('[data-testid="groups-user"]', { hasText: memberB.username }).click();
    await win(page).getByRole('button', { name: 'Save', exact: true }).first().click();
    await expect(win(page).locator('[data-testid="groups-ok"]')).toBeVisible({ timeout: 8_000 });
    await expect(
      win(page).locator('[data-testid="groups-row"]', { hasText: groupName }).locator('[data-testid="groups-badge"]'),
    ).toContainText('2 member(s)', { timeout: 8_000 });
  });

  test('5.6 — enabling upload quota saves and appears in the group', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-row"]', { hasText: groupName }).click();
    const uploadRow = win(page).locator('[data-testid="groups-quota-block"]').first();
    await uploadRow.locator('[data-testid="groups-switch"]').click();
    await uploadRow.locator('input[type="number"]').fill('10');
    await win(page).getByRole('button', { name: 'Save', exact: true }).last().click();
    await expect(win(page).locator('[data-testid="groups-ok"]')).toHaveText('Transfer quotas saved', { timeout: 8_000 });

    const q = await ctx.get(`/api/transfer-quotas/group/${encodeURIComponent(groupName)}`, { headers: adminHeaders });
    expect((await q.json())?.data?.upload_enabled).toBe(true);
  });

  test('5.7 — deleting an empty group removes it from the list', async ({ page }) => {
    const empty = await createGroup(ctx, adminHeaders, uniq('emptygrp_'));
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-row"]', { hasText: empty }).click();
    await win(page).getByRole('button', { name: 'Delete', exact: true }).click();
    await page.locator('[data-slot="dialog-content"]').getByRole('button', { name: 'Confirm', exact: true }).click();
    await expect(win(page).locator('[data-testid="groups-row"]', { hasText: empty })).toHaveCount(0, { timeout: 8_000 });
    await expect(win(page).locator('[data-testid="groups-noselect"]')).toBeVisible();
  });

  test('5.8 — deleting a group with members shows the extra confirm warning', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-row"]', { hasText: groupName }).click();
    await win(page).getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(page.locator('[data-testid="confirm-dialog-message"]')).toHaveText(/lose this membership/i);
    // Dismissed — the group must still exist.
    await page.locator('[data-slot="dialog-content"]').getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(win(page).locator('[data-testid="groups-row"]', { hasText: groupName })).toBeVisible();
  });

  test('5.9 — the protected admin group cannot be deleted', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    const sysRow = win(page).locator('[data-testid="groups-row"]').filter({ has: page.locator('[data-testid="groups-state"].sys') });
    const count = await sysRow.count();
    test.skip(count === 0, 'no protected system group visible on this server');
    await sysRow.first().click();
    await expect(win(page).getByRole('button', { name: 'Delete', exact: true })).toBeDisabled();
  });
});

// ===========================================================================
// 6. Navigation
// ===========================================================================

test.describe('6 — Navigation', () => {
  test('6.1 — Cancel on the create form returns to the no-selection state', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).getByRole('button', { name: 'New Group' }).first().click();
    await win(page).locator('[data-testid="groups-cancel"]', { hasText: 'Cancel' }).click();
    await expect(win(page).locator('[data-testid="groups-noselect"]')).toBeVisible();
  });

  test('6.2 — the back arrow closes the detail pane', async ({ page }) => {
    // The back arrow only renders below the 860px breakpoint (driven by
    // matchMedia in Groups.svelte), so this needs a narrow
    // viewport to even be clickable. At that width the detail pane —
    // including its own no-selection placeholder — collapses away entirely
    // once nothing is selected; "back" means "return to the list".
    await page.setViewportSize({ width: 700, height: 900 });
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-row"]', { hasText: sharedGroup }).click();
    await win(page).locator('[data-testid="groups-icon-btn"].only-narrow').click();
    await expect(win(page).locator('[data-testid="groups-list"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="groups-detail"]')).not.toBeVisible();
  });

  test('6.3 — the window closes and can be reopened', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await page.locator('.hv-cl-btn-close').first().click();
    await expect(win(page)).toHaveCount(0);
    await openLaunchpadApp(page, 'Groups');
    await expect(win(page)).toBeVisible();
  });

  test('6.4 — no manual Refresh button; a window-focus event revalidates in place', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await expect(win(page).getByRole('button', { name: 'Refresh' })).toHaveCount(0);
    await page.waitForTimeout(1_600);
    await page.evaluate(() => {
      document.dispatchEvent(new Event('visibilitychange'));
      window.dispatchEvent(new Event('focus'));
    });
    await page.waitForTimeout(500);
    await expect(win(page)).toBeVisible();
    await expect(win(page).locator('[data-testid="groups-row"]', { hasText: sharedGroup })).toBeVisible();
  });
});

// ===========================================================================
// 7. Responsive
// ===========================================================================

test.describe('7 — Responsive', () => {
  for (const [label, width, height] of [
    ['360px (phone)', 360, 720],
    ['768px (tablet)', 768, 1024],
    ['1920px (desktop)', 1920, 1080],
  ] as const) {
    test(`7.x — renders without horizontal overflow at ${label}`, async ({ page }) => {
      await page.setViewportSize({ width, height });
      await gotoGroups(page, adminTokens);
      const shell = win(page);
      await expect(shell).toBeVisible();
      const box = await shell.boundingBox();
      expect(box).not.toBeNull();
      expect(box!.x).toBeGreaterThanOrEqual(-1);
      expect(box!.x + box!.width).toBeLessThanOrEqual(width + 2);
    });
  }

  test('7.4 — a narrow window stacks the list and detail panes', async ({ page }) => {
    await page.setViewportSize({ width: 400, height: 720 });
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-row"]', { hasText: sharedGroup }).click();
    await expect(win(page).locator('[data-testid="groups-detail"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="groups-list"]')).not.toBeVisible();
  });
});

// ===========================================================================
// 8. Accessibility
// ===========================================================================

test.describe('8 — Accessibility', () => {
  test('8.1 — every visible labelled input has a matching <label>', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).getByRole('button', { name: 'New Group' }).first().click();
    const ids = await win(page)
      .locator('input[id]')
      .evaluateAll((els) => els.filter((e) => (e as HTMLElement).offsetParent !== null).map((e) => e.id));
    for (const id of ids) {
      await expect(win(page).locator(`label[for="${id}"]`)).toHaveCount(1);
    }
  });

  test('8.2 — the Groups window introduces no duplicate IDs into the DOM', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    const dupes = await page.evaluate(() => {
      const seen = new Map<string, number>();
      for (const el of document.querySelectorAll('[id]')) {
        seen.set(el.id, (seen.get(el.id) || 0) + 1);
      }
      return [...seen.entries()].filter(([, n]) => n > 1).map(([id]) => id);
    });
    expect(dupes, `duplicate IDs: ${dupes.join(', ')}`).toEqual([]);
  });

  test('8.3 — quota toggle switches expose role=switch and aria-checked', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-row"]', { hasText: sharedGroup }).click();
    const switches = win(page).locator('[role="switch"]');
    // The quota block renders behind its own async `loadQuota()` fetch (a
    // "Loading…" hint until then) — wait for the first switch rather than
    // counting immediately after the click.
    await switches.first().waitFor({ state: 'visible', timeout: 8_000 });
    const count = await switches.count();
    expect(count).toBeGreaterThan(0);
    for (let i = 0; i < count; i++) {
      expect(['true', 'false']).toContain(await switches.nth(i).getAttribute('aria-checked'));
    }
  });

  test('8.4 — interactive controls are not removed from the tab order', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    const negative = await win(page)
      .locator('button, input, a')
      .evaluateAll((els) => els.filter((e) => e.getAttribute('tabindex') === '-1').length);
    expect(negative).toBe(0);
  });

  test('8.5 — the dialog window exposes an aria-label', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    const dialog = page.locator('.hv-window[role="dialog"]', { has: win(page) });
    expect(await dialog.getAttribute('aria-label')).toBeTruthy();
  });
});

// ===========================================================================
// 9. Performance
// ===========================================================================

test.describe('9 — Performance', () => {
  test('9.1 — the window opens in a reasonable time', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const start = Date.now();
    await openLaunchpadApp(page, 'Groups');
    await page.waitForSelector('[data-testid="groups-page"] [data-testid="groups-row"]', { timeout: 10_000 });
    const elapsed = Date.now() - start;
    console.log(`[PERF] Groups window ready in ${elapsed}ms`);
    expect(elapsed).toBeLessThan(8_000);
  });

  test('9.2 — opening the Groups window does not add an excessive number of DOM nodes', async ({ page }) => {
    // The bare desktop shell (dock, topbar, lazily-rendered monitor widgets)
    // already carries ~30k nodes on its own (measured live) — an absolute
    // cap would just be measuring the desktop shell, not this window, so
    // this checks the *delta* the Groups window itself adds.
    await gotoDesktop(page, adminTokens);
    await waitForDomSettle(page);
    const before = await page.evaluate(() => document.querySelectorAll('*').length);
    await openLaunchpadApp(page, 'Groups');
    await page.waitForSelector('[data-testid="groups-page"] [data-testid="groups-row"]');
    const after = await page.evaluate(() => document.querySelectorAll('*').length);
    console.log(`[PERF] Groups window added ${after - before} DOM nodes`);
    expect(after - before).toBeLessThan(3_000);
  });

  test('9.3 — the initial load issues a bounded number of API calls', async ({ page }) => {
    const calls: string[] = [];
    page.on('request', (r) => {
      if (r.url().includes('/api/groups') || r.url().includes('/api/users')) calls.push(r.url());
    });
    await gotoGroups(page, adminTokens);
    await page.waitForTimeout(500);
    expect(calls.length).toBeLessThan(15);
  });
});

// ===========================================================================
// 10. Security
// ===========================================================================

test.describe('10 — Security', () => {
  test('10.1 — an XSS payload in the group-name field never lands as markup', async ({ page }) => {
    let dialogFired = false;
    page.on('dialog', () => (dialogFired = true));
    await gotoGroups(page, adminTokens);
    await win(page).getByRole('button', { name: 'New Group' }).first().click();
    await win(page).locator('#c-name').fill('<img src=x onerror=alert(1)>');
    await win(page).getByRole('button', { name: 'Create group' }).click();
    await page.waitForTimeout(1_000);
    expect(dialogFired).toBe(false);
    expect(await win(page).locator('img[onerror]').count()).toBe(0);
  });

  test('10.2 — the groups API rejects a tampered/removed session token', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/groups', { headers: { 'x-session-token': 'garbage-token' } });
      return r.status;
    });
    expect(status).toBeGreaterThanOrEqual(400);
  });

  test('10.3 — a non-admin session is redirected away from the Groups window', async ({ page }) => {
    const ctx = sharedCtx;
    const plain = await createUser(ctx, adminHeaders, { fullname: 'Plain User' });
    try {
      const login = await apiLogin(ctx, plain.username, plain.password);
      const plainTokens: Tokens = { ...plain, token: login.token, csrf: login.csrf };
      await gotoDesktop(page, plainTokens);
      await openLaunchpadApp(page, 'Groups');
      // Groups.svelte's onMount redirects non-admins with window.location.href
      // = '/desktop' — a full navigation, so the shell must not stay mounted.
      await expect(win(page)).toHaveCount(0, { timeout: 10_000 });
      expect(page.url()).toContain('/desktop');
    } finally {
      await deleteUser(ctx, adminHeaders, plain.username);
    }
  });

  test('10.4 — the admin group\'s membership cannot be mutated through the members API by this UI', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    const sysRow = win(page).locator('[data-testid="groups-row"]').filter({ has: page.locator('[data-testid="groups-state"].sys') });
    const count = await sysRow.count();
    test.skip(count === 0, 'no protected system group visible on this server');
    await sysRow.first().click();
    // Every membership row is a disabled button — no click can register.
    const rows = win(page).locator('[data-testid="groups-user"]');
    const n = await rows.count();
    for (let i = 0; i < Math.min(n, 5); i++) {
      await expect(rows.nth(i)).toBeDisabled();
    }
  });
});

// ===========================================================================
// 11. Keyboard
// ===========================================================================

test.describe('11 — Keyboard', () => {
  test('11.1 — Tab moves from the name field into the member search box', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).getByRole('button', { name: 'New Group' }).first().click();
    await win(page).locator('#c-name').focus();
    await page.keyboard.press('Tab');
    const active = await page.evaluate(() => document.activeElement?.tagName);
    expect(active).toBe('INPUT');
  });

  test('11.2 — Enter in the search box does not navigate or submit anything', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-search"] input').first().fill(sharedGroup);
    await page.keyboard.press('Enter');
    expect(page.url()).toContain('/desktop');
    await expect(win(page)).toBeVisible();
  });
});

// ===========================================================================
// 12. Console & network
// ===========================================================================

test.describe('12 — Console & network', () => {
  test('12.1 — opening the Groups window logs no uncaught JS errors', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await gotoGroups(page, adminTokens);
    const critical = errors.filter(
      (e) => !e.includes('404') && !e.includes('Failed to load resource') && !e.includes('favicon'),
    );
    expect(critical, critical.join('; ')).toHaveLength(0);
  });

  test('12.2 — creating a group fires exactly one POST /groups', async ({ page }) => {
    const name = uniq('netgrp_');
    const posts: string[] = [];
    page.on('request', (r) => {
      if (r.method() === 'POST' && r.url().endsWith('/api/groups')) posts.push(r.url());
    });
    try {
      await gotoGroups(page, adminTokens);
      await win(page).getByRole('button', { name: 'New Group' }).first().click();
      await win(page).locator('#c-name').fill(name);
      await win(page).getByRole('button', { name: 'Create group' }).click();
      await page.waitForTimeout(1_500);
      expect(posts.length).toBe(1);
    } finally {
      await deleteGroup(sharedCtx, adminHeaders, name);
    }
  });

  test('12.3 — deleting an empty group fires exactly one DELETE /groups/:name', async ({ page }) => {
    const ctx = sharedCtx;
    const name = await createGroup(ctx, adminHeaders, uniq('delnetgrp_'));
    const dels: string[] = [];
    page.on('request', (r) => {
      if (r.method() === 'DELETE' && r.url().includes(`/api/groups/${name}`)) dels.push(r.url());
    });
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-row"]', { hasText: name }).click();
    await win(page).getByRole('button', { name: 'Delete', exact: true }).click();
    await page.locator('[data-slot="dialog-content"]').getByRole('button', { name: 'Confirm', exact: true }).click();
    await page.waitForTimeout(1_500);
    expect(dels.length).toBe(1);
  });
});

// ===========================================================================
// 13. Edge cases
// ===========================================================================

test.describe('13 — Edge cases', () => {
  test('13.1 — a very long group name does not break the layout', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).getByRole('button', { name: 'New Group' }).first().click();
    await win(page).locator('#c-name').fill('a'.repeat(200));
    const box = await win(page).boundingBox();
    expect(box!.x + box!.width).toBeLessThanOrEqual(page.viewportSize()!.width + 2);
  });

  test('13.2 — rapidly toggling the same member does not desync the counter', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).getByRole('button', { name: 'New Group' }).first().click();
    const row = win(page).locator('[data-testid="groups-user"]', { hasText: sharedMember.username });
    for (let i = 0; i < 6; i++) await row.click();
    // 6 clicks (even) → back to "off".
    await expect(row).not.toHaveClass(/\bon\b/);
    await expect(win(page).locator('[data-testid="groups-member-count"]')).toHaveText('0 member(s) selected');
  });

  test('13.3 — opening the window with a cleared cache still renders', async ({ page }) => {
    await seedBrowser(page, adminTokens);
    await page.goto('/desktop');
    await page.evaluate(() => {
      try {
        const keep = ['session-token', 'csrf-token'];
        for (const k of Object.keys(localStorage)) if (!keep.includes(k)) localStorage.removeItem(k);
      } catch { /* ignore */ }
    });
    await page.reload();
    await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
    await openLaunchpadApp(page, 'Groups');
    await expect(win(page)).toBeVisible();
  });

  test('13.4 — closing and reopening the create form discards unsaved input', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).getByRole('button', { name: 'New Group' }).first().click();
    await win(page).locator('#c-name').fill('discard-me');
    await win(page).locator('[data-testid="groups-cancel"]', { hasText: 'Cancel' }).click();
    await win(page).getByRole('button', { name: 'New Group' }).first().click();
    expect(await win(page).locator('#c-name').inputValue()).toBe('');
  });

  test('13.5 — an uppercase/space-padded name is rejected the same as any invalid name', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).getByRole('button', { name: 'New Group' }).first().click();
    await win(page).locator('#c-name').fill('Not Lowercase');
    await win(page).getByRole('button', { name: 'Create group' }).click();
    await expect(win(page).locator('[data-testid="groups-card"] [role="alert"]')).toHaveText('Lowercase letters, numbers, hyphens or underscores only.');
  });
});

// ===========================================================================
// 14. Memory regression
// ===========================================================================

test.describe('14 — Memory regression', () => {
  test.slow();

  test('14.1 — opening and closing the Groups window ×5 does not accumulate DOM nodes', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await waitForDomSettle(page);
    await openLaunchpadApp(page, 'Groups');
    await page.waitForSelector('[data-testid="groups-page"]');
    const count = () => page.evaluate(() => document.querySelectorAll('*').length);
    const before = await count();

    for (let i = 0; i < 5; i++) {
      await page.locator('.hv-cl-btn-close').first().click().catch(() => {});
      await page.waitForTimeout(200);
      await openLaunchpadApp(page, 'Groups').catch(() => {});
      await page.waitForSelector('[data-testid="groups-page"]').catch(() => {});
    }

    const after = await count();
    console.log(`[DOM] Groups window cycle ×5: Δ${after - before} nodes`);
    expect(after - before).toBeLessThanOrEqual(200);
  });

  test('14.2 — selecting/deselecting a group ×5 does not leak heap', async ({ page }) => {
    // Same narrow-breakpoint note as 6.2 above.
    await page.setViewportSize({ width: 700, height: 900 });
    await gotoGroups(page, adminTokens);
    const baseline = await getHeapMB(page);
    for (let i = 0; i < 5; i++) {
      await win(page).locator('[data-testid="groups-row"]', { hasText: sharedGroup }).click();
      await win(page).locator('[data-testid="groups-icon-btn"].only-narrow').click();
      await win(page).locator('[data-testid="groups-list"]').waitFor({ state: 'visible' });
    }
    const after = await getHeapMB(page);
    if (baseline > 0 && after > 0) {
      console.log(`[MEM] select/deselect ×5: Δ${(after - baseline).toFixed(2)} MB`);
      expect(after - baseline).toBeLessThan(5);
    }
  });
});

// ===========================================================================
// 15. i18n — the saved language drives the Groups strings
// ===========================================================================

test.describe('15 — i18n', () => {
  test('15.1 — French: header and buttons are in French', async ({ page }) => {
    await gotoDesktop(page, adminTokens, 'fr');
    await openLaunchpadApp(page, 'Groupes');
    await page.waitForSelector('[data-testid="groups-page"]');
    await expect(win(page).locator('.hv-winhdr-title')).toHaveText('Groupes');
    // Both the header button and the no-selection panel's button share this
    // name when nothing is selected yet — see the English "New Group" fix.
    await expect(win(page).getByRole('button', { name: 'Nouveau groupe' }).first()).toBeVisible();
  });

  test('15.2 — no raw groups.* keys leak into the rendered page', async ({ page }) => {
    await gotoDesktop(page, adminTokens, 'fr');
    await openLaunchpadApp(page, 'Groupes');
    await page.waitForSelector('[data-testid="groups-page"]');
    const text = await win(page).innerText();
    expect(text).not.toMatch(/\bgroups\.[a-z0-9_]+/i);
  });
});

// ===========================================================================
// 16 — Accessibility
// ===========================================================================

test.describe('16 — Accessibility', () => {
  // The avatar palette ($lib/utils/avatars) is shared by Users and Groups and
  // carries white initials: every stop must stay ≥4.5:1 (WCAG 1.4.3). The
  // guard covers both halves of the 2026-09-23 fix — the palette stops
  // (#4B8AD4/#8b5cf6 were below the floor) and Groups rendering dark ink on
  // its tiles (which failed on 5 of the 6 stops).
  test('16.1 — avatar initials keep ≥4.5:1 against their palette colour', async ({ page }) => {
    await gotoGroups(page, adminTokens);
    await win(page).locator('[data-testid="groups-row"]', { hasText: sharedGroup }).click();
    await expect(win(page).locator('[data-testid="groups-detail-h1"]')).toHaveText(sharedGroup);

    const measured = await page.evaluate(() => {
      const lin = (v: number) => {
        const c = v / 255;
        return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
      };
      const lum = (css: string) => {
        const [r = 0, g = 0, b = 0] = (css.match(/[\d.]+/g) ?? []).map(Number);
        return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
      };
      return ['[data-testid="groups-row-avatar"]', '[data-testid="groups-detail-avatar"]'].map((sel) => {
        const el = document.querySelector(sel) as HTMLElement | null;
        // No inline background → the tile is transparent, not palette-coloured.
        if (!el || !el.style.backgroundColor) return { sel, ratio: 0 };
        const cs = getComputedStyle(el);
        const fg = lum(cs.color);
        const bg = lum(cs.backgroundColor);
        return { sel, ratio: (Math.max(fg, bg) + 0.05) / (Math.min(fg, bg) + 0.05) };
      });
    });

    expect(measured, 'both avatar tiles must be rendered').toHaveLength(2);
    for (const { sel, ratio } of measured) {
      expect(ratio, `${sel} initials vs palette background`).toBeGreaterThanOrEqual(4.5);
    }
  });
});
