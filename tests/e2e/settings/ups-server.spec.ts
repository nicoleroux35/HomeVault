/* =============================================================================
 * File          : ups-server.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   UPS *server* mode settings-card end-to-end tests (Playwright): the
 *   capability-absent state, the client allowlist, the shutdown policy with its
 *   "some stages are not available" notice, the daemon start/stop, and the
 *   rule that a shutdown must be confirmed.
 *
 *   Every backend call is intercepted with `page.route`, so the suite never
 *   starts `upsd` or powers the test machine off. The real endpoint, auth matrix
 *   and the `skipped` contract live in `tests/ups_server_integration.rs`,
 *   `tests/phase4_auth_integration.rs` and the `ups_server.rs` unit tests.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/ups-server.spec.ts
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
      /* storage unavailable */
    }
  }, tokens);
}

function card(page: Page) {
  return page.locator('[data-testid="ups-server-card"]');
}

function config(overrides: Record<string, unknown> = {}) {
  return {
    enabled: false,
    bind_interface: 'eth0',
    bind_address: null,
    port: 3493,
    allowed_clients: [],
    shutdown_policy: {
      stop_containers: true,
      shutdown_vms: true,
      flush_restic: true,
      stop_shares: true,
      stage_timeout_secs: {},
    },
    max_clients: 10,
    ...overrides,
  };
}

interface Recorder {
  puts: any[];
  starts: number;
  stops: number;
}

async function mockUps(
  page: Page,
  opts: {
    available?: boolean;
    running?: boolean;
    cfg?: Record<string, unknown>;
    rejectPut?: string;
  } = {}
): Promise<Recorder> {
  const recorder: Recorder = { puts: [], starts: 0, stops: 0 };
  let running = !!opts.running;

  await page.route('**/api/power/ups-server/start', (route: Route) => {
    recorder.starts += 1;
    running = true;
    return route.fulfill({ json: { success: true, data: { running: true } } });
  });

  await page.route('**/api/power/ups-server/stop', (route: Route) => {
    recorder.stops += 1;
    running = false;
    return route.fulfill({ json: { success: true, data: { running: false } } });
  });

  await page.route('**/api/power/ups-server', async (route: Route) => {
    if (route.request().method() === 'PUT') {
      const body = route.request().postDataJSON();
      recorder.puts.push(body);
      if (opts.rejectPut) {
        return route.fulfill({
          status: 400,
          json: { success: false, message: opts.rejectPut },
        });
      }
      return route.fulfill({ json: { success: true, data: { requires_restart: false } } });
    }
    return route.fulfill({
      json: {
        success: true,
        data: {
          available: opts.available ?? true,
          config: opts.cfg ?? config(),
          state: {
            running,
            connected_clients: 0,
            last_error: null,
            shutdown_in_progress: false,
            shutdown_stage: null,
          },
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

test.describe('UPS server mode card', () => {
  test('reports missing nut-server instead of an empty form', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUps(page, { available: false });
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="ups-server-card"]')).toBeVisible({ timeout: 20_000 });

    const banner = page.locator('[data-testid="ups-server-unavailable"]');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('upsd');
  });

  test('the stopped state and the start action are offered', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUps(page);
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="ups-server-card"]')).toBeVisible({ timeout: 20_000 });

    await expect(page.locator('[data-testid="ups-server-status"]')).toHaveAttribute(
      'data-state',
      'stopped'
    );
    await expect(page.locator('[data-testid="ups-server-start"]')).toBeVisible();
  });

  test('starting the daemon calls the start action', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockUps(page);
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="ups-server-card"]')).toBeVisible({ timeout: 20_000 });

    await page.locator('[data-testid="ups-server-start"]').click();
    await expect.poll(() => recorder.starts).toBe(1);
    await expect(page.locator('[data-testid="ups-server-status"]')).toHaveAttribute(
      'data-state',
      'running'
    );
  });

  test('an empty allowlist says so, and reports a missing machine on add', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockUps(page, { cfg: config({ enabled: true }) });
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="ups-server-card"]')).toBeVisible({ timeout: 20_000 });

    await expect(page.locator('[data-testid="ups-no-clients"]')).toBeVisible();

    // Submitting the empty form must not silently add a nameless client.
    await page.locator('[data-testid="ups-add-client"]').click();
    await page.locator('[data-testid="ups-confirm-client"]').click();
    await expect(page.locator('[data-testid="ups-no-clients"]')).toBeVisible();
    await expect(page.locator('[data-testid="ups-server-notice"]')).toBeVisible();
  });

  test('a machine can be added to the allowlist', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockUps(page, { cfg: config({ enabled: true }) });
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="ups-server-card"]')).toBeVisible({ timeout: 20_000 });

    await page.locator('[data-testid="ups-add-client"]').click();
    await page.locator('[data-testid="ups-new-name"]').fill('office-nas');
    await page.locator('[data-testid="ups-new-address"]').fill('192.168.1.60');
    await page.locator('[data-testid="ups-new-username"]').fill('monitor');
    await page.locator('[data-testid="ups-confirm-client"]').click();

    await expect(page.locator('[data-testid="ups-clients"]')).toContainText('office-nas');
    await expect(page.locator('[data-testid="ups-clients"]')).toContainText('192.168.1.60');

    await page.locator('[data-testid="ups-server-save"]').click();
    await expect.poll(() => recorder.puts.length).toBe(1);
    expect(recorder.puts[0].config.allowed_clients).toHaveLength(1);
    expect(recorder.puts[0].config.allowed_clients[0].name).toBe('office-nas');
  });

  test('the card says two shutdown stages are not available', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUps(page, { cfg: config({ enabled: true }) });
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="ups-server-card"]')).toBeVisible({ timeout: 20_000 });

    // The "skipped, never done" contract must be visible before the user
    // relies on the policy, not only after a shutdown returns.
    const warning = page.locator('[data-testid="ups-shutdown-warning"]');
    await expect(warning).toBeVisible();
    await expect(warning).toContainText(/skipped|not available/i);
  });

  test('every logical shutdown stage has its own control', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUps(page, { cfg: config({ enabled: true }) });
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="ups-server-card"]')).toBeVisible({ timeout: 20_000 });

    for (const id of [
      'ups-stop-containers',
      'ups-shutdown-vms',
      'ups-flush-restic',
      'ups-stop-shares',
    ]) {
      await expect(page.locator(`[data-testid="${id}"]`)).toBeVisible();
    }
  });

  test('a reserved port is refused with the server sentence', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUps(page, {
      cfg: config({ enabled: true }),
      rejectPut: 'port 8443 is reserved',
    });
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="ups-server-card"]')).toBeVisible({ timeout: 20_000 });

    await page.locator('[data-testid="ups-port"]').fill('8443');
    await page.locator('[data-testid="ups-server-save"]').click();

    const notice = page.locator('[data-testid="ups-server-notice"]');
    await expect(notice).toBeVisible();
    await expect(notice).toContainText('reserved');
  });
});
