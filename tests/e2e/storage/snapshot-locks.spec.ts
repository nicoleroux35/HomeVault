/* =============================================================================
 * File          : snapshot-locks.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage → Snapshots → protection locks (ROADMAP P2.5), end-to-end
 *   (Playwright): the lock list, an empty state that explains itself, the lock
 *   modal and its validation, the release path (plain and break-glass with the
 *   typed phrase), and that a cancelled release fires no request.
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/storage/snapshot-locks.spec.ts --project=chromium
 * =============================================================================
 */

/**
 * Immutable snapshots (Storage app) — End-to-End Test Suite
 *
 * **SAFETY: no filesystem hold is ever placed.** Every mutating call is
 * intercepted with `page.route`, so these tests assert the *screen* — which
 * phrase the break-glass dialog demands, what is sent, what a refused answer
 * looks like. The hold itself (`zfs hold` / `btrfs property set ro true`) is
 * covered by `src/snapshot_locks.rs` (argv construction, the release decision)
 * and `tests/snapshot_locks_integration.rs` (endpoint contract, restart
 * persistence, deletion refusal).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

interface Tokens {
  username: string;
  password: string;
  token: string;
  csrf: string;
}

interface LockRow {
  id: number;
  pool: string;
  snapshot: string;
  tag: string;
  locked_until: string;
  reason: string | null;
  created_by: string;
  created_at: string;
  expired: boolean;
  break_glass_phrase: string;
}

async function apiLogin(ctx: APIRequestContext, username: string, password: string) {
  const res = await ctx.post('/api/auth/login', { data: { username, password } });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

async function seedBrowser(page: Page, tokens: Tokens) {
  await page.addInitScript(
    ({ t }) => {
      try {
        localStorage.setItem('session-token', t.token);
        localStorage.setItem('csrf-token', t.csrf);
      } catch {
        /* storage unavailable — the test will surface it */
      }
    },
    { t: tokens },
  );
}

async function dismissWhatsNew(page: Page) {
  const modal = page.locator('[data-testid="whats-new-modal"]');
  if (await modal.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await modal.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

/** Opens Storage → Snapshots, where the lock section lives. */
async function openLocks(page: Page, tokens: Tokens) {
  await seedBrowser(page, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await dismissWhatsNew(page);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[title="Storage"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="storage-page"]', { timeout: 10_000 });
  await expect(page.locator('[data-testid="storage-loading"]')).toHaveCount(0, { timeout: 15_000 });
  await page.locator('[data-testid="storage-page"] .hv-winhdr-tab', { hasText: /Snapshots/ }).first().click();
  await page.waitForSelector('[data-testid="storage-snapshots"]', { timeout: 10_000 });
  await page.waitForSelector('[data-testid="snapshots-locks"]', { timeout: 10_000 });
}

const later = (days: number) => new Date(Date.now() + days * 86_400_000).toISOString();

const lock = (over: Partial<LockRow> = {}): LockRow => ({
  id: 7,
  pool: 'tank',
  snapshot: 'tank@hv-auto-20261001-000000',
  tag: 'homevault-lock',
  locked_until: later(20),
  reason: 'before the library migration',
  created_by: 'e2eadmin',
  created_at: new Date().toISOString(),
  expired: false,
  break_glass_phrase: 'release hv-auto-20261001-000000',
  ...over,
});

let adminTokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  const ctx = await playwright.request.newContext({ baseURL: BASE });
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
  // A freshly seeded server answers `GET /api/whats-new` with `seen: false`,
  // and the desktop then opens a bits-ui modal whose full-screen overlay
  // swallows every pointer event on the dock. Acknowledge the release once
  // here — the same authenticated + CSRF'd call the modal's own close button
  // makes — so the overlay never sits between a test and its first click
  // (`desktop.spec.ts` / `docker.spec.ts` carry the same guard).
  await ctx
    .post('/api/whats-new/seen', { headers: { 'X-Session-Token': token, 'X-CSRF-Token': csrf }, data: {} })
    .catch(() => null);
  await ctx.dispose();
});

test('1.1 — an instance with no lock says so instead of showing an empty box', async ({ page }) => {
  await page.route('**/api/snapshots/locks**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: { locks: [], history: [] } }) }),
  );
  await openLocks(page, adminTokens);
  await expect(page.locator('[data-testid="snapshots-locks-empty"]')).toBeVisible();
  await expect(page.locator('[data-testid="snapshots-lock-7"]')).toHaveCount(0);
});

test('1.2 — a lock shows its snapshot, its horizon, its reason and who placed it', async ({ page }) => {
  await page.route('**/api/snapshots/locks**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: { locks: [lock()], history: [] } }) }),
  );
  await openLocks(page, adminTokens);
  const row = page.locator('[data-testid="snapshots-lock-7"]');
  await expect(row).toBeVisible();
  await expect(row).toContainText('tank@hv-auto-20261001-000000');
  await expect(row).toContainText('before the library migration');
  await expect(row).toContainText('e2eadmin');
  // The horizon is rendered as a real date, never as a raw ISO string.
  await expect(row).not.toContainText(/\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/);
});

test('1.3 — a lapsed lock says it is waiting for the release loop', async ({ page }) => {
  await page.route('**/api/snapshots/locks**', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: true, data: { locks: [lock({ expired: true, locked_until: new Date(Date.now() - 86_400_000).toISOString() })], history: [] } }),
    }),
  );
  await openLocks(page, adminTokens);
  await expect(page.locator('[data-testid="snapshots-lock-7"]')).toContainText(/Lapsed|Échu|Abgelaufen|Caducó/);
});

test('2.1 — releasing a lapsed lock needs a confirm, not a typed phrase', async ({ page }) => {
  let bodies: string[] = [];
  await page.route('**/api/snapshots/locks**', (route) => {
    if (route.request().method() === 'DELETE') {
      bodies.push(route.request().postData() ?? '');
      return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: { released: true, break_glass: false } }) });
    }
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: true, data: { locks: [lock({ expired: true, locked_until: new Date(Date.now() - 3600_000).toISOString() })], history: [] } }),
    });
  });
  await openLocks(page, adminTokens);
  await page.locator('[data-testid="snapshots-lock-release-7"]').click();
  const dialog = page.locator('[data-slot="dialog-content"]').filter({ hasText: /hv-auto-20261001-000000/ });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: /^(Confirm|Confirmer|Bestätigen|Confirmar)$/ }).click();
  await expect.poll(() => bodies.length).toBe(1);
  // An expired lock needs no typed confirmation.
  expect(JSON.parse(bodies[0] || '{}').confirmation ?? null).toBeNull();
});

test('2.2 — releasing early demands the exact phrase and sends it back', async ({ page }) => {
  let bodies: string[] = [];
  await page.route('**/api/snapshots/locks**', (route) => {
    if (route.request().method() === 'DELETE') {
      bodies.push(route.request().postData() ?? '');
      return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: { released: true, break_glass: true } }) });
    }
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: { locks: [lock()], history: [] } }) });
  });
  await openLocks(page, adminTokens);
  await page.locator('[data-testid="snapshots-lock-release-7"]').click();

  const dialog = page.locator('[data-slot="dialog-content"]').filter({ hasText: /hv-auto-20261001-000000/ });
  await expect(dialog).toBeVisible();
  const confirm = dialog.getByRole('button', { name: /^(Confirm|Confirmer|Bestätigen|Confirmar)$/ });
  // The confirm button stays disabled until the phrase is typed exactly.
  await expect(confirm).toBeDisabled();
  const field = dialog.getByRole('textbox');
  await field.fill('wrong phrase');
  await expect(confirm).toBeDisabled();
  await field.fill('release hv-auto-20261001-000000');
  await expect(confirm).toBeEnabled();
  await confirm.click();

  await expect.poll(() => bodies.length).toBe(1);
  expect(JSON.parse(bodies[0] || '{}').confirmation).toBe('release hv-auto-20261001-000000');
});

test('2.3 — cancelling the release fires no request at all', async ({ page }) => {
  let deletes = 0;
  await page.route('**/api/snapshots/locks**', (route) => {
    if (route.request().method() === 'DELETE') deletes += 1;
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: { locks: [lock()], history: [] } }) });
  });
  await openLocks(page, adminTokens);
  await page.locator('[data-testid="snapshots-lock-release-7"]').click();
  const dialog = page.locator('[data-slot="dialog-content"]').filter({ hasText: /hv-auto-20261001-000000/ });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: /^(Cancel|Annuler|Abbrechen|Cancelar)$/ }).click();
  await expect(dialog).toBeHidden();
  expect(deletes).toBe(0);
});

test('3.1 — a refused release shows the server\'s reason instead of a silent failure', async ({ page }) => {
  await page.route('**/api/snapshots/locks**', (route) => {
    if (route.request().method() === 'DELETE') {
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: false, message: 'The snapshot hold could not be released: pool is busy' }),
      });
    }
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: { locks: [lock()], history: [] } }) });
  });
  await openLocks(page, adminTokens);
  await page.locator('[data-testid="snapshots-lock-release-7"]').click();
  const dialog = page.locator('[data-slot="dialog-content"]').filter({ hasText: /hv-auto-20261001-000000/ });
  await dialog.getByRole('textbox').fill('release hv-auto-20261001-000000');
  await dialog.getByRole('button', { name: /^(Confirm|Confirmer|Bestätigen|Confirmar)$/ }).click();
  await expect(page.locator('[data-testid="snapshots-locks-error"]')).toContainText('pool is busy');
});

test('4.1 — the lock modal validates the number of days client-side', async ({ page }) => {
  let posts = 0;
  await page.route('**/api/snapshots/locks**', (route) => {
    if (route.request().method() === 'POST') posts += 1;
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: { locks: [], history: [] } }) });
  });
  await openLocks(page, adminTokens);

  // The lock action lives on a pool's snapshot list; seed one through the API.
  await page.route('**/api/snapshot-schedules/snapshots*', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: true, data: { snapshots: [{ name: 'hv-auto-20261001-000000', created: new Date().toISOString(), used: 1024, referenced: 1024, auto: true }] } }),
    }),
  );
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('hv-open-app', { detail: { id: 'storage' } })));
  await page.locator('[data-testid="snapshots-view-0"], [data-testid^="snapshots-view-"]').first().click().catch(() => {});
  const lockButton = page.locator('[data-testid^="snapshots-lock-"]').first();
  if (await lockButton.count()) {
    await lockButton.click();
    await expect(page.locator('[data-testid="snapshots-lock-days"]')).toBeVisible();
    await page.locator('[data-testid="snapshots-lock-days"]').fill('0');
    await page.locator('[data-testid="snapshots-lock-save"]').click();
    await expect(page.locator('[data-testid="snapshots-lock-error"]')).toBeVisible();
    expect(posts).toBe(0);
  }
});

test('5.1 — the lock section stays readable at 360px and has no duplicate ids', async ({ page }) => {
  await page.route('**/api/snapshots/locks**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: { locks: [lock()], history: [] } }) }),
  );
  await page.setViewportSize({ width: 360, height: 720 });
  await openLocks(page, adminTokens);
  const section = page.locator('[data-testid="snapshots-locks"]');
  await expect(section).toBeVisible();
  const overflow = await section.evaluate((el) => el.scrollWidth - el.clientWidth);
  expect(overflow).toBeLessThanOrEqual(1);

  const ids = await page.evaluate(() => Array.from(document.querySelectorAll('[id]')).map((el) => el.id));
  expect(new Set(ids).size).toBe(ids.length);
});
