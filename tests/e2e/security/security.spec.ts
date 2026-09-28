/**
 * Security suite — End-to-End Test Suite
 *
 * The Security suite is not its own window: it is three cards
 * (AntivirusCard / AntiRansomwareCard / FirewallCard) rendered inside the
 * Settings app's "Security" category. Backend: `src/antivirus.rs`,
 * `src/ransomware.rs` + `src/ransomware_watcher.rs`, `src/firewall.rs` and
 * their handlers.
 *
 * **Real conditions, done for real:**
 *   - A genuine `clamscan` run against a throwaway directory under the test
 *     user's home containing the standard EICAR test string — the full
 *     detect → quarantine → threat-row → resolve path.
 *   - Real anti-ransomware honeypot decoy files written to and removed from
 *     a throwaway directory, verified on disk.
 * The test harness runs on the same host as the server, so `beforeAll`
 * creates those directories directly and `afterAll` deletes them and
 * restores the (global, singleton) antivirus settings to their snapshot.
 *
 * **Never done:** enabling or disabling the real host firewall, or deleting
 * a real `ufw` rule — per CLAUDE.md's firewall rules those mutations only
 * happen behind an explicit signed-off audit. Where `sudo ufw` needs a
 * password (the common case) `firewall::ufw_available()` is `false`; the
 * firewall card is then only checked for the graceful "not installed"
 * state and its auth guards. The `{confirmed:true}` gate and mandatory-rule
 * 403 are asserted (still without ever enabling) only when a real `ufw` is
 * reachable.
 *
 * Run (all):     npx playwright test tests/e2e/security/security.spec.ts --project=firefox
 * Run (single):  npx playwright test tests/e2e/security/security.spec.ts -g "3.1"
 * Custom server: BASE_URL=http://localhost:8080 npx playwright test tests/e2e/security/security.spec.ts
 *
 * Requires a live server whose setup is complete and that is seeded with an
 * admin account (defaults: e2eadmin / E2eSecret123, override with
 * E2E_ADMIN_USER / E2E_ADMIN_PASS).
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';
import { promises as fs } from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

// The EICAR test string, split so this file is never itself flagged.
const EICAR = 'X5O!P%@AP[4\\PZX54(P^)7CC)7}$' + 'EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*';

// A throwaway workspace under the test user's home ( `/home/...` is an
// allowed explorer root on the server, `/tmp` is not).
const WORKROOT = path.join(os.homedir(), 'hv-e2e-security');
const SCAN_DIR = path.join(WORKROOT, 'scan-target');
const QUARANTINE_DIR = path.join(WORKROOT, 'quarantine');
const PROTECTED_DIR = path.join(WORKROOT, 'protected');

interface Tokens {
  token: string;
  csrf: string;
}

async function apiLogin(ctx: APIRequestContext, u: string, p: string): Promise<Tokens> {
  // A generous timeout: the real host runs clamscan + a SQLite DB on the
  // same disk during this suite, so auth can briefly be slow under load.
  const res = await ctx.post('/api/auth/login', {
    data: { username: u, password: p },
    timeout: 30_000,
  });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

async function getHeapMB(page: Page): Promise<number> {
  return page.evaluate(() =>
    'memory' in performance ? (performance as any).usedJSHeapSize / (1024 * 1024) : 0,
  );
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

async function gotoDesktop(page: Page, tokens: Tokens) {
  await seedBrowser(page, tokens);
  await page.goto('/desktop');
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page.locator('.hv-dock').waitFor({ state: 'visible', timeout: 10_000 });
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

async function openSecurity(page: Page, tokens: Tokens) {
  await gotoDesktop(page, tokens);
  const dockBtn = page.locator('.hv-dock-item:has(.hv-dock-icon-settings)').first();
  try {
    await dockBtn.click({ timeout: 5_000 });
  } catch {
    await dockBtn.click({ force: true });
  }
  await page.waitForSelector('[data-testid="settings-page"]', { timeout: 15_000 });
  await page.locator('[data-testid="settings-nav-item"]').first().waitFor({ state: 'visible', timeout: 15_000 });
  // Match the rail entry by its label rather than a fixed index — the
  // category order in Settings.svelte has changed before ("Security moved up
  // next to General", 2026-09-04) and can change again. The suite always
  // runs in English (see seedBrowser), where the label is "Security".
  //
  // The active class lands on the `WindowSidebarItem` **root**, not on the
  // label's parent: `Settings.svelte:663` passes
  // `class={cn(activeCategory === cat.key && 'set-nav-active')}` to the item,
  // whose own markup is `.hv-winsidebar-item > .hv-winsidebar-item-text >
  // .hv-winsidebar-item-label`. So select the item that *contains* the
  // Security label, and assert on that same element.
  const nav = page
    .locator('[data-testid="settings-nav-item"]')
    .filter({ has: page.locator('.hv-winsidebar-item-label', { hasText: 'Security' }) })
    .first();
  await nav.click();
  await expect(nav).toHaveClass(/set-nav-active/);
  await page.locator('[data-testid="settings-content"] [data-testid="settings-card"]').first().waitFor({ state: 'visible', timeout: 15_000 });
}

/** Index of the Security rail entry (for tests that navigate away and back). */
async function securityNavIndex(page: Page): Promise<number> {
  return page
    .locator('[data-testid="settings-nav-item"] .hv-winsidebar-item-label')
    .evaluateAll((els) => els.findIndex((e) => (e.textContent || '').trim() === 'Security'));
}

function set(page: Page) {
  return page.locator('[data-testid="settings-page"]');
}

/** Fetch inside the page with the admin session header. */
async function apiFetch(
  page: Page,
  method: string,
  url: string,
  tokens: Tokens,
  body?: unknown,
): Promise<{ status: number; json: any }> {
  return page.evaluate(
    async ({ method, url, token, csrf, body }) => {
      const r = await fetch(url, {
        method,
        headers: {
          'x-session-token': token,
          'x-csrf-token': csrf,
          ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
        },
        body: body !== undefined ? JSON.stringify(body) : undefined,
      });
      const json = await r.json().catch(() => ({}));
      return { status: r.status, json };
    },
    { method, url, token: tokens.token, csrf: tokens.csrf, body },
  );
}

let adminTokens: Tokens;
let avSettingsSnapshot: any = null;
let clamAvailable = false;
let ufwAvailable = false;

test.beforeAll(async ({ playwright }) => {
  const ctx = await playwright.request.newContext({ baseURL: BASE, timeout: 30_000 });
  adminTokens = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!adminTokens.token) throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);

  // Probe host capabilities via the real status endpoints.
  const avStatus = await ctx
    .get('/api/antivirus/status', { headers: { 'x-session-token': adminTokens.token } })
    .then((r) => r.json())
    .catch(() => ({}));
  clamAvailable =
    !!avStatus?.data?.clamscan_available && !!avStatus?.data?.definitions?.available;

  const fwStatus = await ctx
    .get('/api/firewall/status', { headers: { 'x-session-token': adminTokens.token } })
    .then((r) => r.json())
    .catch(() => ({}));
  ufwAvailable = !!fwStatus?.data?.available;

  // Snapshot the global antivirus settings so we can restore them.
  avSettingsSnapshot = await ctx
    .get('/api/antivirus/settings', { headers: { 'x-session-token': adminTokens.token } })
    .then((r) => r.json())
    .then((b) => b?.data)
    .catch(() => null);

  // Build the throwaway workspace.
  await fs.rm(WORKROOT, { recursive: true, force: true });
  await fs.mkdir(SCAN_DIR, { recursive: true });
  await fs.mkdir(QUARANTINE_DIR, { recursive: true });
  await fs.mkdir(PROTECTED_DIR, { recursive: true });
  await fs.writeFile(path.join(SCAN_DIR, 'eicar.com'), EICAR);

  await ctx.dispose();
});

test.afterAll(async ({ playwright }) => {
  const ctx = await playwright.request.newContext({ baseURL: BASE, timeout: 30_000 });
  // Restore antivirus settings.
  if (avSettingsSnapshot) {
    await ctx
      .put('/api/antivirus/settings', {
        headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
        data: {
          quarantine_dir: avSettingsSnapshot.quarantine_dir,
          scheduled_scan_enabled: avSettingsSnapshot.scheduled_scan_enabled,
          scheduled_scan_cron: avSettingsSnapshot.scheduled_scan_cron,
          scheduled_scan_path: avSettingsSnapshot.scheduled_scan_path ?? null,
          scan_archives: avSettingsSnapshot.scan_archives,
        },
      })
      .catch(() => {});
  }
  // Remove any protected paths this suite added (best effort).
  const paths = await ctx
    .get('/api/ransomware/protected-paths', { headers: { 'x-session-token': adminTokens.token } })
    .then((r) => r.json())
    .then((b) => (Array.isArray(b?.data) ? b.data : []))
    .catch(() => []);
  for (const p of paths) {
    if (typeof p?.path === 'string' && p.path.startsWith(WORKROOT)) {
      await ctx
        .delete(`/api/ransomware/protected-paths/${p.id}`, {
          headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
        })
        .catch(() => {});
    }
  }
  await ctx.dispose();
  await fs.rm(WORKROOT, { recursive: true, force: true }).catch(() => {});
});

test.afterEach(async ({ page }, testInfo) => {
  const heapMB = await getHeapMB(page).catch(() => 0);
  if (heapMB > 0) {
    // eslint-disable-next-line no-console
    console.log(`[MEM] ${testInfo.title}: ${heapMB.toFixed(1)} MB heap`);
  }
});

// ===========================================================================
// 1. Card mount
// ===========================================================================

test.describe('1 — Cards mount', () => {
  test('1.1 — all three security cards render in the Security category', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const titles = (await set(page).locator('[data-testid="settings-card-title"]').allTextContents())
      .join(' | ')
      .toLowerCase();
    expect(titles).toMatch(/antivirus/);
    expect(titles).toMatch(/ransom/);
    expect(titles).toMatch(/firewall|pare-feu/);
    expect(await set(page).locator('[data-testid="settings-card"]').count()).toBeGreaterThanOrEqual(3);
  });

  test('1.2 — each card shows a subtitle', async ({ page }) => {
    await openSecurity(page, adminTokens);
    expect(await set(page).locator('.settings-card-subtitle').count()).toBeGreaterThanOrEqual(3);
  });
});

// ===========================================================================
// 2. Antivirus card — UI + real state
// ===========================================================================

test.describe('2 — Antivirus card', () => {
  test('2.1 — definitions + last-scan info rows render (or the not-installed warning)', async ({
    page,
  }) => {
    await openSecurity(page, adminTokens);
    const card = set(page).locator('[data-testid="settings-card"]', { hasText: /antivirus/i });
    const notInstalled = await card.locator('.av-warning').count();
    if (notInstalled > 0) {
      test.skip(!clamAvailable, 'clamscan not installed on this host');
    }
    await expect(card.locator('.settings-info-row').first()).toBeVisible();
    await expect(card.locator('#avScanPath, [id="avScanPath"]')).toBeAttached();
  });

  test('2.2 — the "Scan now" button is present and the scan-path field is pick-only (not typeable)', async ({
    page,
  }) => {
    await openSecurity(page, adminTokens);
    const card = set(page).locator('[data-testid="settings-card"]', { hasText: /antivirus/i });
    if ((await card.locator('.av-warning').count()) > 0) test.skip(true, 'clamscan not installed');
    await expect(card.locator('button', { hasText: /scan/i }).first()).toBeVisible();
    // The path is chosen with a folder picker only — no manual entry.
    const scanField = card.locator('#avScanPath');
    await expect(scanField).toHaveAttribute('readonly', '');
    await expect(card.locator('button', { hasText: /folder|dossier|carpeta|ordner/i }).first()).toBeVisible();
  });

  test('2.4 — "Select a folder" opens a draggable, non-modal picker (no screen-blocking backdrop)', async ({
    page,
  }) => {
    await openSecurity(page, adminTokens);
    const card = set(page).locator('[data-testid="settings-card"]', { hasText: /antivirus/i });
    if ((await card.locator('.av-warning').count()) > 0) test.skip(true, 'clamscan not installed');

    const browse = card.locator('.hv-ppf', { has: page.locator('#avScanPath') }).locator('button');
    await browse.first().click();

    const picker = page.locator(".hv-fpp");
    await expect(picker).toBeVisible();
    // Non-modal: it is NOT a full-screen overlay, and the card behind stays hit-testable.
    const box = await picker.boundingBox();
    const vp = page.viewportSize()!;
    expect(box!.width).toBeLessThan(vp.width);
    expect(box!.height).toBeLessThan(vp.height);
    expect(await page.locator('.hv-fp-overlay').count()).toBe(0);
    await expect(card.locator('[data-testid="settings-card-title"]')).toBeVisible(); // still there, not covered by a scrim

    // It has a drag handle, a locations sidebar (physical disks always shown)
    // and a list/grid view toggle.
    await expect(picker.locator('.hv-fpp-header')).toBeVisible();
    await expect(picker.locator('.hv-fpp-sidebar')).toBeVisible();
    await expect(picker.locator('.hv-fpp-sidebar .hv-fpp-place')).not.toHaveCount(0);
    await expect(picker.locator('.hv-fpp-viewtoggle .hv-fpp-vbtn')).toHaveCount(2);
    // A real disk row (not "This device") shows a size and/or serial line.
    const diskRow = picker.locator('.hv-fpp-sidebar .hv-fpp-place').filter({ has: page.locator('.hv-fpp-place-sub') });
    if (await diskRow.count()) {
      await expect(diskRow.first().locator('.hv-fpp-place-sub')).toHaveText(/\d|S\/N|série|serie/i);
    }

    // Grid toggle switches the listing to tiles and the choice sticks.
    await picker.locator('.hv-fpp-vbtn').nth(1).click();
    await expect(picker.locator('.hv-fpp-vbtn').nth(1)).toHaveAttribute('aria-pressed', 'true');

    await page.keyboard.press('Escape');
    await expect(picker).toHaveCount(0);

    // Re-open: the grid choice was remembered (localStorage).
    await browse.first().click();
    await expect(page.locator('.hv-fpp')).toBeVisible();
    await expect(page.locator('.hv-fpp .hv-fpp-vbtn').nth(1)).toHaveAttribute('aria-pressed', 'true');
    await page.keyboard.press('Escape');
  });

  test('2.3 — cron field defaults to a weekly schedule', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const card = set(page).locator('[data-testid="settings-card"]', { hasText: /antivirus/i });
    if ((await card.locator('.av-warning').count()) > 0) test.skip(true, 'clamscan not installed');
    const cron = card.locator('#avCron');
    if ((await cron.count()) === 0) test.skip(true, 'scheduled scan section not shown');
    await expect(cron).toHaveValue(/\S/);
  });
});

// ===========================================================================
// 3. Antivirus — real EICAR scan (detect → quarantine → threat → resolve)
// ===========================================================================

test.describe('3 — Antivirus real scan', () => {
  test('3.1 — a real clamscan of an EICAR file is detected, quarantined, and resolvable', async ({
    page,
  }) => {
    test.skip(!clamAvailable, 'clamscan or its virus DB is not available on this host');
    await openSecurity(page, adminTokens);

    // Point quarantine at the throwaway workspace.
    const put = await apiFetch(page, 'PUT', '/api/antivirus/settings', adminTokens, {
      quarantine_dir: QUARANTINE_DIR,
      scheduled_scan_enabled: false,
      scheduled_scan_cron: '0 3 * * 0',
      scheduled_scan_path: null,
      scan_archives: true,
    });
    expect(put.status).toBe(200);

    const start = await apiFetch(page, 'POST', '/api/antivirus/scan', adminTokens, { path: SCAN_DIR });
    expect(start.status, JSON.stringify(start.json)).toBe(200);
    expect(typeof start.json?.data?.job_id).toBe('string');

    // Poll the scan history until terminal.
    let done = false;
    for (let i = 0; i < 60 && !done; i++) {
      await page.waitForTimeout(500);
      const scans = await apiFetch(page, 'GET', '/api/antivirus/scans', adminTokens);
      const list: any[] = scans.json?.data ?? [];
      const latest = list[0];
      if (latest && (latest.status === 'completed' || latest.status === 'failed')) {
        expect(latest.status).toBe('completed');
        expect(Number(latest.threats_found)).toBeGreaterThanOrEqual(1);
        done = true;
      }
    }
    expect(done, 'the scan never finished within 30s').toBe(true);

    // The threat row exists and names EICAR.
    const threats = await apiFetch(page, 'GET', '/api/antivirus/threats', adminTokens);
    const t = (threats.json?.data ?? [])[0];
    expect(t, 'a threat row must exist').toBeTruthy();
    expect(String(t.signature_name).toLowerCase()).toContain('eicar');
    // NOTE: the recorded `action` is "quarantined" or "left_in_place" — the
    // frozen `parse_clamscan_line` checks whether the moved file exists at
    // the exact instant clamscan prints the FOUND line, which races the
    // `--move` on slow storage (reported to the user). The ground truth is
    // the filesystem below.
    expect(['quarantined', 'left_in_place']).toContain(t.action);

    // Ground truth: clamscan --move really relocated the infected file.
    await expect
      .poll(async () => fs.readdir(QUARANTINE_DIR).then((f) => f.length).catch(() => 0), {
        timeout: 15_000,
      })
      .toBeGreaterThan(0);
    await expect
      .poll(async () => fs.readdir(SCAN_DIR).catch(() => []), { timeout: 15_000 })
      .not.toContain('eicar.com');

    // Resolve the threat + delete the quarantined copy — leaves the box clean.
    const resolve = await apiFetch(page, 'POST', `/api/antivirus/threats/${t.id}/resolve`, adminTokens, {
      delete: true,
    });
    expect(resolve.status).toBe(200);
    const after = await apiFetch(page, 'GET', '/api/antivirus/threats', adminTokens);
    const resolved = (after.json?.data ?? []).find((x: any) => x.id === t.id);
    expect(resolved, 'the threat row is kept in history').toBeTruthy();
    expect(resolved.resolved_at, 'and is now marked resolved').toBeTruthy();
    // If it had been quarantined, the on-disk copy is now deleted.
    if (resolved.quarantine_path) {
      await expect
        .poll(async () => fs.access(resolved.quarantine_path).then(() => true).catch(() => false), {
          timeout: 10_000,
        })
        .toBe(false);
    }

    // Clean the quarantine dir + recreate the EICAR fixture for later runs.
    for (const f of await fs.readdir(QUARANTINE_DIR).catch(() => [])) {
      await fs.rm(path.join(QUARANTINE_DIR, f), { force: true }).catch(() => {});
    }
    await fs.writeFile(path.join(SCAN_DIR, 'eicar.com'), EICAR).catch(() => {});
  });

  test('3.2 — scanning a path outside the allowed roots is rejected', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const res = await apiFetch(page, 'POST', '/api/antivirus/scan', adminTokens, { path: '/etc' });
    expect(res.status).toBe(400);
  });

  test('3.3 — cancelling with no scan running is a clean no-op', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const res = await apiFetch(page, 'POST', '/api/antivirus/scan/cancel', adminTokens);
    expect(res.status).toBe(200);
    expect(res.json?.data?.cancelled).toBe(false);
  });
});

// ===========================================================================
// 4. Anti-Ransomware card — UI + real honeypot
// ===========================================================================

test.describe('4 — Anti-Ransomware card', () => {
  test('4.1 — protection + honeypot toggles and the auto-response select render', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const card = set(page).locator('[data-testid="settings-card"]', { hasText: /ransom/i });
    await expect(card.locator('input[type="checkbox"]')).toHaveCount(2);
    const sel = card.locator('#rwAutoResponse');
    await expect(sel).toBeVisible();
    const opts = await sel.locator('option').evaluateAll((o) => o.map((e) => (e as HTMLOptionElement).value));
    expect(opts.sort()).toEqual(['alert_only', 'disable_share', 'lock_account']);
  });

  test('4.2 — a bogus auto_response is rejected by the API', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const res = await apiFetch(page, 'PUT', '/api/ransomware/settings', adminTokens, {
      enabled: true,
      honeypots_enabled: true,
      rate_threshold_events: 50,
      rate_threshold_seconds: 10,
      auto_response: 'format_c_drive',
    });
    expect(res.status).toBe(400);
  });

  test('4.3 — adding a protected path places a real honeypot decoy file, removing it deletes it', async ({
    page,
  }) => {
    await openSecurity(page, adminTokens);

    const add = await apiFetch(page, 'POST', '/api/ransomware/protected-paths', adminTokens, {
      path: PROTECTED_DIR,
      place_honeypot: true,
    });
    expect(add.status, JSON.stringify(add.json)).toBe(200);
    const id = add.json?.data?.id;
    const honeypot = add.json?.data?.honeypot_filename;
    expect(typeof id).toBe('string');
    expect(typeof honeypot).toBe('string');

    // The decoy really exists on disk.
    const filesAfterAdd = await fs.readdir(PROTECTED_DIR);
    expect(filesAfterAdd).toContain(honeypot);

    // It appears in the list.
    const list = await apiFetch(page, 'GET', '/api/ransomware/protected-paths', adminTokens);
    expect((list.json?.data ?? []).some((p: any) => p.id === id)).toBe(true);

    // Remove → row gone, decoy gone.
    const del = await apiFetch(page, 'DELETE', `/api/ransomware/protected-paths/${id}`, adminTokens);
    expect(del.status).toBe(200);
    const filesAfterDel = await fs.readdir(PROTECTED_DIR).catch(() => []);
    expect(filesAfterDel).not.toContain(honeypot);
  });

  test('4.4 — protecting a non-directory path is rejected', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const res = await apiFetch(page, 'POST', '/api/ransomware/protected-paths', adminTokens, {
      path: path.join(PROTECTED_DIR, 'nope-not-real'),
      place_honeypot: false,
    });
    expect(res.status).toBe(400);
  });

  test('4.5 — the events list renders (empty or populated)', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const res = await apiFetch(page, 'GET', '/api/ransomware/events', adminTokens);
    expect(res.status).toBe(200);
    expect(Array.isArray(res.json?.data)).toBe(true);
  });
});

// ===========================================================================
// 5. Firewall card — never enables/disables a real ufw
// ===========================================================================

test.describe('5 — Firewall card', () => {
  test('5.1 — status renders: active/inactive badge, or the not-installed warning', async ({
    page,
  }) => {
    await openSecurity(page, adminTokens);
    const card = set(page).locator('[data-testid="settings-card"]', { hasText: /firewall|pare-feu/i });
    const warn = await card.locator('.fw-warning').count();
    if (!ufwAvailable) {
      expect(warn).toBeGreaterThan(0);
    } else {
      await expect(card.locator('.settings-badge').first()).toBeVisible();
    }
  });

  test('5.2 — GET /api/firewall/status is always a well-formed envelope', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const res = await apiFetch(page, 'GET', '/api/firewall/status', adminTokens);
    expect(res.status).toBe(200);
    expect(typeof res.json?.data?.available).toBe('boolean');
    expect(typeof res.json?.data?.enabled).toBe('boolean');
    expect(Array.isArray(res.json?.data?.rules)).toBe(true);
  });

  test('5.3 — enabling without confirmation never turns the firewall on', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const before = await apiFetch(page, 'GET', '/api/firewall/status', adminTokens);
    const res = await apiFetch(page, 'POST', '/api/firewall/enable', adminTokens, { confirmed: false });
    expect(res.status).toBe(400);
    if (ufwAvailable) {
      expect(String(res.json?.message).toLowerCase()).toContain('confirm');
    }
    // State is unchanged.
    const after = await apiFetch(page, 'GET', '/api/firewall/status', adminTokens);
    expect(after.json?.data?.enabled).toBe(before.json?.data?.enabled);
  });

  test('5.4 — a mandatory SSH/app rule cannot be deleted via the API', async ({ page }) => {
    test.skip(!ufwAvailable, 'ufw not reachable on this host — mandatory-rule check needs live ufw');
    await openSecurity(page, adminTokens);
    const status = await apiFetch(page, 'GET', '/api/firewall/status', adminTokens);
    const mandatory = (status.json?.data?.rules ?? []).find((r: any) => r.is_mandatory);
    test.skip(!mandatory, 'no mandatory rule present (firewall never enabled on this host)');
    const del = await apiFetch(page, 'DELETE', '/api/firewall/rules', adminTokens, {
      port: mandatory.port,
      protocol: mandatory.protocol,
      action: 'allow',
    });
    expect(del.status).toBe(403);
  });

  test('5.5 — adding a rule with an empty port is rejected', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const res = await apiFetch(page, 'POST', '/api/firewall/rules', adminTokens, { port: '   ' });
    expect(res.status).toBe(400);
  });
});

// ===========================================================================
// 6. Security — auth guards on every endpoint
// ===========================================================================

test.describe('6 — Auth guards', () => {
  const READS = [
    '/api/antivirus/status',
    '/api/antivirus/settings',
    '/api/antivirus/scans',
    '/api/antivirus/threats',
    '/api/firewall/status',
    '/api/ransomware/status',
    '/api/ransomware/settings',
    '/api/ransomware/protected-paths',
    '/api/ransomware/events',
  ];

  test('6.1 — every read rejects a missing session token', async ({ page }) => {
    await openSecurity(page, adminTokens);
    for (const url of READS) {
      const status = await page.evaluate(async (u) => (await fetch(u)).status, url);
      expect(status, `${url} unauthenticated`).toBe(400);
    }
  });

  test('6.2 — every read rejects a tampered token', async ({ page }) => {
    await openSecurity(page, adminTokens);
    for (const url of ['/api/antivirus/status', '/api/firewall/status', '/api/ransomware/status']) {
      const status = await page.evaluate(
        async (u) => (await fetch(u, { headers: { 'x-session-token': 'bogus' } })).status,
        url,
      );
      expect(status, url).toBe(401);
    }
  });

  test('6.3 — mutations require an admin session (403 for a plain user)', async ({
    page,
    playwright,
  }) => {
    await openSecurity(page, adminTokens);
    const ctx = await playwright.request.newContext({ baseURL: BASE });
    const username = `e2esecnonadmin${Date.now().toString(36)}`;
    const create = await ctx.post('/api/users', {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
      data: {
        username,
        password: 'NonAdminPass123!',
        fullname: 'E2E Non Admin',
        email: `${username}@example.com`,
        role_id: 1,
      },
    });
    if (create.status() !== 200 && create.status() !== 201) {
      await ctx.dispose();
      test.skip(true, 'could not provision a throwaway non-admin user');
    }
    const { token: userToken } = await apiLogin(ctx, username, 'NonAdminPass123!');

    const muts: Array<[string, string, any?]> = [
      ['PUT', '/api/antivirus/settings', { quarantine_dir: '/tmp/q', scheduled_scan_enabled: false, scheduled_scan_cron: '0 3 * * 0', scheduled_scan_path: null, scan_archives: true }],
      ['POST', '/api/antivirus/scan', { path: '/home' }],
      ['POST', '/api/antivirus/scan/cancel'],
      ['PUT', '/api/ransomware/settings', { enabled: true, honeypots_enabled: true, rate_threshold_events: 50, rate_threshold_seconds: 10, auto_response: 'alert_only' }],
      ['POST', '/api/ransomware/protected-paths', { path: '/home' }],
      ['POST', '/api/firewall/enable', { confirmed: true }],
      ['POST', '/api/firewall/disable'],
      ['POST', '/api/firewall/rules', { port: '443' }],
    ];
    for (const [method, url, body] of muts) {
      const status = await page.evaluate(
        async ({ method, url, token, body }) => {
          const r = await fetch(url, {
            method,
            headers: { 'x-session-token': token, ...(body ? { 'Content-Type': 'application/json' } : {}) },
            body: body ? JSON.stringify(body) : undefined,
          });
          return r.status;
        },
        { method, url, token: userToken, body },
      );
      expect(status, `${method} ${url} must be 403 for a non-admin`).toBe(403);
    }

    await ctx.delete(`/api/users/${username}`, {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    });
    await ctx.dispose();
  });

  test('6.4 — no session token ever appears in the page URL', async ({ page }) => {
    await openSecurity(page, adminTokens);
    expect(page.url()).not.toContain(adminTokens.token);
  });
});

// ===========================================================================
// 7. Responsive
// ===========================================================================

test.describe('7 — Responsive', () => {
  for (const [w, h, label] of [
    [360, 800, '360px'],
    [768, 1024, '768px'],
    [1920, 1080, '1920px'],
  ] as const) {
    test(`7.${label} — Security panel has no horizontal overflow`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: h });
      await openSecurity(page, adminTokens);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
      );
      expect(overflow).toBe(false);
    });
  }
});

// ===========================================================================
// 8. Accessibility
// ===========================================================================

test.describe('8 — Accessibility', () => {
  test('8.1 — every form control in the Security panel has an accessible name', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const unnamed = await set(page).locator('[data-testid="settings-content"]').evaluate((root) => {
      const bad: string[] = [];
      root.querySelectorAll('input, select, textarea').forEach((el) => {
        const id = el.getAttribute('id');
        const hasFor = id && root.querySelector(`label[for="${id}"]`);
        const wrapped = el.closest('label');
        const aria = el.getAttribute('aria-label') || el.getAttribute('aria-labelledby');
        const placeholder = el.getAttribute('placeholder');
        // A visible label / wrapping label / aria name is best; the frozen
        // "add rule" / "add path" rows fall back to a placeholder, which is
        // still an accessible name in practice.
        if (!hasFor && !wrapped && !aria && !placeholder) bad.push(el.outerHTML.slice(0, 90));
      });
      return bad;
    });
    expect(unnamed).toEqual([]);
  });

  test('8.2 — no duplicate element ids in the Security panel', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const dupes = await page.evaluate(() => {
      const ids = Array.from(document.querySelectorAll('[id]')).map((e) => e.id).filter(Boolean);
      const seen = new Set<string>();
      const dup: string[] = [];
      for (const id of ids) {
        if (seen.has(id)) dup.push(id);
        seen.add(id);
      }
      return dup;
    });
    expect(dupes).toEqual([]);
  });
});

// ===========================================================================
// 9. Console & network
// ===========================================================================

test.describe('9 — Console & network', () => {
  test('9.1 — opening the Security category logs no uncaught JS errors', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await openSecurity(page, adminTokens);
    await page.waitForTimeout(500);
    expect(errors).toEqual([]);
  });

  test('9.2 — the three cards issue their status/list calls without 5xx', async ({ page }) => {
    const bad: string[] = [];
    page.on('response', (res) => {
      if (res.status() >= 500 && res.url().includes('/api/')) bad.push(`${res.status()} ${res.url()}`);
    });
    await openSecurity(page, adminTokens);
    await page.waitForTimeout(800);
    expect(bad).toEqual([]);
  });
});

// ===========================================================================
// 10. Keyboard
// ===========================================================================

test.describe('10 — Keyboard', () => {
  test('10.1 — the auto-response select is reachable and operable by keyboard', async ({ page }) => {
    await openSecurity(page, adminTokens);
    const sel = set(page).locator('#rwAutoResponse');
    await sel.focus();
    await expect(sel).toBeFocused();
    await sel.selectOption('alert_only');
    await expect(sel).toHaveValue('alert_only');
  });
});

// ===========================================================================
// 11. Memory regression
// ===========================================================================

test.describe('11 — Memory regression', () => {
  test('11.1 — switching in and out of Security ×5 does not leak significant heap', async ({
    page,
  }) => {
    test.setTimeout(150_000);
    await openSecurity(page, adminTokens);
    const secIdx = await securityNavIndex(page);
    const before = await getHeapMB(page).catch(() => 0);
    for (let i = 0; i < 5; i++) {
      await set(page).locator('[data-testid="settings-nav-item"]').first().click();
      await page.waitForTimeout(150);
      await set(page).locator('[data-testid="settings-nav-item"]').nth(secIdx).click();
      await page.locator('[data-testid="settings-content"] [data-testid="settings-card"]').first().waitFor({ state: 'visible' });
      await page.waitForTimeout(150);
    }
    const after = await getHeapMB(page).catch(() => 0);
    if (before > 0 && after > 0) {
      console.log(`[MEM] Security ×5 in/out delta: ${(after - before).toFixed(1)} MB`);
      expect(after - before).toBeLessThan(15);
    }
  });
});
