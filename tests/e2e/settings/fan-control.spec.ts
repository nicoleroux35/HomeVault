/* =============================================================================
 * File          : fan-control.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Fan-control settings-card end-to-end tests (Playwright): the laptop
 *   refusal, the "no controllable fan" state, the profile picker, the manual
 *   speed slider (floored at 20 %), the channel table, the restore-auto action
 *   and the save round-trip.
 *
 *   Every backend call is intercepted with `page.route`, so the suite never
 *   writes a PWM register on the machine that hosts the test run. The real
 *   endpoint and validation live in `tests/fan_control_integration.rs` and
 *   `tests/phase4_auth_integration.rs`; the curve arithmetic lives in the
 *   `fan_control.rs` unit tests.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/fan-control.spec.ts
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
  return page.locator('[data-testid="fan-card"]');
}

function config(overrides: Record<string, unknown> = {}) {
  return {
    profile: 'auto',
    manual_duty: 50,
    channel_overrides: {},
    interval_secs: 10,
    ...overrides,
  };
}

function channel(overrides: Record<string, unknown> = {}) {
  return {
    id: 'hwmon0/pwm1',
    name: 'cpu_fan',
    pwm_num: 1,
    hwmon_path: '/sys/class/hwmon/hwmon0',
    current_pwm: 128,
    current_mode: 2,
    current_temp: 42000,
    supports_auto: true,
    ...overrides,
  };
}

interface Recorder {
  puts: any[];
  restores: number;
}

async function mockFan(
  page: Page,
  opts: {
    laptop?: boolean;
    channels?: any[];
    cfg?: Record<string, unknown>;
    message?: string | null;
    rejectPut?: string;
  } = {}
): Promise<Recorder> {
  const recorder: Recorder = { puts: [], restores: 0 };

  await page.route('**/api/system/fans/config', async (route: Route) => {
    if (route.request().method() === 'PUT') {
      const body = route.request().postDataJSON();
      recorder.puts.push(body);
      if (opts.rejectPut) {
        return route.fulfill({
          status: 500,
          json: { success: false, message: opts.rejectPut },
        });
      }
      return route.fulfill({ json: { success: true, data: { applied: true } } });
    }
    return route.fulfill({ json: { success: true, data: opts.cfg ?? config() } });
  });

  await page.route('**/api/system/fans/restore-auto', (route: Route) => {
    recorder.restores += 1;
    return route.fulfill({ json: { success: true, data: { restored: true } } });
  });

  await page.route('**/api/system/fans', (route: Route) =>
    route.fulfill({
      json: {
        success: true,
        data: {
          channels: opts.channels ?? [channel()],
          is_laptop: !!opts.laptop,
          message: opts.message ?? null,
        },
      },
    })
  );

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

test.describe('Fan control card', () => {
  test('a laptop is refused with an explanation, not an empty form', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockFan(page, { laptop: true, channels: [] });
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="fan-card"]')).toBeVisible({ timeout: 20_000 });

    await expect(page.locator('[data-testid="fan-laptop"]')).toBeVisible();
    // No profile picker and no save button: there is nothing to configure.
    await expect(page.locator('[data-testid="fan-save"]')).toHaveCount(0);
  });

  test('a host with no controllable fan says so', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockFan(page, { channels: [] });
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="fan-card"]')).toBeVisible({ timeout: 20_000 });

    await expect(page.locator('[data-testid="fan-no-channels"]')).toBeVisible();
    // The save button is present but disabled: nothing can be applied.
    await expect(page.locator('[data-testid="fan-save"]')).toBeDisabled();
  });

  test('the channel table reports speed, temperature and firmware control', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockFan(page);
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="fan-card"]')).toBeVisible({ timeout: 20_000 });

    const channels = page.locator('[data-testid="fan-channels"]');
    await expect(channels).toContainText('cpu_fan');
    // 128/255 rounds to 50 %.
    await expect(channels).toContainText('50%');
    await expect(channels).toContainText('42.0°C');
  });

  test('the manual speed slider appears only for the fixed-speed profile', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockFan(page);
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="fan-card"]')).toBeVisible({ timeout: 20_000 });

    await expect(page.locator('[data-testid="fan-manual-duty"]')).toHaveCount(0);

    await card(page).getByRole('radio', { name: /fixed speed/i }).check();
    const slider = page.locator('[data-testid="fan-manual-duty"]');
    await expect(slider).toBeVisible();
    // The 20 % floor is a real attribute, not just a sentence.
    await expect(slider).toHaveAttribute('min', '20');
  });

  test('saving sends the chosen profile', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockFan(page);
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="fan-card"]')).toBeVisible({ timeout: 20_000 });

    await card(page).getByRole('radio', { name: /quiet/i }).check();
    await page.locator('[data-testid="fan-save"]').click();

    await expect.poll(() => recorder.puts.length).toBe(1);
    expect(recorder.puts[0].profile).toBe('quiet');
  });

  test('a refused apply shows the server sentence', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockFan(page, { rejectPut: 'failed to set PWM mode: Permission denied' });
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="fan-card"]')).toBeVisible({ timeout: 20_000 });

    await page.locator('[data-testid="fan-save"]').click();
    const notice = page.locator('[data-testid="fan-notice"]');
    await expect(notice).toBeVisible();
    await expect(notice).toContainText('Permission denied');
  });

  test('giving control back to the firmware calls the restore action', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockFan(page);
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="fan-card"]')).toBeVisible({ timeout: 20_000 });

    await page.locator('[data-testid="fan-restore-auto"]').click();
    await expect.poll(() => recorder.restores).toBe(1);
    await expect(page.locator('[data-testid="fan-notice"]')).toBeVisible();
  });

  test('the profile picker is a real radio group, reachable by keyboard', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockFan(page);
    await openSettings(page, /monitoring/i);
  await expect(page.locator('[data-testid="fan-card"]')).toBeVisible({ timeout: 20_000 });

    const radios = card(page).getByRole('radio');
    const count = await radios.count();
    expect(count).toBeGreaterThanOrEqual(4);

    // Selecting the third profile with the keyboard alone must work: the
    // previous version used click-only `<div>`s.
    await radios.nth(2).focus();
    await page.keyboard.press('Space');
    await expect(radios.nth(2)).toBeChecked();
  });
});
