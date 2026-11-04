/* =============================================================================
 * File          : content-index.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Content-index settings-card end-to-end tests (Playwright, phase D1): off
 *   by default, the confirmation before enabling, the root/exclusion editors,
 *   the save payload, a refused save, the status line, rebuild and "delete my
 *   index".
 *
 *   Every backend call is intercepted, so the suite never walks a real share
 *   or writes the index. The FTS5 schema, the triggers, retention, ACL
 *   filtering and the admin gate live in `tests/content_index_integration.rs`;
 *   the walker/filter/query policy is unit-tested in `src/content_index/`.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/content-index.spec.ts
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
  return page.locator('[data-testid="content-index-card"]');
}

interface Config {
  enabled: boolean;
  roots: string[];
  exclude_names: string[];
  exclude_extensions: string[];
  retention_days: number;
  confirmed: boolean;
}

interface Recorder {
  puts: any[];
  runs: number;
  deletes: number;
}

async function mockIndex(
  page: Page,
  opts: { config?: Partial<Config>; docs?: number; rejectPut?: string; running?: boolean } = {}
): Promise<Recorder> {
  const recorder: Recorder = { puts: [], runs: 0, deletes: 0 };
  let config: Config = {
    enabled: false,
    roots: [],
    exclude_names: ['node_modules', '*.bak'],
    exclude_extensions: ['iso', 'mp4'],
    retention_days: 30,
    confirmed: false,
    ...(opts.config ?? {}),
  };
  let docs = opts.docs ?? 0;

  await page.route('**/api/search/index**', async (route: Route) => {
    const method = route.request().method();
    const url = route.request().url();

    if (url.includes('/run') && method === 'POST') {
      recorder.runs += 1;
      return route.fulfill({ json: { success: true, data: { started: true } } });
    }
    if (method === 'PUT') {
      const body = route.request().postDataJSON();
      recorder.puts.push(body.config);
      if (opts.rejectPut) {
        return route.fulfill({ status: 400, json: { success: false, message: opts.rejectPut } });
      }
      config = body.config;
      return route.fulfill({ json: { success: true, data: { config, state: state() } } });
    }
    if (method === 'DELETE') {
      recorder.deletes += 1;
      config = { ...config, enabled: false, confirmed: false };
      docs = 0;
      return route.fulfill({ json: { success: true, data: { deleted: 3, config } } });
    }
    return route.fulfill({
      json: {
        success: true,
        data: {
          config,
          candidates: ['/srv/photos', '/srv/documents'],
          state: state(),
          docs,
          bytes: docs * 2048,
          limits: {
            max_roots: 16,
            max_docs: 50000,
            max_total_bytes: 268435456,
            max_file_bytes: 1048576,
          },
        },
      },
    });
  });

  function state() {
    return {
      running: opts.running ?? false,
      last_run: docs > 0 ? '2026-11-06T10:00:00Z' : null,
      last_error: null,
      truncated: false,
    };
  }

  return recorder;
}

test.describe('Content index card', () => {
  test('is off by default and only reveals its fields when enabled', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockIndex(page);
    await openSettings(page, /system/i);

    const toggle = page.locator('[data-testid="content-index-enabled"]');
    await expect(toggle).toBeVisible({ timeout: 20_000 });
    await expect(toggle).not.toBeChecked();
    await expect(page.locator('[data-testid="content-index-roots"]')).toHaveCount(0);

    await toggle.check();
    // The confirmation opens instead of silently enabling.
    await expect(page.locator('[data-testid="content-index-confirm-enable"]')).toBeVisible();
    await page.locator('[data-testid="content-index-confirm-enable"]').click();
    await expect(page.locator('[data-testid="content-index-roots"]')).toBeVisible();
  });

  test('saving sends the roots, exclusions, retention and the confirmation', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockIndex(page);
    await openSettings(page, /system/i);
    await expect(page.locator('[data-testid="content-index-enabled"]')).toBeVisible({
      timeout: 20_000,
    });

    await page.locator('[data-testid="content-index-enabled"]').check();
    await page.locator('[data-testid="content-index-confirm-enable"]').click();
    const roots = page.locator('[data-testid="content-index-root"]');
    await expect(roots).toHaveCount(2);
    await roots.first().check();
    await page.locator('[data-testid="content-index-exclude-names"]').fill('node_modules, *.bak');
    await page.locator('[data-testid="content-index-exclude-extensions"]').fill('iso, mp4, pdf');
    await page.locator('[data-testid="content-index-retention"]').fill('14');
    await page.locator('[data-testid="content-index-save"]').click();

    await expect.poll(() => recorder.puts.length).toBe(1);
    expect(recorder.puts[0]).toMatchObject({
      enabled: true,
      confirmed: true,
      roots: ['/srv/photos'],
      exclude_names: ['node_modules', '*.bak'],
      exclude_extensions: ['iso', 'mp4', 'pdf'],
      retention_days: 14,
    });
  });

  test('cancelling the confirmation leaves the index off', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockIndex(page);
    await openSettings(page, /system/i);
    await expect(page.locator('[data-testid="content-index-enabled"]')).toBeVisible({
      timeout: 20_000,
    });

    await page.locator('[data-testid="content-index-enabled"]').check();
    await page.getByRole('dialog', { name: /turn on content search/i }).getByRole('button', {
      name: /cancel/i,
    }).click();
    await expect(page.locator('[data-testid="content-index-enabled"]')).not.toBeChecked();
    expect(recorder.puts).toHaveLength(0);
  });

  test('a refused save surfaces the server sentence', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockIndex(page, { rejectPut: '/etc is not one of this appliance’s Explorer roots.' });
    await openSettings(page, /system/i);
    await expect(page.locator('[data-testid="content-index-enabled"]')).toBeVisible({
      timeout: 20_000,
    });

    await page.locator('[data-testid="content-index-enabled"]').check();
    await page.locator('[data-testid="content-index-confirm-enable"]').click();
    await page.locator('[data-testid="content-index-save"]').click();
    const error = page.locator('[data-testid="content-index-action-error"]');
    await expect(error).toBeVisible({ timeout: 10_000 });
    await expect(error).toContainText('Explorer roots');
  });

  test('shows the indexed size and rebuilds on demand', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockIndex(page, {
      config: { enabled: true, confirmed: true, roots: ['/srv/photos'] },
      docs: 12,
    });
    await openSettings(page, /system/i);

    const status = page.locator('[data-testid="content-index-status"]');
    await expect(status).toContainText('12', { timeout: 20_000 });
    await expect(status).toContainText('24.0 KB');
    await page.locator('[data-testid="content-index-run"]').click();
    await expect.poll(() => recorder.runs).toBe(1);
  });

  test('delete my index names the count and sends the confirmed delete', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockIndex(page, {
      config: { enabled: true, confirmed: true, roots: ['/srv/photos'] },
      docs: 3,
    });
    await openSettings(page, /system/i);

    await expect(page.locator('[data-testid="content-index-delete"]')).toBeEnabled({
      timeout: 20_000,
    });
    await page.locator('[data-testid="content-index-delete"]').click();
    const dialog = page.getByRole('dialog', { name: /delete the content index/i });
    await expect(dialog).toContainText('3');
    await page.locator('[data-testid="content-index-delete-confirm"]').click();
    await expect.poll(() => recorder.deletes).toBe(1);
  });

  test('no horizontal overflow at 360, 768 and 1920 px', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockIndex(page, {
      config: { enabled: true, confirmed: true, roots: ['/srv/photos', '/srv/documents'] },
      docs: 42,
    });
    await openSettings(page, /system/i);
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
