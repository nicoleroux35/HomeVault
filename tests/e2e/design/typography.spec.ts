/* =============================================================================
 * File          : typography.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 18/10/2026
 * Last modified : 18/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Cross-cutting typography guard (Playwright): every rendered text size in
 *   the product must resolve to a step of the native desktop text ladder, and
 *   each recurring role must sit on the step that role owns. Runs over the
 *   login screen and four desktop apps, on the live DOM (computed styles), so
 *   a drifted token, a stray `text-[13px]` or a mis-sized role fails here
 *   instead of being found by eye.
 *
 * Dependencies:
 *   - none (Playwright only)
 *
 * Usage:
 *   npx playwright test tests/e2e/design/typography.spec.ts
 *   BASE_URL=http://localhost:8080 npx playwright test tests/e2e/design/
 * =============================================================================
 */

/**
 * Typography — native text ladder conformance
 *
 * The product renders at the sizes a desktop application uses for the same
 * functions (see `docs/guides/design-system.md` §2.1). The ladder is
 * 11 / 12 / 13 / 15 / 17 / 20 / 22 / 26 / 34, plus three documented
 * non-reading steps: 62 (gauge display), 72 and 48 (gateway clock), 56 (the
 * wizard's sticky clock). 11px is a hard floor — nothing readable renders
 * smaller.
 *
 * Two guarantees are asserted:
 *   1. CONFORMANCE — no visible text node renders at a size outside the
 *      ladder (catches raw `text-[13px]`, a `text-md` that stopped resolving,
 *      a component that invented its own step).
 *   2. ROLE MAPPING — the recurring roles sit where the native scale puts
 *      them: field label / body 13, helper 11, card title 15, window title 15,
 *      metric 20, section eyebrow 11.
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * (defaults: e2eadmin / E2eSecret123, override with E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 *
 * Run:        npx playwright test tests/e2e/design/typography.spec.ts
 * Run single: npx playwright test tests/e2e/design/typography.spec.ts -g "1.1"
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

/** The native ladder, plus the documented decorative display steps. */
const LADDER = [11, 12, 13, 15, 17, 20, 22, 26, 34, 48, 56, 62, 72];
/** Nothing readable renders below this. */
const FLOOR = 11;

interface OffScale {
  size: number;
  tag: string;
  cls: string;
  text: string;
}

interface Tokens {
  token: string;
  csrf: string;
}

async function apiLogin(ctx: APIRequestContext, u: string, p: string): Promise<Tokens> {
  const res = await ctx.post('/api/auth/login', { data: { username: u, password: p } });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

/** Seed the session into localStorage, then land on the desktop. */
async function gotoDesktop(page: Page, tokens: Tokens, lang = 'en') {
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
  await page.goto('/desktop');
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page.locator('.hv-dock').waitFor({ state: 'visible', timeout: 10_000 });
  // The release-notes modal opens over the desktop on a freshly seeded
  // account and its overlay swallows pointer events — dismiss it first.
  const dismiss = page.locator('[data-testid="whats-new-dismiss"]');
  if (await dismiss.count()) await dismiss.first().click().catch(() => {});
  // Every desktop monitor widget defaults to visible and floats over the whole
  // screen; hide them so they never intercept the dock click.
  await page
    .addStyleTag({ content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}' })
    .catch(() => {});
}

/**
 * Collect every element that renders its own visible text at a size outside
 * the ladder. Walks the live DOM: only elements with a direct text node count
 * (so a wrapper inheriting its child's size is not reported twice), and
 * zero-size / hidden boxes are skipped.
 */
async function offScaleSizes(page: Page, root = 'body'): Promise<OffScale[]> {
  return page.evaluate(
    ({ ladder, scope }) => {
      const out: OffScale[] = [];
      const host = document.querySelector(scope) || document.body;
      for (const el of host.querySelectorAll('*')) {
        const hasText = Array.from(el.childNodes).some(
          (n) => n.nodeType === Node.TEXT_NODE && (n.textContent || '').trim(),
        );
        if (!hasText) continue;
        const cs = getComputedStyle(el);
        if (cs.display === 'none' || cs.visibility === 'hidden' || cs.opacity === '0') continue;
        const box = el.getBoundingClientRect();
        if (box.width < 2 || box.height < 2) continue;
        const size = Math.round(parseFloat(cs.fontSize) * 100) / 100;
        if (!ladder.includes(size)) {
          out.push({
            size,
            tag: el.tagName,
            cls: String(el.className).slice(0, 80),
            text: (el.textContent || '').trim().slice(0, 40),
          });
        }
      }
      return out;
    },
    { ladder: LADDER, scope: root },
  );
}

/** Assert the scope is fully on the ladder, with a readable failure. */
async function expectOnLadder(page: Page, where: string, root = 'body') {
  const bad = await offScaleSizes(page, root);
  const sample = bad
    .slice(0, 6)
    .map((b) => `  ${b.size}px <${b.tag}> "${b.text}" .${b.cls}`)
    .join('\n');
  expect(
    bad,
    `${where}: ${bad.length} element(s) render off the native text ladder ` +
      `[${LADDER.join(', ')}]:\n${sample}`,
  ).toEqual([]);
}

/** Computed size of the first match of a selector (throws when absent). */
async function sizeOf(page: Page, selector: string): Promise<number> {
  const el = page.locator(selector).first();
  await expect(el, `selector ${selector} must exist`).toHaveCount(1);
  return el.evaluate((n) => parseFloat(getComputedStyle(n).fontSize));
}

// ---------------------------------------------------------------------------
// 1. Login screen
// ---------------------------------------------------------------------------

test.describe('1 — Login screen', () => {
  test('1.1 — every text element sits on the native ladder', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('#username', { timeout: 20_000 });
    await expectOnLadder(page, 'login');
  });

  test('1.2 — roles land on their native step', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('#username', { timeout: 20_000 });

    // Field label is an uppercase eyebrow → caption step (11).
    expect(await sizeOf(page, 'label[for="username"]'), 'field label').toBe(11);
    // Control text and body prose → body step (13).
    expect(await sizeOf(page, '#username'), 'input text').toBe(13);
    expect(await sizeOf(page, '[data-testid="login-remember"] span'), 'prose').toBe(13);
    // Product name in the card header → title-3 (15).
    expect(await sizeOf(page, '[data-testid="login-brand-title"]'), 'brand title').toBe(15);
    // The clock is a decorative display element, not body text.
    expect(await sizeOf(page, '.text-t-clock'), 'clock').toBe(72);
  });

  test('1.3 — nothing readable renders below the 11px floor', async ({ page }) => {
    await page.goto('/');
    await page.waitForSelector('#username', { timeout: 20_000 });
    const tooSmall = await page.evaluate((floor) => {
      const out: string[] = [];
      for (const el of document.querySelectorAll('body *')) {
        const hasText = Array.from(el.childNodes).some(
          (n) => n.nodeType === Node.TEXT_NODE && (n.textContent || '').trim(),
        );
        if (!hasText) continue;
        const cs = getComputedStyle(el);
        if (cs.display === 'none' || cs.visibility === 'hidden') continue;
        const size = parseFloat(cs.fontSize);
        if (size > 0 && size < floor) out.push(`${size}px <${el.tagName}> "${(el.textContent || '').trim().slice(0, 30)}"`);
      }
      return out;
    }, FLOOR);
    expect(tooSmall, `text below the ${FLOOR}px floor:\n${tooSmall.join('\n')}`).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// 2. Desktop shell + apps
// ---------------------------------------------------------------------------

test.describe('2 — Desktop shell and apps', () => {
  let tokens: Tokens;

  test.beforeAll(async ({ playwright }) => {
    const ctx = await playwright.request.newContext({ baseURL: process.env.BASE_URL || 'http://localhost:8080' });
    tokens = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
    await ctx.dispose();
  });

  test.beforeEach(() => {
    test.skip(!tokens?.token, 'no admin session — seed the server (POST /api/auth/setup)');
  });

  test('2.1 — desktop shell (dock, topbar, menus) is on the ladder', async ({ page }) => {
    await gotoDesktop(page, tokens);
    await expectOnLadder(page, 'desktop shell');
  });

  test('2.2 — window title sits on the title-3 step (15px)', async ({ page }) => {
    await gotoDesktop(page, tokens);
    await page.locator('[data-testid="dock-settings"]').click();
    await page.locator('.hv-winhdr-title').first().waitFor({ state: 'visible', timeout: 15_000 });
    expect(await sizeOf(page, '.hv-winhdr-title'), 'window title').toBe(15);
  });

  for (const app of ['settings', 'users', 'dashboard', 'storage', 'network'] as const) {
    test(`2.3.${app} — the ${app} app renders only ladder sizes`, async ({ page }) => {
      await gotoDesktop(page, tokens);
      // Prefer the dock when the app has an entry (Settings does); otherwise
      // open through the shell's own `hv-open-app` event — users/storage/
      // network live in the launcher, and clicking a missing `dock-<id>` would
      // silently measure the desktop again.
      const dock = page.locator(`[data-testid="dock-${app}"]`);
      if (await dock.count()) {
        await dock.first().click();
      } else {
        await page.evaluate(
          (id) => window.dispatchEvent(new CustomEvent('hv-open-app', { detail: { id } })),
          app,
        );
      }
      // Settings (and the other `OWN_HEADER_APPS`) draw their own header, so
      // there is no `.hv-winhdr-title` to wait on — wait for the app's own root.
      const ready = app === 'settings'
        ? page.locator('[data-testid="settings-page"]')
        : page.locator('.hv-winhdr-title').first();
      await ready.waitFor({ state: 'visible', timeout: 15_000 });
      await page.waitForTimeout(2_000);
      await expectOnLadder(page, app);
    });
  }

  test('2.4 — Settings roles land on their native step', async ({ page }) => {
    await gotoDesktop(page, tokens);
    await page.locator('[data-testid="dock-settings"]').click();
    await page.locator('[data-testid="settings-nav-item"]').first().waitFor({ state: 'visible', timeout: 15_000 });
    await page.waitForTimeout(1_500);

    const scope = '[data-testid="settings-page"]';
    // Form field label → body step (13), never the 12px chrome step.
    expect(await sizeOf(page, `${scope} label`), 'settings field label').toBe(13);
    // Panel header title → title-3 (15).
    expect(await sizeOf(page, '[data-testid="settings-header-title"]'), 'panel title').toBe(15);
    // Section eyebrow is uppercase → caption step (11). It is the element that
    // carries the `settings-card-title` hook (see `settings/vocabulary.ts`).
    const eyebrow = page.locator(`${scope} [data-testid="settings-card-title"]`).first();
    await expect(eyebrow).toHaveCount(1);
    const s = await eyebrow.evaluate((n) => ({
      size: parseFloat(getComputedStyle(n).fontSize),
      transform: getComputedStyle(n).textTransform,
    }));
    expect(s.transform, 'section eyebrow is uppercased').toBe('uppercase');
    expect(s.size, 'section eyebrow').toBe(11);
  });
});
