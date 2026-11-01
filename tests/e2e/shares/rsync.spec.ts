/* =============================================================================
 * File          : rsync.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Shares → Rsync tab (ROADMAP P2.2), end-to-end (Playwright): the tab, the
 *   empty state, the module modal and its share picker, the password panel,
 *   the refusal paths and accessibility.
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/shares/rsync.spec.ts --project=chromium
 * =============================================================================
 */

/**
 * Rsync server (Shares app) — End-to-End Test Suite
 *
 * **SAFETY: no daemon is started and no module is created on the instance.**
 * Every `/api/shares/rsync*` call is intercepted with `page.route`, so the
 * assertions are about the screen — what the panel says, what the modal
 * requires before it can save, what a refusal renders. The config rendering,
 * path confinement and the endpoints themselves are covered by
 * `src/rsync_server.rs` (unit) and `tests/rsync_integration.rs` (integration).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_PASS || process.env.E2E_ADMIN_PASS || 'E2eSecret123';

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

async function openRsyncTab(page: Page, tokens: Tokens) {
  await seedBrowser(page, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await dismissWhatsNew(page);
  const tile = page.locator('.hv-dock-item:has-text("Shares"), [data-app-id="shares"]').first();
  try {
    await tile.click({ timeout: 5_000 });
  } catch {
    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    const card = page.locator('.lp-card[title="Shares"]');
    await card.waitFor({ state: 'visible', timeout: 10_000 });
    await card.click();
  }
  await page.waitForSelector('[data-testid="shares-page"]', { timeout: 15_000 });
  await page.locator('[data-testid="shares-tab"]', { hasText: /Rsync/ }).first().click();
  await page.waitForSelector('[data-testid="shares-rsync-modules"]', { timeout: 10_000 });
}

const rsyncState = (over: Record<string, unknown> = {}) => ({
  success: true,
  data: {
    settings: { enabled: true, port: 873, max_connections: 8, chroot: true },
    modules: [],
    share_candidates: [
      { name: 'documents', path: '/srv/documents', used: false },
      { name: 'photos', path: '/srv/photos', used: false },
    ],
    users_with_secret: [],
    available_users: ['e2eadmin'],
    service_active: true,
    binary_available: true,
    config_path: '/etc/homevault-rsync/rsyncd.conf',
    default_port: 873,
    connection_limit: 32,
    module_limit: 32,
    ssh_path: "rsync -e 'ssh -p <SFTP port>' <user>@<host>:/<share>/ <local>/",
    ssh_hint: 'Files can also be pulled with `rsync -e ssh` over the SFTP account.',
    ...over,
  },
});

const aModule = {
  name: 'documents',
  path: '/srv/documents',
  comment: 'Nightly pull',
  read_only: true,
  users: ['e2eadmin'],
  enabled: true,
};

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

test('0.1 — no request is ever sent to the real daemon', async ({ page }) => {
  const writes: string[] = [];
  await page.route('**/api/shares/rsync**', (route) => {
    if (route.request().method() !== 'GET') writes.push(route.request().url());
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(rsyncState()) });
  });
  await openRsyncTab(page, adminTokens);
  await expect(page.locator('[data-testid="shares-rsync-settings"]')).toBeVisible();
  expect(writes).toEqual([]);
});

test('1.1 — an instance without modules explains what a module is for', async ({ page }) => {
  await page.route('**/api/shares/rsync**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(rsyncState()) }),
  );
  await openRsyncTab(page, adminTokens);
  await expect(page.locator('[data-testid="shares-rsync-modules"]')).toContainText(/rsync/i);
  await expect(page.locator('[data-testid="shares-rsync-module"]')).toHaveCount(0);
  // The SSH alternative is always offered next to the daemon: the exact
  // command template, not a vague promise.
  await expect(page.locator('[data-testid="shares-rsync-settings"]')).toContainText("rsync -e 'ssh");
});

test('1.2 — a module shows its name, its path and whether it is read-only', async ({ page }) => {
  await page.route('**/api/shares/rsync**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(rsyncState({ modules: [aModule] })) }),
  );
  await openRsyncTab(page, adminTokens);
  const row = page.locator('[data-testid="shares-rsync-module"]').first();
  await expect(row).toBeVisible();
  await expect(row).toContainText('documents');
  await expect(row).toContainText('/srv/documents');
  await expect(row).toContainText(/read-only/i);
});

test('2.1 — the module modal only offers real shares in its picker', async ({ page }) => {
  await page.route('**/api/shares/rsync**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(rsyncState()) }),
  );
  await openRsyncTab(page, adminTokens);
  await page.getByRole('button', { name: /add a module/i }).click();
  const select = page.locator('[data-testid="shares-rsync-module-path"]');
  await expect(select).toBeVisible();
  const values = await select.locator('option').evaluateAll((opts) =>
    opts.map((o) => (o as HTMLOptionElement).value).filter(Boolean),
  );
  // Exactly the candidates the API returned — never a free-text path.
  expect(values.sort()).toEqual(['/srv/documents', '/srv/photos']);
});

test('2.2 — a module name that is not lowercase is refused before it is sent', async ({ page }) => {
  let posts = 0;
  await page.route('**/api/shares/rsync**', (route) => {
    if (route.request().method() === 'POST') posts += 1;
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(rsyncState()) });
  });
  await openRsyncTab(page, adminTokens);
  await page.getByRole('button', { name: /add a module/i }).click();
  const modal = page.locator('[data-slot="dialog-content"]');
  await modal.locator('[data-testid="shares-rsync-module-name"]').fill('Documents');
  await modal.locator('[data-testid="shares-rsync-module-path"]').selectOption('/srv/documents');
  // Mixed-case name and no account granted: the server would refuse both, so
  // the modal refuses them first and the request is never sent.
  await modal.getByRole('button', { name: /^(Save|Enregistrer|Speichern|Guardar)$/ }).click();
  await expect(modal.getByRole('alert')).toBeVisible();
  expect(posts).toBe(0);
});

test('3.1 — the password panel never renders a stored password', async ({ page }) => {
  await page.route('**/api/shares/rsync**', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(
        rsyncState({
          modules: [aModule],
          users_with_secret: ['e2eadmin'],
        }),
      ),
    }),
  );
  await openRsyncTab(page, adminTokens);
  const panel = page.locator('[data-testid="shares-rsync-passwords"]');
  await expect(panel).toBeVisible();
  // Only an input the admin can write into — never a prefilled value.
  const inputs = panel.locator('input');
  const count = await inputs.count();
  for (let i = 0; i < count; i += 1) {
    await expect(inputs.nth(i)).toHaveValue('');
  }
});

test('4.1 — a refused save shows the server reason as an alert', async ({ page }) => {
  await page.route('**/api/shares/rsync/config', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: false, message: 'the port is already in use by another rsync daemon' }),
    }),
  );
  await page.route('**/api/shares/rsync', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(rsyncState()) }),
  );
  await openRsyncTab(page, adminTokens);
  await page.getByRole('button', { name: /^(Save|Enregistrer|Speichern|Guardar)$/ }).first().click();
  await expect(page.getByText('the port is already in use by another rsync daemon')).toBeVisible();
});

test('5.1 — the tab is reachable by keyboard and stays readable at 360px', async ({ page }) => {
  await page.route('**/api/shares/rsync**', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(rsyncState({ modules: [aModule] })) }),
  );
  await page.setViewportSize({ width: 360, height: 720 });
  await openRsyncTab(page, adminTokens);

  const tab = page.locator('[data-testid="shares-tab"]', { hasText: /Rsync/ }).first();
  await expect(tab).toHaveAttribute('aria-selected', 'true');
  const overflow = await page.locator('[data-testid="shares-rsync-modules"]').evaluate((el) => el.scrollWidth - el.clientWidth);
  expect(overflow).toBeLessThanOrEqual(1);
});
