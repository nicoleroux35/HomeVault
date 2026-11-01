/* =============================================================================
 * File          : config-backup.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings → System → Full configuration backup card, end-to-end
 *   (Playwright): the export form (secrets switch revealing the passphrase),
 *   the archive list, the two-click restore, the refusals, access control and
 *   accessibility.
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/config-backup.spec.ts --project=firefox
 * =============================================================================
 */

/**
 * Full configuration backup (Settings → System) — End-to-End Test Suite
 *
 * **SAFETY: nothing is exported and nothing is restored.** Both endpoints are
 * intercepted with `page.route`, so the assertions are about the screen; the
 * archive format, the encryption and the restore staging are covered by
 * `src/config_backup.rs`'s unit tests and `tests/config_backup_integration.rs`.
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

async function openSystemCategory(page: Page, tokens: Tokens) {
  await seedBrowser(page, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await dismissWhatsNew(page);

  const dockBtn = page.locator('.hv-dock-item:has(.hv-dock-icon-settings)').first();
  await dockBtn.scrollIntoViewIfNeeded().catch(() => {});
  try {
    await dockBtn.click({ timeout: 5_000 });
  } catch {
    await dockBtn.click({ force: true });
  }
  await page.waitForSelector('[data-testid="settings-page"]', { timeout: 15_000 });
  await page
    .locator('[data-testid="settings-nav-item"]')
    .first()
    .waitFor({ state: 'visible', timeout: 15_000 });

  const systemItem = page.locator('[data-testid="settings-nav-item"]', {
    hasText: /System|Système|System|Sistema/i,
  });
  await systemItem.first().click();
  await page.waitForSelector('[data-testid="config-backup-export"]', { timeout: 15_000 });
}

const statePayload = (over: Record<string, unknown> = {}) => ({
  success: true,
  data: {
    app_version: '1.0.0',
    format_version: 1,
    settings_schema: 40,
    max_archive_bytes: 536870912,
    config_files: [
      { archive: 'configs/smb.conf', host: '/etc/samba/smb.conf' },
      { archive: 'configs/exports', host: '/etc/exports' },
    ],
    archives: [],
    pending_restore: null,
    ...over,
  },
});

let adminTokens: Tokens;
let sharedCtx: APIRequestContext;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await playwright.request.newContext({ baseURL: BASE });
  const { token, csrf } = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
});

test.afterAll(async () => {
  await sharedCtx.dispose();
});

// ===========================================================================
// 1. Shape and export
// ===========================================================================

test('1.1 — the card renders the export form, the contents and the restore input', async ({
  page,
}) => {
  await openSystemCategory(page, adminTokens);
  await expect(page.locator('[data-testid="config-backup-export"]')).toBeVisible();
  await expect(page.locator('[data-testid="config-backup-refresh"]')).toBeVisible();
  await expect(page.locator('[data-testid="config-backup-file"]')).toBeVisible();
  // The version the archive will carry is shown, not implied.
  await expect(page.locator('[data-testid="config-backup-version"]')).not.toBeEmpty();
});

test('1.2 — "include credentials" reveals the passphrase field', async ({ page }) => {
  await openSystemCategory(page, adminTokens);
  const passphrase = page.locator('[data-testid="config-backup-passphrase"]');
  const visibleBefore = await passphrase.count();

  await page.locator('.config-backup-card .settings-toggle-label').first().click();
  await expect(passphrase).toHaveCount(visibleBefore === 1 ? 0 : 1, { timeout: 5_000 });

  // Back to the served state for the next test.
  await page.locator('.config-backup-card .settings-toggle-label').first().click();
});

test('1.3 — a pending restore is announced, because it only applies on restart', async ({
  page,
}) => {
  await page.route('**/api/settings/config-backup', (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(
        statePayload({
          pending_restore: {
            created_at: '2026-10-31T12:00:00+00:00',
            app_version: '1.0.0',
            includes_secrets: false,
            entries: 5,
          },
        }),
      ),
    });
  });
  await openSystemCategory(page, adminTokens);

  const pending = page.locator('[data-testid="config-backup-pending"]');
  await expect(pending).toBeVisible({ timeout: 10_000 });
  await expect(pending).toHaveAttribute('role', 'alert');
  await expect(pending).toContainText(/next start|prochain démarrage|nächsten Start|próximo arranque/i);

  await page.unroute('**/api/settings/config-backup');
});

test('1.4 — an export names the archive, lists it, and can be downloaded', async ({ page }) => {
  await page.route('**/api/settings/config-backup/export', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: { id: '2026-10-31T12-00-00Z', bytes: 4_500_000, includes_secrets: false, encrypted: false },
      }),
    }),
  );
  // The list refresh after the export returns the new archive.
  await page.route('**/api/settings/config-backup', (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(
        statePayload({
          archives: [{ id: '2026-10-31T12-00-00Z', bytes: 4_500_000, created_at: '2026-10-31T12:00:00+00:00' }],
        }),
      ),
    });
  });

  await openSystemCategory(page, adminTokens);
  await page.locator('[data-testid="config-backup-export"]').click();

  const notice = page.locator('[data-testid="config-backup-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'status');
  await expect(notice).toContainText(/4\.5 MB|4,5 MB/);

  const row = page.locator('[data-testid="config-backup-archive"]').first();
  await expect(row).toBeVisible();
  await expect(row.locator('[data-testid="config-backup-download"]')).toBeVisible();

  await page.unroute('**/api/settings/config-backup');
  await page.unroute('**/api/settings/config-backup/export');
});

test('1.5 — an impossible export shows the server reason as an alert', async ({ page }) => {
  await page.route('**/api/settings/config-backup/export', (route) =>
    route.fulfill({
      status: 400,
      contentType: 'application/json',
      body: JSON.stringify({
        success: false,
        message: 'An archive that includes secrets must be encrypted: enter a passphrase',
      }),
    }),
  );
  await openSystemCategory(page, adminTokens);
  await page.locator('.config-backup-card .settings-toggle-label').first().click();
  await page.locator('[data-testid="config-backup-export"]').click();

  const notice = page.locator('[data-testid="config-backup-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'alert');
  await expect(notice).toContainText(/passphrase|phrase secrète|Passphrase|frase de paso/i);

  await page.unroute('**/api/settings/config-backup/export');
});

// ===========================================================================
// 2. Restore (mocked: nothing is staged)
// ===========================================================================

test('2.1 — restoring takes two clicks and reports that a restart is needed', async ({ page }) => {
  let restores = 0;
  await page.route('**/api/settings/config-backup/restore', (route) => {
    restores += 1;
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: {
          staged: true,
          requires_restart: true,
          staged_entries: ['database.db', 'settings.json'],
          message:
            'The archive was validated and staged (2 files). Restart HomeVault to apply it; the current database is kept as homevault.db.pre-restore.',
        },
      }),
    });
  });
  await openSystemCategory(page, adminTokens);

  // A tiny file is enough: the server is mocked, and the UI only needs a File.
  await page.locator('[data-testid="config-backup-file"]').setInputFiles({
    name: 'homevault-config.hvbak',
    mimeType: 'application/octet-stream',
    buffer: Buffer.from('HVCONFIG1 fake archive'),
  });

  const button = page.locator('[data-testid="config-backup-restore-btn"]');
  await button.click();
  // The first click only warns; nothing is sent.
  await expect(button).toContainText(/now\?|maintenant|jetzt|ahora/i);
  expect(restores).toBe(0);

  await button.click();
  await expect.poll(() => restores, { timeout: 10_000 }).toBe(1);
  const notice = page.locator('[data-testid="config-backup-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toContainText(/Restart HomeVault|Redémarrez|Starten Sie|Reinicie/i);

  await page.unroute('**/api/settings/config-backup/restore');
});

test('2.2 — a refused archive shows the server reason as an alert', async ({ page }) => {
  await page.route('**/api/settings/config-backup/restore', (route) =>
    route.fulfill({
      status: 400,
      contentType: 'application/json',
      body: JSON.stringify({
        success: false,
        message:
          'This archive was written by a newer HomeVault (format 2, this build reads 1). Update HomeVault before restoring it.',
      }),
    }),
  );
  await openSystemCategory(page, adminTokens);
  await page.locator('[data-testid="config-backup-file"]').setInputFiles({
    name: 'newer.hvbak',
    mimeType: 'application/octet-stream',
    buffer: Buffer.from('HVCONFIG1 newer'),
  });
  const button = page.locator('[data-testid="config-backup-restore-btn"]');
  await button.click();
  await button.click();

  const notice = page.locator('[data-testid="config-backup-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'alert');
  await expect(notice).toContainText(/newer HomeVault|Update HomeVault/i);

  await page.unroute('**/api/settings/config-backup/restore');
});

// ===========================================================================
// 3. Access control and accessibility
// ===========================================================================

test('3.1 — a non-admin session cannot read the state or export', async ({ page }) => {
  const name = `bakmember${Date.now().toString(36).slice(-4)}`;
  const created = await page.request.post(`${BASE}/api/users`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: { username: name, password: 'MemberSecret123', fullname: 'Backup Member', email: '', role_id: 1 },
  });
  expect(created.ok()).toBeTruthy();
  const member = await apiLogin(sharedCtx, name, 'MemberSecret123');
  expect(member.token).toBeTruthy();

  const state = await page.request.get(`${BASE}/api/settings/config-backup`, {
    headers: { 'x-session-token': member.token },
  });
  expect(state.status()).toBe(403);
  const exportRes = await page.request.post(`${BASE}/api/settings/config-backup/export`, {
    headers: { 'x-session-token': member.token, 'x-csrf-token': member.csrf },
    data: { include_secrets: false },
  });
  expect(exportRes.status()).toBe(403);

  await page.request
    .delete(`${BASE}/api/users/${encodeURIComponent(name)}`, {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    })
    .catch(() => {});
});

test('3.2 — every field has a label and the switches an accessible name', async ({ page }) => {
  await openSystemCategory(page, adminTokens);
  await page.locator('.config-backup-card .settings-toggle-label').first().click();

  for (const id of ['cb-passphrase', 'cb-restore-file']) {
    const labelled = await page.locator(`label[for="${id}"]`).count();
    expect(labelled, `${id} must have exactly one <label for>`).toBe(1);
  }

  const switches = page.locator('.config-backup-card input[type="checkbox"]');
  const count = await switches.count();
  for (let i = 0; i < count; i++) {
    const labelledBy = await switches.nth(i).getAttribute('aria-labelledby');
    if (labelledBy) {
      expect(await page.locator(`#${labelledBy}`).count()).toBe(1);
    } else {
      expect((await switches.nth(i).getAttribute('aria-label')) || '').not.toBe('');
    }
  }
});

test('3.3 — no duplicate element ids on the screen', async ({ page }) => {
  await openSystemCategory(page, adminTokens);
  const duplicates = await page.evaluate(() => {
    const seen = new Map<string, number>();
    document.querySelectorAll('[id]').forEach((el) => {
      seen.set(el.id, (seen.get(el.id) ?? 0) + 1);
    });
    return [...seen.entries()].filter(([, n]) => n > 1).map(([id]) => id);
  });
  expect(duplicates).toEqual([]);
});

test('3.4 — the card does not overflow its panel horizontally', async ({ page }) => {
  await page.route('**/api/settings/config-backup', (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(
        statePayload({
          archives: [
            { id: '2026-10-31T12-00-00Z', bytes: 4_500_000, created_at: '2026-10-31T12:00:00+00:00' },
            { id: '2026-10-30T03-00-00Z', bytes: 4_400_000, created_at: '2026-10-30T03:00:00+00:00' },
          ],
        }),
      ),
    });
  });
  await openSystemCategory(page, adminTokens);
  const overflow = await page.evaluate(() => {
    const panel = document.querySelector('[data-testid="settings-page"]');
    if (!panel) return -1;
    const box = panel.getBoundingClientRect();
    let worst = 0;
    panel.querySelectorAll('[data-testid^="config-backup-"]').forEach((el) => {
      const r = el.getBoundingClientRect();
      worst = Math.max(worst, r.right - box.right);
    });
    return Math.round(worst);
  });
  expect(overflow).toBeLessThanOrEqual(0);
  await page.unroute('**/api/settings/config-backup');
});
