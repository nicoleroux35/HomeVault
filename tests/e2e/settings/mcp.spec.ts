/* =============================================================================
 * File          : mcp.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 19/10/2026
 * Last modified : 19/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings → AI Assistant → External agents (MCP gateway) end-to-end
 *   tests (Playwright): admin card content, kill-switch, token lifecycle,
 *   non-admin invisibility and secret hygiene.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * External agents (MCP) — Settings → AI Assistant
 *
 * The gateway configuration (kill-switch, global permissions, revocable
 * tokens) is a server-wide, admin-only setting. It used to be a side panel
 * inside the Chat app; since 2026-10-19 it is the `McpCard` of the Settings
 * screen's `ai` category (`frontend/src/lib/components/settings/McpCard.svelte`),
 * and the Chat app must no longer expose it (test 15.7).
 *
 * The card probes `GET /api/mcp/status`, which is admin-only, and renders
 * nothing when that probe fails — so the admin check has no client-side
 * "is admin" flag to drift.
 *
 * Run (all):     npx playwright test tests/e2e/settings/mcp.spec.ts --project=firefox
 * Run (single):  npx playwright test tests/e2e/settings/mcp.spec.ts -g "15.3"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/settings/mcp.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
/** Where `/api/*` and `/mcp` live. Defaults to the page origin; set it when
 *  the browser talks to the Vite dev server (`BASE_URL=http://localhost:5199`),
 *  whose proxy only covers `/api` — `/mcp` would otherwise hit Vite and
 *  answer its SPA fallback instead of the gateway's 401. */
const API_BASE = process.env.API_URL || BASE;
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

interface Tokens {
  username: string;
  password: string;
  token: string;
  csrf: string;
}

async function newCtx(playwright: any): Promise<APIRequestContext> {
  return playwright.request.newContext({ baseURL: API_BASE });
}

async function apiLogin(
  ctx: APIRequestContext,
  username: string,
  password: string,
): Promise<{ token: string; csrf: string }> {
  const res = await ctx.post('/api/auth/login', { data: { username, password } });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

/**
 * A non-admin session that can actually use the API.
 *
 * `POST /api/users` creates the account with an admin-chosen password, so
 * `password_change_guard` answers `403 password_change_required` to every
 * call until the owner picks their own. Log in, change it, log in again —
 * otherwise these authorization tests would assert a 403 that comes from the
 * forced-change gate instead of the admin gate they are about.
 */
async function apiLoginAsViewer(
  ctx: APIRequestContext,
  username: string,
  initialPassword: string,
  newPassword = 'Viewer67890!',
): Promise<{ token: string; csrf: string }> {
  const first = await apiLogin(ctx, username, initialPassword);
  await ctx.put(`/api/users/${username}/password`, {
    headers: { 'x-session-token': first.token, 'x-csrf-token': first.csrf },
    data: { current_password: initialPassword, new_password: newPassword },
  });
  return apiLogin(ctx, username, newPassword);
}

async function seedBrowser(page: Page, tokens: Tokens, lang = 'en') {
  await page.addInitScript(
    ({ t, l }) => {
      try {
        localStorage.setItem('session-token', t.token);
        localStorage.setItem('csrf-token', t.csrf);
        localStorage.setItem('hv-language', l);
      } catch {
        /* storage unavailable — a test will surface it */
      }
    },
    { t: tokens, l: lang },
  );
}

async function gotoDesktop(page: Page, tokens: Tokens, lang = 'en') {
  await seedBrowser(page, tokens, lang);
  await page.goto('/desktop');
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page.locator('.hv-dock').waitFor({ state: 'visible', timeout: 10_000 });
  // Desktop monitor widgets float over the whole screen (position:fixed) and
  // would intercept clicks meant for the dock or the Settings window.
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});

  // A fresh account (the throwaway reader of 15.5) is shown the
  // once-per-version "What's New" modal, whose dialog overlay intercepts
  // every dock click — the seeded admin has already dismissed it.
  const whatsNew = page.locator('[data-testid="whats-new-modal"]');
  await whatsNew.waitFor({ state: 'visible', timeout: 1_000 }).catch(() => {});
  if (await whatsNew.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await whatsNew.waitFor({ state: 'hidden', timeout: 5_000 }).catch(() => {});
  }
}

/** Open Settings from the dock and wait for the panel to hydrate. */
async function openSettings(page: Page, tokens: Tokens, lang = 'en') {
  await gotoDesktop(page, tokens, lang);
  const dockBtn = page.locator('.hv-dock-item:has(.hv-dock-icon-settings)').first();
  await dockBtn.scrollIntoViewIfNeeded().catch(() => {});
  try {
    await dockBtn.click({ timeout: 5_000 });
  } catch {
    await dockBtn.click({ force: true });
  }
  await page.waitForSelector('[data-testid="settings-page"]', { timeout: 15_000 });
  await page.locator('[data-testid="settings-nav-item"]').first().waitFor({ state: 'visible', timeout: 15_000 });
  return page.locator('[data-testid="settings-page"]');
}

/** The Settings window as a scoping root. */
function set(page: Page) {
  return page.locator('[data-testid="settings-page"]');
}

/** Open the AI Assistant category (the group that hosts the gateway card). */
async function openAiCategory(page: Page) {
  const item = set(page)
    .locator('[data-testid="settings-nav-item"]')
    .filter({ hasText: 'AI Assistant' });
  await item.click();
  await expect(item).toHaveClass(/set-nav-active/);
  await page.waitForTimeout(120);
}

/** The external-agent card, scoped inside the Settings window. */
function mcpCard(page: Page) {
  return set(page)
    .locator('[data-testid="settings-card"]')
    .filter({ hasText: /External agents/i });
}

async function api(page: Page, t: Tokens | null, method: string, path: string, body?: unknown) {
  return page.evaluate(
    async ({ t, method, path, body }) => {
      const headers: Record<string, string> = {};
      if (t) {
        headers['x-session-token'] = t.token;
        headers['x-csrf-token'] = t.csrf;
      }
      if (body !== undefined) headers['Content-Type'] = 'application/json';
      const r = await fetch(path, {
        method,
        headers,
        body: body !== undefined ? JSON.stringify(body) : undefined,
      });
      const txt = await r.text();
      let j: any = {};
      try {
        j = JSON.parse(txt);
      } catch {
        j = { _raw: txt };
      }
      return { http: r.status, success: j?.success, error: j?.error || j?.message || '', data: j?.data };
    },
    { t, method, path, body },
  );
}

let adminTokens: Tokens;
let sharedCtx: APIRequestContext;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
});

test.afterAll(async () => {
  await sharedCtx?.dispose();
});

/**
 * Admin REST call out of the browser, for fixtures that must exist *before*
 * a page navigates (a throwaway reader account). `api()` above evaluates
 * `fetch` inside the page, so it needs a document — relative URLs are not
 * valid on `about:blank`.
 */
async function rest(method: string, path: string, body?: unknown) {
  const r = await sharedCtx.fetch(path, {
    method,
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: body as any,
  });
  const j = await r.json().catch(() => ({}));
  return { http: r.status(), success: j?.success, error: j?.error || j?.message || '', data: j?.data };
}

// ===========================================================================
// 15 — External agents (MCP) in Settings → AI Assistant
// ===========================================================================

test.describe('15 — External agents (MCP) in Settings → AI Assistant', () => {
  test('15.1 — the AI Assistant category carries the External agents card', async ({ page }) => {
    await openSettings(page, adminTokens);
    await openAiCategory(page);
    const card = mcpCard(page);
    await expect(card).toBeVisible({ timeout: 15_000 });
    // The card is part of the AI Assistant group, not a separate category.
    await expect(set(page).locator('[data-testid="settings-header-title"]')).toHaveText('AI Assistant');
  });

  test('15.2 — the card shows the endpoint, the kill-switch, permissions and the token form', async ({ page }) => {
    await openSettings(page, adminTokens);
    await openAiCategory(page);
    const card = mcpCard(page);
    await expect(card).toBeVisible({ timeout: 15_000 });

    // Agent endpoint, in full, on the card itself.
    await expect(card.locator('[data-testid="mcp-endpoint"]')).toContainText('/mcp');
    // Kill-switch and the permission controls.
    await expect(card.locator('[data-testid="mcp-enabled"]')).toBeVisible();
    for (const id of ['mcp-perm-apps', 'mcp-perm-files', 'mcp-perm-vms', 'mcp-token-name']) {
      await expect(card.locator(`#${id}`)).toBeVisible();
    }
    // Token form: the create button is there and carries an accessible name.
    await expect(card.getByRole('button', { name: 'Create a token' })).toBeVisible();
  });

  test('15.3 — the enable switch flips the server state and restores it', async ({ page }) => {
    await openSettings(page, adminTokens);
    await openAiCategory(page);
    const card = mcpCard(page);
    await expect(card).toBeVisible({ timeout: 15_000 });

    const before = await api(page, adminTokens, 'GET', '/api/mcp/status');
    expect(before.success).toBe(true);
    const was = !!before.data?.status?.enabled;

    await card.locator('[data-testid="mcp-enabled"]').click();
    await expect
      .poll(async () => (await api(page, adminTokens, 'GET', '/api/mcp/status')).data?.status?.enabled, {
        timeout: 10_000,
      })
      .toBe(!was);

    // Restore the previous state — the kill-switch must not leak across tests.
    await card.locator('[data-testid="mcp-enabled"]').click();
    await expect
      .poll(async () => (await api(page, adminTokens, 'GET', '/api/mcp/status')).data?.status?.enabled, {
        timeout: 10_000,
      })
      .toBe(was);
  });

  test('15.4 — a token is created once, listed by prefix, then revoked', async ({ page }) => {
    await openSettings(page, adminTokens);
    await openAiCategory(page);
    const card = mcpCard(page);
    await expect(card).toBeVisible({ timeout: 15_000 });

    const name = `e2e-${Date.now().toString(36)}`;
    await card.locator('#mcp-token-name').fill(name);
    await card.getByRole('button', { name: 'Create a token' }).click();

    // The plaintext value is shown exactly once…
    await expect(card.getByText('hv_mcp_').first()).toBeVisible({ timeout: 10_000 });
    const shown = (await card.getByText('hv_mcp_').first().innerText()).trim();
    expect(shown.startsWith('hv_mcp_')).toBe(true);

    // …then only the prefix row remains, and revoking removes it.
    const row = card.locator('[data-testid="mcp-token-row"]', { hasText: name });
    await expect(row).toBeVisible({ timeout: 10_000 });
    await row.getByRole('button', { name: 'Revoke this token' }).click();
    await expect(row).toHaveCount(0, { timeout: 10_000 });
  });

  test('15.5 — a non-admin never sees the card; /mcp needs a bearer', async ({ page, playwright }) => {
    // A throwaway reader account, deleted at the end of the test.
    const uname = `mcpreader${Date.now().toString(36)}`;
    const created = await rest('POST', '/api/users', {
      username: uname,
      password: 'ReaderSecret123',
      fullname: 'MCP Reader',
      email: 'reader@example.com',
      role_id: 1,
    });
    if (!created.success) test.skip(true, 'could not create a reader account');
    try {
      const ctx = await newCtx(playwright);
      const { token, csrf } = await apiLoginAsViewer(ctx, uname, 'ReaderSecret123');
      await ctx.dispose();
      if (!token) test.skip(true, 'could not log in as the reader account');

      // `apiLoginAsViewer` already replaced the admin-chosen password, so the
      // session it returns only authenticates with the new one.
      const reader: Tokens = { username: uname, password: 'Viewer67890!', token, csrf };
      await openSettings(page, reader);
      await openAiCategory(page);
      // The AI card is there for everyone; the gateway card is admin-only.
      await expect(set(page).locator('#aiModel')).toBeVisible({ timeout: 15_000 });
      await expect(mcpCard(page)).toHaveCount(0);

      // A bearer-less agent call is rejected — no session, no token, no entry.
      const anon = await sharedCtx.post('/mcp', {
        data: { jsonrpc: '2.0', id: 1, method: 'ping', params: {} },
      });
      expect(anon.status()).toBe(401);
    } finally {
      await rest('DELETE', `/api/users/${uname}`);
    }
  });

  test('15.6 — no token secret ever lands in localStorage or the URL', async ({ page }) => {
    await openSettings(page, adminTokens);
    await openAiCategory(page);
    const card = mcpCard(page);
    await expect(card).toBeVisible({ timeout: 15_000 });
    const leaked = await page.evaluate(() => {
      const dump = JSON.stringify({ ...localStorage });
      return dump.includes('hv_mcp_') || location.href.includes('hv_mcp_');
    });
    expect(leaked).toBe(false);
  });

  test('15.7 — the Chat app no longer hosts the gateway panel', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    const card = page.locator('.lp-card[title="AI Assistant"]');
    await card.waitFor({ state: 'visible', timeout: 10_000 });
    await card.click();
    await page.waitForSelector('[data-testid="chat-page"]', { timeout: 10_000 });

    const chat = page.locator('[data-testid="chat-page"]');
    // Neither the toggle nor the old side panel survives in the chat window.
    await expect(chat.getByRole('button', { name: 'External agents' })).toHaveCount(0);
    await expect(chat.locator('[data-testid="mcp-panel"]')).toHaveCount(0);
    await expect(chat.locator('[data-testid="mcp-enabled"]')).toHaveCount(0);
  });
});
