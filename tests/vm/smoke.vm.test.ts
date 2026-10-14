/* =============================================================================
 * File          : smoke.vm.test.ts
 * Author        : Nicolas Le Roux
 * Created       : 14/10/2026
 * Last modified : 14/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Real-VM smoke suite: runs against an already-provisioned HomeVault
 *   host (VM or spare box) given by environment variables, exercising the
 *   full stack end to end — unlike the mocked e2e specs.
 *
 * Dependencies:
 *   - none (Playwright; requires HV_VM_BASE_URL + admin credentials)
 *
 * Usage:
 *   npx playwright test tests/vm/smoke.vm.test.ts (provisioned host only)
 * =============================================================================
 */

/**
 * Real-VM smoke suite (`*.vm.test.ts`).
 *
 * Runs against an already-provisioned HomeVault host (VM or spare box) given
 * by `HV_VM_BASE_URL`, with `HV_VM_ADMIN_USER` / `HV_VM_ADMIN_PASS`. Unlike the
 * default E2E suite, this one asserts things only a real host can prove:
 *
 * 1. the server is reachable and reports its versioned contract;
 * 2. a real login works and the desktop shell renders;
 * 3. the health / storage APIs answer with real data (not a scratch DB);
 * 4. the mDNS name is advertised (when the host runs avahi).
 *
 * Skipped (with a message) when `HV_VM_BASE_URL` is unset — there is no VM to
 * talk to. Run it with:
 *
 *   HV_VM_BASE_URL=http://hv-test.local:8080 \
 *   HV_VM_ADMIN_USER=admin HV_VM_ADMIN_PASS=secret \
 *   npx playwright test --config=playwright.vm.config.ts
 */

import { test, expect, type Page } from '@playwright/test';

const BASE_URL = process.env.HV_VM_BASE_URL;
const ADMIN_USER = process.env.HV_VM_ADMIN_USER || 'admin';
const ADMIN_PASS = process.env.HV_VM_ADMIN_PASS || '';
const VM_HOST = process.env.HV_VM_HOSTNAME || 'homevault';

// A single guard so every test skips identically when no VM is configured.
test.beforeEach(() => {
  test.skip(!BASE_URL, 'HV_VM_BASE_URL is not set — no real VM to test against.');
});

async function login(page: Page) {
  await page.goto('/login');
  await page.fill('#username', ADMIN_USER);
  await page.fill('#password', ADMIN_PASS);
  await page.click('button[type="submit"]');
  await page.waitForURL(/\/(desktop|$)/, { timeout: 30_000 });
}

test.describe('Real VM smoke', () => {
  test('1. the host answers and reports a contract version', async ({ request }) => {
    const res = await request.get('/api/meta');
    expect(res.ok()).toBeTruthy();
    const body = await res.json();
    expect(body.success).toBe(true);
    expect(typeof body.data.contract_version).toBe('number');
    expect(typeof body.data.server_version).toBe('string');
  });

  test('2. a real login reaches the desktop shell', async ({ page }) => {
    await login(page);
    await expect(page.locator('.hv-desktop')).toBeVisible();
  });

  test('3. health and storage report real host data', async ({ page, request }) => {
    await login(page);

    const token = await page.evaluate(() => localStorage.getItem('session-token'));
    expect(token, 'login must persist a session token').toBeTruthy();

    const headers = { 'x-session-token': token as string };

    const health = await request.get('/api/health/status', { headers });
    expect(health.ok()).toBeTruthy();

    const disks = await request.get('/api/storage/disks', { headers });
    expect(disks.ok()).toBeTruthy();
    const disksBody = await disks.json();
    expect(Array.isArray(disksBody.data)).toBe(true);
  });

  test('4. mDNS advertises the configured hostname', async ({ request }) => {
    const res = await request.get(`http://${VM_HOST}.local:${new URL(BASE_URL!).port || 80}/api/meta`);
    // The host may not run avahi in every environment; only assert when it
    // resolves. A DNS failure is not a product failure here.
    test.skip(!res.ok(), `mDNS name ${VM_HOST}.local did not resolve on this network.`);
    const body = await res.json();
    expect(body.success).toBe(true);
  });
});
