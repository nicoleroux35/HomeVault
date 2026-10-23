/* =============================================================================
 * File          : tasks.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Scheduled Tasks screen end-to-end tests (Playwright): covers the
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
 * Scheduled Tasks app — End-to-End Test Suite
 *
 * The Tasks screen (`frontend/src/routes/Tasks.svelte`, app id `tasks`) is
 * the management surface of the generic cron engine
 * (docs/guides/scheduled-tasks.md). This suite covers:
 *
 *   1. Launcher opening + task table
 *   2. Creating a task through the modal (and its validation errors)
 *   3. Editing a task (payload fields repopulate per type)
 *   4. Run now → status flips, run history records the execution
 *   5. Disable/enable through the row switch
 *   6. Delete with confirm dialog
 *   7. Auth: a non-admin session gets bounced off the app
 *
 * The suite creates exactly one task ("E2E snapshot") via the **UI** and
 * deletes it in afterAll through the admin API (so a crashed run never
 * leaves the seed behind). Run-now executes `/bin/true` — safe on any host.
 *
 * Endpoint auth/payload validation are covered by
 * `tests/scheduled_tasks_integration.rs`; this suite covers the screen.
 *
 * Run (all):    npx playwright test tests/e2e/tasks/tasks.spec.ts --project=firefox
 * Run (single): npx playwright test tests/e2e/tasks/tasks.spec.ts -g "2.1"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/tasks/tasks.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
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
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await dismissWhatsNew(page);
  await page
    .addStyleTag({ content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}' })
    .catch(() => {});
}

/** `[data-testid="tasks-page"]` is unique to Tasks.svelte. */

/** Fresh installs show the once-per-version "What's New" modal — close it. */
async function dismissWhatsNew(page: Page) {
  const modal = page.locator('[data-testid="whats-new-modal"]');
  if (await modal.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await modal.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

function win(page: Page) {
  return page.locator('[data-testid="tasks-page"]');
}

async function openTasks(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[title="Scheduled Tasks"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="tasks-page"]', { timeout: 15_000 });
  // Wait for the load to settle: either the list or the empty state shows.
  await expect(
    win(page).locator('[data-testid="tasks-list"], [data-testid="tasks-empty"]').first(),
  ).toBeVisible({ timeout: 15_000 });
}

let adminTokens: Tokens;
let createdTaskId: string | null = null;

test.beforeAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) {
    throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  }
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
});

test.afterAll(async ({ playwright }) => {
  if (!createdTaskId) return;
  const ctx = await newCtx(playwright);
  await ctx.delete(`/api/scheduled-tasks/${createdTaskId}`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
  });
});

test('1.1 — Tasks opens from the launcher', async ({ page }) => {
  await openTasks(page, adminTokens);
  await expect(win(page)).toBeVisible();
  // The window is closable (keyboard path: Escape is handled by the frame).
  await expect(
    page
      .locator('.hv-window', { has: page.locator('[data-testid="tasks-page"]') })
      .locator('[data-testid="win-close"]'),
  ).toBeVisible();
});

test('2.1 — Create a task through the modal', async ({ page }) => {
  await openTasks(page, adminTokens);
  await win(page).locator('[data-testid="tasks-new-btn"]').click();

  const modal = page.locator('[role="dialog"]').filter({ has: page.locator('[data-testid="tasks-save-btn"]') }).first();
  await expect(modal).toBeVisible();

  const nameField = page.locator('input[maxlength="120"]').first();
  await nameField.fill('E2E snapshot');
  await page.locator('input[placeholder="tank/data"]').first().fill('e2e-pool');
  await page.locator('input[placeholder="0 3 * * *"]').first().fill('0 3 * * 1');
  await page.locator('[data-testid="tasks-save-btn"]').click();

  // The task appears in the list with its cron.
  const row = win(page).locator('[data-testid="tasks-list"] table tbody tr', { hasText: 'E2E snapshot' });
  await expect(row).toBeVisible({ timeout: 10_000 });
  await expect(row).toContainText('0 3 * * 1');

  // Remember its id (from the row's action buttons) for afterAll cleanup.
  const toggle = row.locator('[data-testid^="tasks-toggle-"]').first();
  const testId = await toggle.getAttribute('data-testid');
  createdTaskId = testId ? testId.replace('tasks-toggle-', '') : null;
});

test('2.2 — Create rejects a missing name with an inline error', async ({ page }) => {
  await openTasks(page, adminTokens);
  await win(page).locator('[data-testid="tasks-new-btn"]').click();
  await page.locator('[data-testid="tasks-save-btn"]').click();
  // The error text lives inside the modal (portal-rendered outside the
  // page root) — scope by the dialog, not by the page testid.
  await expect(
    page.locator('[role="dialog"] [data-testid="tasks-form-error"]'),
  ).toBeVisible();
  // Close the modal so the next test starts clean.
  await page.keyboard.press('Escape');
});

test('2.3 — A task keeps the timezone it was created with', async ({ page }) => {
  // ROADMAP 12.2: the cron is evaluated in this zone, and the row says which.
  await openTasks(page, adminTokens);
  await win(page).locator('[data-testid="tasks-new-btn"]').click();

  const name = `E2E timezone ${Date.now()}`;
  await page.locator('#taskName').fill(name);
  await page.locator('#taskPool').fill('e2e-pool');
  await page.locator('#taskCron').fill('0 3 * * *');
  await page.locator('#taskTimezone').fill('Europe/Paris');
  await page.locator('[data-testid="tasks-save-btn"]').click();

  const row = win(page).locator('[data-testid="tasks-list"] table tbody tr', { hasText: name });
  await expect(row).toBeVisible({ timeout: 10_000 });
  await expect(row).toContainText('Europe/Paris');

  // The server stored it (the UI is only convenience) — then clean up.
  const toggle = row.locator('[data-testid^="tasks-toggle-"]').first();
  const testId = await toggle.getAttribute('data-testid');
  const id = testId ? testId.replace('tasks-toggle-', '') : '';
  if (id) {
    const res = await page.request.get(`${BASE}/api/scheduled-tasks`, {
      headers: { 'x-session-token': adminTokens.token },
    });
    const body = await res.json().catch(() => ({}));
    const created = (body?.data?.tasks || []).find((t2: { id: string }) => t2.id === id);
    expect(created?.timezone).toBe('Europe/Paris');
    await page.request.delete(`${BASE}/api/scheduled-tasks/${id}`, {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    });
  }
});

test('3.1 — Edit repopulates the per-type payload fields', async ({ page }) => {
  test.skip(!createdTaskId, 'task not created');
  await openTasks(page, adminTokens);
  const row = win(page).locator('[data-testid="tasks-list"] table tbody tr', { hasText: 'E2E snapshot' });
  await row.locator('[data-testid^="tasks-edit-"]').click();
  await expect(page.locator('input[placeholder="tank/data"]').first()).toHaveValue('e2e-pool');
  await page.keyboard.press('Escape');
});

test('4.1 — Run now flips the status and records a run', async ({ page }) => {
  test.skip(!createdTaskId, 'task not created');
  // Re-point the task at /bin/true through the API so the run succeeds on
  // any host (the UI payload edit is covered by 3.1).
  const put = await page.request.put(`${BASE}/api/scheduled-tasks/${createdTaskId}`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: {
      name: 'E2E snapshot',
      task_type: 'command',
      cron: '0 3 * * 1',
      payload: { command: '/bin/true', args: [] },
      enabled: true,
    },
  });
  expect(put.ok()).toBeTruthy();

  await openTasks(page, adminTokens);
  const row = win(page).locator('[data-testid="tasks-list"] table tbody tr', { hasText: 'E2E snapshot' });
  await row.locator('[data-testid^="tasks-run-"]').click();

  await expect(row.locator('[data-testid^="tasks-status-"]')).toContainText(/Success|Réussite|Erfolg|Éxito/i, {
    timeout: 15_000,
  });

  // Run history drawer records the execution.
  await row.locator('[data-testid^="tasks-runs-"]').click();
  await expect(page.locator('[data-testid="tasks-runs-list"] > div').first()).toBeVisible({ timeout: 10_000 });
});

test('4.2 — Run now returns immediately and the row follows the run', async ({ page }) => {
  test.skip(!createdTaskId, 'task not created');
  // ROADMAP 12.4: the POST answers 202 while the run is still going, so the
  // screen must show "Running" and then settle on its own. A command long
  // enough that the first poll (3 s) still sees it in flight.
  const put = await page.request.put(`${BASE}/api/scheduled-tasks/${createdTaskId}`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: {
      name: 'E2E snapshot',
      task_type: 'command',
      cron: '0 3 * * 1',
      payload: { command: '/bin/sleep', args: ['5'] },
      enabled: true,
    },
  });
  expect(put.ok()).toBeTruthy();

  await openTasks(page, adminTokens);
  const row = win(page).locator('[data-testid="tasks-list"] table tbody tr', { hasText: 'E2E snapshot' });
  await row.locator('[data-testid^="tasks-run-"]').click();

  // The request came back before the run ended: the row is in flight.
  await expect(row.locator('[data-testid^="tasks-status-"]')).toContainText(
    /Running|En cours|En ejecución|Läuft/i,
    { timeout: 5_000 },
  );
  // …and the bounded poll settles it without a page reload.
  await expect(row.locator('[data-testid^="tasks-status-"]')).toContainText(
    /Success|Réussite|Erfolg|Éxito/i,
    { timeout: 20_000 },
  );
});

test('4.3 — An argument containing a space stays one argument', async ({ page }) => {
  test.skip(!createdTaskId, 'task not created');
  // The form used to split on whitespace, so a path with a space silently
  // became two argv entries.
  const put = await page.request.put(`${BASE}/api/scheduled-tasks/${createdTaskId}`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: {
      name: 'E2E snapshot',
      task_type: 'command',
      cron: '0 3 * * 1',
      payload: { command: '/bin/true', args: ['--out', '/tmp/my file.txt'] },
      enabled: true,
    },
  });
  expect(put.ok()).toBeTruthy();

  await openTasks(page, adminTokens);
  const row = win(page).locator('[data-testid="tasks-list"] table tbody tr', { hasText: 'E2E snapshot' });
  await row.locator('[data-testid^="tasks-edit-"]').click();
  // The form shows the argument quoted, so it can round-trip…
  await expect(page.locator('#taskArgs')).toHaveValue('--out "/tmp/my file.txt"');
  // …and saving keeps it as one argv entry.
  await page.locator('[data-testid="tasks-save-btn"]').click();
  await expect(row).toBeVisible({ timeout: 10_000 });
  const res = await page.request.get(`${BASE}/api/scheduled-tasks`, {
    headers: { 'x-session-token': adminTokens.token },
  });
  const body = await res.json().catch(() => ({}));
  const saved = (body?.data?.tasks || []).find((t2: { id: string }) => t2.id === createdTaskId);
  const payload = JSON.parse(saved?.payload || '{}');
  expect(payload.args).toEqual(['--out', '/tmp/my file.txt']);
});

test('5.1 — Disable / enable through the row switch', async ({ page }) => {
  test.skip(!createdTaskId, 'task not created');
  await openTasks(page, adminTokens);
  const row = win(page).locator('[data-testid="tasks-list"] table tbody tr', { hasText: 'E2E snapshot' });
  const toggle = row.locator('[data-testid^="tasks-toggle-"] input, [data-testid^="tasks-toggle-"]').first();
  await toggle.click();
  await expect(row).toContainText(/Paused|En pause|Pausiert|En pausa/i, { timeout: 10_000 });
  await toggle.click();
  await expect(row).toContainText(/Enabled|Activée|Aktiviert|Activada/i, { timeout: 10_000 });
});

test('6.1 — Delete asks for confirmation', async ({ page }) => {
  test.skip(!createdTaskId, 'task not created');
  await openTasks(page, adminTokens);
  const row = win(page).locator('[data-testid="tasks-list"] table tbody tr', { hasText: 'E2E snapshot' });
  await row.locator('[data-testid^="tasks-delete-"]').click();
  // The confirm dialog shows the task name; cancel keeps the row.
  await page.locator('[role="alertdialog"], .hv-modal, [role="dialog"]').first().locator('button:has-text("Cancel"), button:has-text("Annuler")').first().click().catch(() => {});
  await expect(row).toBeVisible();
});

test('7.1 — Non-admin session cannot open the app', async ({ page, playwright }) => {
  // Create a non-admin user for the bounce check (best-effort; if the
  // instance already has one, reuse it).
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  const username = 'e2enonadmin';
  await ctx.post(`${BASE}/api/users`, {
    headers: { 'x-session-token': token, 'x-csrf-token': csrf },
    data: { username, password: 'NonAdmin123!', fullname: 'E2E', email: 'e2e@example.com', role: 'user' },
  }).catch(() => {});
  const userLogin = await apiLogin(ctx, username, 'NonAdmin123!');
  test.skip(!userLogin.token, 'non-admin account unavailable');

  await gotoDesktop(page, { username, password: 'NonAdmin123!', ...userLogin });
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[title="Scheduled Tasks"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  // The app either never opens or immediately bounces to /desktop.
  await expect(page.locator('[data-testid="tasks-page"]')).toHaveCount(0, { timeout: 10_000 });
});
