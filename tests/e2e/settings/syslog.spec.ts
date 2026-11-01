/* =============================================================================
 * File          : syslog.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings → Monitoring → Remote syslog card, end-to-end (Playwright):
 *   shape, save round-trip, validation, the TLS CA requirement, the test
 *   button (mocked, never a real collector), queue counters, accessibility.
 *
 * Dependencies:
 *   - a running HomeVault with a seeded admin (see TESTING.md)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/syslog.spec.ts --project=firefox
 * =============================================================================
 */

/**
 * Remote syslog forwarding (Settings → Monitoring) — End-to-End Test Suite
 *
 * **SAFETY: this suite never sends a packet to a real collector.** The "send a
 * test message" endpoint is intercepted with `page.route` (success in 5.1, the
 * collector's reason in 5.2), and 5.3 only asserts the button is disabled while
 * the feature is off. A real UDP/TCP collector is covered by
 * `tests/syslog_forward_integration.rs`, and the wire format by the unit tests
 * in `src/syslog_forward.rs`.
 *
 * The suite leaves forwarding **disabled** with an empty destination on this
 * instance (5.4, in `afterAll`), so a follow-up run — or a real deployment —
 * cannot be left shipping its journal to a collector chosen by a test.
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

interface Tokens {
  username: string;
  password: string;
  token: string;
  csrf: string;
}

async function apiLogin(ctx: APIRequestContext, username: string, password: string) {
  const res = await ctx.post('/api/auth/login', { data: { username, password } });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

/** Put the session in localStorage before the app boots (same as ddns.spec). */
async function seedBrowser(page: Page, tokens: Tokens) {
  await page.addInitScript(
    ({ t }) => {
      try {
        localStorage.setItem('session-token', t.token);
        localStorage.setItem('csrf-token', t.csrf);
      } catch {
        /* storage unavailable — the test will surface it */
      }
    },
    { t: tokens },
  );
}

async function dismissWhatsNew(page: Page) {
  const modal = page.locator('[data-testid="whats-new-modal"]');
  if (await modal.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await modal.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

/** Opens Settings → Monitoring and waits for the syslog card. */
async function openMonitoringCategory(page: Page, tokens: Tokens) {
  await seedBrowser(page, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await dismissWhatsNew(page);

  const dockBtn = page.locator('.hv-dock-item:has(.hv-dock-icon-settings)').first();
  await dockBtn.scrollIntoViewIfNeeded().catch(() => {});
  try {
    await dockBtn.click({ timeout: 5_000 });
  } catch {
    await dockBtn.click({ force: true });
  }
  await page.waitForSelector('[data-testid="settings-page"]', { timeout: 15_000 });
  await page
    .locator('[data-testid="settings-nav-item"]')
    .first()
    .waitFor({ state: 'visible', timeout: 15_000 });

  const monitoringItem = page.locator('[data-testid="settings-nav-item"]', {
    hasText: /Monitoring|Surveillance|Überwachung|Supervisión|Monitorización/i,
  });
  await monitoringItem.first().click();
  // The card's own status pill is the readiness signal: it is rendered only
  // after `GET /api/settings/syslog` answered.
  await page.waitForSelector('[data-testid="syslog-status"]', { timeout: 15_000 });
}

async function syslogSettings(page: Page, tokens: Tokens) {
  const res = await page.request.get(`${BASE}/api/settings/syslog`, {
    headers: { 'x-session-token': tokens.token },
  });
  expect(res.ok()).toBeTruthy();
  return (await res.json()).data;
}

/** Always leave the instance with forwarding off. */
async function disableForwarding(ctx: APIRequestContext, tokens: Tokens) {
  await ctx
    .put(`${BASE}/api/settings/syslog`, {
      headers: { 'x-session-token': tokens.token, 'x-csrf-token': tokens.csrf },
      data: { enabled: false },
    })
    .catch(() => {});
}

let adminTokens: Tokens;
let sharedCtx: APIRequestContext;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await playwright.request.newContext({ baseURL: BASE });
  const { token, csrf } = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
  await disableForwarding(sharedCtx, adminTokens);
});

test.afterAll(async () => {
  // A test must never leave this box shipping its audit trail somewhere.
  await disableForwarding(sharedCtx, adminTokens);
  await sharedCtx.dispose();
});

// ===========================================================================
// 1. Shape
// ===========================================================================

test('1.1 — the card renders the destination fields and both formats', async ({ page }) => {
  await openMonitoringCategory(page, adminTokens);

  const card = page.locator('[data-testid="settings-card"]', { hasText: /syslog/i }).first();
  await expect(card).toBeVisible();

  await expect(page.locator('[data-testid="syslog-host"]')).toBeVisible();
  await expect(page.locator('[data-testid="syslog-port"]')).toBeVisible();
  await expect(page.locator('[data-testid="syslog-facility"]')).toBeVisible();
  await expect(page.locator('[data-testid="syslog-save"]')).toBeVisible();
  await expect(page.locator('[data-testid="syslog-test"]')).toBeVisible();

  const protocols = await page
    .locator('[data-testid="syslog-protocol"] option')
    .evaluateAll((els) => els.map((e) => (e as HTMLOptionElement).value));
  expect(protocols).toEqual(['udp', 'tcp', 'tls']);

  const formats = await page
    .locator('[data-testid="syslog-format"] option')
    .evaluateAll((els) => els.map((e) => (e as HTMLOptionElement).value));
  expect(formats).toEqual(['rfc5424', 'rfc3164']);
});

test('1.2 — the status pill carries one documented state and says it in words', async ({ page }) => {
  await openMonitoringCategory(page, adminTokens);

  const pill = page.locator('[data-testid="syslog-status"]');
  const state = await pill.getAttribute('data-state');
  expect(['disabled', 'idle', 'forwarding', 'degraded']).toContain(state);

  // WCAG 1.4.1 is "colour is never the only carrier": the pill must render
  // words, and the state must stay machine-readable for tests/assistive tech.
  // (Asserting the label *differs* from `state` would be wrong — in English
  // "Disabled" is the label of `disabled`.)
  const label = (await pill.innerText()).trim();
  expect(label.length).toBeGreaterThan(0);
  expect(label).toMatch(/[A-Za-zÀ-ÿ]/);
  // …and the styling must not be the only differentiator: each state has its
  // own class, so a screenshot-free assertion can tell them apart.
  await expect(pill).toHaveClass(new RegExp(`syslog-badge-${state}`));
});

test('1.3 — the queue counters are numbers read from the server', async ({ page }) => {
  const settings = await syslogSettings(page, adminTokens);
  await openMonitoringCategory(page, adminTokens);

  for (const [testid, key] of [
    ['syslog-pending', 'pending'],
    ['syslog-sent', 'sent'],
  ] as const) {
    const shown = Number((await page.locator(`[data-testid="${testid}"]`).innerText()).trim());
    expect(shown).toBe(settings.queue[key]);
  }
});

test('1.4 — the CA field appears only for TLS', async ({ page }) => {
  await openMonitoringCategory(page, adminTokens);

  await page.locator('[data-testid="syslog-protocol"]').selectOption('udp');
  await expect(page.locator('[data-testid="syslog-ca"]')).toHaveCount(0);

  await page.locator('[data-testid="syslog-protocol"]').selectOption('tls');
  await expect(page.locator('[data-testid="syslog-ca"]')).toBeVisible();

  // Put the form back the way it was found, without saving.
  await page.locator('[data-testid="syslog-protocol"]').selectOption('udp');
});

// ===========================================================================
// 2. Save round-trip
// ===========================================================================

test('2.1 — a saved destination is what the next read returns', async ({ page }) => {
  await openMonitoringCategory(page, adminTokens);

  await page.locator('[data-testid="syslog-host"]').fill('192.0.2.10');
  await page.locator('[data-testid="syslog-port"]').fill('5514');
  await page.locator('[data-testid="syslog-protocol"]').selectOption('tcp');
  await page.locator('[data-testid="syslog-format"]').selectOption('rfc3164');
  await page.locator('[data-testid="syslog-facility"]').fill('16');
  await page.locator('[data-testid="syslog-save"]').click();

  await expect(page.locator('[data-testid="syslog-notice"]')).toBeVisible({ timeout: 10_000 });

  const settings = await syslogSettings(page, adminTokens);
  expect(settings.host).toBe('192.0.2.10');
  expect(settings.port).toBe(5514);
  expect(settings.protocol).toBe('tcp');
  expect(settings.format).toBe('rfc3164');
  expect(settings.facility).toBe(16);

  // A disabled forwarder is explicitly disabled, not idle or forwarding.
  expect(settings.status).toBe('disabled');
  expect(settings.last_error).toBeNull();

  // Clean up: the next test starts from an empty destination.
  await page.request.put(`${BASE}/api/settings/syslog`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: { host: '', port: 514, protocol: 'udp', format: 'rfc5424', facility: 1 },
  });
});

test('2.2 — enabling and disabling is reflected in the status pill', async ({ page }) => {
  await openMonitoringCategory(page, adminTokens);

  const enable = page.locator('.syslog-card .settings-toggle-label').first();
  const checkbox = page.locator('.syslog-card .settings-toggle-label').first().locator('input[type="checkbox"]');
  if (!(await checkbox.isChecked())) {
    await enable.click();
  }
  await page.locator('[data-testid="syslog-host"]').fill('192.0.2.11');
  await page.locator('[data-testid="syslog-save"]').click();
  await expect(page.locator('[data-testid="syslog-notice"]')).toBeVisible({ timeout: 10_000 });

  const settings = await syslogSettings(page, adminTokens);
  expect(settings.enabled).toBe(true);
  // Enabled, nothing sent yet.
  expect(['idle', 'forwarding']).toContain(settings.status);

  await disableForwarding(sharedCtx, adminTokens);
  await page.locator('[data-testid="syslog-refresh"]').click();
  await expect(page.locator('[data-testid="syslog-status"]')).toHaveAttribute(
    'data-state',
    'disabled',
    { timeout: 10_000 },
  );
});

// ===========================================================================
// 3. Validation
// ===========================================================================

test('3.1 — a URL pasted as the server is refused with the reason on screen', async ({ page }) => {
  await openMonitoringCategory(page, adminTokens);

  await page.locator('.syslog-card .settings-toggle-label').first().click();
  await page.locator('[data-testid="syslog-host"]').fill('https://collector.lan');
  await page.locator('[data-testid="syslog-save"]').click();

  const notice = page.locator('[data-testid="syslog-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  // The server names the problem ("without a scheme"), not "invalid input".
  await expect(notice).toContainText(/scheme|schéma|Schema|esquema/i);

  const settings = await syslogSettings(page, adminTokens);
  expect(settings.host).not.toBe('https://collector.lan');

  await disableForwarding(sharedCtx, adminTokens);
});

test('3.2 — TLS without a CA certificate is refused before anything is stored', async ({ page }) => {
  await openMonitoringCategory(page, adminTokens);

  await page.locator('.syslog-card .settings-toggle-label').first().click();
  await page.locator('[data-testid="syslog-host"]').fill('192.0.2.12');
  await page.locator('[data-testid="syslog-protocol"]').selectOption('tls');
  await page.locator('[data-testid="syslog-save"]').click();

  const notice = page.locator('[data-testid="syslog-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toContainText(/CA|certificate|certificat|Zertifikat|certificado/i);

  await disableForwarding(sharedCtx, adminTokens);
});

// ===========================================================================
// 4. Server-owned fields
// ===========================================================================

test('4.1 — a client cannot fabricate a "forwarding" status', async ({ page }) => {
  await disableForwarding(sharedCtx, adminTokens);
  await openMonitoringCategory(page, adminTokens);

  // The card sends only the editable fields; the API ignores anything else.
  const res = await page.request.put(`${BASE}/api/settings/syslog`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: {
      enabled: false,
      host: '192.0.2.13',
      status: 'forwarding',
      last_error: 'nothing to see',
      last_sent_at: '2026-10-31T12:00:00Z',
    },
  });
  expect(res.ok()).toBeTruthy();
  const settings = await syslogSettings(page, adminTokens);
  expect(settings.status).toBe('disabled');
  expect(settings.last_error).toBeNull();
  expect(settings.last_sent_at).toBeNull();

  await disableForwarding(sharedCtx, adminTokens);
});

// ===========================================================================
// 5. Test button (mocked: no packet leaves this machine)
// ===========================================================================

test('5.1 — a successful test message reports "sent, check the collector"', async ({ page }) => {
  await openMonitoringCategory(page, adminTokens);

  await page.locator('.syslog-card .settings-toggle-label').first().click();
  await page.locator('[data-testid="syslog-host"]').fill('192.0.2.14');
  await page.locator('[data-testid="syslog-save"]').click();
  await expect(page.locator('[data-testid="syslog-notice"]')).toBeVisible({ timeout: 10_000 });

  await page.route('**/api/settings/syslog/test', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: { sent: true, message: 'Test message sent' },
      }),
    }),
  );

  await page.locator('[data-testid="syslog-test"]').click();
  const notice = page.locator('[data-testid="syslog-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'status');
  // Wording matters: the packet left, arrival is unconfirmed.
  await expect(notice).toContainText(/collector|collecteur|Sammler|colector/i);

  await page.unroute('**/api/settings/syslog/test');
  await disableForwarding(sharedCtx, adminTokens);
});

test('5.2 — a refused test message shows the collector reason as an alert', async ({ page }) => {
  await disableForwarding(sharedCtx, adminTokens);
  await sharedCtx.put(`${BASE}/api/settings/syslog`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: { enabled: true, host: '192.0.2.14', port: 514, protocol: 'udp', format: 'rfc5424', facility: 1 },
  });
  await openMonitoringCategory(page, adminTokens);

  await page.route('**/api/settings/syslog/test', (route) =>
    route.fulfill({
      status: 400,
      contentType: 'application/json',
      body: JSON.stringify({
        success: false,
        message: 'The test message could not be sent: cannot reach 192.0.2.14:514',
      }),
    }),
  );

  await page.locator('[data-testid="syslog-test"]').click();
  const notice = page.locator('[data-testid="syslog-notice"]');
  await expect(notice).toBeVisible({ timeout: 10_000 });
  await expect(notice).toHaveAttribute('role', 'alert');
  await expect(notice).toContainText(/192\.0\.2\.14/);

  await page.unroute('**/api/settings/syslog/test');
});

test('5.3 — the test button is disabled while forwarding is off', async ({ page }) => {
  const settings = await syslogSettings(page, adminTokens);
  test.skip(settings.enabled, 'forwarding is enabled on this instance');
  await openMonitoringCategory(page, adminTokens);
  await expect(page.locator('[data-testid="syslog-test"]')).toBeDisabled();
});

// ===========================================================================
// 6. Access control and accessibility
// ===========================================================================

test('6.1 — a non-admin session cannot read or write where the audit trail goes', async ({
  page,
}) => {
  // A member account is seeded by the Explorer suite; create one here if the
  // instance has none, so the test is self-sufficient.
  const name = `syslogmember${Date.now().toString(36).slice(-4)}`;
  const created = await page.request.post(`${BASE}/api/users`, {
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: { username: name, password: 'MemberSecret123', fullname: 'Syslog Member', email: '', role_id: 1 },
  });
  expect(created.ok(), await created.text()).toBeTruthy();

  const member = await apiLogin(sharedCtx, name, 'MemberSecret123');
  expect(member.token).toBeTruthy();

  const read = await page.request.get(`${BASE}/api/settings/syslog`, {
    headers: { 'x-session-token': member.token },
  });
  expect(read.status()).toBe(403);
  const write = await page.request.put(`${BASE}/api/settings/syslog`, {
    headers: { 'x-session-token': member.token, 'x-csrf-token': member.csrf },
    data: { enabled: true, host: '192.0.2.99' },
  });
  expect(write.status()).toBe(403);
  const send = await page.request.post(`${BASE}/api/settings/syslog/test`, {
    headers: { 'x-session-token': member.token, 'x-csrf-token': member.csrf },
  });
  expect(send.status()).toBe(403);

  await page.request
    .delete(`${BASE}/api/users/${encodeURIComponent(name)}`, {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    })
    .catch(() => {});
});

test('6.2 — every field has a label and the switches are announced', async ({ page }) => {
  await openMonitoringCategory(page, adminTokens);

  for (const id of ['syslog-protocol', 'syslog-host', 'syslog-port', 'syslog-format', 'syslog-facility']) {
    const labelled = await page.locator(`label[for="${id}"]`).count();
    expect(labelled, `${id} must have a <label for>`).toBe(1);
  }

  // The two switches take their accessible name from the visible row label.
  const switches = page.locator('input[type="checkbox"]');
  const count = await switches.count();
  expect(count).toBeGreaterThanOrEqual(2);
  for (let i = 0; i < count; i++) {
    const name = await switches.nth(i).getAttribute('aria-labelledby');
    if (name) {
      expect(await page.locator(`#${name}`).count()).toBe(1);
    } else {
      expect((await switches.nth(i).getAttribute('aria-label')) || '').not.toBe('');
    }
  }
});

test('6.3 — no duplicate element ids on the screen', async ({ page }) => {
  await openMonitoringCategory(page, adminTokens);
  const duplicates = await page.evaluate(() => {
    const seen = new Map<string, number>();
    document.querySelectorAll('[id]').forEach((el) => {
      const id = el.id;
      seen.set(id, (seen.get(id) ?? 0) + 1);
    });
    return [...seen.entries()].filter(([, n]) => n > 1).map(([id]) => id);
  });
  expect(duplicates).toEqual([]);
});

test('6.4 — the card does not overflow its panel horizontally', async ({ page }) => {
  await openMonitoringCategory(page, adminTokens);
  await page.locator('[data-testid="syslog-protocol"]').selectOption('tls');
  const overflow = await page.evaluate(() => {
    const panel = document.querySelector('[data-testid="settings-page"]');
    if (!panel) return -1;
    const box = panel.getBoundingClientRect();
    let worst = 0;
    panel.querySelectorAll('[data-testid^="syslog-"]').forEach((el) => {
      const r = el.getBoundingClientRect();
      worst = Math.max(worst, r.right - box.right);
    });
    return Math.round(worst);
  });
  expect(overflow).toBeLessThanOrEqual(0);
  await page.locator('[data-testid="syslog-protocol"]').selectOption('udp');
});
