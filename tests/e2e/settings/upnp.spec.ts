/* =============================================================================
 * File          : upnp.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Router port forwarding settings-card end-to-end tests (Playwright, phase
 *   C3): the missing-`upnpc` state, a discovery failure, the mapping list,
 *   the add flow with its confirmation, a refused add and the remove flow.
 *
 *   Every backend call is intercepted with `page.route`, so the suite never
 *   opens or closes a port on the router of the machine hosting the run — and
 *   works on a host with no UPnP gateway at all. The real endpoints, the admin
 *   gate and every pre-`upnpc` refusal live in `tests/upnp_integration.rs`; the
 *   listing parser, the address policy and the exact argv are unit-tested in
 *   `src/upnp.rs`.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/upnp.spec.ts
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
  return page.locator('[data-testid="upnp-card"]');
}

interface Mapping {
  protocol: 'tcp' | 'udp';
  external_port: number;
  internal_host: string;
  internal_port: number;
  description: string;
  lease_seconds: number;
}

interface Recorder {
  posts: any[];
  deletes: any[];
}

/**
 * Intercepts `/api/network/upnp*`. `available` drives the missing-client path,
 * `error` the router-refusal path, and the add/remove calls are recorded so the
 * exact payload can be asserted.
 */
async function mockUpnp(
  page: Page,
  opts: {
    available?: boolean;
    error?: string | null;
    mappings?: Mapping[];
    rejectAdd?: string;
  } = {}
): Promise<Recorder> {
  const recorder: Recorder = { posts: [], deletes: [] };
  let mappings: Mapping[] = opts.mappings ?? [];
  const available = opts.available ?? true;

  await page.route('**/api/network/upnp**', async (route: Route) => {
    const method = route.request().method();
    const isMappings = route.request().url().includes('/mappings');

    if (isMappings && method === 'POST') {
      const body = route.request().postDataJSON();
      recorder.posts.push(body);
      if (opts.rejectAdd) {
        return route.fulfill({
          status: 502,
          json: { success: false, message: opts.rejectAdd },
        });
      }
      const mapping: Mapping = {
        protocol: body.protocol,
        external_port: Number(body.external_port),
        internal_host: body.internal_host,
        internal_port: Number(body.internal_port),
        description: body.description ? `HomeVault: ${body.description}` : 'HomeVault',
        lease_seconds: Number(body.lease_seconds),
      };
      mappings = [...mappings, mapping];
      return route.fulfill({
        json: {
          success: true,
          data: {
            snapshot: { gateway: 'http://192.168.1.1:5000/ctl', external_ip: '203.0.113.5', local_address: '192.168.1.10', mappings },
            error: null,
            message: 'The mapping is added.',
          },
        },
      });
    }

    if (isMappings && method === 'DELETE') {
      const body = route.request().postDataJSON();
      recorder.deletes.push(body);
      mappings = mappings.filter(
        (m) => !(m.protocol === body.protocol && m.external_port === Number(body.external_port))
      );
      return route.fulfill({
        json: {
          success: true,
          data: {
            snapshot: { gateway: 'http://192.168.1.1:5000/ctl', external_ip: '203.0.113.5', local_address: '192.168.1.10', mappings },
            error: null,
            message: 'The mapping is removed.',
          },
        },
      });
    }

    return route.fulfill({
      json: {
        success: true,
        data: {
          available,
          gateway: available ? 'http://192.168.1.1:5000/ctl' : null,
          external_ip: available ? '203.0.113.5' : null,
          local_address: available ? '192.168.1.10' : null,
          mappings: available ? mappings : [],
          error: available ? (opts.error ?? null) : null,
          suggested_host: available ? '192.168.1.10' : null,
        },
      },
    });
  });

  return recorder;
}

test.describe('Router port forwarding card', () => {
  test('says upnpc is not installed instead of showing a dead form', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUpnp(page, { available: false });
    await openSettings(page, /security/i);
    await expect(page.locator('[data-testid="upnp-unavailable"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-testid="upnp-unavailable"]')).toContainText('miniupnpc');
    await expect(page.locator('[data-testid="upnp-add"]')).toHaveCount(0);
  });

  test('a router that does not answer says why, instead of an empty table', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockUpnp(page, { error: 'No UPnP gateway answered.' });
    await openSettings(page, /security/i);
    const error = page.locator('[data-testid="upnp-discovery-error"]');
    await expect(error).toBeVisible({ timeout: 20_000 });
    await expect(error).toContainText(/No UPnP gateway answered/i);
  });

  test('lists the router mappings and the external address', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUpnp(page, {
      mappings: [
        {
          protocol: 'tcp',
          external_port: 8080,
          internal_host: '192.168.1.50',
          internal_port: 80,
          description: 'HomeVault: web',
          lease_seconds: 3600,
        },
        {
          protocol: 'udp',
          external_port: 5353,
          internal_host: '192.168.1.10',
          internal_port: 5353,
          description: 'Jellyfin',
          lease_seconds: 0,
        },
      ],
    });
    await openSettings(page, /security/i);

    await expect(page.locator('[data-testid="upnp-external-ip"]')).toHaveText('203.0.113.5', {
      timeout: 20_000,
    });
    await expect(page.locator('[data-testid="upnp-mapping"]')).toHaveCount(2);
    await expect(page.locator('[data-testid="upnp-mapping"]').first()).toContainText('8080');
    await expect(page.locator('[data-testid="upnp-mapping"]').first()).toContainText(
      '192.168.1.50:80'
    );
    // A permanent lease is labelled, not rendered as `0`.
    await expect(page.locator('[data-testid="upnp-mapping"]').nth(1)).toContainText(/Permanent/i);
  });

  test('the empty state says what to do next', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUpnp(page);
    await openSettings(page, /security/i);
    await expect(page.locator('[data-testid="upnp-empty"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-testid="upnp-add"]')).toBeVisible();
  });

  test('adding a mapping requires the confirmation and sends the exact payload', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockUpnp(page);
    await openSettings(page, /security/i);

    await page.locator('[data-testid="upnp-add"]').click();
    await expect(page.locator('[data-testid="upnp-internal-host"]')).toBeVisible();
    // The appliance's own LAN address is offered, never typed blind.
    await expect(page.locator('[data-testid="upnp-internal-host"]')).toHaveValue('192.168.1.10');

    const save = page.locator('[data-testid="upnp-save"]');
    await expect(save).toBeDisabled();
    await page.locator('[data-testid="upnp-external-port"]').fill('8443');
    await page.locator('[data-testid="upnp-internal-host"]').fill('192.168.1.50');
    await page.locator('[data-testid="upnp-internal-port"]').fill('443');
    await page.locator('[data-testid="upnp-description"]').fill('web');
    await page.locator('[data-testid="upnp-lease"]').selectOption('3600');
    await page.locator('[data-testid="upnp-confirm"]').check();
    await expect(save).toBeEnabled();
    await save.click();

    await expect.poll(() => recorder.posts.length).toBe(1);
    expect(recorder.posts[0]).toMatchObject({
      protocol: 'tcp',
      external_port: 8443,
      internal_host: '192.168.1.50',
      internal_port: 443,
      description: 'web',
      lease_seconds: 3600,
      confirmed: true,
    });
    await expect(page.locator('[data-testid="upnp-mapping"]')).toHaveCount(1, { timeout: 10_000 });
  });

  test('a refused mapping shows the router sentence, not a generic failure', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockUpnp(page, { rejectAdd: 'ConflictInMappingEntry' });
    await openSettings(page, /security/i);

    await page.locator('[data-testid="upnp-add"]').click();
    await page.locator('[data-testid="upnp-internal-host"]').fill('192.168.1.50');
    await page.locator('[data-testid="upnp-confirm"]').check();
    await page.locator('[data-testid="upnp-save"]').click();

    const error = page.locator('[data-testid="upnp-form-error"]');
    await expect(error).toBeVisible({ timeout: 10_000 });
    await expect(error).toContainText('ConflictInMappingEntry');
  });

  test('removing a mapping names it and sends a confirmed delete', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockUpnp(page, {
      mappings: [
        {
          protocol: 'tcp',
          external_port: 8080,
          internal_host: '192.168.1.50',
          internal_port: 80,
          description: 'HomeVault: web',
          lease_seconds: 3600,
        },
      ],
    });
    await openSettings(page, /security/i);

    const row = page.locator('[data-testid="upnp-mapping"]').first();
    await expect(row).toBeVisible({ timeout: 20_000 });
    await row.locator('[data-testid="upnp-remove"]').click();

    const dialog = page.getByRole('dialog', { name: /remove the mapping/i });
    await expect(dialog).toContainText('8080');
    await expect(dialog).toContainText('192.168.1.50:80');
    await page.locator('[data-testid="upnp-remove-confirm"]').click();

    await expect.poll(() => recorder.deletes.length).toBe(1);
    expect(recorder.deletes[0]).toMatchObject({
      protocol: 'tcp',
      external_port: 8080,
      confirmed: true,
    });
    await expect(page.locator('[data-testid="upnp-mapping"]')).toHaveCount(0, { timeout: 10_000 });
  });

  test('the add dialog is reachable and dismissible by keyboard', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUpnp(page);
    await openSettings(page, /security/i);

    const add = page.locator('[data-testid="upnp-add"]');
    await expect(add).toBeVisible({ timeout: 20_000 });
    await add.focus();
    await page.keyboard.press('Enter');
    await expect(page.locator('[data-testid="upnp-internal-host"]')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.locator('[data-testid="upnp-internal-host"]')).toHaveCount(0, {
      timeout: 10_000,
    });
    await expect(add).toBeFocused();
  });

  test('no horizontal overflow at 360, 768 and 1920 px', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockUpnp(page, {
      mappings: [
        {
          protocol: 'tcp',
          external_port: 8080,
          internal_host: '192.168.1.50',
          internal_port: 80,
          description: 'HomeVault: a rather long description for a mapping',
          lease_seconds: 86400,
        },
      ],
    });
    await openSettings(page, /security/i);
    await expect(card(page)).toBeVisible({ timeout: 20_000 });

    for (const width of [360, 768, 1920]) {
      await page.setViewportSize({ width, height: 900 });
      await page.waitForTimeout(150);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth
      );
      expect(overflow, `${width}px`).toBeLessThanOrEqual(1);
    }
  });
});
