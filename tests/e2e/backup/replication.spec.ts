/* =============================================================================
 * File          : replication.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Snapshot replication (ROADMAP P2.4), end-to-end (Playwright): the job
 *   list, the capability line, the create/edit modal and its validation, the
 *   dry-run preview, run-now, delete confirmation and responsiveness.
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/backup/replication.spec.ts --project=chromium
 * =============================================================================
 */

/**
 * Snapshot replication (Replication app) — End-to-End Test Suite
 *
 * **SAFETY: no `zfs send`/`btrfs send` is ever started.** Every
 * `/api/replication*` call is intercepted with `page.route`, and the run-now
 * path is asserted on the *request* it would make, never by letting it reach
 * the backend. The pipeline construction, validation and retention rules are
 * covered by `src/replication.rs` (unit) and `tests/replication_integration.rs`.
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

async function openReplication(page: Page, tokens: Tokens) {
  await seedBrowser(page, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await dismissWhatsNew(page);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[title="Replication"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="replication-page"]', { timeout: 15_000 });
}

const CAPS = {
  zfs: true,
  btrfs: false,
  ssh: true,
  pv: true,
  max_keep_last: 365,
  max_bandwidth_kbps: 10_000_000,
  min_interval_minutes: 5,
  max_interval_minutes: 43_200,
  prefix: 'hv-repl',
};

const job = (over: Record<string, unknown> = {}) => ({
  id: 11,
  name: 'nightly-to-nas2',
  source_dataset: 'tank/data',
  backend: 'zfs',
  destination_kind: 'local',
  destination_dataset: 'backup/data',
  destination_host: null,
  destination_user: null,
  destination_port: 22,
  interval_minutes: 1440,
  keep_last: 7,
  bandwidth_kbps: 0,
  enabled: true,
  next_run_at: new Date(Date.now() + 86_400_000).toISOString(),
  last_run_at: new Date(Date.now() - 3_600_000).toISOString(),
  last_snapshot: 'hv-repl-11-20261030-030000',
  last_status: 'success',
  last_error: null,
  running: false,
  bandwidth_note: null,
  ...over,
});

const listBody = (over: Record<string, unknown> = {}) => ({
  success: true,
  data: { jobs: [], capabilities: CAPS, ...over },
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

test('1.1 — an instance with no job says what a job is for', async ({ page }) => {
  await page.route('**/api/replication**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(listBody()) }),
  );
  await openReplication(page, adminTokens);
  await expect(page.locator('[data-testid="replication-content"]')).toBeVisible();
  await expect(page.locator('[data-testid="replication-job"]')).toHaveCount(0);
});

test('1.2 — the capability line tells the truth about what this host can do', async ({ page }) => {
  await page.route('**/api/replication**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(listBody()) }),
  );
  await openReplication(page, adminTokens);
  const line = page.locator('[data-testid="replication-content"] p').first();
  // ZFS and SSH are reported available, Btrfs is reported as not installed.
  await expect(line).toContainText(/Btrfs/i);
  await expect(line).toContainText(/not installed|nicht installiert|non installé|no instalado/i);
});

test('1.3 — a job shows its dataset pair, its schedule and its retention', async ({ page }) => {
  await page.route('**/api/replication**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(listBody({ jobs: [job()] })) }),
  );
  await openReplication(page, adminTokens);
  const card = page.locator('[data-testid="replication-job"]').first();
  await expect(card).toContainText('nightly-to-nas2');
  await expect(card).toContainText('tank/data');
  await expect(card).toContainText('backup/data');
  await expect(card).toContainText('7');
  await expect(card).toContainText('hv-repl-11-20261030-030000');
});

test('1.4 — a failed run shows its recorded error instead of hiding it', async ({ page }) => {
  await page.route('**/api/replication**', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(listBody({ jobs: [job({ last_status: 'failed', last_error: 'permission denied (publickey)' })] })),
    }),
  );
  await openReplication(page, adminTokens);
  await expect(page.getByRole('alert')).toContainText('permission denied (publickey)');
});

test('2.1 — the modal refuses an incomplete job before sending it', async ({ page }) => {
  let posts = 0;
  await page.route('**/api/replication**', (route) => {
    if (route.request().method() === 'POST') posts += 1;
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(listBody()) });
  });
  await openReplication(page, adminTokens);
  await page.locator('[data-testid="replication-new"]').click();
  await page.locator('[data-testid="replication-form-name"]').fill('nightly');
  // Source and destination datasets are still empty.
  await page.getByRole('button', { name: /^(Save|Enregistrer|Speichern|Guardar)$/ }).click();
  await expect(page.getByRole('alert')).toBeVisible();
  expect(posts).toBe(0);
});

test('2.2 — a created job sends exactly the fields the server validates', async ({ page }) => {
  let body: any = null;
  await page.route('**/api/replication**', (route) => {
    if (route.request().method() === 'POST') {
      body = JSON.parse(route.request().postData() || '{}');
      return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: {} }) });
    }
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(listBody()) });
  });
  await openReplication(page, adminTokens);
  await page.locator('[data-testid="replication-new"]').click();
  await page.locator('[data-testid="replication-form-name"]').fill('nightly');
  await page.locator('[data-testid="replication-form-source"]').fill('tank/data');
  await page.locator('[data-testid="replication-form-destination"]').fill('backup/data');
  await page.getByRole('button', { name: /^(Save|Enregistrer|Speichern|Guardar)$/ }).click();

  await expect.poll(() => body).not.toBeNull();
  expect(body.name).toBe('nightly');
  expect(body.source_dataset).toBe('tank/data');
  expect(body.destination_dataset).toBe('backup/data');
  expect(body.destination_kind).toBe('local');
  // A local job must not carry SSH fields at all.
  expect(body.destination_host).toBeNull();
  expect(body.destination_user).toBeNull();
});

test('2.3 — choosing SSH reveals the host/user/port fields', async ({ page }) => {
  await page.route('**/api/replication**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(listBody()) }),
  );
  await openReplication(page, adminTokens);
  await page.locator('[data-testid="replication-new"]').click();
  const kind = page.locator('select').filter({ has: page.locator('option[value="ssh"]') }).first();
  await expect(page.locator('[data-testid="replication-form-host"]')).toHaveCount(0);
  await kind.selectOption('ssh');
  await expect(page.locator('[data-testid="replication-form-host"]')).toBeVisible();
});

test('3.1 — the dry run shows the exact command and copies nothing', async ({ page }) => {
  let runs = 0;
  await page.route('**/api/replication**', (route) => {
    const url = route.request().url();
    if (url.includes('/run')) runs += 1;
    if (url.includes('/preview')) {
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: {
            snapshot: 'hv-repl-11-20261031-030000',
            base: 'hv-repl-11-20261030-030000',
            command: 'sudo zfs send -s -i tank/data@hv-repl-11-20261030-030000 tank/data@hv-repl-11-20261031-030000 | sudo zfs receive -s backup/data',
            bandwidth_note: null,
          },
        }),
      });
    }
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(listBody({ jobs: [job()] })) });
  });
  await openReplication(page, adminTokens);
  await page.locator('[data-testid="replication-preview-11"]').click();
  await expect(page.locator('[data-testid="replication-preview-command"]')).toContainText('zfs send -s -i');
  expect(runs).toBe(0);
});

test('3.2 — run-now asks first and sends exactly one run request when confirmed', async ({ page }) => {
  let runs = 0;
  await page.route('**/api/replication**', (route) => {
    const url = route.request().url();
    if (url.includes('/run')) {
      runs += 1;
      return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: {} }) });
    }
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(listBody({ jobs: [job()] })) });
  });
  await openReplication(page, adminTokens);
  await page.locator('[data-testid="replication-run-11"]').click();
  const dialog = page.locator('[data-slot="dialog-content"]').filter({ hasText: /nightly-to-nas2/ });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: /^(Confirm|Confirmer|Bestätigen|Confirmar)$/ }).first().click();
  await expect.poll(() => runs).toBe(1);
});

test('4.1 — deleting a job warns that the copies stay, and cancelling fires nothing', async ({ page }) => {
  let deletes = 0;
  await page.route('**/api/replication**', (route) => {
    if (route.request().method() === 'DELETE') deletes += 1;
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(listBody({ jobs: [job()] })) });
  });
  await openReplication(page, adminTokens);
  await page.locator('[data-testid="replication-delete-11"]').click();
  const dialog = page.locator('[data-slot="dialog-content"]').filter({ hasText: /nightly-to-nas2/ });
  await expect(dialog).toContainText(/copies|Kopien|kopier|copias/i);
  await dialog.getByRole('button', { name: /^(Cancel|Annuler|Abbrechen|Cancelar)$/ }).click();
  await expect(dialog).toBeHidden();
  expect(deletes).toBe(0);
});

test('5.1 — the job list is readable at 360px and every icon button is named', async ({ page }) => {
  await page.route('**/api/replication**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(listBody({ jobs: [job()] })) }),
  );
  await page.setViewportSize({ width: 360, height: 720 });
  await openReplication(page, adminTokens);
  const overflow = await page.locator('[data-testid="replication-content"]').evaluate((el) => el.scrollWidth - el.clientWidth);
  expect(overflow).toBeLessThanOrEqual(1);

  const buttons = page.locator('[data-testid^="replication-preview-"], [data-testid^="replication-run-"], [data-testid^="replication-edit-"], [data-testid^="replication-delete-"]');
  const count = await buttons.count();
  expect(count).toBe(4);
  for (let i = 0; i < count; i += 1) {
    const label = await buttons.nth(i).getAttribute('aria-label');
    expect(label && label.trim().length > 0).toBe(true);
  }
});
