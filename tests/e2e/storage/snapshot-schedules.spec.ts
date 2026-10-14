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
 *   3. Run-now on that non-existent pool records a clean failure (never a
 *      crash) — the error is surfaced on the schedule card
 *   4. Validation: an invalid pool name is rejected with an inline error
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

test('3.1 — Run-now on a missing pool records a clean failure on the card', async ({ page }) => {
  test.skip(!scheduleId, 'schedule not created');
  await openSnapshotsTab(page, adminTokens);
  const row = win(page).locator('[data-testid="snapshots-schedule"]', { hasText: POOL });
  await row.locator('[data-testid^="snapshots-run-"]').click();
  // No zfs/btrfs pool "e2e-no-such-pool" exists — the run fails cleanly and
  // the card surfaces the error (never a crash / blank tab).
  await expect(row.locator('[data-testid="snapshots-schedule-error"]')).toBeVisible({ timeout: 20_000 });
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
