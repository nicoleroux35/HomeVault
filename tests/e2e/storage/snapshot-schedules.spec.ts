/* =============================================================================
 * File          : snapshot-schedules.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage → Snapshots tab end-to-end tests (Playwright): covers the
 *   screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Snapshot schedules (Storage app, "Snapshots" tab) — End-to-End Test Suite
 *
 * The tab (`Storage.svelte`, `storage.snapshots.*` i18n) manages cron-driven
 * pool snapshots with GFS retention (docs/guides/snapshots.md). This suite:
 *
 *   1. Opens the tab; empty state or schedule cards render
 *   2. Creates a schedule through the modal (UI write) for a pool name that
 *      does NOT exist on the host, then deletes it via the confirm dialog
 *   3. Refuses a second schedule with the same pool and prefix (409 → inline
 *      modal error), the pair being what identifies a schedule
 *   4. Run-now on that non-existent pool records a clean failure (never a
 *      crash) — the error is surfaced on the schedule card
 *   5. Validation: an invalid pool name is rejected with an inline error
 *
 * The suite never touches a real pool: "e2e-no-such-pool" fails cleanly at
 * zfs/btrfs time and is cleaned up in afterAll through the admin API.
 *
 * Run (all):    npx playwright test tests/e2e/storage/snapshot-schedules.spec.ts --project=chromium
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/storage/snapshot-schedules.spec.ts
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const POOL = 'e2e-no-such-pool';

interface Tokens {
  username: string;
  password: string;
  token: string;
  csrf: string;
}

async function newCtx(playwright: any): Promise<APIRequestContext> {
  return playwright.request.newContext({ baseURL: BASE });
}

async function apiLogin(ctx: APIRequestContext, username: string, password: string) {
  const res = await ctx.post('/api/auth/login', { data: { username, password } });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
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

/** Fresh installs show the once-per-version "What's New" modal — close it. */
async function dismissWhatsNew(page: Page) {
  const modal = page.locator('[data-testid="whats-new-modal"]');
  if (await modal.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await modal.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

function win(page: Page) {
  return page.locator('[data-testid="storage-page"]');
}

async function openSnapshotsTab(page: Page, tokens: Tokens, lang?: string) {
  await seedBrowser(page, tokens, lang);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await dismissWhatsNew(page);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[title="Storage"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="storage-page"]', { timeout: 10_000 });
  await expect(win(page).locator('[data-testid="storage-loading"]')).toHaveCount(0, { timeout: 15_000 });
  await win(page).locator('.hv-winhdr-tab', { hasText: /Snapshots/ }).first().click();
  await page.waitForSelector('[data-testid="storage-snapshots"]', { timeout: 10_000 });
}

let adminTokens: Tokens;
let scheduleId: string | null = null;

test.beforeAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };

  // Start from a clean slate: (pool, prefix) is unique, so a schedule left
  // behind by an interrupted run would make the create in 2.1 answer 409.
  const headers = { 'x-session-token': token, 'x-csrf-token': csrf };
  const res = await ctx.get(`${BASE}/api/snapshot-schedules`, { headers });
  const body = await res.json().catch(() => ({}));
  for (const schedule of body?.data?.schedules ?? []) {
    if (schedule.pool === POOL) {
      await ctx.delete(`${BASE}/api/snapshot-schedules/${schedule.id}`, { headers });
    }
  }
});

test.afterAll(async ({ playwright }) => {
  if (!scheduleId) return;
  const ctx = await newCtx(playwright);
  await ctx.delete(`${BASE}/api/snapshot-schedules/${scheduleId}`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
  });
});

test('1.1 — The Snapshots tab opens with an empty state or schedule cards', async ({ page }) => {
  await openSnapshotsTab(page, adminTokens);
  await expect(
    win(page).locator('[data-testid="snapshots-empty"], [data-testid="snapshots-schedule"]').first(),
  ).toBeVisible({ timeout: 10_000 });
  // The "New schedule" action is present either way.
  await expect(win(page).locator('[data-testid="snapshots-new-btn"]')).toBeVisible();
});

test('2.1 — Create a schedule through the modal', async ({ page }) => {
  await openSnapshotsTab(page, adminTokens);
  await win(page).locator('[data-testid="snapshots-new-btn"]').click();
  await page.locator('input[placeholder="tank/data"]').first().fill(POOL);
  await page.locator('input[placeholder="0 3 * * *"]').first().fill('0 5 * * 0');
  await page.locator('[data-testid="snapshots-save-btn"]').click();

  const row = win(page).locator('[data-testid="snapshots-schedule"]', { hasText: POOL });
  await expect(row).toBeVisible({ timeout: 10_000 });
  await expect(row).toContainText('0 5 * * 0');

  // Grab the id from a row action button for afterAll cleanup.
  const run = row.locator('[data-testid^="snapshots-run-"]').first();
  const testId = await run.getAttribute('data-testid');
  scheduleId = testId ? testId.replace('snapshots-run-', '') : null;
});

test('2.2 — A second schedule on the same pool and prefix is refused inline', async ({ page }) => {
  test.skip(!scheduleId, 'schedule not created');
  await openSnapshotsTab(page, adminTokens);
  await win(page).locator('[data-testid="snapshots-new-btn"]').click();
  // Same pool, same prefix as the schedule created in 2.1: the two rows
  // would prune each other's snapshots, so the API answers 409 and the modal
  // must say so instead of closing silently.
  await page.locator('input[placeholder="tank/data"]').first().fill(POOL);
  await page.locator('[data-testid="snapshots-save-btn"]').click();

  const error = page.locator('[data-testid="snapshots-form-error"]');
  await expect(error).toBeVisible({ timeout: 10_000 });
  await expect(error).toContainText('already exists');
  // The modal stays open with the operator's input intact (no data lost).
  await expect(page.locator('input[placeholder="tank/data"]').first()).toHaveValue(POOL);
});

test('3.1 — Run-now on a missing pool records a clean failure on the card', async ({ page }) => {
  test.skip(!scheduleId, 'schedule not created');
  await openSnapshotsTab(page, adminTokens);
  const row = win(page).locator('[data-testid="snapshots-schedule"]', { hasText: POOL });
  await row.locator('[data-testid^="snapshots-run-"]').click();
  // No zfs/btrfs pool "e2e-no-such-pool" exists — the run fails cleanly and
  // the card surfaces the error (never a crash / blank tab).
  await expect(row.locator('[data-testid="snapshots-schedule-error"]')).toBeVisible({ timeout: 20_000 });
});

test('5.1 — a share target sends share and share_type, not just a pool', async ({ page }) => {
  let posted: any = null;
  await page.route('**/api/shares', async (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: [{ name: 'photos', share_type: 'smb', path: '/tank/photos', enabled: true }],
      }),
    });
  });
  await page.route('**/api/snapshot-schedules', async (route) => {
    if (route.request().method() !== 'POST') return route.continue();
    posted = route.request().postDataJSON();
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: true, data: { id: 'fake-share-schedule' } }),
    });
  });

  await openSnapshotsTab(page, adminTokens);
  await win(page).locator('[data-testid="snapshots-new-btn"]').click();
  const target = page.locator('[data-testid="snapshots-target"]');
  await expect(target).toBeVisible();
  await target.selectOption('smb/photos');
  await page.locator('input[placeholder="tank/data"]').first().fill('tank');
  await page.locator('[data-testid="snapshots-save-btn"]').click();

  await expect.poll(() => posted).toEqual(
    expect.objectContaining({ pool: 'tank', share: 'photos', share_type: 'smb' }),
  );
});

test('5.2 — a share snapshot restores only after the overwrite confirmation', async ({ page }) => {
  let restored: any = null;
  await page.route('**/api/snapshot-schedules', async (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: {
          schedules: [
            {
              id: 'share-schedule',
              pool: 'tank',
              share: 'photos',
              share_type: 'smb',
              enabled: true,
              cron: '0 3 * * *',
              prefix: 'hv-auto',
              recursive: true,
              gfs_hourly: 24,
              gfs_daily: 7,
              gfs_weekly: 4,
              gfs_monthly: 12,
              gfs_yearly: 1,
              last_run_at: null,
              last_status: null,
              last_error: null,
              next_run_at: null,
            },
          ],
        },
      }),
    });
  });
  await page.route('**/api/shares/photos/smb/snapshots', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: {
          share: { name: 'photos', share_type: 'smb', path: '/tank/photos' },
          pool: 'tank',
          snapshots: [
            {
              name: 'hv-auto-20261105-030000',
              created: '1760000000',
              used: 0,
              referenced: 0,
              browse_path: '/tank/photos/.zfs/snapshot/hv-auto-20261105-030000',
            },
          ],
        },
      }),
    });
  });
  await page.route('**/api/shares/photos/smb/snapshots/hv-auto-20261105-030000/restore', async (route) => {
    restored = route.request().postDataJSON();
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: true, data: { restored: true } }),
    });
  });

  await openSnapshotsTab(page, adminTokens);
  const row = win(page).locator('[data-testid="snapshots-schedule"]', { hasText: 'share: photos' });
  await row.locator('[data-testid^="snapshots-view-"]').click();
  await win(page).locator('[data-testid="snapshots-restore-hv-auto-20261105-030000"]').click();

  const submit = page.locator('[data-testid="snapshots-restore-submit"]');
  await expect(submit).toBeDisabled();
  await page.locator('[data-testid="snapshots-restore-path"]').fill('photos/2024/image.jpg');
  await expect(submit).toBeDisabled();
  await page.locator('[data-testid="snapshots-restore-confirm"]').check();
  await expect(submit).toBeEnabled();
  await submit.click();

  await expect.poll(() => restored).toEqual({
    path: 'photos/2024/image.jpg',
    confirm: true,
  });
});

test('4.1 — Delete goes through the confirm dialog', async ({ page }) => {
  test.skip(!scheduleId, 'schedule not created');
  await openSnapshotsTab(page, adminTokens);
  const row = win(page).locator('[data-testid="snapshots-schedule"]', { hasText: POOL });
  await row.locator('[data-testid^="snapshots-delete-"]').click();
  // Confirm inside the confirm-dialog (scoped by its message testid — the
  // window frame itself is also a dialog).
  const dialog = page
    .locator('[data-testid="confirm-dialog-message"]')
    .locator('xpath=ancestor-or-self::*[@role="dialog"][1]');
  await expect(dialog).toBeVisible({ timeout: 5000 });
  await dialog.locator('button:has-text("Confirm"), button:has-text("Confirmer")').first().click();
  await expect(row).toHaveCount(0, { timeout: 10_000 });
  scheduleId = null; // cleaned up
});
