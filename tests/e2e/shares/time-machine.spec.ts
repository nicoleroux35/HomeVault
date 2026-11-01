/* =============================================================================
 * File          : time-machine.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Shares → SMB → Time Machine (ROADMAP P2.1), end-to-end (Playwright): the
 *   card, the per-share toggle, the quota field, the rollback on refusal and
 *   the honest copy about AFP/`_adisk._tcp`.
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/shares/time-machine.spec.ts --project=chromium
 * =============================================================================
 */

/**
 * Time Machine over SMB (Shares app) — End-to-End Test Suite
 *
 * **SAFETY: `smb.conf` is never rewritten.** The SMB share list and the
 * Time Machine endpoints are intercepted with `page.route`, so every
 * assertion is about the screen. The generated Samba block, the quota ceiling
 * and the `_adisk._tcp` advertisement are covered by `src/shares.rs` (unit),
 * `src/mdns.rs` (unit) and `tests/time_machine_integration.rs`.
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

const SHARES = [
  { name: 'macbackup', share_type: 'SMB', path: '/srv/macbackup', enabled: true, comment: 'Mac backups' },
  { name: 'documents', share_type: 'SMB', path: '/srv/documents', enabled: true, comment: 'Docs' },
  { name: 'nfsdata', share_type: 'NFS', path: '/srv/nfsdata', enabled: true, comment: 'Not SMB' },
];

/**
 * The real `GET /api/shares/smb/config` payload (see docs/API.md, "SMB
 * config"). Keep it in sync with the endpoint: an invented shape here once
 * made the whole SMB tab fail to render for reasons the test could not see.
 */
const SMB_CONFIG = {
  workgroup: 'WORKGROUP',
  security_mode: 'user',
  protocol_version: 'SMB3',
  allow_guest: false,
  create_mask: '0755',
  directory_mask: '0755',
  additional_params: [],
};

/** Opens Shares → SMB, where the Time Machine card lives. */
async function openSmbTab(page: Page, tokens: Tokens) {
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
  await page.locator('[data-testid="shares-tab"]', { hasText: /SMB/ }).first().click();
  await page.waitForSelector('[data-testid="shares-time-machine"]', { timeout: 10_000 });
}

/** The share list, the SMB config and the Time Machine map, all mocked. */
async function mockShares(page: Page, tm: Record<string, { enabled: boolean; quota_gb: number }>, onPut?: (body: any) => void) {
  await page.route('**/api/shares/smb/time-machine', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: tm }) }),
  );
  await page.route('**/api/shares/*/smb/time-machine', (route) => {
    if (route.request().method() !== 'PUT') return route.fallback();
    const body = JSON.parse(route.request().postData() || '{}');
    onPut?.(body);
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: {} }) });
  });
  await page.route('**/api/shares/smb/config', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: true, data: SMB_CONFIG }),
    }),
  );
  await page.route('**/api/shares', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: SHARES }) }),
  );
}

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

test('1.1 — only SMB shares get a Time Machine row', async ({ page }) => {
  await mockShares(page, { macbackup: { enabled: false, quota_gb: 0 }, documents: { enabled: false, quota_gb: 0 } });
  await openSmbTab(page, adminTokens);
  const rows = page.locator('[data-testid="shares-time-machine-row"]');
  await expect(rows).toHaveCount(2);
  await expect(page.locator('[data-testid="shares-time-machine"]')).toContainText('macbackup');
  await expect(page.locator('[data-testid="shares-time-machine"]')).toContainText('documents');
  // The NFS share is never offered as a Time Machine destination.
  await expect(page.locator('[data-testid="shares-time-machine"]')).not.toContainText('nfsdata');
});

test('1.2 — the card says AFP is not used, so nobody waits for an AFP mount', async ({ page }) => {
  await mockShares(page, { macbackup: { enabled: false, quota_gb: 0 } });
  await openSmbTab(page, adminTokens);
  const card = page.locator('[data-testid="shares-time-machine"]');
  await expect(card).toContainText(/AFP/i);
  await expect(card).toContainText(/SMB/i);
});

test('2.1 — enabling Time Machine sends the toggle and reveals the size field', async ({ page }) => {
  const sent: any[] = [];
  await mockShares(page, { macbackup: { enabled: false, quota_gb: 0 } }, (body) => sent.push(body));
  await openSmbTab(page, adminTokens);

  const row = page.locator('[data-testid="shares-time-machine-row"]').first();
  await expect(row.locator('[data-testid="shares-tm-quota"]')).toHaveCount(0);
  await row.getByRole('switch').click();

  await expect.poll(() => sent.length).toBe(1);
  expect(sent[0].enabled).toBe(true);
  // 0 = unlimited, which is the honest default rather than an invented size.
  expect(sent[0].quota_gb).toBe(0);
  await expect(row.locator('[data-testid="shares-tm-quota"]')).toBeVisible();
});

test('2.2 — a refused toggle rolls the switch back and says why', async ({ page }) => {
  await page.route('**/api/shares/smb/time-machine', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: { macbackup: { enabled: false, quota_gb: 0 } } }) }),
  );
  await page.route('**/api/shares/*/smb/time-machine', (route) => {
    if (route.request().method() !== 'PUT') return route.fallback();
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: false, message: 'the share path does not exist on disk' }),
    });
  });
  await page.route('**/api/shares/smb/config', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: SMB_CONFIG }) }),
  );
  await page.route('**/api/shares', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ success: true, data: SHARES }) }),
  );

  await openSmbTab(page, adminTokens);
  const row = page.locator('[data-testid="shares-time-machine-row"]').first();
  const toggle = row.getByRole('switch');
  await expect(toggle).toHaveAttribute('aria-checked', 'false');
  await toggle.click();
  // The optimistic flip is undone, so the switch never lies about the state.
  await expect(toggle).toHaveAttribute('aria-checked', 'false');
  await expect(page.getByText('the share path does not exist on disk')).toBeVisible();
});

test('3.1 — a maximum size is sent in gigabytes and 0 stays unlimited', async ({ page }) => {
  const sent: any[] = [];
  await mockShares(page, { macbackup: { enabled: true, quota_gb: 0 } }, (body) => sent.push(body));
  await openSmbTab(page, adminTokens);

  const row = page.locator('[data-testid="shares-time-machine-row"]').first();
  const field = row.locator('[data-testid="shares-tm-quota"]');
  await expect(field).toBeVisible();
  await expect(field).toHaveAttribute('placeholder', /Unlimited|Illimité|Unbegrenzt|Sin límite/i);

  await field.fill('500');
  await row.getByRole('button', { name: /^(Save|Enregistrer|Speichern|Guardar)$/ }).click();
  await expect.poll(() => sent.length).toBe(1);
  expect(sent[0]).toEqual({ enabled: true, quota_gb: 500 });

  await field.fill('');
  await row.getByRole('button', { name: /^(Save|Enregistrer|Speichern|Guardar)$/ }).click();
  await expect.poll(() => sent.length).toBe(2);
  expect(sent[1]).toEqual({ enabled: true, quota_gb: 0 });
});

test('4.1 — the Time Machine card is readable at 360px', async ({ page }) => {
  await mockShares(page, { macbackup: { enabled: true, quota_gb: 500 }, documents: { enabled: false, quota_gb: 0 } });
  await page.setViewportSize({ width: 360, height: 720 });
  await openSmbTab(page, adminTokens);
  const overflow = await page.locator('[data-testid="shares-time-machine"]').evaluate((el) => el.scrollWidth - el.clientWidth);
  expect(overflow).toBeLessThanOrEqual(1);
});
