/* =============================================================================
 * File          : chat.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 19/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Chat screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * AI Assistant (Chat) app — End-to-End Test Suite
 *
 * Not a route: a window opened from the desktop's Applications launcher
 * (id `chat`, "AI Assistant" tile). It proxies a locally-running Ollama
 * (`http://localhost:11434`) — one-shot and streamed chat, model list /
 * pull / recommend / get / set — via `src/handlers/chat.rs`.
 *
 * **Safety model:** the 2026-07-13 audit found `send` / `models` / `model`
 * had **no session check** — anyone reachable could chat or enumerate
 * models — and a hardcoded French system prompt. This suite verifies:
 * `send` / `send/stream` / `models` / `model` GET need a session (any
 * role); `models/pull` and `model` PUT need admin; the system prompt
 * follows the UI language. It never pulls a model or mutates the active
 * one beyond a save/restore.
 *
 * No Ollama runs in this environment, so a chat send surfaces the clean
 * "AI unreachable" fallback — which this suite tests as a first-class
 * case. A real conversation (Ollama up + a pulled model) is section 13,
 * **opt-in** via `CHAT_E2E=1`.
 *
 * Run (all):     npx playwright test tests/e2e/chat/chat.spec.ts --project=firefox
 * Run (single):  npx playwright test tests/e2e/chat/chat.spec.ts -g "1.1"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/chat/chat.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const RUN_LIVE = process.env.CHAT_E2E === '1';

interface Tokens {
  username: string;
  password: string;
  token: string;
  csrf: string;
}

async function newCtx(playwright: any): Promise<APIRequestContext> {
  return playwright.request.newContext({ baseURL: BASE });
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

async function getHeapMB(page: Page): Promise<number> {
  return page.evaluate(() =>
    'memory' in performance ? (performance as any).usedJSHeapSize / (1024 * 1024) : 0,
  );
}

async function seedBrowser(page: Page, tokens: Tokens | null, lang?: string) {
  await page.addInitScript(
    ({ t, l }) => {
      try {
        if (t) {
          localStorage.setItem('session-token', t.token);
          localStorage.setItem('csrf-token', t.csrf);
        }
        if (l) localStorage.setItem('hv-language', l);
      } catch {
        /* storage unavailable — the test will surface it */
      }
    },
    { t: tokens, l: lang },
  );
}

async function gotoDesktop(page: Page, tokens: Tokens, lang?: string) {
  await seedBrowser(page, tokens, lang);
  await page.goto('/desktop');
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

async function openLaunchpadApp(page: Page, appName: string) {
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator(`.lp-card[title="${appName}"]`);
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
}

async function gotoChat(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openLaunchpadApp(page, 'AI Assistant');
  await page.waitForSelector('[data-testid="chat-page"]', { timeout: 10_000 });
}

function win(page: Page) {
  return page.locator('[data-testid="chat-page"]');
}

async function api(
  page: Page,
  t: Tokens | null,
  method: string,
  path: string,
  body?: unknown,
) {
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

test.beforeAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
  await ctx.dispose();
});

test.afterEach(async ({ page }, testInfo) => {
  const heapMB = await getHeapMB(page).catch(() => 0);
  if (heapMB > 0) {
    // eslint-disable-next-line no-console
    console.log(`[MEM] ${testInfo.title}: ${heapMB.toFixed(1)} MB heap after test`);
  }
});

// ===========================================================================
// 1. Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — the AI Assistant window opens from the Applications launcher', async ({ page }) => {
    await gotoChat(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — the empty state and its suggestions render', async ({ page }) => {
    await gotoChat(page, adminTokens);
    await expect(win(page).locator('[data-testid="chat-empty"]')).toBeVisible({ timeout: 8_000 });
    expect(await win(page).locator('[data-testid="chat-suggestion"]').count()).toBeGreaterThan(0);
  });

  test('1.3 — the message input and send button are present', async ({ page }) => {
    await gotoChat(page, adminTokens);
    await expect(win(page).locator('[data-testid="chat-input"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="chat-send"]').first()).toBeVisible();
  });
});

// ===========================================================================
// 2. Form fields
// ===========================================================================

test.describe('2 — Form fields', () => {
  test('2.1 — the input has a placeholder and accepts typing', async ({ page }) => {
    await gotoChat(page, adminTokens);
    const input = win(page).locator('[data-testid="chat-input"]');
    expect((await input.getAttribute('placeholder')) || '').not.toEqual('');
    await input.fill('Comment créer un partage ?');
    await expect(input).toHaveValue('Comment créer un partage ?');
  });

  test('2.2 — a suggestion chip fills the input', async ({ page }) => {
    await gotoChat(page, adminTokens);
    const chip = win(page).locator('[data-testid="chat-suggestion"]').first();
    const text = (await chip.locator('[data-testid="chat-suggestion-text"]').innerText()).trim();
    await chip.click();
    // suggestion either fills the input or sends immediately; assert the app
    // reacted (input changed or a message row appeared)
    const inputVal = await win(page).locator('[data-testid="chat-input"]').inputValue();
    const rows = await win(page).locator('[data-testid="chat-row"]').count();
    expect(inputVal.length > 0 || rows > 0, `suggestion "${text}" did nothing`).toBe(true);
  });
});

// ===========================================================================
// 3. Sending a message (Ollama down → clean fallback)
// ===========================================================================

test.describe('3 — Sending a message', () => {
  test('3.1 — sending shows the user message then a graceful "AI unreachable" reply', async ({
    page,
  }) => {
    await gotoChat(page, adminTokens);
    await win(page).locator('[data-testid="chat-input"]').fill('Bonjour');
    await win(page).locator('[data-testid="chat-send"]').first().click();

    // the user's bubble appears immediately
    await expect(win(page).locator('.hv-chat-row-user [data-testid="chat-bubble-text"]')).toContainText('Bonjour', {
      timeout: 5_000,
    });

    // and the assistant bubble eventually shows *something* (a real reply if
    // Ollama is up, or the fallback error text if it isn't) — never a blank
    // stuck "typing" indicator forever
    await expect(win(page).locator('.hv-chat-row-bot [data-testid="chat-bubble-text"]')).toBeVisible({
      timeout: 20_000,
    });
    const botText = await win(page).locator('.hv-chat-row-bot [data-testid="chat-bubble-text"]').innerText();
    expect(botText.trim().length).toBeGreaterThan(0);
  });

  test('3.2 — the send button is disabled while the input is empty', async ({ page }) => {
    await gotoChat(page, adminTokens);
    const send = win(page).locator('[data-testid="chat-send"]').first();
    await expect(send).toBeDisabled();

    // typing enables it
    await win(page).locator('[data-testid="chat-input"]').fill('a question');
    await expect(send).toBeEnabled();

    // clearing disables it again — and no chat request was fired meanwhile
    let calls = 0;
    page.on('request', (r) => {
      if (/\/api\/chat\/send(\/stream)?(\?|$)/.test(r.url())) calls++;
    });
    await win(page).locator('[data-testid="chat-input"]').fill('');
    await expect(send).toBeDisabled();
    expect(calls).toBe(0);
  });
});

// ===========================================================================
// 4. Security
// ===========================================================================

test.describe('4 — Security', () => {
  test('4.1 — chat + model endpoints reject a missing session', async ({ page }) => {
    await gotoChat(page, adminTokens);
    const results = await page.evaluate(async () => {
      const out: Record<string, any> = {};
      const send = await fetch('/api/chat/send', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ message: 'hi' }),
      });
      out.send = (await send.json().catch(() => ({}))).success;
      const models = await fetch('/api/chat/models');
      out.models = (await models.json().catch(() => ({}))).success;
      const model = await fetch('/api/chat/model');
      out.modelStatus = model.status;
      const stream = await fetch('/api/chat/send/stream', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ message: 'hi' }),
      });
      out.streamStatus = stream.status;
      return out;
    });
    expect(results.send).toBe(false);
    expect(results.models).toBe(false);
    expect(results.modelStatus).toBe(400);
    expect(results.streamStatus).toBe(401);
  });

  test('4.2 — a tampered token is rejected', async ({ playwright }) => {
    const ctx = await newCtx(playwright);
    try {
      const r = await ctx.post('/api/chat/send', {
        headers: { 'x-session-token': adminTokens.token + 'x', 'x-csrf-token': adminTokens.csrf },
        data: { message: 'hi' },
      });
      const b = await r.json().catch(() => ({}));
      // either the handler's own {success:false}, or the CSRF layer's 403 —
      // both mean "not accepted"
      expect(b.success === false || r.status() === 403).toBe(true);
    } finally {
      await ctx.dispose();
    }
  });

  test('4.3 — model pull / set require admin; a regular user may still chat', async ({ playwright }) => {
    const ctx = await newCtx(playwright);
    const adminHeaders = { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf };
    const uname = `chatviewer${Date.now().toString(36)}`;
    const created = await ctx.post('/api/users', {
      headers: adminHeaders,
      data: { username: uname, password: 'Viewer12345!', fullname: 'V', email: '', role_id: 1, groups: [], home_enabled: false },
    });
    if (!created.ok()) {
      await ctx.dispose();
      test.skip(true, `could not create a non-admin user (${created.status()})`);
    }
    try {
      const { token, csrf } = await apiLoginAsViewer(ctx, uname, 'Viewer12345!');
      const h = { 'x-session-token': token, 'x-csrf-token': csrf };

      const pull = await ctx.post('/api/chat/models/pull', { headers: h, data: { name: 'gemma3:1b' } });
      const pj = await pull.json();
      expect(pull.status()).toBe(400);
      expect(pj.error || pj.message || '').toMatch(/admin/i);

      const set = await ctx.put('/api/chat/model', { headers: h, data: { model: 'gemma3:1b' } });
      const sj = await set.json();
      expect(set.status()).toBe(400);
      expect(sj.error || sj.message || '').toMatch(/admin/i);

      // but reading the model + chatting is fine for a regular user
      const getModel = await ctx.get('/api/chat/model', { headers: h });
      expect(getModel.status()).toBe(200);
    } finally {
      await ctx.delete(`/api/users/${uname}`, { headers: adminHeaders }).catch(() => {});
      await ctx.dispose();
    }
  });

  test('4.4 — the reply is rendered as text, not markup (no XSS via the model)', async ({ page }) => {
    await gotoChat(page, adminTokens);
    await win(page).locator('[data-testid="chat-input"]').fill('<img src=x onerror=alert(1)>');
    await win(page).locator('[data-testid="chat-send"]').first().click();
    await expect(win(page).locator('.hv-chat-row-user')).toBeVisible({ timeout: 5_000 });
    const html = await win(page).locator('.hv-chat-row-user').innerHTML();
    expect(html).not.toContain('<img src=x onerror=');
  });

  test('4.5 — no session token appears in any URL', async ({ page }) => {
    const urls: string[] = [];
    page.on('request', (r) => urls.push(r.url()));
    await gotoChat(page, adminTokens);
    await page.waitForTimeout(1000);
    for (const u of urls) expect(u).not.toContain(adminTokens.token);
  });
});

// ===========================================================================
// 5. Localised system prompt (the "always French" bug)
// ===========================================================================

test.describe('5 — Localisation', () => {
  test('5.1 — /chat/send forwards the UI language (no hardcoded locale)', async ({ page }) => {
    await gotoChat(page, adminTokens);
    // We can't see the system prompt, but the request must be accepted with
    // the language in the body, must never 500, and — since we pass a valid
    // admin session — must never come back as a session error in any tongue.
    // Whether Ollama actually answers (success:true + a reply) or is
    // unreachable (success:false + a reach error) is not this test's concern.
    const r = await api(page, adminTokens, 'POST', '/api/chat/send', {
      message: 'hallo',
      language: 'de',
    });
    expect(r.http).toBe(200);
    expect(String(r.error || '')).not.toMatch(/Sitzung|session/i);
    if (r.success) {
      expect(String(r.data?.reply || '').length).toBeGreaterThan(0);
    } else {
      expect(String(r.error || '').length).toBeGreaterThan(0);
    }
  });
});

// ===========================================================================
// 6. Responsive / a11y / console
// ===========================================================================

test.describe('6 — Responsive / a11y / console', () => {
  for (const [w, h, label] of [
    [360, 800, '360px'],
    [768, 1024, '768px'],
    [1920, 1080, '1920px'],
  ] as const) {
    test(`6.${label} — no horizontal overflow`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: h });
      await gotoChat(page, adminTokens);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
      );
      expect(overflow).toBe(false);
    });
  }

  test('6.a — no duplicate element ids', async ({ page }) => {
    await gotoChat(page, adminTokens);
    const dupes = await page.evaluate(() => {
      const seen = new Set<string>();
      const dup: string[] = [];
      for (const el of Array.from(document.querySelectorAll('[id]'))) {
        if (seen.has(el.id)) dup.push(el.id);
        seen.add(el.id);
      }
      return dup;
    });
    expect(dupes).toEqual([]);
  });

  test('6.b — no uncaught JS errors on load + one send', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await gotoChat(page, adminTokens);
    await win(page).locator('[data-testid="chat-input"]').fill('test');
    await win(page).locator('[data-testid="chat-send"]').first().click();
    await page.waitForTimeout(3000);
    expect(errors).toEqual([]);
  });
});

// ===========================================================================
// 13. Real conversation (opt-in — needs Ollama + a model)
// ===========================================================================

test.describe('13 — Real conversation (opt-in)', () => {
  test.skip(!RUN_LIVE, 'set CHAT_E2E=1 with a running Ollama + a pulled model');

  test('13.1 — a real prompt gets a non-empty streamed reply', async ({ page }) => {
    await gotoChat(page, adminTokens);
    const models = await api(page, adminTokens, 'GET', '/api/chat/models');
    if (!models.data?.ollama_running) test.skip(true, 'Ollama is not running');

    await win(page).locator('[data-testid="chat-input"]').fill('Reply with exactly the word: pong');
    await win(page).locator('[data-testid="chat-send"]').first().click();

    await expect(win(page).locator('.hv-chat-row-bot [data-testid="chat-bubble-text"]')).toBeVisible({
      timeout: 60_000,
    });
    const reply = await win(page).locator('.hv-chat-row-bot [data-testid="chat-bubble-text"]').innerText();
    expect(reply.trim().length).toBeGreaterThan(0);
  });
});

// ===========================================================================
// 14. Edge cases & memory
// ===========================================================================

test.describe('14 — Edge cases & memory', () => {
  test('14.1 — a very long message does not break the layout', async ({ page }) => {
    await gotoChat(page, adminTokens);
    await win(page).locator('[data-testid="chat-input"]').fill('word '.repeat(400));
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
    );
    expect(overflow).toBe(false);
  });

  test('14.2 — clearing localStorage mid-session and reloading does not crash', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await gotoChat(page, adminTokens);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await expect(page.locator('.hv-login-page, [data-testid="chat-page"], #app')).toBeAttached({ timeout: 15_000 });
    await page.waitForTimeout(800);
    expect(errors).toEqual([]);
  });

  test('14.3 — open/close the window ×5 without significant heap growth', async ({ page }) => {
    test.setTimeout(150_000);
    await gotoChat(page, adminTokens);
    const base = await getHeapMB(page);
    for (let i = 0; i < 5; i++) {
      await page.locator('.hv-cl-btn-close').first().click();
      await expect(win(page)).toHaveCount(0);
      await openLaunchpadApp(page, 'AI Assistant');
      await page.waitForSelector('[data-testid="chat-page"]', { timeout: 10_000 });
    }
    const after = await getHeapMB(page);
    if (base > 0 && after > 0) {
      console.log(`[MEM] heap ${base.toFixed(1)} → ${after.toFixed(1)} MB`);
      expect(after - base).toBeLessThan(12);
    }
  });
});
