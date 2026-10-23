/* =============================================================================
 * File          : knowledge-base.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 22/10/2026 by Nicolas Le Roux
 * Last modified : 22/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings → AI Assistant → Knowledge Base (RAG) end-to-end tests
 *   (Playwright): the index status card, the embedding-model gate on the
 *   reindex action, the admin-only reindex API, and the empty-knowledge-base
 *   notice surfaced in the Chat app.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Knowledge base (RAG) — Settings → AI Assistant
 *
 * The `/api/rag/*` endpoints existed and answered since the base was built,
 * but nothing in the product called them: the index stayed empty and the
 * assistant answered "I don't know" for shipped features (ROADMAP 4.1). The
 * `KnowledgeBaseCard` is now the surface — status, embedding-model install,
 * reindex — and the Chat app points at it when the index is empty.
 *
 * These tests never require Ollama: they assert the card's state for whatever
 * model set the host reports, so they hold whether or not `nomic-embed-text`
 * is installed.
 *
 * Run (all):     npx playwright test tests/e2e/settings/knowledge-base.spec.ts --project=firefox
 * Run (single):  npx playwright test tests/e2e/settings/knowledge-base.spec.ts -g "16.1"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/settings/knowledge-base.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
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
 * A non-admin session that can actually use the API (`POST /api/users`
 * creates the account with an admin-chosen password, so the account's owner
 * must set their own before any call goes through).
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
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
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
  await page
    .locator('[data-testid="settings-nav-item"]')
    .first()
    .waitFor({ state: 'visible', timeout: 15_000 });
  return page.locator('[data-testid="settings-page"]');
}

function set(page: Page) {
  return page.locator('[data-testid="settings-page"]');
}

async function openAiCategory(page: Page) {
  const item = set(page).locator('[data-testid="settings-nav-item"]').filter({ hasText: 'AI Assistant' });
  await item.click();
  await expect(item).toHaveClass(/set-nav-active/);
  await page.waitForTimeout(120);
}

/**
 * The knowledge-base card, scoped inside the Settings window.
 *
 * Scoped by its own `kb-summary` descendant rather than by text: the MCP card
 * sits in the same category and its permissions copy also says "knowledge
 * base", so a text filter would resolve to two cards (Playwright strict mode).
 */
function kbCard(page: Page) {
  return set(page)
    .locator('[data-testid="settings-card"]')
    .filter({ has: page.locator('[data-testid="kb-summary"]') });
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

async function rest(method: string, path: string, body?: unknown) {
  const r = await sharedCtx.fetch(path, {
    method,
    headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    data: body as any,
  });
  const j = await r.json().catch(() => ({}));
  return { http: r.status(), success: j?.success, error: j?.error || j?.message || '', data: j?.data };
}

/** Whether the host currently reports the knowledge base's embedding model. */
async function embeddingInstalledViaApi(page: Page): Promise<boolean> {
  const status = await api(page, adminTokens, 'GET', '/api/rag/status');
  const model = status.data?.embedding_model as string | undefined;
  if (!model) return false;
  const models = await api(page, adminTokens, 'GET', '/api/chat/models');
  return (models.data?.models ?? []).some((m: any) => m.name === model);
}

// ===========================================================================
// 16 — Knowledge base (RAG) in Settings → AI Assistant
// ===========================================================================

test.describe('16 — Knowledge base (RAG) in Settings → AI Assistant', () => {
  test('16.1 — the AI Assistant category carries the Knowledge Base card', async ({ page }) => {
    await openSettings(page, adminTokens);
    await openAiCategory(page);
    const card = kbCard(page);
    await expect(card).toBeVisible({ timeout: 15_000 });

    // Status summary (documents + chunks) and the state badge are on the card.
    await expect(card.locator('[data-testid="kb-summary"]')).toBeVisible();
    await expect(card.locator('[data-testid="kb-state"]')).toBeVisible();

    // It belongs to the AI Assistant group, not a category of its own.
    await expect(set(page).locator('[data-testid="settings-header-title"]')).toHaveText('AI Assistant');
  });

  test('16.2 — the reindex action follows the embedding-model state', async ({ page }) => {
    await openSettings(page, adminTokens);
    await openAiCategory(page);
    const card = kbCard(page);
    await expect(card).toBeVisible({ timeout: 15_000 });

    const installed = await embeddingInstalledViaApi(page);
    const reindex = card.locator('[data-testid="kb-reindex"]');
    await expect(reindex).toBeVisible();

    if (installed) {
      // Model present: the index can be built, so the action is available.
      await expect(card.locator('[data-testid="kb-embedding-installed"]')).toBeVisible();
      await expect(reindex).toBeEnabled();
    } else {
      // Model absent: the card offers to install it and refuses to index,
      // explaining why — never a silent no-op.
      await expect(card.locator('[data-testid="kb-install-embedding"]')).toBeVisible();
      await expect(reindex).toBeDisabled();
    }
  });

  test('16.3 — the reindex API is admin-only; the status is session-level', async ({ page, playwright }) => {
    // Status is readable by any signed-in user (it drives the Chat hint).
    await gotoDesktop(page, adminTokens);
    const status = await api(page, adminTokens, 'GET', '/api/rag/status');
    expect(status.success).toBe(true);
    expect(status.data?.embedding_model).toBeTruthy();

    // A reader account can read the status but not trigger a reindex.
    const uname = `kbreader${Date.now().toString(36)}`;
    const created = await rest('POST', '/api/users', {
      username: uname,
      password: 'ReaderSecret123',
      fullname: 'KB Reader',
      email: 'kbreader@example.com',
      role_id: 1,
    });
    if (!created.success) test.skip(true, 'could not create a reader account');
    try {
      const ctx = await newCtx(playwright);
      const { token, csrf } = await apiLoginAsViewer(ctx, uname, 'ReaderSecret123');
      await ctx.dispose();
      if (!token) test.skip(true, 'could not log in as the reader account');

      const reader: Tokens = { username: uname, password: 'Viewer67890!', token, csrf };
      await gotoDesktop(page, reader);

      const readerStatus = await api(page, reader, 'GET', '/api/rag/status');
      expect(readerStatus.success).toBe(true);

      const readerIndex = await api(page, reader, 'POST', '/api/rag/index', {});
      expect(readerIndex.success).toBe(false);
      expect(String(readerIndex.error).toLowerCase()).toContain('admin');
    } finally {
      await rest('DELETE', `/api/users/${uname}`);
    }
  });

  test('16.4 — an empty knowledge base is surfaced in the Chat app', async ({ page }) => {
    await gotoDesktop(page, adminTokens);

    // The notice is the honest alternative to answering "I don't know": it
    // only exists while the index is genuinely empty.
    const status = await api(page, adminTokens, 'GET', '/api/rag/status');
    if ((status.data?.total_documents ?? 0) > 0) {
      test.skip(true, 'the knowledge base is already indexed on this server');
    }

    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    const launcherCard = page.locator('.lp-card[title="AI Assistant"]');
    await launcherCard.waitFor({ state: 'visible', timeout: 10_000 });
    await launcherCard.click();
    await page.waitForSelector('[data-testid="chat-page"]', { timeout: 10_000 });

    const chat = page.locator('[data-testid="chat-page"]');
    await expect(chat.locator('[data-testid="chat-kb-notice"]')).toBeVisible({ timeout: 10_000 });
    // The admin is offered the action, not just told about the state.
    await expect(chat.locator('[data-testid="chat-kb-index"]')).toBeVisible();
  });
});
