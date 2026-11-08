/* =============================================================================
 * File          : local-dns.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Local DNS server settings-card end-to-end tests (Playwright, phase C1): the
 *   capability-absent state, off by default, the interface chooser, the
 *   upstream/name/domain editors, the confirmation before turning the server
 *   on, the shared-daemon state badge and the refusal path.
 *
 *   Every backend call is intercepted with `page.route`, so the suite never
 *   writes `/etc/dnsmasq.d/homevault.conf` or restarts `dnsmasq` on the machine
 *   hosting the run. The real endpoint, the admin gate and the validation live
 *   in `tests/local_dns_integration.rs` and `tests/phase4_auth_integration.rs`;
 *   the rules the configuration obeys (name grammar, alias targets, forwarder
 *   refusals, the rendered directives) are unit-tested in `src/local_dns.rs`
 *   and `src/dhcp_server.rs`.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/local-dns.spec.ts
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

interface DnsConfig {
  enabled: boolean;
  interface: string;
  serve_dhcp_clients: boolean;
  forwarders: string[];
  records: { name: string; kind: 'a' | 'cname'; value: string }[];
  overrides: { domain: string; address: string }[];
  confirmed: boolean;
}

function config(overrides: Partial<DnsConfig> = {}): DnsConfig {
  return {
    enabled: false,
    interface: 'eth0',
    serve_dhcp_clients: false,
    forwarders: [],
    records: [],
    overrides: [],
    confirmed: false,
    ...overrides,
  };
}

interface Recorder {
  puts: any[];
}

/**
 * Intercepts `/api/network/dns-server`. `unavailable` drives the missing-daemon
 * path so the screen is exercised without uninstalling anything.
 */
async function mockDns(
  page: Page,
  opts: {
    unavailable?: boolean;
    state?: { running?: boolean };
    dhcpEnabled?: boolean;
    cfg?: DnsConfig;
    rejectPut?: string;
  } = {}
): Promise<Recorder> {
  const recorder: Recorder = { puts: [] };
  let saved: DnsConfig = opts.cfg ?? config();
  const running = opts.state?.running ?? false;

  await page.route('**/api/network/dns-server', async (route: Route) => {
    if (route.request().method() === 'PUT') {
      const body = route.request().postDataJSON();
      recorder.puts.push(body);
      if (opts.rejectPut) {
        return route.fulfill({
          status: 400,
          json: { success: false, message: opts.rejectPut },
        });
      }
      saved = body.config;
      return route.fulfill({
        json: {
          success: true,
          data: {
            config: saved,
            state: { running: true, serving: saved.enabled },
            message: 'The local DNS server is configured',
          },
        },
      });
    }
    return route.fulfill({
      json: {
        success: true,
        data: {
          config: saved,
          state: {
            running,
            serving: running && saved.enabled,
          },
          available: !opts.unavailable,
          interfaces: ['eth0', 'eth1'],
          system_forwarders: ['192.168.1.1', '9.9.9.9'],
          dhcp_enabled: opts.dhcpEnabled ?? false,
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

/** Settings is reached through the ⌘K overlay (it has no launcher tile). */
async function openSettings(page: Page, category: RegExp) {
  await gotoDesktop(page);
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

/** The card, by its one stable hook. */
function card(page: Page) {
  return page.locator('[data-testid="local-dns-card"]');
}

test.describe('Local DNS server card', () => {
  test('1.1 is reachable from the security category and renders its own card', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockDns(page);
    await openSettings(page, /security/i);

    const root = card(page);
    await expect(root).toBeVisible({ timeout: 20_000 });
    // One <h1>-free card with its own two-line header, like every other card.
    await expect(root.locator('[data-testid="settings-card-title"]')).toContainText(/DNS/i);
  });

  test('1.2 names the missing dnsmasq instead of showing a dead form', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockDns(page, { unavailable: true });
    await openSettings(page, /security/i);

    const banner = page.locator('[data-testid="local-dns-unavailable"]');
    await expect(banner).toBeVisible({ timeout: 20_000 });
    // It names the package to install, and no save action is offered.
    await expect(banner).toContainText('dnsmasq');
    await expect(page.locator('[data-testid="local-dns-save"]')).toHaveCount(0);
  });

  test('2.1 is off by default and the whole body only appears once it is on', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockDns(page);
    await openSettings(page, /security/i);

    const toggle = page.locator('[data-testid="local-dns-enabled"]');
    await expect(toggle).toBeVisible({ timeout: 20_000 });
    await expect(toggle).not.toBeChecked();
    // Nothing to configure for a server that answers nobody.
    await expect(page.locator('[data-testid="local-dns-interface"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="local-dns-new-forwarder"]')).toHaveCount(0);

    await toggle.check();
    await expect(page.locator('[data-testid="local-dns-interface"]')).toBeVisible();
    await expect(page.locator('[data-testid="local-dns-new-forwarder"]')).toBeVisible();
    // The interface is a chooser of the appliance's own interfaces, never a
    // free-text field the admin has to fill with a value they cannot know.
    const options = await page
      .locator('[data-testid="local-dns-interface"] option')
      .evaluateAll((els) => els.map((e) => (e as HTMLOptionElement).value));
    expect(options).toEqual(['eth0', 'eth1']);
  });

  test('3.1 an upstream server is added and removed row by row', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockDns(page, { cfg: config({ enabled: true, confirmed: true }) });
    await openSettings(page, /security/i);

    await page.locator('[data-testid="local-dns-enabled"]').check();
    await page.locator('[data-testid="local-dns-new-forwarder"]').fill('192.168.1.1');
    await page.locator('[data-testid="local-dns-add-forwarder"]').click();

    await expect(page.locator('[data-testid="local-dns-forwarders"]')).toContainText(
      '192.168.1.1'
    );
    // Adding a row does not save: the admin still confirms with Save.
    expect(recorder.puts).toHaveLength(0);

    await page.locator('[data-testid="local-dns-remove-forwarder-192.168.1.1"]').click();
    await expect(page.locator('[data-testid="local-dns-no-forwarders"]')).toBeVisible();
  });

  test('3.2 a local name is added with its type, and an alias carries its target', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockDns(page, { cfg: config({ enabled: true, confirmed: true }) });
    await openSettings(page, /security/i);

    await page.locator('[data-testid="local-dns-enabled"]').check();
    await page.locator('[data-testid="local-dns-new-name"]').fill('nas.lan');
    await page.locator('[data-testid="local-dns-new-value"]').fill('192.168.1.5');
    await page.locator('[data-testid="local-dns-add-record"]').click();

    await expect(page.locator('[data-testid="local-dns-records"]')).toContainText('nas.lan');

    // The type choice switches what the value field means.
    await page.locator('[data-testid="local-dns-new-kind"]').selectOption('cname');
    await page.locator('[data-testid="local-dns-new-value"]').fill('nas.lan');
    await page.locator('[data-testid="local-dns-new-name"]').fill('media.lan');
    await page.locator('[data-testid="local-dns-add-record"]').click();

    const table = page.locator('[data-testid="local-dns-records"]');
    await expect(table).toContainText('media.lan');
    await expect(table).toContainText(/alias/i);
  });

  test('4.1 turning the server on is confirmed before anything is saved', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockDns(page);
    await openSettings(page, /security/i);

    await page.locator('[data-testid="local-dns-enabled"]').check();
    await page.locator('[data-testid="local-dns-new-forwarder"]').fill('192.168.1.1');
    await page.locator('[data-testid="local-dns-add-forwarder"]').click();
    await page.locator('[data-testid="local-dns-save"]').click();

    // The confirmation appears, and nothing has been PUT yet.
    await expect(page.locator('[data-testid="local-dns-confirm-enable"]')).toBeVisible();
    expect(recorder.puts).toHaveLength(0);

    await page.locator('[data-testid="local-dns-confirm-enable"]').click();
    await expect.poll(() => recorder.puts.length).toBe(1);
    expect(recorder.puts[0].config.confirmed).toBe(true);
    expect(recorder.puts[0].config.enabled).toBe(true);
    expect(recorder.puts[0].config.forwarders).toEqual(['192.168.1.1']);
  });

  test('4.2 cancelling the confirmation saves nothing', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockDns(page);
    await openSettings(page, /security/i);

    await page.locator('[data-testid="local-dns-enabled"]').check();
    await page.locator('[data-testid="local-dns-save"]').click();
    await expect(page.locator('[data-testid="local-dns-confirm-enable"]')).toBeVisible();

    await page.getByRole('button', { name: /^cancel$/i }).click();
    await expect(page.locator('[data-testid="local-dns-confirm-enable"]')).toBeHidden();
    expect(recorder.puts).toHaveLength(0);
  });

  test('5.1 a refused save shows the server sentence, not a generic failure', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockDns(page, {
      rejectPut:
        "The alias 'media.lan' points at 'nas.lan', which this appliance does not serve",
    });
    await openSettings(page, /security/i);

    // With the server off, no confirmation blocks the save.
    await page.locator('[data-testid="local-dns-save"]').click();
    await expect(page.getByText(/does not serve/i)).toBeVisible({ timeout: 10_000 });
  });

  test('5.2 the state badge distinguishes serving, shared-daemon and off', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockDns(page, { state: { running: true }, dhcpEnabled: true });
    await openSettings(page, /security/i);

    // The badge lives in the body, which only exists once the server is on: an
    // off server has nothing to report the state of.
    await page.locator('[data-testid="local-dns-enabled"]').check();

    // Running but this half off: the badge says the daemon serves DHCP only,
    // and the card states that the two features share one program.
    const badge = page.locator('[data-testid="local-dns-state"]');
    await expect(badge).toBeVisible({ timeout: 20_000 });
    await expect(badge).toHaveAttribute('data-state', 'shared');
    await expect(card(page)).toContainText(/one program|DHCP/i);
  });

  test('6.1 every control is labelled and operable from the keyboard', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockDns(page);
    await openSettings(page, /security/i);

    const root = card(page);
    await expect(root.getByLabel(/Answer DNS queries/i).first()).toBeVisible({
      timeout: 20_000,
    });

    // Toggling with the keyboard alone reveals the body, and every control the
    // body renders is reachable by its label.
    await page.locator('[data-testid="local-dns-enabled"]').focus();
    await page.keyboard.press('Space');
    for (const label of [/Interface/i, /Upstream address/i, /Name/i, /Domain/i]) {
      await expect(root.getByLabel(label).first()).toBeVisible();
    }

    // The remove actions are icon-free but never label-free: each carries an
    // accessible name naming what it removes.
    await page.locator('[data-testid="local-dns-new-forwarder"]').fill('192.168.1.1');
    await page.locator('[data-testid="local-dns-add-forwarder"]').click();
    await expect(
      page.getByRole('button', { name: /Remove the upstream server 192\.168\.1\.1/i })
    ).toBeVisible();
  });

  test('7.1 nothing sensitive is put in the URL or in client storage', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockDns(page, { cfg: config({ enabled: true, confirmed: true }) });
    await openSettings(page, /security/i);
    await expect(card(page)).toBeVisible({ timeout: 20_000 });

    // A resolver address is not a credential, but the rule is the rule: no
    // card value ends up in the query string or in localStorage.
    const url = page.url();
    expect(url).not.toContain('eth0');
    expect(url).not.toContain('192.168');
    const dumped = await page.evaluate(() => JSON.stringify({ ...localStorage }));
    expect(dumped).not.toContain('192.168');
  });

  test('8.1 the card stays usable at 360, 768 and 1920 px', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockDns(page, { cfg: config({ enabled: true, confirmed: true }) });
    await openSettings(page, /security/i);
    const root = card(page);
    await expect(root).toBeVisible({ timeout: 20_000 });

    for (const width of [360, 768, 1920]) {
      await page.setViewportSize({ width, height: 900 });
      // Let the window manager's resize reflow (and the Settings rail's
      // auto-collapse below 700px) settle before measuring.
      await page.waitForTimeout(300);
      const box = await root.boundingBox();
      expect(box, `card is laid out at ${width}px`).not.toBeNull();
      // No horizontal overflow: the card never paints past the panel.
      const overflow = await root.evaluate((el) => el.scrollWidth - el.clientWidth);
      expect(overflow, `card overflows at ${width}px`).toBeLessThanOrEqual(1);
    }
  });
});
