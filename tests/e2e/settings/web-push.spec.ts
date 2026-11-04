/* =============================================================================
 * File          : web-push.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Web Push settings-card end-to-end tests (Playwright, phase D2): the
 *   unavailable server half, an insecure context, the empty state, enabling
 *   this device (with a scripted Push API), a denied permission, and removing
 *   a device.
 *
 *   The backend calls are intercepted and the browser Push API is scripted, so
 *   the suite never contacts a push service and works without an HTTPS
 *   certificate. The real endpoints, the VAPID key surface, every subscription
 *   refusal and per-user isolation live in `tests/web_push_integration.rs`;
 *   the RFC 8291/8292 cryptograpy is pinned by the RFC vector in
 *   `src/web_push/crypto.rs`.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/web-push.spec.ts
 * ============================================================================= */

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

async function openSettings(page: Page, category: RegExp) {
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
  await page.keyboard.press('Control+k');
  const dialog = page.getByRole('dialog').filter({ has: page.locator('input') }).first();
  await dialog.waitFor({ state: 'visible', timeout: 10_000 });
  await dialog.locator('input').first().fill('Open Settings');
  const openEntry = dialog.getByRole('option', { name: /^Open Settings/i }).first();
  await openEntry.waitFor({ state: 'visible', timeout: 10_000 });
  await openEntry.click();
  const rail = page.locator('nav[aria-label] [data-testid="settings-nav-item"]');
  await rail.first().waitFor({ state: 'visible', timeout: 20_000 });
  await rail.filter({ hasText: category }).first().click();
}

function card(page: Page) {
  return page.locator('[data-testid="push-card"]');
}

/** A scripted Push API: permission granted, one fake subscription. */
async function stubPushApi(
  page: Page,
  opts: { permission?: NotificationPermission; secure?: boolean } = {}
) {
  await page.addInitScript(
    ({ permission, secure }) => {
      if (!secure) {
        Object.defineProperty(window, 'isSecureContext', {
          configurable: true,
          get: () => false,
        });
      }
      const subscription = {
        endpoint: 'https://fcm.googleapis.com/wp/e2e-token',
        getKey: (name: string) => {
          const bytes = name === 'p256dh' ? new Uint8Array(65).fill(1) : new Uint8Array(16).fill(2);
          return bytes.buffer;
        },
        unsubscribe: async () => true,
      };
      const registration = {
        pushManager: {
          getSubscription: async () => null,
          subscribe: async () => subscription,
        },
      };
      Object.defineProperty(navigator, 'serviceWorker', {
        configurable: true,
        value: {
          register: async () => registration,
          ready: Promise.resolve(registration),
          getRegistration: async () => registration,
        },
      });
      Object.defineProperty(window, 'PushManager', {
        configurable: true,
        value: function PushManager() {},
      });
      Object.defineProperty(Notification, 'permission', {
        configurable: true,
        get: () => permission ?? 'default',
      });
      Object.defineProperty(Notification, 'requestPermission', {
        configurable: true,
        value: async () => permission ?? 'granted',
      });
    },
    { permission: opts.permission ?? 'granted', secure: opts.secure ?? true }
  );
}

interface Device {
  id: string;
  device_label: string;
  created_at: string;
  last_success_at: string | null;
  last_failure_at: string | null;
  failure_count: number;
  endpoint_host: string;
}

interface Recorder {
  posts: any[];
  deletes: any[];
}

async function mockPush(
  page: Page,
  opts: {
    available?: boolean;
    error?: string | null;
    devices?: Device[];
    rejectSubscribe?: string;
  } = {}
): Promise<Recorder> {
  const recorder: Recorder = { posts: [], deletes: [] };
  let devices: Device[] = opts.devices ?? [];
  const available = opts.available ?? true;

  await page.route('**/api/notifications/push**', async (route: Route) => {
    const method = route.request().method();
    const url = route.request().url();

    if (method === 'POST' && url.includes('/subscribe')) {
      const body = route.request().postDataJSON();
      recorder.posts.push(body);
      if (opts.rejectSubscribe) {
        return route.fulfill({
          status: 400,
          json: { success: false, message: opts.rejectSubscribe },
        });
      }
      const device: Device = {
        id: 'dev-1',
        device_label: 'E2E browser',
        created_at: new Date().toISOString(),
        last_success_at: null,
        last_failure_at: null,
        failure_count: 0,
        endpoint_host: 'fcm.googleapis.com',
      };
      devices = [device];
      return route.fulfill({
        json: { success: true, data: { subscription: device } },
      });
    }

    if (method === 'DELETE' && url.match(/\/subscribe\//)) {
      const id = url.split('/').pop();
      recorder.deletes.push({ id });
      devices = devices.filter((device) => device.id !== id);
      return route.fulfill({ json: { success: true, data: { id } } });
    }

    return route.fulfill({
      json: {
        success: true,
        data: {
          available,
          public_key: available
            ? 'BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8'
            : null,
          subscriptions: devices,
          max_devices: 10,
          error: available ? (opts.error ?? null) : null,
        },
      },
    });
  });

  return recorder;
}

test.describe('Web Push card', () => {
  test('says the server cannot prepare keys instead of showing a dead button', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await stubPushApi(page);
    await mockPush(page, { available: false });
    await openSettings(page, /notifications/i);
    await expect(page.locator('[data-testid="push-unavailable"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-testid="push-enable"]')).toHaveCount(0);
  });

  test('explains that an insecure context cannot receive push', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await stubPushApi(page, { secure: false });
    await mockPush(page);
    await openSettings(page, /notifications/i);
    const unsupported = page.locator('[data-testid="push-unsupported"]');
    await expect(unsupported).toBeVisible({ timeout: 20_000 });
    await expect(unsupported).toContainText(/HTTPS/i);
    await expect(page.locator('[data-testid="push-enable"]')).toHaveCount(0);
  });

  test('an account with no device sees the empty state and the enable action', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await stubPushApi(page);
    await mockPush(page);
    await openSettings(page, /notifications/i);
    await expect(page.locator('[data-testid="push-empty"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-testid="push-enable"]')).toBeVisible();
  });

  test('enabling this device registers the browser subscription', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await stubPushApi(page);
    const recorder = await mockPush(page);
    await openSettings(page, /notifications/i);

    await page.locator('[data-testid="push-enable"]').click();
    await expect.poll(() => recorder.posts.length).toBe(1);

    const sent = recorder.posts[0];
    expect(sent.endpoint).toBe('https://fcm.googleapis.com/wp/e2e-token');
    expect(typeof sent.keys.p256dh).toBe('string');
    expect(sent.keys.p256dh.length).toBeGreaterThan(40);
    expect(typeof sent.keys.auth).toBe('string');
    expect(sent.keys.auth.length).toBeGreaterThan(10);

    await expect(page.locator('[data-testid="push-device"]')).toHaveCount(1, { timeout: 10_000 });
    await expect(page.locator('[data-testid="push-this-device"]')).toBeVisible();
    await expect(page.locator('[data-testid="push-this-device-status"]')).toBeVisible();
  });

  test('a denied permission is a state with its own sentence', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await stubPushApi(page, { permission: 'denied' });
    const recorder = await mockPush(page);
    await openSettings(page, /notifications/i);

    await expect(page.locator('[data-testid="push-permission"]')).toBeVisible({ timeout: 20_000 });
    await page.locator('[data-testid="push-enable"]').click();
    await expect(page.locator('[data-testid="push-action-error"]')).toBeVisible({ timeout: 10_000 });
    expect(recorder.posts).toHaveLength(0);
  });

  test('removing a device names it and sends the delete', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await stubPushApi(page);
    const recorder = await mockPush(page, {
      devices: [
        {
          id: 'dev-1',
          device_label: 'Alice laptop',
          created_at: '2026-11-06T10:00:00Z',
          last_success_at: '2026-11-06T10:05:00Z',
          last_failure_at: null,
          failure_count: 0,
          endpoint_host: 'fcm.googleapis.com',
        },
      ],
    });
    await openSettings(page, /notifications/i);

    const row = page.locator('[data-testid="push-device"]').first();
    await expect(row).toBeVisible({ timeout: 20_000 });
    await row.locator('[data-testid="push-remove"]').click();
    const dialog = page.getByRole('dialog', { name: /remove this device/i });
    await expect(dialog).toContainText('Alice laptop');
    await page.locator('[data-testid="push-remove-confirm"]').click();

    await expect.poll(() => recorder.deletes.length).toBe(1);
    expect(recorder.deletes[0].id).toBe('dev-1');
    await expect(page.locator('[data-testid="push-device"]')).toHaveCount(0, { timeout: 10_000 });
  });

  test('no horizontal overflow at 360, 768 and 1920 px, and no uncaught error', async ({
    page,
    request,
  }) => {
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await seedSession(page, await apiLogin(request));
    await stubPushApi(page);
    await mockPush(page, {
      devices: [
        {
          id: 'dev-1',
          device_label: 'A rather long browser label on a phone',
          created_at: '2026-11-06T10:00:00Z',
          last_success_at: '2026-11-06T10:05:00Z',
          last_failure_at: null,
          failure_count: 0,
          endpoint_host: 'fcm.googleapis.com',
        },
      ],
    });
    await openSettings(page, /notifications/i);
    await expect(card(page)).toBeVisible({ timeout: 20_000 });

    for (const width of [360, 768, 1920]) {
      await page.setViewportSize({ width, height: 900 });
      await page.waitForTimeout(150);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth
      );
      expect(overflow, `${width}px`).toBeLessThanOrEqual(1);
    }
    expect(errors).toEqual([]);
  });
});
