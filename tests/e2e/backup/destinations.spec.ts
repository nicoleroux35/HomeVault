/* =============================================================================
 * File          : destinations.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Backup destination (ROADMAP P2.3), end-to-end (Playwright): the settings
 *   card, the tenant list and its empty state, the one-time secrets panel, the
 *   quota editor, the refusal paths, the missing-binary warnings and the
 *   responsive/accessibility checks.
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/backup/destinations.spec.ts --project=chromium
 * =============================================================================
 */

/**
 * Backup destination (Destinations app) — End-to-End Test Suite
 *
 * **SAFETY: nothing is created on the instance.** Every `/api/backup-target*`
 * call is intercepted with `page.route`. The receive role, the append-only
 * rest-server unit, the quota revocation and the integrity check are covered
 * by `src/backup_target.rs` (unit), `tests/backup_target_integration.rs`
 * (endpoints + a real push/restore) and `src/backup_target_scheduler.rs`.
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

/** Opens the Backup destination app from the desktop. */
async function openDestinations(page: Page, tokens: Tokens) {
  await seedBrowser(page, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await dismissWhatsNew(page);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[title="Backup destination"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="destinations-page"]', { timeout: 15_000 });
}

const tenant = (over: Record<string, unknown> = {}) => ({
  id: 3,
  name: 'laptop-julie',
  repo_path: '/var/lib/homevault/backup-target/laptop-julie',
  repository_url: 'rest:http://homevault.local:8001/laptop-julie/',
  quota_bytes: 107_374_182_400,
  used_bytes: 32_212_254_720,
  quota_percent: 30,
  quota_verdict: 'under',
  over_quota: false,
  has_token: true,
  verify_enabled: true,
  last_seen_at: new Date().toISOString(),
  last_verified_at: new Date().toISOString(),
  last_verify_status: 'success',
  last_verify_error: null,
  created_at: new Date().toISOString(),
  ...over,
});

const targetState = (over: Record<string, unknown> = {}) => ({
  success: true,
  data: {
    settings: { enabled: true, port: 8001, listen: '0.0.0.0' },
    tenants: [],
    rest_server_available: true,
    restic_available: true,
    service_active: true,
    default_port: 8001,
    repos_root: '/var/lib/homevault/backup-target',
    ...over,
  },
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

test('1.1 — an instance that receives nothing explains what the role is for', async ({ page }) => {
  await page.route('**/api/backup-target**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(targetState()) }),
  );
  await openDestinations(page, adminTokens);
  await expect(page.locator('[data-testid="destinations-settings"]')).toBeVisible();
  await expect(page.locator('[data-testid="destinations-tenant"]')).toHaveCount(0);
  // The empty state says what to do next, and the storage root is stated.
  await expect(page.locator('[data-testid="destinations-tenants"]')).toContainText(/var\/lib\/homevault\/backup-target/);
});

test('1.2 — a tenant shows its repository address, its usage and its quota verdict', async ({ page }) => {
  await page.route('**/api/backup-target**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(targetState({ tenants: [tenant()] })) }),
  );
  await openDestinations(page, adminTokens);
  const row = page.locator('[data-testid="destinations-tenant"]').first();
  await expect(row).toContainText('laptop-julie');
  await expect(row).toContainText('rest:http://homevault.local:8001/laptop-julie/');
  await expect(row).toContainText('30%');
  await expect(row).not.toContainText(/over quota/i);
});

test('1.3 — a tenant over its quota is called out, not silently broken', async ({ page }) => {
  await page.route('**/api/backup-target**', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(
        targetState({ tenants: [tenant({ over_quota: true, quota_verdict: 'exceeded', quota_percent: 104 })] }),
      ),
    }),
  );
  await openDestinations(page, adminTokens);
  await expect(page.locator('[data-testid="destinations-tenant"]').first()).toContainText(/over quota/i);
});

test('2.1 — creating a tenant reveals the secrets exactly once, and never in the URL', async ({ page }) => {
  let posts = 0;
  await page.route('**/api/backup-target/tenants', (route) => {
    if (route.request().method() !== 'POST') return route.fallback();
    posts += 1;
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: {
          token: 'tok_E2EONLY_0123456789abcdef',
          repository_password: 'pw_E2EONLY_0123456789abcdef',
          repository_url: 'rest:http://homevault.local:8001/laptop-julie/',
        },
      }),
    });
  });
  await page.route('**/api/backup-target', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(targetState()) }),
  );

  const seen: string[] = [];
  page.on('request', (req) => seen.push(req.url()));

  await openDestinations(page, adminTokens);
  await page.getByRole('button', { name: /add a computer/i }).click();
  await page.locator('[data-testid="destinations-name"]').fill('laptop-julie');
  await page.locator('[data-testid="destinations-create-quota"]').fill('100');
  await page.getByRole('button', { name: /^(Create|Créer|Erstellen|Crear)$/ }).click();

  await expect(page.locator('[data-testid="destinations-token"]')).toHaveValue('tok_E2EONLY_0123456789abcdef');
  await expect(page.locator('[data-testid="destinations-password"]')).toHaveValue('pw_E2EONLY_0123456789abcdef');
  expect(posts).toBe(1);

  // A secret must never end up in a URL or in localStorage.
  expect(seen.some((u) => u.includes('tok_E2EONLY') || u.includes('pw_E2EONLY'))).toBe(false);
  const stored = await page.evaluate(() => JSON.stringify(localStorage));
  expect(stored).not.toContain('tok_E2EONLY');
  expect(stored).not.toContain('pw_E2EONLY');
});

test('2.2 — a refused creation shows the server reason inside the modal', async ({ page }) => {
  await page.route('**/api/backup-target/tenants', (route) => {
    if (route.request().method() !== 'POST') return route.fallback();
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: false, message: 'a tenant named “laptop-julie” already exists' }),
    });
  });
  await page.route('**/api/backup-target', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(targetState()) }),
  );
  await openDestinations(page, adminTokens);
  await page.getByRole('button', { name: /add a computer/i }).click();
  await page.locator('[data-testid="destinations-name"]').fill('laptop-julie');
  await page.getByRole('button', { name: /^(Create|Créer|Erstellen|Crear)$/ }).click();
  await expect(page.getByRole('alert')).toContainText('already exists');
});

test('2.3 — rotating a token asks first and never fires when cancelled', async ({ page }) => {
  let rotates = 0;
  await page.route('**/api/backup-target/tenants/*/rotate', (route) => {
    rotates += 1;
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: {} }) });
  });
  await page.route('**/api/backup-target**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(targetState({ tenants: [tenant()] })) }),
  );
  await openDestinations(page, adminTokens);
  await page.locator('[data-testid="destinations-rotate"]').first().click();
  const dialog = page.locator('[data-slot="dialog-content"]').filter({ hasText: /laptop-julie/ });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: /^(Cancel|Annuler|Abbrechen|Cancelar)$/ }).click();
  await expect(dialog).toBeHidden();
  expect(rotates).toBe(0);
});

test('3.1 — the screen warns when the programs it needs are missing', async ({ page }) => {
  await page.route('**/api/backup-target**', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(targetState({ rest_server_available: false, restic_available: false })),
    }),
  );
  await openDestinations(page, adminTokens);
  await expect(page.getByRole('alert').first()).toBeVisible();
  await expect(page.locator('[data-testid="destinations-content"]')).toBeVisible();
});

test('3.2 — a failed load says so instead of showing an empty screen', async ({ page }) => {
  await page.route('**/api/backup-target**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: false, message: 'database is locked' }) }),
  );
  await openDestinations(page, adminTokens);
  await expect(page.getByRole('alert')).toContainText('database is locked');
});

test('4.1 — the screen is readable at 360px with no duplicate ids', async ({ page }) => {
  await page.route('**/api/backup-target**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(targetState({ tenants: [tenant()] })) }),
  );
  await page.setViewportSize({ width: 360, height: 720 });
  await openDestinations(page, adminTokens);
  const overflow = await page.locator('[data-testid="destinations-content"]').evaluate((el) => el.scrollWidth - el.clientWidth);
  expect(overflow).toBeLessThanOrEqual(1);

  const ids = await page.evaluate(() => Array.from(document.querySelectorAll('[id]')).map((el) => el.id));
  expect(new Set(ids).size).toBe(ids.length);
});
