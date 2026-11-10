/* =============================================================================
 * File          : usb-copy.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   USB one-touch copy screen end-to-end tests (Playwright): the empty states,
 *   rule creation, rule editing, the "copy now" action, delete, and the device
 *   list with its matched-rule badge.
 *
 *   Every backend call is intercepted with `page.route`, so the suite never
 *   mounts a device or writes to a share on the machine that hosts the test
 *   run. The real endpoint, auth matrix and validation live in
 *   `tests/usb_copy_integration.rs` and `tests/phase4_auth_integration.rs`.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/usb-copy.spec.ts
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

function profile(overrides: Record<string, unknown> = {}) {
  return {
    id: 'p1',
    name: 'Camera Photos',
    device_uuid: '1234-5678',
    device_label: 'CAMERA',
    source_path: '/DCIM',
    destination_path: '/mnt/photos',
    verify_checksums: true,
    strategy: 'incremental',
    versions_to_keep: 10,
    conflict: 'overwrite',
    auto_eject: false,
    enabled: true,
    last_run: null,
    last_status: null,
    ...overrides,
  };
}

function device(overrides: Record<string, unknown> = {}) {
  return {
    device: '/dev/sdb1',
    uuid: '1234-5678',
    label: 'CAMERA',
    size: 34359738368,
    mount_point: '/media/CAMERA',
    removable: true,
    mounted: true,
    matching_profile: null,
    ...overrides,
  };
}

interface Recorder {
  creates: any[];
  updates: any[];
  deletes: string[];
  executes: string[];
}

async function mockUsb(
  page: Page,
  opts: {
    profiles?: any[];
    devices?: any[];
    executeMessage?: string;
  } = {}
): Promise<Recorder> {
  const recorder: Recorder = { creates: [], updates: [], deletes: [], executes: [] };
  let profiles = opts.profiles ?? [];

  await page.route('**/api/usb-copy/devices', (route: Route) =>
    route.fulfill({ json: { success: true, data: opts.devices ?? [] } })
  );

  await page.route('**/api/usb-copy/execute/*', (route: Route) => {
    const id = route.request().url().split('/').pop() ?? '';
    recorder.executes.push(id);
    return route.fulfill({
      json: {
        success: true,
        data: {
          status: 'started',
          message: opts.executeMessage ?? 'Copy operation started in background',
        },
      },
    });
  });

  await page.route('**/api/usb-copy/profiles/*', async (route: Route) => {
    const id = route.request().url().split('/').pop() ?? '';
    const method = route.request().method();
    if (method === 'PUT') {
      const body = route.request().postDataJSON();
      recorder.updates.push(body);
      profiles = profiles.map((p) => (p.id === id ? body : p));
      return route.fulfill({ json: { success: true, data: body } });
    }
    if (method === 'DELETE') {
      recorder.deletes.push(id);
      profiles = profiles.filter((p) => p.id !== id);
      return route.fulfill({ json: { success: true, data: { deleted: id } } });
    }
    return route.fulfill({ json: { success: true, data: null } });
  });

  await page.route('**/api/usb-copy/profiles', async (route: Route) => {
    if (route.request().method() === 'POST') {
      const body = route.request().postDataJSON();
      recorder.creates.push(body);
      profiles = [...profiles, body];
      return route.fulfill({ json: { success: true, data: body } });
    }
    return route.fulfill({ json: { success: true, data: profiles } });
  });

  return recorder;
}

async function gotoDesktop(page: Page) {
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
  const whatsNew = page.locator('[data-testid="whats-new-modal"]');
  if (await whatsNew.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await whatsNew.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

/** The shell forces `/desktop`, so the window is opened through the launcher. */
async function openUsbCopy(page: Page) {
  await gotoDesktop(page);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const tile = page.locator('.lp-card[data-app-id="usb-copy"]');
  await tile.waitFor({ state: 'visible', timeout: 10_000 });
  await tile.click();
  await expect(page.locator('[data-testid="usb-new-profile"]')).toBeVisible({ timeout: 20_000 });
}

test.describe('USB one-touch copy', () => {
  test('the empty state says what to do next', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUsb(page);
    await openUsbCopy(page);

    await expect(page.locator('[data-testid="usb-no-profiles"]')).toBeVisible();
    await expect(page.locator('[data-testid="usb-no-devices"]')).toBeVisible();
  });

  test('exactly one h1 is rendered on the screen', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUsb(page);
    await openUsbCopy(page);

    // The window chrome renders the screen's single h1; the in-body title is
    // an h2 so there is never a second one.
    await expect(page.locator('h1')).toHaveCount(1);
  });

  test('a rule can be created from the dialog', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockUsb(page, { devices: [device()] });
    await openUsbCopy(page);

    await page.locator('[data-testid="usb-new-profile"]').click();
    await page.locator('[data-testid="usb-name"]').fill('Camera Photos');
    await page.locator('[data-testid="usb-uuid"]').selectOption('1234-5678');
    await page.locator('[data-testid="usb-source"]').fill('/DCIM');
    await page.locator('[data-testid="usb-destination"]').fill('/mnt/photos');
    await page.locator('[data-testid="usb-strategy"]').selectOption('versioned');
    await page.locator('[data-testid="usb-versions-to-keep"]').fill('5');
    await page.locator('[data-testid="usb-conflict"]').selectOption('rename');
    await page.locator('[data-testid="usb-save"]').click();

    await expect.poll(() => recorder.creates.length).toBe(1);
    expect(recorder.creates[0].name).toBe('Camera Photos');
    expect(recorder.creates[0].device_uuid).toBe('1234-5678');
    expect(recorder.creates[0].strategy).toBe('versioned');
    expect(recorder.creates[0].versions_to_keep).toBe(5);
    expect(recorder.creates[0].conflict).toBe('rename');
    // The dialog closes and the notice confirms the save.
    await expect(page.locator('[data-testid="usb-name"]')).toBeHidden();
    await expect(page.locator('[data-testid="usb-notice"]')).toBeVisible();
  });

  test('a rule with no name is refused client-side', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockUsb(page, { devices: [device()] });
    await openUsbCopy(page);

    await page.locator('[data-testid="usb-new-profile"]').click();
    await page.locator('[data-testid="usb-save"]').click();

    await expect(page.locator('[data-testid="usb-notice"]')).toBeVisible();
    expect(recorder.creates).toHaveLength(0);
  });

  test('an existing rule shows its details and can be run', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockUsb(page, { profiles: [profile()] });
    await openUsbCopy(page);

    const list = page.locator('[data-testid="usb-profiles"]');
    await expect(list).toContainText('Camera Photos');
    await expect(list).toContainText('/DCIM');
    await expect(list).toContainText('/mnt/photos');

    await page.locator('[data-testid="usb-run-p1"]').click();
    await expect.poll(() => recorder.executes).toEqual(['p1']);
    await expect(page.locator('[data-testid="usb-notice"]')).toContainText(
      'Copy operation started'
    );
  });

  test('the copy mode is shown on the rule', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUsb(page, { profiles: [profile({ strategy: 'photo_import' })] });
    await openUsbCopy(page);

    await expect(page.locator('[data-testid="usb-profiles"]')).toContainText(
      'Photo import'
    );
  });

  test('a disabled rule cannot be run', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockUsb(page, {
      profiles: [profile({ enabled: false })],
    });
    await openUsbCopy(page);

    await expect(page.locator('[data-testid="usb-run-p1"]')).toBeDisabled();
    expect(recorder.executes).toHaveLength(0);
  });

  test('a rule can be deleted', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockUsb(page, { profiles: [profile()] });
    await openUsbCopy(page);

    await page.locator('[data-testid="usb-delete-p1"]').click();
    await expect.poll(() => recorder.deletes).toEqual(['p1']);
    await expect(page.locator('[data-testid="usb-no-profiles"]')).toBeVisible();
  });

  test('an attached device shows the rule it matched', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUsb(page, {
      profiles: [profile()],
      devices: [device({ matching_profile: 'p1' })],
    });
    await openUsbCopy(page);

    const devices = page.locator('[data-testid="usb-devices"]');
    await expect(devices).toContainText('CAMERA');
    await expect(devices).toContainText('Camera Photos');
    // 34 GiB rendered in a human unit, never raw bytes.
    await expect(devices).toContainText('GB');
  });
});
