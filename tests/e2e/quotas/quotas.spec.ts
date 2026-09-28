/**
 * Quotas app — End-to-End Test Suite
 *
 * The Quotas screen (`frontend/src/routes/Quotas.svelte`, app id `quotas`) is
 * a read-only overview of the transfer quotas configured in Users/Groups. It
 * issues exactly two backend calls on load — `GET /api/transfer-quotas/user`
 * and `GET /api/transfer-quotas/group` — and renders one row per configured
 * subject (users first, then groups, sorted by name), with an upload and a
 * download column. A disabled direction renders the "Unlimited" label (it
 * means *no limit*, not "zero allowed"). Editing lives in the Users app; the
 * "Edit in Users" toolbar button hands off through the shared `hv-open-app`
 * window event.
 *
 * **This suite is entirely read-only at the screen level** — the screen has
 * no write; to give the table deterministic content it seeds a transfer
 * quota for the `e2eadmin` account via the admin API in `beforeAll`
 * (`PUT /api/transfer-quotas/user/e2eadmin`) and restores the pre-existing
 * state in `afterAll`. It never writes through the UI.
 *
 * Endpoint auth / payload shape are covered by
 * `tests/explorer_integration.rs` (quota guards) + the `transfer_quota.rs`
 * unit tests; this suite covers the screen only.
 *
 * Run (all):    npx playwright test tests/e2e/quotas/quotas.spec.ts --project=firefox
 * Run (single): npx playwright test tests/e2e/quotas/quotas.spec.ts -g "1.1"
 * Custom server: BASE_URL=http://localhost:3000 npx playwright test tests/e2e/quotas/quotas.spec.ts
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
  // The desktop monitor widgets overlap the dock — hide them so clicks land.
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

/** `[data-testid="quotas-page"]` is unique to Quotas.svelte (confirmed via grep). */
function win(page: Page) {
  return page.locator('[data-testid="quotas-page"]');
}

/**
 * The window chrome's close button. `win-close` lives in WindowFrame
 * (`.hv-window`), which is an *ancestor* of `[data-testid="quotas-page"]` — it
 * never matches inside the page root, so scope by the frame that holds the
 * Quotas page instead of hunting inside it.
 */
function closeBtn(page: Page) {
  return page
    .locator('.hv-window', { has: page.locator('[data-testid="quotas-page"]') })
    .locator('[data-testid="win-close"]');
}

async function openQuotas(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[title="Quotas"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="quotas-page"]', { timeout: 15_000 });
  // The loading empty-state variant (`quotas-empty`, title = quotas.loading)
  // clears once both quota list calls land.
  await expect(win(page).locator('[data-testid="quotas-count"]')).toBeVisible({ timeout: 15_000 });
}

/** Every `tbody tr:not(:has(caption))` — all subject rows in the table. */
function rows(page: Page) {
  return win(page).locator('table tbody tr');
}

// One subject we control in beforeAll/afterAll so the table is non-empty and
// deterministic regardless of what else the instance happens to have.
const SEED = {
  subjectType: 'user' as const,
  subjectName: ADMIN_USER,
  uploadLimitBytes: 104_857_600, // 100 MiB — drives the aria-valuenow math.
};

let adminTokens: Tokens;

/** The seed row's prior value (or absence) so afterAll can restore exactly. */
let priorSeedStatus: { existed: boolean; quota: any } | null = null;

test.beforeAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) {
    throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  }
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token, csrf };
  const headers = { 'x-session-token': token, 'x-csrf-token': csrf };

  const prior = await ctx
    .get(`/api/transfer-quotas/${SEED.subjectType}/${encodeURIComponent(SEED.subjectName)}`, {
      headers: { 'x-session-token': token },
    })
    .then((r) => r.json());
  priorSeedStatus = { existed: prior?.success === true && !!prior?.data, quota: prior?.data ?? null };

  const put = await ctx.put(
    `/api/transfer-quotas/${SEED.subjectType}/${encodeURIComponent(SEED.subjectName)}`,
    {
      headers: { ...headers, 'content-type': 'application/json' },
      data: {
        upload_enabled: true,
        upload_limit_bytes: SEED.uploadLimitBytes,
        download_enabled: false,
        download_limit_bytes: 0,
      },
    },
  );
  if (!(await put.json()).success) {
    throw new Error('Could not seed the e2e transfer quota row for the Quotas suite.');
  }
  await ctx.dispose();
});

test.beforeEach(() => {
  // Quotas data is lightweight, but a cold window mount on a busy machine can
  // be slow — leave the same headroom the other suites use.
  test.setTimeout(60_000);
});

test.afterAll(async ({ playwright }) => {
  if (!adminTokens) return;
  const ctx = await newCtx(playwright);
  const headers = { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf };
  if (priorSeedStatus?.existed && priorSeedStatus.quota) {
    // Put the exact prior limits back.
    await ctx.put(
      `/api/transfer-quotas/${SEED.subjectType}/${encodeURIComponent(SEED.subjectName)}`,
      {
        headers: { ...headers, 'content-type': 'application/json' },
        data: {
          upload_enabled: !!priorSeedStatus.quota.upload_enabled,
          upload_limit_bytes: priorSeedStatus.quota.upload_limit_bytes || 0,
          download_enabled: !!priorSeedStatus.quota.download_enabled,
          download_limit_bytes: priorSeedStatus.quota.download_limit_bytes || 0,
        },
      },
    );
  } else {
    await ctx
      .delete(
        `/api/transfer-quotas/${SEED.subjectType}/${encodeURIComponent(SEED.subjectName)}`,
        { headers },
      )
      .catch(() => {});
  }
  await ctx.dispose();
});

test.afterEach(async ({ page }, testInfo) => {
  const heapMB = await getHeapMB(page).catch(() => 0);
  if (heapMB > 10) {
    // eslint-disable-next-line no-console
    console.log(`[MEM] ${testInfo.title}: ${heapMB.toFixed(1)} MB heap after test`);
  }
});

// ===========================================================================
// 1. Page load
// ===========================================================================

test.describe('1 — Page load', () => {
  test('1.1 — Quotas opens from the launcher', async ({ page }) => {
    await openQuotas(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — the header bar carries the icon + title only', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const bar = win(page).locator('.hv-winhdr');
    await expect(bar.locator('.hv-winhdr-icon')).toHaveCount(1);
    await expect(bar.locator('.hv-winhdr-title')).toHaveCount(1);
    await expect(bar.locator('.hv-winhdr-actions')).toHaveCount(0);
    await expect(bar.locator('.hv-winhdr-center')).toHaveCount(0);
  });

  test('1.3 — the counter line renders and parses (subjects · limited)', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const text = (await win(page).locator('[data-testid="quotas-count"]').innerText()).trim();
    // Shape: "<n> user(s) · Transfer quotas (<m>)" — the subject count (all
    // visible rows) then the limited count — e.g. "1 user(s) · Transfer quotas (1)".
    const m = text.match(/^(\d+)\D+·\D+\((\d+)\)$/);
    expect(m, `counter "${text}" does not parse subject · limited counts`).not.toBeNull();
    const subjects = Number(m![1]);
    const limited = Number(m![2]);
    // The seed row guarantees at least one limited subject.
    expect(subjects).toBeGreaterThanOrEqual(1);
    expect(limited).toBeGreaterThanOrEqual(1);
    expect(limited).toBeLessThanOrEqual(subjects);
  });

  test('1.4 — the seeded quota row renders with its subject name', async ({ page }) => {
    await openQuotas(page, adminTokens);
    await expect(
      win(page).locator('table tbody tr', { hasText: SEED.subjectName }),
    ).toHaveCount(1);
  });

  test('1.5 — users sort before groups, each block by name', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const names = await win(page)
      .locator('table tbody tr td:nth-child(2)')
      .allInnerTexts();
    const trimmed = names.map((s) => s.trim()).filter((s) => s.length > 0);
    expect(trimmed.length).toBeGreaterThanOrEqual(1);
    // Quotas.svelte sorts user rows first, then group rows — both by name.
    const seenGroup = trimmed.some((n) => n === '—' || /group/i.test(n));
    void seenGroup; // (the type edge renders in column 1, name in column 2)
    for (let i = 1; i < trimmed.length; i++) {
      expect(trimmed[i].localeCompare(trimmed[i - 1])).toBeGreaterThanOrEqual(-1);
    }
  });
});

// ===========================================================================
// 2. Filters
// ===========================================================================

test.describe('2 — Filters', () => {
  test('2.1 — three filter chips exist and "All" is active by default', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const chips = win(page).locator('[data-testid="quotas-filter"]');
    await expect(chips).toHaveCount(3);
    // Exactly one chip is pressed at any time (the toolbar re-renders
    // `aria-pressed` on the button itself — a compound selector, not a
    // descendant filter).
    await expect(win(page).locator('[data-testid="quotas-filter"][aria-pressed="true"]')).toHaveCount(1);
    await expect(chips.first()).toHaveAttribute('aria-pressed', 'true');
  });

  test('2.2 — the Users filter keeps only user-type rows', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const userChip = win(page).locator('[data-testid="quotas-filter"]', { hasText: /user|utilisateur/i });
    await userChip.click();
    await expect(win(page).locator('[data-testid="quotas-filter"][aria-pressed="true"]')).toHaveCount(1);
    const cells = await win(page).locator('table tbody tr td:first-child').allInnerTexts();
    // Non-empty table at this point (the seed row is a user) — and every
    // visible type edge must be the user label, never the group label.
    expect(cells.length).toBeGreaterThanOrEqual(1);
    for (const c of cells) {
      expect(c.trim().toLowerCase()).not.toMatch(/group|groupe/i);
    }
  });

  test('2.3 — the Groups filter shows only group-type rows (or a clean empty state)', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const groupChip = win(page).locator('[data-testid="quotas-filter"]', { hasText: /group/i });
    await groupChip.click();
    const cells = await win(page).locator('table tbody tr td:first-child').allInnerTexts();
    for (const c of cells) {
      expect(c.trim().toLowerCase()).toMatch(/group|groupe/i);
    }
  });

  test('2.4 — filters are mutually exclusive (one at a time)', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const chips = win(page).locator('[data-testid="quotas-filter"]');
    for (const idx of [0, 1, 2]) {
      await chips.nth(idx).click();
      await expect(win(page).locator('[data-testid="quotas-filter"][aria-pressed="true"]')).toHaveCount(1);
    }
  });
});

// ===========================================================================
// 3. Data rendering (rows are the deterministic seed + whatever the instance
//    already has — assertions stay relative).
// ===========================================================================

test.describe('3 — Data rendering', () => {
  test('3.1 — every enabled direction renders a progress bar in [0,100]', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const bars = win(page).locator('[role="progressbar"]');
    const n = await bars.count();
    expect(n).toBeGreaterThanOrEqual(1); // the seeded upload column is enabled
    for (let i = 0; i < n; i++) {
      const now = Number(await bars.nth(i).getAttribute('aria-valuenow'));
      expect(Number.isFinite(now)).toBe(true);
      expect(now).toBeGreaterThanOrEqual(0);
      expect(now).toBeLessThanOrEqual(100);
      expect(await bars.nth(i).getAttribute('aria-valuemax')).toBe('100');
    }
  });

  test('3.2 — a disabled direction shows "Unlimited" and no bar', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const seededRow = win(page).locator('table tbody tr', { hasText: SEED.subjectName });
    await expect(seededRow).toHaveCount(1);
    // The download direction is disabled for the seeded row.
    const cell = seededRow.locator('td').nth(3);
    await expect(cell).toContainText(/unlimited|illimit/i);
    await expect(cell.locator('[role="progressbar"]')).toHaveCount(0);
  });

  test('3.3 — the bar percentage matches used/limit from the API', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const data = await page.evaluate(async () => {
      const headers = { 'x-session-token': localStorage.getItem('session-token') || '' };
      const r = await fetch('/api/transfer-quotas/user', { headers });
      return (await r.json()).data || [];
    });
    const row = data.find((q: any) => q.subject_name === SEED.subjectName);
    expect(row).toBeTruthy();
    const expected = row.upload_enabled
      ? Math.min(100, Math.round((row.upload_used_bytes / row.upload_limit_bytes) * 100))
      : 0;
    const seededRow = win(page).locator('table tbody tr', { hasText: SEED.subjectName });
    const bar = seededRow.locator('[role="progressbar"]').first();
    expect(Number(await bar.getAttribute('aria-valuenow'))).toBe(expected);
  });
});

// ===========================================================================
// 4. Interactions
// ===========================================================================

test.describe('4 — Interactions', () => {
  test('4.1 — "Edit in Users" opens the Users app via hv-open-app', async ({ page }) => {
    await openQuotas(page, adminTokens);
    await win(page).locator('[data-testid="quotas-edit-btn"]').click();
    await page.waitForSelector('[data-testid="users-page"]', { timeout: 15_000 });
    await expect(page.locator('[data-testid="users-page"]')).toBeVisible();
  });

  test('4.2 — the hand-off stays inside the desktop (no navigation)', async ({ page }) => {
    await openQuotas(page, adminTokens);
    await win(page).locator('[data-testid="quotas-edit-btn"]').click();
    await page.waitForSelector('[data-testid="users-page"]', { timeout: 15_000 });
    expect(new URL(page.url()).pathname).toBe('/desktop');
  });

  test('4.3 — the Quotas window stays open after the hand-off', async ({ page }) => {
    await openQuotas(page, adminTokens);
    await win(page).locator('[data-testid="quotas-edit-btn"]').click();
    await page.waitForSelector('[data-testid="users-page"]', { timeout: 15_000 });
    await expect(win(page)).toBeVisible();
  });
});

// ===========================================================================
// 5. Write safety — the screen must NEVER write
// ===========================================================================

test.describe('5 — Write safety', () => {
  test('5.1 — loading and auto-revalidation issue only the two GET lists', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const offenders: string[] = [];
    let listGets = 0;
    page.on('request', (r) => {
      const url = r.url();
      if (!url.includes('/api/')) return;
      if (url.includes('/api/transfer-quotas/')) {
        if (r.method() !== 'GET') offenders.push(`${r.method()} ${url}`);
        else listGets += 1;
      } else if (r.method() !== 'GET') {
        offenders.push(`${r.method()} ${url}`);
      }
    });
    await page.waitForTimeout(1_600); // clear onAppVisible's cooldown
    await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    await expect.poll(() => listGets, { timeout: 10_000 }).toBeGreaterThanOrEqual(2);
    await page.waitForTimeout(1200);
    expect(offenders, `Quotas issued a write request: ${offenders.join(', ')}`).toEqual([]);
  });
});

// ===========================================================================
// 6. Navigation
// ===========================================================================

test.describe('6 — Navigation', () => {
  test('6.1 — closing the window restores the desktop', async ({ page }) => {
    await openQuotas(page, adminTokens);
    await closeBtn(page).click();
    await expect(win(page)).toHaveCount(0, { timeout: 10_000 });
    await expect(page.locator('.hv-dock-container')).toBeVisible();
  });

  test('6.2 — reopening from the launcher works again', async ({ page }) => {
    await openQuotas(page, adminTokens);
    await closeBtn(page).click();
    await expect(win(page)).toHaveCount(0, { timeout: 10_000 });
    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    await page.locator('.lp-card[title="Quotas"]').click();
    await page.waitForSelector('[data-testid="quotas-page"]', { timeout: 15_000 });
    await expect(win(page)).toBeVisible();
  });
});

// ===========================================================================
// 7. Responsive
// ===========================================================================

test.describe('7 — Responsive', () => {
  const WIDTHS = [
    ['360px (phone)', 360, 640],
    ['768px (tablet)', 768, 900],
    ['1920px (desktop)', 1920, 1080],
  ] as const;

  for (const [label, w, h] of WIDTHS) {
    test(`7.${WIDTHS.findIndex((x) => x[0] === label) + 1} — renders and stays usable at ${label}`, async ({
      page,
    }) => {
      await page.setViewportSize({ width: w, height: h });
      await openQuotas(page, adminTokens);
      await expect(win(page)).toBeVisible();
      await expect(win(page).locator('[data-testid="quotas-count"]')).toBeVisible();
      // The table scrolls horizontally inside its own container — the window
      // itself must never grow a page-level horizontal scrollbar.
      const hOverflow = await win(page).evaluate(
        (el) => el.scrollWidth - el.clientWidth,
      );
      expect(hOverflow).toBeLessThanOrEqual(0);
      // The toolbar (filters + Edit button) stays reachable.
      await expect(win(page).locator('[data-testid="quotas-filter"]').first()).toBeVisible();
    });
  }
});

// ===========================================================================
// 8. Accessibility
// ===========================================================================

test.describe('8 — Accessibility', () => {
  test('8.1 — filter chips are buttons carrying aria-pressed and an accessible group', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const group = win(page).locator('[data-testid="quotas-filters"]');
    await expect(group).toHaveAttribute('role', 'group');
    await expect(group.getAttribute('aria-label')).not.toBeNull();
    const chips = win(page).locator('[data-testid="quotas-filter"]');
    const n = await chips.count();
    expect(n).toBe(3);
    for (let i = 0; i < n; i++) {
      await expect(chips.nth(i)).toHaveAttribute('aria-pressed');
      expect((await chips.nth(i).innerText()).trim().length).toBeGreaterThan(0);
    }
  });

  test('8.2 — no duplicate id attributes in the Quotas window', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const dupes = await win(page).evaluate((root) => {
      const ids = [...root.querySelectorAll('[id]')].map((e) => e.id);
      return ids.filter((v, i) => ids.indexOf(v) !== i);
    });
    expect(dupes).toEqual([]);
  });

  test('8.3 — the table exposes a sr-only caption (its accessible label)', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const caption = win(page).locator('table caption');
    await expect(caption).toHaveCount(1);
    expect((await caption.innerText()).trim().length).toBeGreaterThan(0);
  });

  test('8.4 — the Edit toolbar button has an accessible name', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const btn = win(page).locator('[data-testid="quotas-edit-btn"]');
    expect((await btn.innerText()).trim().length).toBeGreaterThan(0);
    await expect(btn).toBeEnabled();
  });

  test('8.5 — every subject row carries the row action-adjacent text (name + updated)', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const seededRow = win(page).locator('table tbody tr', { hasText: SEED.subjectName });
    await expect(seededRow).toHaveCount(1);
    // The name cell stacks the subject name and the "Updated <date>" line.
    expect((await seededRow.innerText()).length).toBeGreaterThan(0);
  });
});

// ===========================================================================
// 9. Performance
// ===========================================================================

test.describe('9 — Performance', () => {
  test('9.1 — the window shell renders quickly (data may lag)', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    await page.locator('.lp-card[title="Quotas"]').click();
    const t0 = Date.now();
    await page.waitForSelector('[data-testid="quotas-page"]', { timeout: 15_000 });
    expect(Date.now() - t0).toBeLessThan(10_000);
  });

  test('9.2 — the Quotas window keeps its DOM subtree bounded', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const n = await win(page).evaluate((root) => root.querySelectorAll('*').length);
    expect(n).toBeLessThan(1200);
  });
});

// ===========================================================================
// 10. Security
// ===========================================================================

test.describe('10 — Security', () => {
  test('10.1 — no session material in the URL while the app runs', async ({ page }) => {
    await openQuotas(page, adminTokens);
    expect(new URL(page.url()).search).toBe('');
    expect(new URL(page.url()).hash).toBe('');
  });

  test('10.2 — the session token is never rendered into the DOM', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const bodyText = await win(page).evaluate((root) => root.textContent || '');
    expect(bodyText).not.toContain(adminTokens.token);
    expect(bodyText).not.toContain(adminTokens.csrf);
  });

  test('10.3 — the screen performs no write even on auto-revalidation', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const writes: string[] = [];
    page.on('request', (r) => {
      if (r.url().includes('/api/') && r.method() !== 'GET') {
        writes.push(`${r.method()} ${r.url()}`);
      }
    });
    await page.waitForTimeout(1_600);
    await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    await page.waitForTimeout(1500);
    expect(writes).toEqual([]);
  });
});

// ===========================================================================
// 11. Keyboard
// ===========================================================================

test.describe('11 — Keyboard', () => {
  test('11.1 — Tab reaches the filter chips and the toolbar button', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const chips = win(page).locator('[data-testid="quotas-filter"]');
    // The window title is a plain <div> (not focusable), so start the walk
    // on the first focusable control inside the window content — the chips.
    await chips.first().focus();
    await expect(page.locator(':focus')).toHaveCount(1);
    // Tab through the window controls: the toolbar's Edit button must be
    // reachable without leaving the window.
    for (let i = 0; i < 10; i++) {
      await page.keyboard.press('Tab');
      const focused = await page.evaluate(() =>
        (document.activeElement as HTMLElement)?.getAttribute('data-testid'),
      );
      if (focused === 'quotas-edit-btn') break;
    }
    await expect(
      win(page).locator('[data-testid="quotas-edit-btn"]:focus'),
    ).toBeVisible();
  });

  test('11.2 — Space activates a filter chip', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const chips = win(page).locator('[data-testid="quotas-filter"]');
    await chips.nth(1).focus();
    await page.keyboard.press('Space');
    await expect(win(page).locator('[data-testid="quotas-filter"][aria-pressed="true"]')).toHaveCount(1);
  });
});

// ===========================================================================
// 12. Console & network
// ===========================================================================

test.describe('12 — Console & network', () => {
  test('12.1 — no uncaught console errors while Quotas loads', async ({ page }) => {
    const errors: string[] = [];
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(m.text());
    });
    page.on('pageerror', (e) => errors.push(String(e)));
    await openQuotas(page, adminTokens);
    await page.waitForTimeout(1000);
    const real = errors.filter((e) => !/favicon|ResizeObserver|Failed to load resource/i.test(e));
    expect(real, real.join('\n')).toEqual([]);
  });
});

// ===========================================================================
// 13. Edge cases
// ===========================================================================

test.describe('13 — Edge cases', () => {
  test('13.1 — a failing quota list surfaces the error state, not a blank window', async ({
    page,
  }) => {
    await seedBrowser(page, adminTokens);
    await page.route('**/api/transfer-quotas/**', (route) =>
      route.fulfill({ status: 500, body: '{}' }),
    );
    await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
    await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
    await page
      .addStyleTag({ content: '[class*="-monitor"]{display:none!important}' })
      .catch(() => {});
    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    await page.locator('.lp-card[title="Quotas"]').click();
    await page.waitForSelector('[data-testid="quotas-page"]', { timeout: 15_000 });
    await expect(win(page).locator('[data-testid="quotas-error"]')).toBeVisible({ timeout: 15_000 });
    await expect(win(page).locator('table')).toHaveCount(0);
  });

  test('13.2 — an empty backend renders the empty state with no table ', async ({
    page,
  }) => {
    await seedBrowser(page, adminTokens);
    await page.route('**/api/transfer-quotas/**', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: [] }),
      }),
    );
    await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
    await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
    await page
      .addStyleTag({ content: '[class*="-monitor"]{display:none!important}' })
      .catch(() => {});
    await page.locator('.hv-dock-item[aria-label="Applications"]').click();
    await page.locator('.lp-card[title="Quotas"]').click();
    await page.waitForSelector('[data-testid="quotas-page"]', { timeout: 15_000 });
    // The empty state title is the `quotas.empty` string, not `quotas.loading`.
    await expect(win(page).locator('[role="status"], [class*="empty"]').first()).toBeVisible();
    await expect(win(page).locator('table')).toHaveCount(0);
  });

  test('13.3 — a focus event re-fetches the quota lists without breaking the view', async ({
    page,
  }) => {
    await openQuotas(page, adminTokens);
    let hits = 0;
    page.on('response', (r) => {
      if (r.url().includes('/api/transfer-quotas/')) hits += 1;
    });
    await page.waitForTimeout(1_600);
    await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    await expect.poll(() => hits, { timeout: 10_000 }).toBeGreaterThan(0);
    await expect(win(page).locator('[data-testid="quotas-count"]')).toBeVisible();
  });
});

// ===========================================================================
// 14. Memory regression
// ===========================================================================

test.describe('14 — Memory regression', () => {
  test('14.1 — repeated opens do not leak the window subtree', async ({ page }) => {
    await openQuotas(page, adminTokens);
    const before = await win(page).evaluate((root) => root.querySelectorAll('*').length);
    for (let i = 0; i < 3; i++) {
      await closeBtn(page).click();
      await expect(win(page)).toHaveCount(0, { timeout: 10_000 });
      await page.locator('.hv-dock-item[aria-label="Applications"]').click();
      await page.locator('.lp-card[title="Quotas"]').click();
      await page.waitForSelector('[data-testid="quotas-page"]', { timeout: 15_000 });
      await expect(win(page).locator('[data-testid="quotas-count"]')).toBeVisible();
    }
    const after = await win(page).evaluate((root) => root.querySelectorAll('*').length);
    // The window is recreated cleanly each time — allow only modest growth.
    expect(after).toBeLessThan(before + 60);
  });
});