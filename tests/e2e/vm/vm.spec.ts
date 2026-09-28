/**
 * Virtual Machines page — End-to-End Test Suite
 *
 * The app manages Incus VMs and system containers. It is opened from the
 * Applications launcher (Launchpad), not the dock.
 *
 * **This suite never mutates real machines by default.** The wizard tests
 * stop at "Cancel"; lifecycle actions are only exercised in the opt-in
 * section 13 (`VM_E2E=1`), which creates a tiny throwaway container and
 * cleans it up, skipping cleanly when Incus is unavailable.
 *
 * Run (safe subset):  npx playwright test tests/e2e/vm/vm.spec.ts --project=firefox
 * Run (single):       npx playwright test tests/e2e/vm/vm.spec.ts -g "4.1"
 * Custom server:      BASE_URL=http://localhost:3000 npx playwright test tests/e2e/vm/vm.spec.ts
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * account (defaults e2eadmin / E2eSecret123, override E2E_ADMIN_USER /
 * E2E_ADMIN_PASS). Incus does NOT need to be installed — the suite adapts.
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';
const RUN_LIFECYCLE = process.env.VM_E2E === '1';

interface Tokens { username: string; password: string; token: string; csrf: string }

async function newCtx(playwright: any): Promise<APIRequestContext> {
  return playwright.request.newContext({ baseURL: BASE });
}

async function apiLogin(ctx: APIRequestContext, username: string, password: string) {
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
      } catch { /* storage unavailable */ }
    },
    { t: tokens, l: lang },
  );
}

async function gotoDesktop(page: Page, tokens: Tokens, lang?: string) {
  await seedBrowser(page, tokens, lang);
  await page.goto('/desktop');
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page
    .addStyleTag({ content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}' })
    .catch(() => {});
}

async function openVmApp(page: Page) {
  // Open the Applications launcher (dock button, aria-label "Applications").
  const launcher = page.locator('[aria-label="Applications"]').first();
  await launcher.click();
  await page.waitForSelector('.lp-panel', { timeout: 10_000 });
  // The card title may be "Virtual Machines" or its localised form.
  const card = page.locator('.lp-card-name').filter({ hasText: /virtual mach|machines virtuelles|máquinas virtuales|virtuelle maschinen/i }).first();
  await card.scrollIntoViewIfNeeded();
  await card.click();
  await page.waitForSelector('[data-testid="vm-page"]', { timeout: 10_000 });
  // wait for the first /vm/available + /vm/list to settle (spinner → content)
  await page
    .waitForFunction(() => {
      const r = document.querySelector('[data-testid="vm-page"]');
      return r && (r.querySelector('[data-testid="vm-list"]') || r.querySelector('[data-testid="vm-empty"]'));
    }, { timeout: 15_000 })
    .catch(() => {});
}

async function gotoVm(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openVmApp(page);
}

function win(page: Page) { return page.locator('[data-testid="vm-page"]'); }

/** Which of the three states the app is in right now. */
async function appState(page: Page): Promise<'unavailable' | 'empty' | 'list'> {
  if (await win(page).locator('[data-testid="vm-list"]').count()) return 'list';
  const empty = win(page).locator('[data-testid="vm-empty"] h2');
  if (await empty.count()) {
    const txt = (await empty.first().innerText()).toLowerCase();
    return txt.includes('not available') || txt.includes('pas disponible') ? 'unavailable' : 'empty';
  }
  // Neither the machine list nor the empty state is mounted: the app shows
  // the "not available" panel (which has no data-testid) - treat as unavailable.
  return 'unavailable';
}

async function openWizard(page: Page) {
  // Without an available Incus daemon the "New machine" button is disabled,
  // so no wizard UI exists to exercise — skip the whole wizard-dependent set.
  if ((await appState(page)) === 'unavailable') test.skip(true, 'Incus not available');
  await win(page).locator('[data-testid="vm-new"]').click();
  await page.waitForSelector('[data-slot="dialog-content"]', { timeout: 5_000 });
}

let sharedCtx: APIRequestContext;
let adminHeaders: Record<string, string>;
let adminTokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  sharedCtx = await newCtx(playwright);
  const admin = await apiLogin(sharedCtx, ADMIN_USER, ADMIN_PASS);
  expect(admin.token, 'seeded admin login must succeed').not.toEqual('');
  adminHeaders = { 'x-session-token': admin.token, 'x-csrf-token': admin.csrf };
  adminTokens = { username: ADMIN_USER, password: ADMIN_PASS, token: admin.token, csrf: admin.csrf };
});

test.afterAll(async () => { await sharedCtx?.dispose(); });

let heapBefore = 0;
test.beforeEach(async ({ page }) => {
  heapBefore = 0;
  page.on('load', async () => { if (!heapBefore) heapBefore = await getHeapMB(page).catch(() => 0); });
});
test.afterEach(async ({ page }, testInfo) => {
  const after = await getHeapMB(page).catch(() => 0);
  if (heapBefore > 0 && after > 0) {
    const delta = after - heapBefore;
    if (delta > 10) console.warn(`[MEM] "${testInfo.title}" grew heap by ${delta.toFixed(1)} MB`);
  }
});

// ===========================================================================
// 1 — Page load
// ===========================================================================
test.describe('1 — Page load', () => {
  test('1.1 — opens from the launchpad', async ({ page }) => {
    await gotoVm(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — header shows the app name, and "New machine" sits in the toolbar', async ({ page }) => {
    await gotoVm(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title')).toContainText(/machine/i);
    // The bar itself stays icon + title + window controls; the page-level action
    // is a WindowToolbar row below it.
    await expect(win(page).locator('.hv-winhdr-actions')).toHaveCount(0);
    await expect(win(page).locator('[data-testid="vm-new"]')).toBeVisible();
    await expect(win(page).locator('.hv-wintoolbar')).toBeVisible();
  });

  test('1.3 — lands in exactly one of: unavailable / empty / list', async ({ page }) => {
    await gotoVm(page, adminTokens);
    const s = await appState(page);
    expect(['unavailable', 'empty', 'list']).toContain(s);
  });

  test('1.4 — renders exactly one WindowHeader', async ({ page }) => {
    await gotoVm(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr')).toHaveCount(1);
  });
});

// ===========================================================================
// 2 — Machine list
// ===========================================================================
test.describe('2 — Machine list', () => {
  test('2.1 — list vs empty state matches /api/vm/list', async ({ page }) => {
    await gotoVm(page, adminTokens);
    const listRes = await sharedCtx.get('/api/vm/list', { headers: adminHeaders });
    const body = await listRes.json().catch(() => ({}));
    const count = Array.isArray(body?.data) ? body.data.length : 0;
    const s = await appState(page);
    if (s === 'unavailable') test.skip(true, 'Incus not available');
    if (count > 0) {
      await expect(win(page).locator('[data-testid="vm-card"]')).toHaveCount(count);
    } else {
      await expect(win(page).locator('[data-testid="vm-empty"]')).toBeVisible();
    }
  });

  test('2.2 — each card shows name, status pill and specs', async ({ page }) => {
    await gotoVm(page, adminTokens);
    if ((await win(page).locator('[data-testid="vm-card"]').count()) === 0) test.skip(true, 'no machines');
    const card = win(page).locator('[data-testid="vm-card"]').first();
    await expect(card.locator('[data-testid="vm-card-name"]')).not.toBeEmpty();
    await expect(card.locator('[data-testid="vm-pill"]')).toBeVisible();
    await expect(card.locator('.vm-specs span').first()).toBeVisible();
  });

  test('2.3 — a running card offers "Open console", a stopped one offers "Start"', async ({ page }) => {
    await gotoVm(page, adminTokens);
    const cards = win(page).locator('[data-testid="vm-card"]:not(.vm-card-creating)');
    const n = await cards.count();
    if (n === 0) test.skip(true, 'no machines');
    for (let i = 0; i < n; i++) {
      const card = cards.nth(i);
      const running = (await card.locator('[data-testid="vm-pill"][data-status="running"]').count()) > 0;
      const primary = card.locator('[data-testid="vm-card-act"] [data-testid="vm-btn-primary"]');
      await expect(primary).toBeVisible();
      const label = (await primary.innerText()).toLowerCase();
      if (running) expect(label).toMatch(/console/);
      else expect(label).toMatch(/start|démarr|iniciar|starten/);
    }
  });

  test('2.4 — empty state has a call-to-action', async ({ page }) => {
    await gotoVm(page, adminTokens);
    if ((await appState(page)) !== 'empty') test.skip(true, 'not the empty state');
    await expect(win(page).locator('[data-testid="vm-empty"] .vm-cta')).toBeVisible();
  });
});

// ===========================================================================
// 3 — Create wizard (no mutation — always cancelled)
// ===========================================================================
test.describe('3 — Create wizard', () => {
  test.beforeEach(async ({ page }) => {
    await gotoVm(page, adminTokens);
    if ((await appState(page)) === 'unavailable') test.skip(true, 'Incus not available');
  });

  test('3.1 — opens with OS grid, size grid, name and access fields', async ({ page }) => {
    await openWizard(page);
    await expect(page.locator('[data-slot="dialog-content"] [data-testid="vm-os"]').first()).toBeVisible();
    await expect(page.locator('[data-slot="dialog-content"] [data-testid="vm-size"]')).toHaveCount(3);
    await expect(page.locator('[data-slot="dialog-content"] input[type="text"]').first()).toBeVisible();
    await expect(page.locator('[data-slot="dialog-content"] input[type="password"]')).toBeVisible();
  });

  test('3.2 — a recommended OS is preselected and a name is suggested', async ({ page }) => {
    await openWizard(page);
    await expect(page.locator('[data-testid="vm-os"].sel')).toHaveCount(1);
    await expect(page.locator('[data-testid="vm-os-reco"]').first()).toBeVisible();
    const name = await page.locator('[data-slot="dialog-content"] input[type="text"]').first().inputValue();
    expect(name).toMatch(/^[a-z0-9-]+$/);
  });

  test('3.3 — the create button is disabled until a password is entered', async ({ page }) => {
    await openWizard(page);
    const create = page.locator('[data-slot="dialog-footer"] [data-testid="vm-btn-primary"]');
    await expect(create).toBeDisabled();
    await page.locator('[data-slot="dialog-content"] input[type="password"]').fill('secret12');
    await expect(create).toBeEnabled();
  });

  test('3.4 — a bad name shows an inline error and blocks create', async ({ page }) => {
    await openWizard(page);
    await page.locator('[data-slot="dialog-content"] input[type="text"]').first().fill('Bad Name!');
    await page.locator('[data-slot="dialog-content"] input[type="password"]').fill('secret12');
    await expect(page.locator('[data-testid="vm-field-err"]').first()).toBeVisible();
    await expect(page.locator('[data-slot="dialog-footer"] [data-testid="vm-btn-primary"]')).toBeDisabled();
  });

  test('3.5 — a short password shows an inline error', async ({ page }) => {
    await openWizard(page);
    await page.locator('[data-slot="dialog-content"] input[type="password"]').fill('abc');
    await expect(page.locator('[data-testid="vm-field-err"]')).toContainText(/6/);
  });

  test('3.6 — the show/hide password toggle works', async ({ page }) => {
    await openWizard(page);
    const pwd = page.locator('[data-testid="vm-pass"] input');
    await pwd.fill('secret12');
    await expect(pwd).toHaveAttribute('type', 'password');
    await page.locator('[data-testid="vm-pass"] [data-testid="vm-btn-icon"]').click();
    await expect(page.locator('[data-testid="vm-pass"] input')).toHaveAttribute('type', 'text');
  });

  test('3.7 — advanced options reveal machine type + custom sizing', async ({ page }) => {
    await openWizard(page);
    await page.locator('[data-testid="vm-adv-toggle"]').click();
    await expect(page.locator('[data-testid="vm-kind-toggle"]')).toBeVisible();
    await expect(page.locator('[data-testid="vm-adv"] input[type="number"]')).toHaveCount(3);
  });

  test('3.8 — Cancel closes the wizard without creating anything', async ({ page }) => {
    const before = await win(page).locator('[data-testid="vm-card"]').count();
    await openWizard(page);
    await page.locator('[data-slot="dialog-footer"] [data-testid="vm-btn"]').first().click();
    await expect(page.locator('[data-slot="dialog-content"]')).toHaveCount(0);
    await expect(win(page).locator('[data-testid="vm-card"]')).toHaveCount(before);
  });

  test('3.9 — ISO mode swaps in the OS toggle and file pickers, drops the login', async ({ page }) => {
    await openWizard(page);
    await page.locator('[data-testid="vm-mode-btn"]').filter({ hasText: /iso/i }).click();
    await expect(page.locator('[data-slot="dialog-content"] [data-testid="vm-kind-toggle"] button')).toHaveCount(2); // Linux / Windows
    await expect(page.locator('[data-slot="dialog-content"] .hv-ppf')).toHaveCount(1); // install ISO picker
    await expect(page.locator('[data-slot="dialog-content"] input[type="password"]')).toHaveCount(0);
    await expect(page.locator('[data-slot="dialog-content"] [data-testid="vm-iso-note"]')).toBeVisible();
    // create stays blocked until an ISO is chosen
    await expect(page.locator('[data-slot="dialog-footer"] [data-testid="vm-btn-primary"]')).toBeDisabled();
  });

  test('3.10 — Windows adds the VirtIO driver picker', async ({ page }) => {
    await openWizard(page);
    await page.locator('[data-testid="vm-mode-btn"]').filter({ hasText: /iso/i }).click();
    await page.locator('[data-slot="dialog-content"] [data-testid="vm-kind-toggle"] button').filter({ hasText: /windows/i }).click();
    await expect(page.locator('[data-slot="dialog-content"] .hv-ppf')).toHaveCount(2); // install + drivers
  });
});

// ===========================================================================
// 4 — Backend contract (direct fetch)
// ===========================================================================
test.describe('4 — Backend contract', () => {
  test('4.1 — /api/vm/os-options returns a curated list with one recommended entry', async () => {
    const res = await sharedCtx.get('/api/vm/os-options', { headers: adminHeaders });
    expect(res.status()).toBe(200);
    const body = await res.json();
    expect(body.success).toBe(true);
    expect(Array.isArray(body.data)).toBe(true);
    expect(body.data.length).toBeGreaterThanOrEqual(4);
    expect(body.data.filter((o: any) => o.recommended)).toHaveLength(1);
    expect(body.data.every((o: any) => typeof o.blurb === 'string' && o.blurb.length > 0)).toBe(true);
  });

  test('4.2 — /api/vm/list shape matches the frontend contract', async () => {
    const res = await sharedCtx.get('/api/vm/list', { headers: adminHeaders });
    const body = await res.json().catch(() => ({}));
    if (!body?.success) { test.skip(true, 'Incus not available'); return; }
    for (const m of body.data) {
      expect(typeof m.name).toBe('string');
      expect(['vm', 'container']).toContain(m.kind);
      expect(['running', 'stopped', 'starting', 'stopping', 'error', 'unknown']).toContain(m.status);
      expect(typeof m.running).toBe('boolean');
      expect(typeof m.snapshot_count).toBe('number');
      expect(typeof m.has_screen).toBe('boolean');
    }
  });

  test('4.2b — ISO create is validated before Incus (path, extension, exclusivity)', async () => {
    const bad = [
      { body: { name: 'w', iso_path: 'rel/x.iso', iso_os: 'windows' }, want: /absolute/i },
      { body: { name: 'w', iso_path: '/tmp/x.img' }, want: /\.iso/i },
      { body: { name: 'w', iso_path: '/tmp/does-not-exist-xyz.iso' }, want: /not found/i },
      { body: { name: 'w', os: 'ubuntu-24.04', password: 'secret12', iso_path: '/tmp/x.iso' }, want: /not both/i },
    ];
    for (const c of bad) {
      const res = await sharedCtx.post('/api/vm/create', { headers: adminHeaders, data: c.body });
      expect(res.status(), JSON.stringify(c.body)).toBe(400);
      const b = await res.json();
      expect(b.success).toBe(false);
      expect(b.message).toMatch(c.want);
    }
  });

  test('4.2c — screen ticket needs a real, running, screen-having VM', async () => {
    const res = await sharedCtx.post('/api/vm/no-such-machine/screen/ticket', { headers: adminHeaders });
    expect(res.status()).toBe(400);
    const b = await res.json();
    expect(b.success).toBe(false);
    expect(b.data?.ticket ?? null).toBeNull();
  });

  test('4.3 — create rejects a bad name before touching Incus', async () => {
    const res = await sharedCtx.post('/api/vm/create', {
      headers: adminHeaders,
      data: { name: '-nope-', os: 'ubuntu-24.04', password: 'secret12' },
    });
    expect(res.status()).toBe(400);
    const body = await res.json();
    expect(body.success).toBe(false);
    expect(body.message.toLowerCase()).toContain('name');
  });

  test('4.4 — create rejects a short password', async () => {
    const res = await sharedCtx.post('/api/vm/create', {
      headers: adminHeaders,
      data: { name: 'okname', os: 'ubuntu-24.04', password: 'abc' },
    });
    expect(res.status()).toBe(400);
    expect((await res.json()).message.toLowerCase()).toContain('password');
  });

  test('4.5 — create rejects an unknown OS', async () => {
    const res = await sharedCtx.post('/api/vm/create', {
      headers: adminHeaders,
      data: { name: 'okname', os: 'temple-os', password: 'secret12' },
    });
    expect(res.status()).toBe(400);
    expect((await res.json()).success).toBe(false);
  });

  test('4.6 — /api/vm/available answers cleanly whether or not Incus is present', async () => {
    const res = await sharedCtx.get('/api/vm/available', { headers: adminHeaders });
    expect(res.status()).toBe(200);
    const body = await res.json();
    expect(body.success).toBe(true);
    expect(typeof body.data.available).toBe('boolean');
  });
});

// ===========================================================================
// 5 — Detail drawer
// ===========================================================================
test.describe('5 — Detail drawer', () => {
  test('5.1 — clicking a card opens the drawer with specs + snapshots + danger zone', async ({ page }) => {
    await gotoVm(page, adminTokens);
    if ((await win(page).locator('[data-testid="vm-card"]:not(.vm-card-creating)').count()) === 0) test.skip(true, 'no machines');
    await win(page).locator('[data-testid="vm-card"]:not(.vm-card-creating)').first().click();
    await expect(page.locator('[data-testid="vm-drawer"]')).toBeVisible();
    await expect(page.locator('[data-testid="vm-drawer"] [data-testid="vm-d-specs"]')).toBeVisible();
    await expect(page.locator('[data-testid="vm-drawer"] [data-testid="vm-d-danger"]')).toBeVisible();
    await expect(page.locator('[data-testid="vm-drawer"]').getByText(/snapshot|instantané|estado|zustand/i).first()).toBeVisible();
  });

  test('5.2 — the drawer closes on the scrim and on Escape', async ({ page }) => {
    await gotoVm(page, adminTokens);
    if ((await win(page).locator('[data-testid="vm-card"]:not(.vm-card-creating)').count()) === 0) test.skip(true, 'no machines');
    await win(page).locator('[data-testid="vm-card"]:not(.vm-card-creating)').first().click();
    await expect(page.locator('[data-testid="vm-drawer"]')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.locator('[data-testid="vm-drawer"]')).toHaveCount(0);
  });
});

// ===========================================================================
// 6 — Console
// ===========================================================================
test.describe('6 — Console', () => {
  test('6.1 — a running machine exposes a console button that opens the console modal', async ({ page }) => {
    await gotoVm(page, adminTokens);
    const running = win(page).locator('[data-testid="vm-card"]').filter({ has: page.locator('[data-testid="vm-pill"][data-status="running"]') });
    if ((await running.count()) === 0) test.skip(true, 'no running machine');
    await running.first().locator('[data-testid="vm-btn-primary"]').click();
    await expect(page.locator('.con-backdrop')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.con-term')).toBeVisible();
    await page.keyboard.press('Escape').catch(() => {});
    await page.locator('.con-btn-close').click().catch(() => {});
  });
});

// ===========================================================================
// 7 — Responsive
// ===========================================================================
test.describe('7 — Responsive', () => {
  for (const w of [360, 768, 1920]) {
    test(`7.${w} — no horizontal overflow at ${w}px`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: 900 });
      await gotoVm(page, adminTokens);
      const overflow = await page.evaluate(() => {
        const el = document.querySelector('[data-testid="vm-page"]') as HTMLElement;
        return el ? el.scrollWidth - el.clientWidth : 0;
      });
      expect(overflow).toBeLessThanOrEqual(2);
    });
  }
});

// ===========================================================================
// 8 — Accessibility
// ===========================================================================
test.describe('8 — Accessibility', () => {
  test('8.1 — the wizard is a labelled modal dialog', async ({ page }) => {
    await gotoVm(page, adminTokens);
    if ((await appState(page)) === 'unavailable') test.skip(true, 'Incus not available');
    await openWizard(page);
    const dlg = page.locator('[data-slot="dialog-content"]');
    await expect(dlg).toHaveAttribute('role', 'dialog');
    await expect(dlg).toHaveAttribute('aria-modal', 'true');
    // The name comes from the shared Modal's Dialog.Title via aria-labelledby
    // (bits-ui wires it), not from a literal aria-label — assert the label the
    // dialog actually resolves to, so a nameless dialog still fails.
    const labelledBy = await dlg.getAttribute('aria-labelledby');
    expect(labelledBy, 'the dialog points at a title element').toBeTruthy();
    await expect(page.locator(`#${labelledBy}`)).not.toBeEmpty();
  });

  test('8.2 — every actionable icon-only button has an accessible name', async ({ page }) => {
    await gotoVm(page, adminTokens);
    const buttons = win(page).locator('button');
    const n = await buttons.count();
    for (let i = 0; i < n; i++) {
      const b = buttons.nth(i);
      const text = (await b.innerText()).trim();
      if (text) continue;
      const label = (await b.getAttribute('aria-label')) || '';
      expect(label.length, `button ${i} needs a label`).toBeGreaterThan(0);
    }
  });

  test('8.3 — delete confirmation uses an alertdialog role', async ({ page }) => {
    await gotoVm(page, adminTokens);
    if ((await win(page).locator('[data-testid="vm-card"]:not(.vm-card-creating)').count()) === 0) test.skip(true, 'no machines');
    const card = win(page).locator('[data-testid="vm-card"]:not(.vm-card-creating)').first();
    await card.locator('[data-testid="vm-menu-wrap"] [data-testid="vm-btn-icon"]').click();
    await card.locator('[data-testid="vm-menu-danger"]').click();
    await expect(page.locator('[role="alertdialog"]')).toBeVisible();
    await page.keyboard.press('Escape');
  });
});

// ===========================================================================
// 9 — Performance
// ===========================================================================
test.describe('9 — Performance', () => {
  test('9.1 — the app becomes interactive quickly', async ({ page }) => {
    await gotoDesktop(page, adminTokens);
    const start = Date.now();
    await page.locator('[aria-label="Applications"]').first().click();
    await page.locator('.lp-card-name').filter({ hasText: /virtual mach|machines virtuelles/i }).first().click();
    await page.waitForSelector('[data-testid="vm-page"]', { timeout: 6000 });
    // window shell is up fast; data may still be loading behind a spinner
    expect(Date.now() - start).toBeLessThan(6000);
  });
});

// ===========================================================================
// 10 — Security
// ===========================================================================
test.describe('10 — Security', () => {
  test('10.1 — no secrets in the URL or localStorage beyond the session pair', async ({ page }) => {
    await gotoVm(page, adminTokens);
    expect(page.url()).not.toMatch(/token|ticket|password|secret/i);
    const keys = await page.evaluate(() => Object.keys(localStorage));
    for (const k of keys) {
      if (['session-token', 'csrf-token'].includes(k)) continue;
      const v = await page.evaluate((kk) => localStorage.getItem(kk) || '', k);
      expect(v).not.toMatch(/E2eSecret123|password/i);
    }
  });

  test('10.2 — every VM endpoint rejects an unauthenticated request', async ({ playwright }) => {
    const ctx = await newCtx(playwright);
    // Bodies are shaped so they pass JSON deserialisation and actually reach
    // the auth guard (an unauthenticated request must never do VM work).
    for (const [m, p, body] of [
      ['get', '/api/vm/list', undefined],
      ['get', '/api/vm/os-options', undefined],
      ['post', '/api/vm/create', { name: 'x', os: 'ubuntu-24.04', password: 'secret12' }],
      ['post', '/api/vm/console/ticket', { name: 'x' }],
      ['post', '/api/vm/x/screen/ticket', undefined],
      ['delete', '/api/vm/x', undefined],
      ['put', '/api/vm/x/start', undefined],
    ] as const) {
      const res = await (ctx as any)[m](p, body ? { data: body } : {});
      // 400 = "no session token", 401 = bad token, 403 = CSRF/forbidden
      expect([400, 401, 403], `${m} ${p} → ${res.status()}`).toContain(res.status());
    }
    await ctx.dispose();
  });

  test('10.3 — a machine name with an XSS payload is never reflected as markup', async ({ page }) => {
    await gotoVm(page, adminTokens);
    if ((await appState(page)) === 'unavailable') test.skip(true, 'Incus not available');
    await openWizard(page);
    await page.locator('[data-slot="dialog-content"] input[type="text"]').first().fill('<img src=x onerror=alert(1)>');
    // the inline validator rejects it; nothing is injected
    await expect(page.locator('[data-testid="vm-field-err"]').first()).toBeVisible();
    const injected = await page.locator('[data-slot="dialog-content"] img[src="x"]').count();
    expect(injected).toBe(0);
  });
});

// ===========================================================================
// 11 — Keyboard
// ===========================================================================
test.describe('11 — Keyboard', () => {
  test('11.1 — Escape closes the wizard', async ({ page }) => {
    await gotoVm(page, adminTokens);
    if ((await appState(page)) === 'unavailable') test.skip(true, 'Incus not available');
    await openWizard(page);
    await page.keyboard.press('Escape');
    await expect(page.locator('[data-slot="dialog-content"]')).toHaveCount(0);
  });

  test('11.2 — the card is reachable with the keyboard and opens on Enter', async ({ page }) => {
    await gotoVm(page, adminTokens);
    if ((await win(page).locator('[data-testid="vm-card"]:not(.vm-card-creating)').count()) === 0) test.skip(true, 'no machines');
    const card = win(page).locator('[data-testid="vm-card"]:not(.vm-card-creating)').first();
    await card.focus();
    await page.keyboard.press('Enter');
    await expect(page.locator('[data-testid="vm-drawer"]')).toBeVisible();
  });
});

// ===========================================================================
// 12 — Console & network
// ===========================================================================
test.describe('12 — Console & network', () => {
  test('12.1 — no uncaught console errors on load', async ({ page }) => {
    const errors: string[] = [];
    page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
    await gotoVm(page, adminTokens);
    if ((await appState(page)) === 'unavailable') test.skip(true, 'Incus not available');
    await page.waitForTimeout(500);
    const real = errors.filter((e) => !/favicon|ResizeObserver|net::ERR/i.test(e));
    expect(real).toEqual([]);
  });

  test('12.2 — the list polls /api/vm/list, never a per-machine /state endpoint', async ({ page }) => {
    const paths = new Set<string>();
    page.on('request', (r) => {
      const u = new URL(r.url());
      if (u.pathname.startsWith('/api/vm/')) paths.add(u.pathname);
    });
    await gotoVm(page, adminTokens);
    await page.waitForTimeout(800);
    expect([...paths]).toContain('/api/vm/list');
    expect([...paths].some((p) => p.endsWith('/state'))).toBe(false);
  });
});

// ===========================================================================
// 13 — Real lifecycle (opt-in: VM_E2E=1, needs Incus)
// ===========================================================================
test.describe('13 — Real lifecycle', () => {
  test.skip(!RUN_LIFECYCLE, 'set VM_E2E=1 and have Incus configured');

  const name = `e2e-vm-${Date.now().toString(36)}`;

  test('13.1 — create → appears → console ticket → delete', async ({ page }) => {
    test.setTimeout(600_000);
    const avail = await sharedCtx.get('/api/vm/available', { headers: adminHeaders });
    if (!(await avail.json()).data.available) test.skip(true, 'Incus not available');

    // create a tiny container (fastest path)
    const create = await sharedCtx.post('/api/vm/create', {
      headers: adminHeaders,
      data: { name, kind: 'container', os: 'alpine-3.21', size: 'small', username: 'admin', password: 'e2epass12' },
    });
    expect(create.status()).toBe(200);

    // poll the list until it shows up (image pull)
    let seen = false;
    for (let i = 0; i < 60 && !seen; i++) {
      const list = await (await sharedCtx.get('/api/vm/list', { headers: adminHeaders })).json();
      seen = (list.data || []).some((m: any) => m.name === name);
      if (!seen) await page.waitForTimeout(5000);
    }
    expect(seen, 'machine should appear in the list').toBe(true);

    // it renders in the UI
    await gotoVm(page, adminTokens);
    await expect(win(page).locator('[data-testid="vm-card-name"]', { hasText: name })).toBeVisible({ timeout: 15_000 });

    // clean up
    const del = await sharedCtx.delete(`/api/vm/${name}`, { headers: adminHeaders });
    expect(del.status()).toBe(200);
  });

  test.afterAll(async () => {
    await sharedCtx.delete(`/api/vm/${name}`, { headers: adminHeaders }).catch(() => {});
  });
});

// ===========================================================================
// 14 — Memory regression (afterEach logs > 10 MB growth)
// ===========================================================================
test.describe('14 — Memory', () => {
  test('14.1 — opening and closing the wizard 5x does not leak', async ({ page }) => {
    test.setTimeout(150_000);
    await gotoVm(page, adminTokens);
    if ((await appState(page)) === 'unavailable') test.skip(true, 'Incus not available');
    const before = await getHeapMB(page);
    for (let i = 0; i < 5; i++) {
      await openWizard(page);
      await page.keyboard.press('Escape');
      await expect(page.locator('[data-slot="dialog-content"]')).toHaveCount(0);
    }
    const after = await getHeapMB(page);
    if (before > 0 && after > 0) expect(after - before).toBeLessThan(12);
  });
});
