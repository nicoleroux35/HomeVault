/* =============================================================================
 * File          : snmp-agent.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 04/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Local SNMP agent settings-card end-to-end tests (Playwright): the
 *   capability-absent state, off-by-default, the scope choice, the community
 *   field, the save round-trip and the refusal path.
 *
 *   Every backend call is intercepted with `page.route`, so the suite never
 *   writes `/etc/snmp/snmpd.conf` or restarts `snmpd` on the machine hosting
 *   the run. The real endpoint, the admin gate and the `503` capability
 *   contract live in `tests/snmp_integration.rs`; the rules the settings obey
 *   (never `public`, private-only address, no directive injection) are
 *   unit-tested in `src/snmp_agent.rs`.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/snmp-agent.spec.ts
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

interface AgentState {
  enabled?: boolean;
  scope?: 'loopback' | 'lan';
  lan_address?: string | null;
  version?: 'v2c' | 'v3';
  has_community?: boolean;
  v3_username?: string;
  has_v3_auth_secret?: boolean;
  has_v3_priv_secret?: boolean;
  location?: string;
  contact?: string;
}

interface Recorder {
  puts: any[];
}

/**
 * Intercepts `/api/snmp/agent`. `unavailable` drives the `503` capability
 * path, so the "snmpd is not installed" screen is exercised without
 * uninstalling anything.
 */
async function mockAgent(
  page: Page,
  opts: { unavailable?: boolean; state?: AgentState; rejectPut?: string } = {}
): Promise<Recorder> {
  const recorder: Recorder = { puts: [] };
  let state: AgentState = {
    enabled: false,
    scope: 'loopback',
    lan_address: null,
    has_community: false,
    location: '',
    contact: '',
    ...opts.state,
  };

  await page.route('**/api/snmp/agent', async (route: Route) => {
    if (opts.unavailable) {
      return route.fulfill({
        status: 503,
        json: { success: false, message: 'snmpd is not installed on this appliance' },
      });
    }
    if (route.request().method() === 'PUT') {
      const body = route.request().postDataJSON();
      recorder.puts.push(body);
      if (opts.rejectPut) {
        return route.fulfill({
          status: 400,
          json: { success: false, message: opts.rejectPut },
        });
      }
      // The real API never echoes a secret back; the mock must not either,
      // otherwise a test could pass on a payload the server would never send.
      const { community, v3_auth_secret, v3_priv_secret, ...rest } = body;
      state = {
        ...state,
        ...rest,
        has_community: community ? true : state.has_community,
        has_v3_auth_secret: v3_auth_secret ? true : state.has_v3_auth_secret,
        has_v3_priv_secret: v3_priv_secret ? true : state.has_v3_priv_secret,
      };
      return route.fulfill({ json: { success: true, data: state } });
    }
    return route.fulfill({
      json: {
        success: true,
        data: {
          ...state,
          installed: true,
          serving: !!state.enabled,
          lan_candidates: ['192.168.1.10', '10.0.0.5'],
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

/** The agent card, by its one stable hook. */
function card(page: Page) {
  return page.locator('[data-testid="snmp-agent-enabled"]').locator('xpath=ancestor::*[contains(@class,"settings-card")][1]');
}

test.describe('SNMP agent card', () => {
  test('says the agent is not installed instead of showing a dead form', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockAgent(page, { unavailable: true });
    await openSettings(page, /monitoring/i);
    await expect(page.locator('[data-testid="snmp-agent-unavailable"]')).toBeVisible({
      timeout: 20_000,
    });
    // The banner names the package to install, and no switch is offered.
    await expect(page.locator('[data-testid="snmp-agent-unavailable"]')).toContainText('snmpd');
    await expect(page.locator('[data-testid="snmp-agent-enabled"]')).toHaveCount(0);
  });

  test('is off by default, and turning it on reveals the scope choice', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockAgent(page);
    await openSettings(page, /monitoring/i);

    const toggle = page.locator('[data-testid="snmp-agent-enabled"]');
    await expect(toggle).toBeVisible({ timeout: 20_000 });
    await expect(toggle).not.toBeChecked();
    // The scope and community fields only exist once the agent is on: nothing
    // to configure for a feature that answers nobody.
    await expect(page.locator('[data-testid="snmp-agent-scope"]')).toHaveCount(0);

    await toggle.check();
    await expect(page.locator('[data-testid="snmp-agent-scope"]')).toBeVisible();
    await expect(page.locator('[data-testid="snmp-agent-community"]')).toBeVisible();
    // Loopback is the default scope: no address field for it.
    await expect(page.locator('[data-testid="snmp-agent-address"]')).toHaveCount(0);
  });

  test('choosing the LAN scope offers only the appliance\'s own addresses', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockAgent(page);
    await openSettings(page, /monitoring/i);

    await page.locator('[data-testid="snmp-agent-enabled"]').check();
    await page.locator('[data-testid="snmp-agent-scope"]').selectOption('lan');

    const address = page.locator('[data-testid="snmp-agent-address"]');
    await expect(address).toBeVisible();
    // A select of real addresses, never a free-text field the admin has to
    // fill with a value they cannot know.
    const options = await address.locator('option').evaluateAll((els) =>
      els.map((e) => (e as HTMLOptionElement).value)
    );
    expect(options).toEqual(['192.168.1.10', '10.0.0.5']);
  });

  test('saving sends only what changed, and an empty community keeps the current one', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockAgent(page, { state: { has_community: true } });
    await openSettings(page, /monitoring/i);

    await page.locator('[data-testid="snmp-agent-enabled"]').check();
    await page.locator('#snmp-agent-location').fill('Rack, garage');
    await page.locator('[data-testid="snmp-agent-save"]').click();

    await expect.poll(() => recorder.puts.length).toBe(1);
    const sent = recorder.puts[0];
    expect(sent.enabled).toBe(true);
    expect(sent.scope).toBe('loopback');
    expect(sent.location).toBe('Rack, garage');
    // The community is not round-tripped: an empty field must not blank it.
    expect(sent.community).toBeUndefined();
  });

  test('a refused save surfaces the server\'s own reason', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockAgent(page, {
      rejectPut: "'public' and 'private' are the two strings every scanner tries first — choose another community",
    });
    await openSettings(page, /monitoring/i);

    await page.locator('[data-testid="snmp-agent-enabled"]').check();
    await page.locator('[data-testid="snmp-agent-community"]').fill('public');
    await page.locator('[data-testid="snmp-agent-save"]').click();

    // The reason is shown verbatim in the toast, not a generic failure.
    await expect(page.getByText(/every scanner tries first/i)).toBeVisible({ timeout: 10_000 });
  });

  test('SNMPv3 replaces the community with a dedicated user and two passphrases', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockAgent(page, { state: { version: 'v2c' } });
    await openSettings(page, /monitoring/i);

    await page.locator('[data-testid="snmp-agent-enabled"]').check();
    // v2c is the default: the community field is shown, no v3 field.
    await expect(page.locator('[data-testid="snmp-agent-community"]')).toBeVisible();
    await expect(page.locator('[data-testid="snmp-agent-v3-auth"]')).toHaveCount(0);

    await page.locator('[data-testid="snmp-agent-version"]').selectOption('v3');

    // Switching versions swaps the credential fields; the community is gone.
    await expect(page.locator('[data-testid="snmp-agent-community"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="snmp-agent-v3-username"]')).toHaveValue('homevault');
    await expect(page.locator('[data-testid="snmp-agent-v3-auth"]')).toBeVisible();
    await expect(page.locator('[data-testid="snmp-agent-v3-priv"]')).toBeVisible();

    await page.locator('[data-testid="snmp-agent-v3-username"]').fill('monitor');
    await page.locator('[data-testid="snmp-agent-v3-auth"]').fill('authpass123');
    await page.locator('[data-testid="snmp-agent-v3-priv"]').fill('privpass456');
    await page.locator('[data-testid="snmp-agent-save"]').click();

    await expect.poll(() => recorder.puts.length).toBe(1);
    const sent = recorder.puts[0];
    expect(sent.version).toBe('v3');
    expect(sent.v3_username).toBe('monitor');
    expect(sent.v3_auth_secret).toBe('authpass123');
    expect(sent.v3_priv_secret).toBe('privpass456');
    // No v2c credential is sent when the agent runs v3.
    expect(sent.community).toBeUndefined();
  });

  test('an already-stored v3 passphrase is not round-tripped', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    const recorder = await mockAgent(page, {
      state: { version: 'v3', has_v3_auth_secret: true, has_v3_priv_secret: true },
    });
    await openSettings(page, /monitoring/i);

    await page.locator('[data-testid="snmp-agent-enabled"]').check();
    // The fields are present (version loaded as v3) and empty: a stored secret
    // can be kept but never read back.
    await expect(page.locator('[data-testid="snmp-agent-v3-auth"]')).toHaveValue('');
    await expect(page.locator('[data-testid="snmp-agent-v3-priv"]')).toHaveValue('');

    await page.locator('[data-testid="snmp-agent-save"]').click();
    await expect.poll(() => recorder.puts.length).toBe(1);
    const sent = recorder.puts[0];
    expect(sent.version).toBe('v3');
    expect(sent.v3_auth_secret).toBeUndefined();
    expect(sent.v3_priv_secret).toBeUndefined();
  });
});
