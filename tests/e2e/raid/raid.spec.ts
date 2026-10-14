/* =============================================================================
 * File          : raid.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Raid screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * RAID app — End-to-End Test Suite (rewritten 2026-09-07 for the
 * creation-only re-scope)
 *
 * The RAID app is not a route: it is a window opened from the desktop's
 * Applications launcher (id `raid`, "RAID" tile). Since 2026-09-06 it is a
 * **creation-only wizard** — it turns physical disks into one "virtual
 * disk" (mdadm array, ZFS vdev, or native Btrfs). It has no list, no
 * rebuild, no delete: managing existing virtual disks lives in the
 * **Storage** app's "Virtual disks" tab (see `storage.spec.ts`).
 *
 * **This suite never creates a real virtual disk against the seeded
 * dev/CI backend.** Every wizard-driven test stops before "Confirm
 * creation". The backend's own validation (level / device-count / name /
 * filesystem / device existence) is exercised directly via `fetch` on
 * `POST /api/storage/raid`, which 400s before ever touching
 * `mdadm`/`zpool`/`mkfs`. The one real-hardware test is opt-in
 * (`RAID_E2E_DEVICES=/dev/loop0,/dev/loop1`) and creates → checks it shows
 * up in Storage's virtual-disks list → destroys it there.
 *
 * Run (all, safe):   npx playwright test tests/e2e/raid/raid.spec.ts --project=firefox
 * Run (single):      npx playwright test tests/e2e/raid/raid.spec.ts -g "4.1"
 * Run (category):    npx playwright test tests/e2e/raid/raid.spec.ts -g "Security"
 * Custom server:     BASE_URL=http://localhost:3000 npx playwright test tests/e2e/raid/raid.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const USER_ROLE_ID = 1;

/** Comma-separated disposable devices for the opt-in real lifecycle test. */
const REAL_DEVICES = (process.env.RAID_E2E_DEVICES || '')
  .split(',')
  .map((s) => s.trim())
  .filter(Boolean);

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
    .addStyleTag({ content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}' })
    .catch(() => {});
}

async function openLaunchpadApp(page: Page, appName: string) {
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator(`.lp-card[title="${appName}"]`);
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
}

async function gotoRaid(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openLaunchpadApp(page, 'RAID');
  await page.waitForSelector('[data-testid="raid-page"]', { timeout: 10_000 });
  await expect(win(page).locator('[data-testid="raid-wizard"]')).toBeVisible({ timeout: 10_000 });
}

/** The RAID window as a scoping root. */
function win(page: Page) {
  return page.locator('[data-testid="raid-page"]');
}

/** A step-1 RAID-type card, matched on its exact `[data-testid="raid-type-name"]` heading (so
 * "RAID 1" doesn't also match "RAID 10"). */
function raidTypeCard(page: Page, name: string) {
  return win(page)
    .locator('[data-testid="raid-type-card"]')
    .filter({ has: page.locator('[data-testid="raid-type-name"]', { hasText: new RegExp(`^${name}$`) }) });
}

const btn = (page: Page, name: RegExp) => win(page).getByRole('button', { name });

/** Pick a type on step 1 and advance to step 2. */
async function toStep2(page: Page, type = 'RAID 1') {
  await raidTypeCard(page, type).click();
  await btn(page, /continue|continuer/i).click();
  await expect(win(page).locator('[data-testid="raid-disk-head"]')).toBeVisible();
}

/** How many real, selectable disks the host actually offers. */
async function selectableDiskCount(page: Page): Promise<number> {
  return win(page).locator('[data-testid="raid-disk-row"].selectable').count();
}

// --- CSRF-aware in-page fetch on the create endpoint --------------------

async function apiCreate(
  page: Page,
  t: Tokens,
  body: { name: string; level: string; devices: string[]; filesystem: string },
) {
  return page.evaluate(
    async ({ token, csrf, body }) => {
      const r = await fetch('/api/storage/raid', {
        method: 'POST',
        headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      const j = await r.json().catch(() => ({}));
      return { http: r.status, success: j?.success, message: j?.message };
    },
    { token: t.token, csrf: t.csrf, body },
  );
}

let sharedCtx: APIRequestContext;
let adminHeaders: Record<string, string>;
let adminTokens: Tokens;
let plainUser: Tokens;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await newCtx(playwright);
  const admin = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  expect(admin.token, 'seeded admin login must succeed').not.toEqual('');
  adminHeaders = { 'x-session-token': admin.token, 'x-csrf-token': admin.csrf };
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token: admin.token, csrf: admin.csrf };

  // Throwaway non-admin for the security section.
  const username = `raid_${Date.now().toString(36)}`;
  const password = 'RaidUserPassw0rd!';
  await sharedCtx.post('/api/users', {
    headers: adminHeaders,
    data: { username, password, fullname: 'RAID E2E', email: '', role_id: USER_ROLE_ID, groups: [], home_enabled: false },
  });
  const lg = await apiLogin(sharedCtx, username, password);
  plainUser = { username, password, token: lg.token, csrf: lg.csrf };
});

test.afterAll(async () => {
  if (plainUser?.username) {
    await sharedCtx.delete(`/api/users/${plainUser.username}`, { headers: adminHeaders }).catch(() => {});
  }
  await sharedCtx?.dispose();
});

test.afterEach(async ({ page }, testInfo) => {
  const heapMB = await getHeapMB(page).catch(() => 0);
  if (heapMB > 0) {
    // eslint-disable-next-line no-console
    console.log(`[MEM] ${testInfo.title}: ${heapMB.toFixed(1)} MB heap`);
  }
});

// ===========================================================================
// 1. Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — RAID window opens from the Applications launcher', async ({ page }) => {
    await expect(page.locator('[data-testid="raid-page"]')).toHaveCount(0);
    await gotoRaid(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — header reads "Create a virtual disk", no "pool"/"array" wording', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    await expect(win(page)).toContainText(/create a virtual disk|créer un disque virtuel/i);
    const text = (await win(page).innerText()).toLowerCase();
    expect(text).not.toMatch(/\bpool\b|\bgrappe/);
  });

  test('1.3 — the creation wizard is shown inline (not behind a "New" button)', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    await expect(win(page).locator('[data-testid="raid-wizard"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="raid-type-grid"]')).toBeVisible();
  });

  test('1.4 — no arrays list / stats / rebuild UI remains', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    await expect(win(page).locator('.rl-array-card, .rl-stat-card, .rl-empty-card')).toHaveCount(0);
  });

  test('1.5 — a note points to the Storage app for managing existing disks', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    await expect(win(page).locator('[data-testid="raid-storage-note"]')).toContainText(/storage|stockage/i);
  });

  test('1.6 — step indicator starts at step 1 of 3', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    await expect(win(page).locator('[data-testid="raid-eyebrow"]')).toContainText(/1.*3|1 \/ 3/);
  });
});

// ===========================================================================
// 2. Form fields
// ===========================================================================

test.describe('2 — Form fields', () => {
  test('2.1 — step 1 shows the 5 RAID levels with min-disk + protection', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    await expect(win(page).locator('[data-testid="raid-type-card"]')).toHaveCount(5);
    for (const lvl of ['RAID 0', 'RAID 1', 'RAID 5', 'RAID 6', 'RAID 10']) {
      await expect(raidTypeCard(page, lvl)).toHaveCount(1);
    }
  });

  test('2.2 — RAID 5 carries the "Recommended" badge', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    await expect(raidTypeCard(page, 'RAID 5').locator('[data-testid="raid-recommended"]')).toBeVisible();
  });

  test('2.3 — step 3 offers exactly 4 filesystems incl. ZFS and Btrfs', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    if ((await selectableDiskCount(page)) < 2) {
      test.skip(true, 'host has fewer than 2 free disks — cannot reach step 3 in the UI');
    }
    await toStep2(page);
    const rows = win(page).locator('[data-testid="raid-disk-row"].selectable input[type="checkbox"]');
    await rows.nth(0).check();
    await rows.nth(1).check();
    await btn(page, /continue|continuer/i).click();
    await expect(win(page).locator('[data-testid="raid-fs-card"]')).toHaveCount(4);
    for (const fs of ['ext4', 'XFS', 'Btrfs', 'ZFS']) {
      await expect(win(page).locator('[data-testid="raid-fs-card"]', { hasText: fs })).toBeVisible();
    }
  });

  test('2.4 — the name field (step 3) is a text input with a placeholder', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    if ((await selectableDiskCount(page)) < 2) test.skip(true, 'no free disks to reach step 3');
    await toStep2(page);
    const rows = win(page).locator('[data-testid="raid-disk-row"].selectable input[type="checkbox"]');
    await rows.nth(0).check();
    await rows.nth(1).check();
    await btn(page, /continue|continuer/i).click();
    const name = win(page).locator('#raidName');
    await expect(name).toHaveAttribute('type', 'text');
    await expect(name).toHaveAttribute('placeholder', /.+/);
  });
});

// ===========================================================================
// 3. Validation (UI gating + server-side, never touching mdadm/zpool)
// ===========================================================================

test.describe('3 — Validation', () => {
  test('3.1 — "Continue" on step 1 is disabled until a level is picked', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    await expect(btn(page, /continue|continuer/i)).toBeDisabled();
    await raidTypeCard(page, 'RAID 1').click();
    await expect(btn(page, /continue|continuer/i)).toBeEnabled();
  });

  test('3.2 — step 2 "Continue" requires the level\'s minimum disk count', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const n = await (async () => {
      await raidTypeCard(page, 'RAID 5').click();
      await btn(page, /continue|continuer/i).click();
      return win(page).locator('[data-testid="raid-disk-row"].selectable').count();
    })();
    if (n < 1) test.skip(true, 'no free disks on the host');
    // RAID 5 needs 3 — one disk selected must not be enough.
    await win(page).locator('[data-testid="raid-disk-row"].selectable input[type="checkbox"]').first().check();
    await expect(btn(page, /continue|continuer/i)).toBeDisabled();
  });

  test('3.3 — server rejects an invalid RAID level (no mdadm touched)', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const r = await apiCreate(page, adminTokens, {
      name: 'e2e-bad-level', level: 'raid99', devices: ['/dev/nope1', '/dev/nope2'], filesystem: 'ext4',
    });
    expect(r.http).toBe(400);
    expect(r.success).toBe(false);
  });

  test('3.4 — server rejects too few devices for the level', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const r = await apiCreate(page, adminTokens, {
      name: 'e2e-few', level: 'raid6', devices: ['/dev/nope1', '/dev/nope2'], filesystem: 'ext4',
    });
    expect(r.http).toBe(400);
    expect(r.success).toBe(false);
  });

  test('3.5 — server rejects an invalid array name', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const r = await apiCreate(page, adminTokens, {
      name: 'bad name/../x', level: 'raid1', devices: ['/dev/nope1', '/dev/nope2'], filesystem: 'ext4',
    });
    expect(r.http).toBe(400);
    expect(r.success).toBe(false);
  });

  test('3.6 — server rejects an unknown filesystem', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const r = await apiCreate(page, adminTokens, {
      name: 'e2e-badfs', level: 'raid1', devices: ['/dev/nope1', '/dev/nope2'], filesystem: 'reiserfs',
    });
    expect(r.http).toBe(400);
    expect(r.success).toBe(false);
  });

  test('3.7 — server rejects a nonexistent device path', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const r = await apiCreate(page, adminTokens, {
      name: 'e2e-nodev', level: 'raid1', devices: ['/dev/definitely-not-a-disk-xyz', '/dev/also-not-1'], filesystem: 'ext4',
    });
    expect(r.http).toBe(400);
    expect(r.success).toBe(false);
  });

  test('3.8 — a Btrfs virtual disk with fake devices is also rejected cleanly (no mkfs)', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const r = await apiCreate(page, adminTokens, {
      name: 'e2e-btrfs', level: 'raid1', devices: ['/dev/nope-btrfs-1', '/dev/nope-btrfs-2'], filesystem: 'btrfs',
    });
    expect(r.http).not.toBe(200);
    expect(r.success).toBe(false);
  });
});

// ===========================================================================
// 4. Interactions
// ===========================================================================

test.describe('4 — Interactions', () => {
  test('4.1 — picking a level highlights its card', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const card = raidTypeCard(page, 'RAID 1');
    await card.click();
    await expect(card).toHaveClass(/selected/);
  });

  test('4.2 — checking / unchecking a disk toggles its row', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    await raidTypeCard(page, 'RAID 1').click();
    await btn(page, /continue|continuer/i).click();
    if ((await selectableDiskCount(page)) < 1) test.skip(true, 'no free disks on the host');
    const row = win(page).locator('[data-testid="raid-disk-row"].selectable').first();
    await row.locator('input[type="checkbox"]').check();
    await expect(row).toHaveClass(/selected/);
    await row.locator('input[type="checkbox"]').uncheck();
    await expect(row).not.toHaveClass(/selected/);
  });

  test('4.3 — the progress bar advances with the step', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const bar = win(page).locator('[data-testid="raid-progress"] > span');
    const w1 = await bar.evaluate((el) => (el as HTMLElement).style.width);
    await raidTypeCard(page, 'RAID 1').click();
    await btn(page, /continue|continuer/i).click();
    const w2 = await bar.evaluate((el) => (el as HTMLElement).style.width);
    expect(parseFloat(w2)).toBeGreaterThan(parseFloat(w1));
  });
});

// ===========================================================================
// 5. Submission flows
// ===========================================================================

test.describe('5 — Submission flows', () => {
  test('5.1 — with a valid name + disks, "Confirm creation" is enabled (not submitted here)', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    if ((await selectableDiskCount(page)) < 2) test.skip(true, 'no free disks to reach step 3');
    await toStep2(page);
    const rows = win(page).locator('[data-testid="raid-disk-row"].selectable input[type="checkbox"]');
    await rows.nth(0).check();
    await rows.nth(1).check();
    await btn(page, /continue|continuer/i).click();
    await win(page).locator('#raidName').fill('e2e-would-create');
    await expect(btn(page, /confirm creation|confirmer la création/i)).toBeEnabled();
    // Deliberately do NOT click it — that would build a real array.
  });

  test('5.2 — step 3 with a blank name keeps "Confirm creation" disabled', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    if ((await selectableDiskCount(page)) < 2) test.skip(true, 'no free disks to reach step 3');
    await toStep2(page);
    const rows = win(page).locator('[data-testid="raid-disk-row"].selectable input[type="checkbox"]');
    await rows.nth(0).check();
    await rows.nth(1).check();
    await btn(page, /continue|continuer/i).click();
    await win(page).locator('#raidName').fill('');
    await expect(btn(page, /confirm creation|confirmer la création/i)).toBeDisabled();
  });
});

// ===========================================================================
// 6. Navigation
// ===========================================================================

test.describe('6 — Navigation', () => {
  test('6.1 — "Back" returns to the previous step', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    await raidTypeCard(page, 'RAID 1').click();
    await btn(page, /continue|continuer/i).click();
    await expect(win(page).locator('[data-testid="raid-disk-head"]')).toBeVisible();
    await btn(page, /^back$|retour/i).click();
    await expect(win(page).locator('[data-testid="raid-type-grid"]')).toBeVisible();
  });

  test('6.2 — step 1 has no "Back" button', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    await expect(btn(page, /^back$|retour/i)).toHaveCount(0);
  });
});

// ===========================================================================
// 7. Responsive
// ===========================================================================

test.describe('7 — Responsive', () => {
  for (const w of [360, 768, 1920]) {
    test(`7.x — no horizontal overflow at ${w}px`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: 820 });
      await gotoRaid(page, adminTokens);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(overflow).toBeLessThanOrEqual(2);
      await expect(win(page).locator('[data-testid="raid-wizard"]')).toBeVisible();
    });
  }
});

// ===========================================================================
// 8. Accessibility
// ===========================================================================

test.describe('8 — Accessibility', () => {
  test('8.1 — the name input has an associated label', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    if ((await selectableDiskCount(page)) < 2) test.skip(true, 'no free disks to reach step 3');
    await toStep2(page);
    const rows = win(page).locator('[data-testid="raid-disk-row"].selectable input[type="checkbox"]');
    await rows.nth(0).check();
    await rows.nth(1).check();
    await btn(page, /continue|continuer/i).click();
    await expect(page.locator('label[for="raidName"]')).toBeVisible();
  });

  test('8.2 — no duplicate element IDs on the page', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const dupes = await page.evaluate(() => {
      const seen = new Set<string>();
      const dup: string[] = [];
      for (const el of Array.from(document.querySelectorAll('[id]'))) {
        if (seen.has(el.id)) dup.push(el.id);
        seen.add(el.id);
      }
      return dup;
    });
    expect(dupes).toEqual([]);
  });

  test('8.3 — every RAID-type card is a real <button>', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const tags = await win(page)
      .locator('[data-testid="raid-type-card"]')
      .evaluateAll((els) => els.map((e) => e.tagName));
    expect(tags.every((t) => t === 'BUTTON')).toBe(true);
  });
});

// ===========================================================================
// 9. Performance
// ===========================================================================

test.describe('9 — Performance', () => {
  test('9.1 — the window is interactive quickly', async ({ page }) => {
    const start = Date.now();
    await gotoRaid(page, adminTokens);
    expect(Date.now() - start).toBeLessThan(15_000);
  });

  test('9.2 — the RAID window DOM stays small', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const nodes = await win(page).evaluate((el) => el.querySelectorAll('*').length);
    expect(nodes).toBeLessThan(1500);
  });
});

// ===========================================================================
// 10. Security
// ===========================================================================

test.describe('10 — Security', () => {
  test('10.1 — POST /api/storage/raid rejects a request with no session', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const r = await page.evaluate(async () => {
      const res = await fetch('/api/storage/raid', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 'x', level: 'raid1', devices: ['/dev/a', '/dev/b'], filesystem: 'ext4' }),
      });
      return { http: res.status };
    });
    // `require_admin` with no `x-session-token` header returns 400 (the
    // guard's no-token branch), not 401/403 — the point is that it never
    // reaches mdadm / succeeds.
    expect([400, 401, 403]).toContain(r.http);
  });

  test('10.2 — a non-admin session cannot create a virtual disk', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const r = await apiCreate(page, plainUser, {
      name: 'e2e-nonadmin', level: 'raid1', devices: ['/dev/a', '/dev/b'], filesystem: 'ext4',
    });
    expect([401, 403]).toContain(r.http);
    expect(r.success).not.toBe(true);
  });

  test('10.3 — the session token never appears in the URL', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    expect(page.url()).not.toContain(adminTokens.token);
  });
});

// ===========================================================================
// 11. Keyboard
// ===========================================================================

test.describe('11 — Keyboard', () => {
  test('11.1 — a RAID-type card can be focused and activated with the keyboard', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const card = raidTypeCard(page, 'RAID 1');
    await card.focus();
    await page.keyboard.press('Enter');
    await expect(card).toHaveClass(/selected/);
  });
});

// ===========================================================================
// 12. Console & network
// ===========================================================================

test.describe('12 — Console & network', () => {
  test('12.1 — no console errors while stepping through the wizard', async ({ page }) => {
    const errors: string[] = [];
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(m.text());
    });
    await gotoRaid(page, adminTokens);
    await raidTypeCard(page, 'RAID 1').click();
    await btn(page, /continue|continuer/i).click();
    const real = errors.filter((e) => !/favicon|404|401|ResizeObserver/i.test(e));
    expect(real).toEqual([]);
  });

  test('12.2 — opening the app makes a bounded number of API calls', async ({ page }) => {
    const calls: string[] = [];
    page.on('request', (r) => {
      if (r.url().includes('/api/storage/')) calls.push(r.url());
    });
    await gotoRaid(page, adminTokens);
    expect(calls.length).toBeLessThan(8);
  });
});

// ===========================================================================
// 13. Real virtual-disk lifecycle (opt-in) — create in RAID, manage in Storage
// ===========================================================================

test.describe('13 — Real lifecycle (opt-in)', () => {
  test('13.1 — create a mirror, see it in Storage, destroy it there', async ({ page }) => {
    test.skip(
      REAL_DEVICES.length < 2,
      'set RAID_E2E_DEVICES=/dev/loopX,/dev/loopY (disposable) to run this',
    );
    // The in-page fetch below needs a real origin (about:blank has no base URL).
    await page.goto(BASE);
    const name = `e2emd${Date.now().toString(36)}`.slice(0, 20);

    // Create via the RAID endpoint (mdadm mirror).
    const c = await apiCreate(page, adminTokens, {
      name, level: 'raid1', devices: REAL_DEVICES.slice(0, 2), filesystem: 'ext4',
    });
    expect(c.success, `create failed: ${c.message}`).toBe(true);

    try {
      // It must now show up in Storage's unified virtual-disks list.
      const list = await page.evaluate(
        async ({ token }) => {
          const r = await fetch('/api/storage/virtual-disks', { headers: { 'x-session-token': token } });
          const j = await r.json().catch(() => ({}));
          return (j?.data as any[]) || [];
        },
        { token: adminTokens.token },
      );
      expect(list.some((v) => v.name === name)).toBe(true);
    } finally {
      // Destroy it through Storage's dispatch endpoint.
      await page.evaluate(
        async ({ token, csrf, name }) => {
          await fetch(`/api/storage/virtual-disks/${encodeURIComponent(name)}`, {
            method: 'DELETE',
            headers: { 'x-session-token': token, 'x-csrf-token': csrf },
          });
        },
        { token: adminTokens.token, csrf: adminTokens.csrf, name },
      );
    }
  });
});

// ===========================================================================
// 14. Edge cases
// ===========================================================================

test.describe('14 — Edge cases', () => {
  test('14.1 — rapid re-clicks on a type card do not break the wizard', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const card = raidTypeCard(page, 'RAID 1');
    for (let i = 0; i < 6; i++) await card.click();
    await expect(btn(page, /continue|continuer/i)).toBeEnabled();
  });

  test('14.2 — a ZFS + RAID 10 request is handled by the server without a crash', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    const r = await apiCreate(page, adminTokens, {
      name: 'e2e-zfs10', level: 'raid10', devices: ['/dev/z1', '/dev/z2', '/dev/z3', '/dev/z4'], filesystem: 'zfs',
    });
    expect(r.http).not.toBe(500);
    expect(r.success).not.toBe(true); // fake devices → clean rejection
  });

  test('14.3 — switching level after reaching step 2 keeps the wizard consistent', async ({ page }) => {
    await gotoRaid(page, adminTokens);
    await raidTypeCard(page, 'RAID 1').click();
    await btn(page, /continue|continuer/i).click();
    await btn(page, /^back$|retour/i).click();
    await raidTypeCard(page, 'RAID 6').click();
    await btn(page, /continue|continuer/i).click();
    await expect(win(page).locator('[data-testid="raid-disk-head"]')).toBeVisible();
  });
});
