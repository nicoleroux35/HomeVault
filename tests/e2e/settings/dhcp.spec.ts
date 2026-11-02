/* =============================================================================
 * File          : dhcp.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   DHCP/TFTP server settings-card end-to-end tests (Playwright): the
 *   capability-absent state, the empty state, the range editor, static
 *   reservations, the confirmation modal before turning the server on, and the
 *   save round-trip.
 *
 *   Every backend call is intercepted with `page.route`, so the suite never
 *   starts `dnsmasq` on the machine that hosts the test run and the assertions
 *   are deterministic. The real endpoint, auth matrix and validation live in
 *   `tests/dhcp_server_integration.rs` and `tests/phase4_auth_integration.rs`.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/dhcp.spec.ts
 * =============================================================================
 */

import { test, expect, type Page, type APIRequestContext, type Route } from '@playwright/test';

const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

interface Tokens {
  token: string;
  csrf: string;
}

async function apiLogin(ctx: APIRequestContext): Promise<Tokens> {
  const res = await ctx.post('/api/auth/login', {
    data: { username: ADMIN_USER, password: ADMIN_PASS },
  });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

async function seedSession(page: Page, tokens: Tokens) {
  await page.addInitScript((t) => {
    try {
      localStorage.setItem('session-token', t.token);
      localStorage.setItem('csrf-token', t.csrf);
    } catch {
      /* storage unavailable — the test will surface it */
    }
  }, tokens);
}

/** The DHCP card is unique on the page by its root test hook. */
function card(page: Page) {
  return page.locator('[data-testid="dhcp-card"]');
}

function config(overrides: Record<string, unknown> = {}) {
  return {
    dhcp_enabled: false,
    tftp_enabled: false,
    interface: 'eth0',
    dhcp_range_start: '192.168.1.100',
    dhcp_range_end: '192.168.1.200',
    netmask: '255.255.255.0',
    gateway: null,
    dns_servers: [],
    lease_time_secs: 86400,
    reservations: {},
    tftp_root: '/srv/tftp',
    pxe_boot_file: null,
    confirmed: false,
    ...overrides,
  };
}

interface Recorder {
  puts: any[];
  starts: number;
  stops: number;
}

/**
 * Intercepts every DHCP call. `available` and `state` are caller-controlled so
 * the capability-absent path can be exercised without uninstalling anything.
 */
async function mockDhcp(
  page: Page,
  opts: {
    available?: boolean;
    running?: boolean;
    cfg?: Record<string, unknown>;
    leases?: any[];
    rejectPut?: string;
  } = {}
): Promise<Recorder> {
  const recorder: Recorder = { puts: [], starts: 0, stops: 0 };
  let running = !!opts.running;

  await page.route('**/api/network/dhcp-server/leases', (route: Route) =>
    route.fulfill({ json: { success: true, data: opts.leases ?? [] } })
  );

  await page.route('**/api/network/dhcp-server/start', (route: Route) => {
    recorder.starts += 1;
    running = true;
    return route.fulfill({ json: { success: true, data: { running: true } } });
  });

  await page.route('**/api/network/dhcp-server/stop', (route: Route) => {
    recorder.stops += 1;
    running = false;
    return route.fulfill({ json: { success: true, data: { running: false } } });
  });

  await page.route('**/api/network/dhcp-server', async (route: Route) => {
    if (route.request().method() === 'PUT') {
      const body = route.request().postDataJSON();
      recorder.puts.push(body);
      if (opts.rejectPut) {
        return route.fulfill({
          status: 400,
          json: { success: false, message: opts.rejectPut },
        });
      }
      return route.fulfill({ json: { success: true, data: { saved: true } } });
    }
    return route.fulfill({
      json: {
        success: true,
        data: {
          config: opts.cfg ?? config(),
          state: { running, leases: opts.leases ?? [], conflict_detected: false, last_error: null },
          available: opts.available ?? true,
        },
      },
    });
  });

  return recorder;
}

async function gotoDesktop(page: Page) {
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  // Monitor widgets overlap the dock on this layout — hide them so clicks land.
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
  // A freshly seeded server opens the "What's New" modal, whose overlay
  // swallows the first dock click.
  const whatsNew = page.locator('[data-testid="whats-new-modal"]');
  if (await whatsNew.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await whatsNew.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

/** Opens an app through the dock launcher. The shell forces `/desktop`, so a
 *  deep link does not open a window. */
async function openApp(page: Page, appId: string) {
  await gotoDesktop(page);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const tile = page.locator(`.lp-card[data-app-id="${appId}"]`);
  await tile.waitFor({ state: 'visible', timeout: 10_000 });
  await tile.click();
}

/**
 * Opens Settings through the global search overlay.
 *
 * Settings has no launcher tile: it is reachable from the top-bar user menu or
 * from the `Ctrl+K` search ("Open Settings"). The search overlay is the stable,
 * keyboard-driven path, so the spec uses it rather than a menu whose markup is
 * a layout concern.
 */
async function openSettings(page: Page, category: RegExp) {
  await gotoDesktop(page);

  // Settings has no launcher tile: it opens from the ⌘K overlay's
  // "Open Settings" action.
  await page.keyboard.press('Control+k');
  const dialog = page.getByRole('dialog').filter({ has: page.locator('input') }).first();
  await dialog.waitFor({ state: 'visible', timeout: 10_000 });
  await dialog.locator('input').first().fill('Open Settings');
  const openEntry = dialog.getByRole('option', { name: /^Open Settings/i }).first();
  await openEntry.waitFor({ state: 'visible', timeout: 10_000 });
  await openEntry.click();

  // Then pick the rail category that owns the card.
  const rail = page.locator('nav[aria-label] [data-testid="settings-nav-item"]');
  await rail.first().waitFor({ state: 'visible', timeout: 20_000 });
  await rail.filter({ hasText: category }).first().click();
}

test.describe('DHCP server card', () => {
  test('reports a missing dnsmasq instead of an empty form', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockDhcp(page, { available: false });
    await openSettings(page, /security/i);
  await expect(page.locator('[data-testid="dhcp-card"]')).toBeVisible({ timeout: 20_000 });

    await expect(page.locator('[data-testid="dhcp-unavailable"]')).toBeVisible();
    // The form is still rendered (so nothing looks broken), but the banner
    // names the package to install.
    await expect(page.locator('[data-testid="dhcp-unavailable"]')).toContainText('dnsmasq');
  });

  test('shows the stopped state and the save action on an empty configuration', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockDhcp(page);
    await openSettings(page, /security/i);
  await expect(page.locator('[data-testid="dhcp-card"]')).toBeVisible({ timeout: 20_000 });

    await expect(page.locator('[data-testid="dhcp-status"]')).toHaveAttribute(
      'data-state',
      'stopped'
    );
    await expect(page.locator('[data-testid="dhcp-save"]')).toBeVisible();
    await expect(page.locator('[data-testid="dhcp-start"]')).toBeVisible();
  });

  test('a static reservation round-trips through the form', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockDhcp(page);
    await openSettings(page, /security/i);
  await expect(page.locator('[data-testid="dhcp-card"]')).toBeVisible({ timeout: 20_000 });

    // Turn DHCP on first, which reveals the range editor and reservations.
    await page.locator('[data-testid="dhcp-enable"]').check();
    await expect(page.locator('[data-testid="dhcp-new-mac"]')).toBeVisible();

    await page.locator('[data-testid="dhcp-new-mac"]').fill('aa:bb:cc:dd:ee:ff');
    await page.locator('[data-testid="dhcp-new-ip"]').fill('192.168.1.50');

    const addButton = card(page).getByRole('button', {
      name: /reservation|fixed address/i,
    });
    await addButton.first().click();

    // The row appears without a save yet.
    await expect(page.locator('[data-testid="dhcp-reservations"]')).toContainText(
      'aa:bb:cc:dd:ee:ff'
    );
    expect(recorder.puts).toHaveLength(0);
  });

  test('turning the server on is confirmed before anything is saved', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockDhcp(page);
    await openSettings(page, /security/i);
  await expect(page.locator('[data-testid="dhcp-card"]')).toBeVisible({ timeout: 20_000 });

    await page.locator('[data-testid="dhcp-enable"]').check();
    await page.locator('[data-testid="dhcp-save"]').click();

    // The confirmation appears and nothing has been PUT yet.
    await expect(page.locator('[data-testid="dhcp-confirm-enable"]')).toBeVisible();
    expect(recorder.puts).toHaveLength(0);

    await page.locator('[data-testid="dhcp-confirm-enable"]').click();
    await expect.poll(() => recorder.puts.length).toBe(1);
    expect(recorder.puts[0].config.confirmed).toBe(true);
    expect(recorder.puts[0].config.dhcp_enabled).toBe(true);
  });

  test('cancelling the confirmation saves nothing', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockDhcp(page);
    await openSettings(page, /security/i);
  await expect(page.locator('[data-testid="dhcp-card"]')).toBeVisible({ timeout: 20_000 });

    await page.locator('[data-testid="dhcp-enable"]').check();
    await page.locator('[data-testid="dhcp-save"]').click();
    await expect(page.locator('[data-testid="dhcp-confirm-enable"]')).toBeVisible();

    await card(page).page().getByRole('button', { name: /^cancel$/i }).click();
    await expect(page.locator('[data-testid="dhcp-confirm-enable"]')).toBeHidden();
    expect(recorder.puts).toHaveLength(0);
  });

  test('a refused save shows the server sentence, not a generic failure', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockDhcp(page, {
      rejectPut: 'DHCP server already active on interface eth0',
    });
    await openSettings(page, /security/i);
  await expect(page.locator('[data-testid="dhcp-card"]')).toBeVisible({ timeout: 20_000 });

    // With neither switch on, no confirmation blocks the save.
    await page.locator('[data-testid="dhcp-save"]').click();
    const notice = page.locator('[data-testid="dhcp-notice"]');
    await expect(notice).toBeVisible();
    await expect(notice).toContainText('already active on interface eth0');
  });

  test('the active lease table is rendered when the server runs', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockDhcp(page, {
      running: true,
      cfg: config({ dhcp_enabled: true }),
      leases: [
        { mac: 'aa:bb:cc:dd:ee:ff', ip: '192.168.1.101', hostname: 'laptop', expires_at: 1900000000 },
      ],
    });
    await openSettings(page, /security/i);
  await expect(page.locator('[data-testid="dhcp-card"]')).toBeVisible({ timeout: 20_000 });

    await expect(page.locator('[data-testid="dhcp-status"]')).toHaveAttribute(
      'data-state',
      'running'
    );
    await expect(page.locator('[data-testid="dhcp-leases"]')).toContainText('192.168.1.101');
    await expect(page.locator('[data-testid="dhcp-leases"]')).toContainText('laptop');
  });

  test('the range editor requires an accessible label on every field', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockDhcp(page, { cfg: config({ dhcp_enabled: true }) });
    await openSettings(page, /security/i);
  await expect(page.locator('[data-testid="dhcp-card"]')).toBeVisible({ timeout: 20_000 });

    // Every input the card renders must be reachable by its label, which is
    // what `getByLabel` exercises.
    for (const label of [
      /first address/i,
      /last address/i,
      /subnet mask/i,
      /gateway/i,
      /lease time/i,
    ]) {
      await expect(card(page).getByLabel(label).first()).toBeVisible();
    }
  });
});
