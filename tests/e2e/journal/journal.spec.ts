/**
 * System Journal app — End-to-End Test Suite
 *
 * The System Journal is a window opened from the desktop's Applications
 * launcher (id `journal`, "System Journal" tile). It aggregates HomeVault's
 * own rotating application log (source `app`), the systemd journal
 * (`system`, via `sudo journalctl`) and a few `/var/log` text files, with
 * source / unit / priority / free-text filters, an auto-cleanup preference,
 * and a destructive "Clear" of the `app` logs.
 *
 * **This suite never clears the real application logs and never leaves the
 * auto-cleanup preference changed** — the Clear flow is only exercised as
 * far as opening + cancelling its confirmation modal, and the cleanup
 * modal reads the current value and restores it if a test changes it.
 * Everything else (source list, entry list, filters) is read-only and
 * safe to drive for real. Backend auth guards are checked by direct
 * `fetch`, mirroring `tests/journal_integration.rs`.
 *
 * Run (all):    npx playwright test tests/e2e/journal/journal.spec.ts --project=firefox
 * Run (single): npx playwright test tests/e2e/journal/journal.spec.ts -g "4.1"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/journal/journal.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults: e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
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
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

async function openJournal(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[title="System Journal"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="journal-page"]', { timeout: 15_000 });
  // The first entry list resolves (reading the `app` log file is fast).
  await expect(page.locator('[data-testid="journal-page"] [data-testid="journal-loading"]')).toHaveCount(0, { timeout: 20_000 });
}

/** `[data-testid="journal-page"]` is unique to Journal.svelte (confirmed via grep). */
function win(page: Page) {
  return page.locator('[data-testid="journal-page"]');
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
  if (heapMB > 10) {
    // eslint-disable-next-line no-console
    console.log(`[MEM] ${testInfo.title}: ${heapMB.toFixed(1)} MB heap`);
  }
});

// ===========================================================================
// 1. Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — Journal window opens from the launcher', async ({ page }) => {
    await openJournal(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — header shows the title only (standardized header)', async ({ page }) => {
    await openJournal(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title')).not.toBeEmpty();
    // `.hv-winhdr-subtitle` no longer exists (the prop was removed from
    // `WindowHeader` on 2026-09-14), so this locator matched nothing.
    // Assert the bar's real shape instead: the icon tile + the title.
    await expect(win(page).locator('.hv-winhdr > *')).toHaveCount(2);
  });

  test('1.3 — the toolbar renders the source select and the search field', async ({ page }) => {
    await openJournal(page, adminTokens);
    await expect(win(page).locator('#jr-source')).toBeVisible();
    await expect(win(page).locator('#jr-search')).toBeVisible();
    await expect(win(page).locator('[data-testid="journal-apply"]')).toBeVisible();
  });

  test('1.4 — the entry list resolves to entries or a clean empty state', async ({ page }) => {
    await openJournal(page, adminTokens);
    const list = win(page).locator('[data-testid="journal-list"]');
    await expect(list).toBeVisible();
    const entries = await list.locator('[data-testid="journal-entry"]').count();
    const empty = await list.locator('[data-testid="journal-empty"]').count();
    expect(entries + empty).toBeGreaterThan(0);
  });

  test('1.5 — the footer shows an entry count', async ({ page }) => {
    await openJournal(page, adminTokens);
    await expect(win(page).locator('[data-testid="journal-foot"]')).toHaveText(/\d+/);
  });

  test('1.6 — auto-refresh sits in the toolbar with the filters, never in the header bar', async ({
    page,
  }) => {
    await openJournal(page, adminTokens);
    // The bar is [APP ICON][WINDOW TITLE] + the window-control trio owned by the
    // frame: the page puts no control of its own inside `.hv-winhdr`.
    await expect(win(page).locator('.hv-winhdr button, .hv-winhdr input')).toHaveCount(0);
    await expect(win(page).locator('.hv-winhdr [data-testid="journal-autorefresh"]')).toHaveCount(0);

    const toggle = win(page).locator('.hv-wintoolbar [data-testid="journal-autorefresh"] input');
    await expect(toggle).toBeVisible();
    await expect(toggle).not.toBeChecked();
    await toggle.click();
    await expect(toggle).toBeChecked();
  });
});

// ===========================================================================
// 2. Source selector
// ===========================================================================

test.describe('2 — Source selector', () => {
  test('2.1 — the source select lists at least "app" and "system"', async ({ page }) => {
    await openJournal(page, adminTokens);
    const values = await win(page).locator('#jr-source option').evaluateAll((opts) =>
      opts.map((o) => (o as HTMLOptionElement).value),
    );
    expect(values).toContain('app');
    expect(values).toContain('system');
  });

  test('2.2 — switching to "system" reveals the unit + priority filters', async ({ page }) => {
    await openJournal(page, adminTokens);
    await expect(win(page).locator('#jr-unit')).toHaveCount(0);
    await win(page).locator('#jr-source').selectOption('system');
    await expect(win(page).locator('#jr-unit')).toBeVisible();
    await expect(win(page).locator('#jr-prio')).toBeVisible();
  });

  test('2.3 — switching back to "app" hides the unit + priority filters', async ({ page }) => {
    await openJournal(page, adminTokens);
    await win(page).locator('#jr-source').selectOption('system');
    await expect(win(page).locator('#jr-unit')).toBeVisible();
    await win(page).locator('#jr-source').selectOption('app');
    await expect(win(page).locator('#jr-unit')).toHaveCount(0);
  });
});

// ===========================================================================
// 3. Filters
// ===========================================================================

test.describe('3 — Filters', () => {
  test('3.1 — Apply re-queries the entries endpoint with the search term', async ({ page }) => {
    await openJournal(page, adminTokens);
    let queried = '';
    page.on('request', (r) => {
      const m = r.url().match(/\/api\/journal\/entries\?([^#]*)/);
      if (m) queried = m[1];
    });
    await win(page).locator('#jr-search').fill('homevault');
    await win(page).locator('[data-testid="journal-apply"]').click();
    await expect.poll(() => queried, { timeout: 8000 }).toContain('search=homevault');
  });

  test('3.2 — Enter in the search box triggers a query', async ({ page }) => {
    await openJournal(page, adminTokens);
    let hit = false;
    page.on('request', (r) => {
      if (/\/api\/journal\/entries\?.*search=abc/.test(r.url())) hit = true;
    });
    await win(page).locator('#jr-search').fill('abc');
    await win(page).locator('#jr-search').press('Enter');
    await expect.poll(() => hit, { timeout: 8000 }).toBe(true);
  });
});

// ===========================================================================
// 4. Clear flow (never actually clears)
// ===========================================================================

test.describe('4 — Clear flow', () => {
  test('4.1 — the Clear button opens a confirmation modal that can be cancelled', async ({ page }) => {
    await openJournal(page, adminTokens);
    await page.locator('[data-testid="journal-clear"]').click();
    // The ui/dialog primitive's own `data-slot` — the window frame also
    // carries role="dialog", so match the modal root, not the role.
    const modal = page.locator('[data-slot="dialog-content"]');
    await expect(modal).toBeVisible({ timeout: 5000 });
    // Cancel — do NOT confirm (that wipes the real app logs).
    await modal.getByRole('button', { name: /cancel|annuler/i }).click();
    await expect(modal).toHaveCount(0, { timeout: 5000 });
  });

  test('4.2 — DELETE /api/journal/clear is admin-gated (checked without a token)', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/journal/clear', { method: 'DELETE' });
      return r.status;
    });
    expect(status).not.toBe(200); // no token → 400
  });
});

// ===========================================================================
// 5. Cleanup settings (reads current, restores any change)
// ===========================================================================

test.describe('5 — Cleanup settings', () => {
  test('5.1 — the settings modal shows the enable toggle + interval field', async ({ page }) => {
    await openJournal(page, adminTokens);
    await page.locator('[data-testid="journal-settings"]').click();
    await expect(page.locator('[data-testid="journal-cleanup-toggle"]')).toBeVisible({ timeout: 5000 });
    await expect(page.locator('#jr-cleanup-days')).toBeVisible();
    // Close without saving.
    await page.locator('[data-testid="journal-cleanup-toggle"]').press('Escape').catch(() => {});
    await page.keyboard.press('Escape');
  });

  test('5.2 — the interval field enforces a minimum of 1', async ({ page }) => {
    await openJournal(page, adminTokens);
    await page.locator('[data-testid="journal-settings"]').click();
    const field = page.locator('#jr-cleanup-days');
    await expect(field).toHaveAttribute('min', '1');
    await page.keyboard.press('Escape');
  });
});

// ===========================================================================
// 6. Navigation
// ===========================================================================

test.describe('6 — Navigation', () => {
  test('6.1 — the window can be closed and reopened', async ({ page }) => {
    await openJournal(page, adminTokens);
    await page.locator('.hv-cl-btn-close').first().click();
    await expect(win(page)).toHaveCount(0, { timeout: 10_000 });
    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    await page.locator('.lp-card[title="System Journal"]').click();
    await expect(win(page)).toBeVisible();
  });
});

// ===========================================================================
// 7. Responsive
// ===========================================================================

test.describe('7 — Responsive', () => {
  for (const [w, h] of [
    [360, 740],
    [768, 1024],
    [1920, 1080],
  ] as const) {
    test(`7.${w} — no horizontal overflow at ${w}px`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: h });
      await openJournal(page, adminTokens);
      const overflow = await win(page).evaluate((el) => el.scrollWidth - el.clientWidth);
      expect(overflow).toBeLessThanOrEqual(2);
    });
  }
});

// ===========================================================================
// 8. Accessibility
// ===========================================================================

test.describe('8 — Accessibility', () => {
  test('8.1 — the source select and search input have associated labels', async ({ page }) => {
    await openJournal(page, adminTokens);
    for (const id of ['jr-source', 'jr-search']) {
      const hasLabel = await page.locator(`label[for="${id}"]`).count();
      expect(hasLabel, id).toBeGreaterThan(0);
    }
  });

  test('8.2 — no duplicate id attributes in the Journal window', async ({ page }) => {
    await openJournal(page, adminTokens);
    const dupes = await win(page).evaluate((root) => {
      const ids = [...root.querySelectorAll('[id]')].map((e) => e.id);
      return ids.filter((v, i) => ids.indexOf(v) !== i);
    });
    expect(dupes).toEqual([]);
  });
});

// ===========================================================================
// 9. Console & network
// ===========================================================================

test.describe('9 — Console & network', () => {
  test('9.1 — no uncaught console errors on load', async ({ page }) => {
    const errors: string[] = [];
    page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
    page.on('pageerror', (e) => errors.push(String(e)));
    await openJournal(page, adminTokens);
    await page.waitForTimeout(800);
    const real = errors.filter((e) => !/favicon|ResizeObserver|Failed to load resource/i.test(e));
    expect(real, real.join('\n')).toEqual([]);
  });

  test('9.2 — load calls only the three journal endpoints', async ({ page }) => {
    const seen = new Set<string>();
    page.on('request', (r) => {
      const m = r.url().match(/\/api\/journal\/([a-z-]+)/);
      if (m) seen.add(m[1]);
    });
    await openJournal(page, adminTokens);
    const extras = [...seen].filter(
      (k) => !['sources', 'entries', 'cleanup-settings'].includes(k),
    );
    expect(extras, `unexpected: ${extras.join(', ')}`).toEqual([]);
  });
});

// ===========================================================================
// 10. Security
// ===========================================================================

test.describe('10 — Security', () => {
  test('10.1 — every journal endpoint rejects an unauthenticated request', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const codes = await page.evaluate(async () => {
      const gets = ['/api/journal/sources', '/api/journal/entries', '/api/journal/cleanup-settings'];
      const out: number[] = [];
      for (const u of gets) out.push((await fetch(u)).status);
      return out;
    });
    for (const c of codes) expect(c).not.toBe(200);
  });

  test('10.2 — a bogus token is rejected with 401 on the reads', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/journal/sources', { headers: { 'x-session-token': 'nope' } });
      return r.status;
    });
    expect(status).toBe(401);
  });

  test('10.3 — no token / secret ends up in the URL or localStorage', async ({ page }) => {
    await openJournal(page, adminTokens);
    expect(page.url()).not.toMatch(/token|password|secret/i);
    const dump = await page.evaluate(() => JSON.stringify(localStorage));
    expect(dump).not.toContain(ADMIN_PASS);
  });
});

// ===========================================================================
// 11. Edge cases
// ===========================================================================

test.describe('11 — Edge cases', () => {
  test('11.1 — rapidly switching sources keeps the list consistent', async ({ page }) => {
    await openJournal(page, adminTokens);
    const sel = win(page).locator('#jr-source');
    for (const s of ['system', 'app', 'system', 'app']) await sel.selectOption(s);
    await expect(win(page).locator('[data-testid="journal-list"]')).toBeVisible();
    await expect(win(page).locator('#jr-unit')).toHaveCount(0); // ended on "app"
  });

  test('11.2 — a "system" query on a host without passwordless sudo fails cleanly (no crash)', async ({ page }) => {
    await openJournal(page, adminTokens);
    await win(page).locator('#jr-source').selectOption('system');
    await win(page).locator('[data-testid="journal-apply"]').click();
    await page.waitForTimeout(1500);
    // Either entries load, or the empty / error state shows — never a blank crash.
    await expect(win(page).locator('[data-testid="journal-list"]')).toBeVisible();
    await expect(win(page)).toBeVisible();
  });
});

// ===========================================================================
// 12. Memory regression
// ===========================================================================

test.describe('12 — Memory regression', () => {
  test('12.1 — open/close the Journal 5× without DOM node accumulation', async ({ page }) => {
    test.setTimeout(150_000);
    await gotoDesktop(page, adminTokens);
    let baseline = 0;
    for (let i = 0; i < 5; i++) {
      await page.locator('.hv-dock-item[aria-label="Applications"]').click();
      await page.locator('.lp-card[title="System Journal"]').click();
      await page.waitForSelector('[data-testid="journal-page"]', { timeout: 15_000 });
      await page.locator('.hv-cl-btn-close').first().click();
      await expect(win(page)).toHaveCount(0, { timeout: 10_000 });
      const n = await page.evaluate(() => document.querySelectorAll('*').length);
      if (i === 0) baseline = n;
      else expect(n).toBeLessThan(baseline + 500);
    }
  });
});
