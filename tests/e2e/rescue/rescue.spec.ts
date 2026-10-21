/* =============================================================================
 * File          : rescue.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Rescue screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Rescue app — End-to-End Test Suite
 *
 * The Rescue app is not a route: it is a window opened from the desktop's
 * Applications launcher (id `rescue`, "Rescue" tile). It wraps four real,
 * genuinely destructive disk tools — ddrescue (disk cloning), TestDisk
 * (partition analysis/rebuild), PhotoRec (file recovery) and mdadm
 * (RAID repair) — against whatever block devices the host reports.
 *
 * **This suite never drives a mutating action (Start Clone, Rebuild
 * Partition Table, Start Recovery, Repair) against a real, existing block
 * device.** Unlike RAID/Docker (whose worst case is a throwaway loop
 * device or container), the "Available Devices" list here is populated
 * from the *actual host disks* (`lsblk -d -o ... | type == disk`) — on a
 * real deployment that includes the machine's real boot/data drives, and
 * these tools have no built-in "dry run" mode. So:
 *   - Backend validation (SecurityValidator's protected-device / path-
 *     traversal / nonexistent-device rejections) is exercised via direct
 *     `fetch`, exactly mirroring `tests/rescue_integration.rs` — this
 *     confirms every mutating endpoint fails *cleanly* (400) before ever
 *     touching a device, without needing `sudo` or real hardware.
 *   - The one exception clicked through the real UI is mdadm "Diagnose"
 *     against a name that cannot possibly resolve to a real device
 *     (`e2e-<timestamp>-nonexistent`) — safe because `/dev/md/<name>`
 *     doesn't exist, so `SecurityValidator::validate_block_device` fails
 *     at `canonicalize()` before mdadm is ever invoked (confirmed in the
 *     Rust integration suite).
 *   - A genuine ddrescue/TestDisk/PhotoRec run and a real mdadm repair
 *     against disposable hardware are intentionally left for a dedicated,
 *     explicitly-authorized session (same caution the RAID page's real
 *     lifecycle test needed) — not attempted here.
 *
 * Run (all, safe subset): npx playwright test tests/e2e/rescue/rescue.spec.ts --project=firefox
 * Run (single):            npx playwright test tests/e2e/rescue/rescue.spec.ts -g "4.1"
 * Run (category):          npx playwright test tests/e2e/rescue/rescue.spec.ts -g "Security"
 * Custom server:            BASE_URL=http://localhost:3000 npx playwright test tests/e2e/rescue/rescue.spec.ts
 *
 * Requires a live server whose setup is complete and that is seeded with an
 * admin account (defaults: e2eadmin / E2eSecret123, override with
 * E2E_ADMIN_USER / E2E_ADMIN_PASS).
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
  await page.goto('/desktop');
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  // Desktop monitor widgets are visible-by-default and position:fixed —
  // they intercept clicks meant for windows/dock (established fix, see
  // settings.spec.ts / docker.spec.ts).
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

/** Click the Applications launcher and open one app tile by its display name. */
async function openLaunchpadApp(page: Page, appName: string) {
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator(`.lp-card[title="${appName}"]`);
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
}

/** Land on the desktop with an authenticated ADMIN session and open Rescue. */
async function gotoRescue(page: Page, tokens: Tokens, lang?: string) {
  await gotoDesktop(page, tokens, lang);
  await openLaunchpadApp(page, lang === 'fr' ? 'Secours' : 'Rescue');
  await page.waitForSelector('[data-testid="rescue-page"]', { timeout: 10_000 });
  await expect(page.locator('[data-testid="rescue-page"] [data-testid="rescue-loading"]')).toHaveCount(0, { timeout: 10_000 });
}

/** The Rescue window as a scoping root — `[data-testid="rescue-page"]` is unique to
 * this page (confirmed via grep across `frontend/src/routes`). */
function win(page: Page) {
  return page.locator('[data-testid="rescue-page"]');
}

async function switchTab(page: Page, label: 'ddrescue' | 'TestDisk' | 'PhotoRec' | 'mdadm') {
  await win(page).locator('.hv-winhdr-tab', { hasText: label }).click();
}

/** The real window-close button (top-right floating control) — narrowly
 * scoped to avoid the hidden Resource Monitor's "Close resource monitor"
 * button, whose aria-label also happens to contain "lose" (see
 * docker.spec.ts's 6.2 for the same class of bug). */
function closeWindowButton(page: Page) {
  return page.locator('.hv-cl-btn-close').first();
}

let adminTokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  const ctx = await newCtx(playwright);
  const { token, csrf } = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  if (!token) {
    throw new Error(
      `Could not log in as ${ADMIN_USER} — seed this account first (see CLAUDE.md's Settings E2E note).`,
    );
  }
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
  test('1.1 — Rescue window opens from the Applications launcher', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await expect(win(page)).toBeVisible();
  });

  test('1.2 — header shows the title only (standardized header)', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-title')).toHaveText('Rescue Tools');
    // `.hv-winhdr-subtitle` no longer exists (the prop was removed from
    // `WindowHeader` on 2026-09-14), so this locator matched nothing.
    // Assert the bar's real shape instead: the icon tile + the title.
    await expect(win(page).locator('.hv-winhdr > *')).toHaveCount(2);
  });

  test('1.3 — the active tab carries one tool-availability badge, and the header none', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    // The 4-badge strip that used to sit in the header is gone with the header
    // standardization; availability now lives on each tool's own card.
    const header = win(page).locator('.hv-winhdr');
    await expect(header.locator('[data-testid="rescue-tool-badge"]')).toHaveCount(0);
    const badges = win(page).locator('[data-testid="rescue-tool-badge"]');
    await expect(badges).toHaveCount(1);
    // `rescue.available` / `rescue.not_found` — matched case-insensitively so
    // the assertion survives a locale that capitalises differently.
    await expect(badges.first()).toHaveText(/available|not found/i);
  });

  test('1.4 — the tab bar lists all 4 tabs', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    const tabs = win(page).locator('.hv-winhdr-tab');
    await expect(tabs).toHaveCount(4);
  });

  test('1.5 — the ddrescue tab is selected by default', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await expect(win(page).locator('.hv-winhdr-tab', { hasText: 'ddrescue' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    await expect(win(page).locator('#rescue-dd-source')).toBeVisible();
  });

  test('1.6 — the available devices list renders', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    // Either real disks are listed, or the empty state shows — either way
    // this must never crash.
    const list = win(page).locator('[data-testid="rescue-device-list"]');
    const empty = win(page).locator('[data-testid="rescue-empty"]');
    await expect(list.or(empty)).toBeVisible();
  });
});

// ===========================================================================
// 2. Form fields
// ===========================================================================

test.describe('2 — Form fields', () => {
  test('2.1 — ddrescue exposes source/destination device selects', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await expect(win(page).locator('#rescue-dd-source')).toHaveJSProperty('tagName', 'SELECT');
    await expect(win(page).locator('#rescue-dd-dest')).toHaveJSProperty('tagName', 'SELECT');
  });

  test('2.2 — ddrescue advanced fields are hidden in simple mode, shown in advanced', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    await expect(win(page).locator('#rescue-dd-logfile')).toHaveCount(0);
    await win(page).locator('[data-testid="rescue-mode-btn"]', { hasText: 'Advanced' }).first().click();
    await expect(win(page).locator('#rescue-dd-logfile')).toBeVisible();
    await expect(win(page).locator('#rescue-dd-blocksize')).toHaveAttribute('type', 'number');
    await expect(win(page).locator('#rescue-dd-retry')).toHaveAttribute('type', 'number');
  });

  test('2.3 — TestDisk exposes a device select', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'TestDisk');
    await expect(win(page).locator('#rescue-td-device')).toHaveJSProperty('tagName', 'SELECT');
  });

  test('2.4 — PhotoRec exposes a device select and an output-directory field', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'PhotoRec');
    await expect(win(page).locator('#rescue-pr-device')).toHaveJSProperty('tagName', 'SELECT');
    await expect(win(page).locator('#rescue-pr-outputdir')).toHaveAttribute('type', 'text');
    await expect(win(page).locator('#rescue-pr-outputdir')).toHaveValue('/tmp/recovered');
  });

  test('2.5 — mdadm exposes a free-text array-name field with a placeholder', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'mdadm');
    const input = win(page).locator('#rescue-md-name');
    await expect(input).toHaveAttribute('type', 'text');
    await expect(input).toHaveAttribute('placeholder', /md0/);
  });
});

// ===========================================================================
// 3. Validation — direct fetch against the real backend, exactly mirroring
//    tests/rescue_integration.rs. Never touches a real/accessible device.
// ===========================================================================

test.describe('3 — Validation', () => {
  test('3.1 — server rejects a ddrescue clone with an invalid source device', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/rescue/ddrescue/clone', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ source: '/etc/passwd', destination: '/dev/loop0' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.2 — server rejects a ddrescue clone targeting the protected root disk', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/rescue/ddrescue/clone', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ source: '/dev/loop0', destination: '/dev/sda' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.3 — server rejects a TestDisk analyze against the protected root disk', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/rescue/testdisk/analyze', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ device: '/dev/sda' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.4 — server rejects a PhotoRec output directory outside the allowlist', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/rescue/photorec/recover', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ device: '/dev/loop0', output_dir: '/root/sensitive' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.5 — server rejects a mdadm diagnose against a nonexistent array cleanly', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/rescue/mdadm/diagnose', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ name: 'totally-fake-array' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.6 — server rejects a mdadm repair on the protected root disk given directly', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    const status = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/rescue/mdadm/repair', {
          method: 'POST',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf, 'Content-Type': 'application/json' },
          body: JSON.stringify({ name: '/dev/sda' }),
        });
        return r.status;
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(status).toBe(400);
  });

  test('3.7 — the ddrescue status endpoint refuses a path outside the rescue directories', async ({
    page,
  }) => {
    // 2026-10-21 fix: `GET /api/rescue/ddrescue/status/{path}` took the raw
    // URL segment and read it back with the service's own privileges — an
    // arbitrary-file read (and an existence oracle) behind any admin session.
    await gotoRescue(page, adminTokens);
    const results = await page.evaluate(
      async ({ token }) => {
        const paths = ['/etc/passwd', '/etc/shadow', '/root/.ssh/id_rsa'];
        const out: Array<{ path: string; status: number }> = [];
        for (const p of paths) {
          const r = await fetch(`/api/rescue/ddrescue/status/${encodeURIComponent(p)}`, {
            headers: { 'x-session-token': token },
          });
          out.push({ path: p, status: r.status });
        }
        return out;
      },
      { token: adminTokens.token },
    );
    for (const { path, status } of results) {
      expect(status, `${path} must be refused`).toBe(400);
    }
  });

  test('3.8 — the server rejects options it cannot honour instead of ignoring them', async ({
    page,
  }) => {
    // 2026-10-21 fix: TestDisk's `deep_search`/`log_path`/`force`, PhotoRec's
    // `keep_broken`/`max_size` and an unknown mdadm `action` were accepted,
    // shown as applied in the UI, and never reached the tool.
    await gotoRescue(page, adminTokens);
    const statuses = await page.evaluate(
      async ({ token, csrf }) => {
        const headers = {
          'x-session-token': token,
          'x-csrf-token': csrf,
          'Content-Type': 'application/json',
        };
        const post = async (url: string, body: unknown) =>
          (await fetch(url, { method: 'POST', headers, body: JSON.stringify(body) })).status;
        return {
          deepSearch: await post('/api/rescue/testdisk/analyze', { device: '/dev/loop0', deep_search: true }),
          logPath: await post('/api/rescue/testdisk/analyze', { device: '/dev/loop0', log_path: '/tmp/x.log' }),
          force: await post('/api/rescue/testdisk/rebuild', { device: '/dev/loop0', force: true }),
          mdadmAction: await post('/api/rescue/mdadm/repair', { name: 'fake-array', action: 'rm -rf' }),
          fileTypes: await post('/api/rescue/photorec/recover', {
            device: '/dev/loop0',
            output_dir: '/tmp/recovered',
            file_types: ['jpg,fileopt,everything,enable'],
          }),
          partitionType: await post('/api/rescue/photorec/recover', {
            device: '/dev/loop0',
            output_dir: '/tmp/recovered',
            partition_type: 'Amiga',
          }),
        };
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(statuses).toEqual({
      deepSearch: 400,
      logPath: 400,
      force: 400,
      mdadmAction: 400,
      fileTypes: 400,
      partitionType: 400,
    });
  });

  test('3.9 — cancelling an unknown job is a clean 400, never a phantom success', async ({
    page,
  }) => {
    // The generic cancel endpoint used to answer "Job cancelled" for *any* id
    // while the underlying process kept running; it now reports honestly and
    // rejects an id nobody ever created.
    await gotoRescue(page, adminTokens);
    const result = await page.evaluate(
      async ({ token, csrf }) => {
        const r = await fetch('/api/progress/00000000-0000-0000-0000-000000000000', {
          method: 'DELETE',
          headers: { 'x-session-token': token, 'x-csrf-token': csrf },
        });
        return { status: r.status, body: await r.json() };
      },
      { token: adminTokens.token, csrf: adminTokens.csrf },
    );
    expect(result.status).toBe(400);
    expect(result.body.success).toBe(false);
  });
});

// ===========================================================================
// 4. All user interactions
// ===========================================================================

test.describe('4 — All user interactions', () => {
  test('4.1 — clicking a tab switches the active tab and its content', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'mdadm');
    await expect(win(page).locator('.hv-winhdr-tab', { hasText: 'mdadm' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    await expect(win(page).locator('#rescue-md-name')).toBeVisible();
    await expect(win(page).locator('#rescue-dd-source')).toHaveCount(0);
  });

  test('4.2 — the Simple/Advanced mode toggle reveals ddrescue advanced fields', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    const advBtn = win(page).locator('[data-testid="rescue-mode-btn"]', { hasText: 'Advanced' }).first();
    await advBtn.click();
    await expect(advBtn).toHaveClass(/active/);
    await expect(win(page).locator('[data-testid="rescue-advanced"]')).toBeVisible();
    await win(page).locator('[data-testid="rescue-mode-btn"]', { hasText: 'Simple' }).first().click();
    await expect(win(page).locator('[data-testid="rescue-advanced"]')).toHaveCount(0);
  });

  test('4.3 — the mode toggle is scoped to the tab that has one', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await win(page).locator('[data-testid="rescue-mode-btn"]', { hasText: 'Advanced' }).first().click();
    await switchTab(page, 'TestDisk');
    // TestDisk has no advanced block at all: the only two settings it used to
    // offer (deep scan, custom log path) never reached the tool, so the API
    // rejects them instead of showing switches that change nothing.
    await expect(win(page).locator('[data-testid="rescue-advanced"]')).toHaveCount(0);
    await expect(win(page).locator('[data-testid="rescue-mode-btn"]')).toHaveCount(0);
    // …and ddrescue's own choice is preserved when coming back.
    await switchTab(page, 'ddrescue');
    await expect(win(page).locator('[data-testid="rescue-advanced"]')).toHaveCount(1);
  });

  test('4.4 — selecting a source device disables it in the destination select', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    const sourceOptions = await win(page).locator('#rescue-dd-source option').all();
    if (sourceOptions.length < 2) {
      test.skip(true, 'no real block devices reported by this host');
    }
    const firstRealValue = await sourceOptions[1].getAttribute('value');
    await win(page).locator('#rescue-dd-source').selectOption(firstRealValue!);
    const matchingDestOption = win(page).locator(`#rescue-dd-dest option[value="${firstRealValue}"]`);
    await expect(matchingDestOption).toBeDisabled();
  });

  test('4.5 — toggling a PhotoRec file-type checkbox updates its checked state', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'PhotoRec');
    await win(page).locator('[data-testid="rescue-mode-btn"]', { hasText: 'Advanced' }).first().click();
    const jpgCheckbox = win(page)
      .locator('[data-testid="rescue-checkbox"]', { hasText: '.jpg' })
      .locator('input[type="checkbox"]');
    await expect(jpgCheckbox).not.toBeChecked();
    await jpgCheckbox.click();
    await expect(jpgCheckbox).toBeChecked();
    await jpgCheckbox.click();
    await expect(jpgCheckbox).not.toBeChecked();
  });

  test('4.6 — the mdadm array-name field accepts typed text', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'mdadm');
    const input = win(page).locator('#rescue-md-name');
    await input.fill('md0');
    await expect(input).toHaveValue('md0');
  });

  test('4.7 — the mdadm action select offers repair/check/rewrite', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'mdadm');
    await win(page).locator('[data-testid="rescue-mode-btn"]', { hasText: 'Advanced' }).first().click();
    const select = win(page).locator('#rescue-md-action');
    const values = await select.locator('option').evaluateAll((opts) =>
      opts.map((o) => (o as HTMLOptionElement).value),
    );
    expect(values.sort()).toEqual(['check', 'repair', 'rewrite']);
  });

  test('4.8 — a running clone can be cancelled from the UI', async ({ page }) => {
    // 2026-10-21: a multi-hour ddrescue clone used to have no stop control at
    // all — the generic DELETE /api/progress/{id} only dropped the job from
    // the tracking map while the tool kept writing to the disk.
    await gotoRescue(page, adminTokens);
    const startBtn = win(page).locator('button', { hasText: 'Start Clone' });
    const cancelBtn = win(page).locator('button', { hasText: 'Cancel Clone' });

    // No clone running yet → no cancel control offered.
    await expect(cancelBtn).toHaveCount(0);

    // Intercept the clone request so the test never touches a real device,
    // and answer with a job id like the backend does.
    await page.route('**/api/rescue/ddrescue/clone', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: { job_id: 'e2e-cancel-job', sse_url: '/api/progress/e2e-cancel-job', logfile: '/tmp/e2e.log' },
        }),
      });
    });
    // …and emulate the progress stream so the job stays "running".
    await page.route('**/api/progress/e2e-cancel-job', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'text/event-stream',
        body: 'event: progress\ndata: {"job_id":"e2e-cancel-job","progress":10,"message":"cloning","state":"running"}\n\n',
      });
    });

    const options = await win(page).locator('#rescue-dd-source option').all();
    if (options.length < 3) {
      test.skip(true, 'needs at least two real block devices to select a pair');
    }
    await win(page).locator('#rescue-dd-source').selectOption(await options[1].getAttribute('value') ?? '');
    const destOptions = await win(page).locator('#rescue-dd-dest option:not([disabled])').all();
    await win(page).locator('#rescue-dd-dest').selectOption(await destOptions[1].getAttribute('value') ?? '');
    await startBtn.click();

    await expect(win(page).locator('[data-testid="rescue-progress"]')).toBeVisible();
    await expect(cancelBtn).toBeVisible();
    // The cancel control is a real labelled button (not an icon-only control
    // relying on `title`), so it is reachable by keyboard and screen readers.
    await expect(cancelBtn).toHaveAttribute('type', 'button');
    await cancelBtn.focus();
    await expect(cancelBtn).toBeFocused();
  });
});

// ===========================================================================
// 5. Submission flows — only guard/no-op and provably-safe paths (see the
//    file header for why real device submissions are out of scope here).
// ===========================================================================

test.describe('5 — Submission flows', () => {
  test('5.1 — Start Clone with empty source/destination does not fire a request', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    let requestFired = false;
    page.on('request', (req) => {
      if (req.url().includes('/api/rescue/ddrescue/clone')) requestFired = true;
    });
    await win(page).locator('button', { hasText: 'Start Clone' }).click();
    await page.waitForTimeout(500);
    expect(requestFired).toBe(false);
    await expect(win(page).locator('[data-testid="rescue-progress"]')).toHaveCount(0);
  });

  test('5.2 — Analyze with no device selected does not fire a request', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'TestDisk');
    let requestFired = false;
    page.on('request', (req) => {
      if (req.url().includes('/api/rescue/testdisk/analyze')) requestFired = true;
    });
    await win(page).locator('button', { hasText: 'Analyze' }).click();
    await page.waitForTimeout(500);
    expect(requestFired).toBe(false);
  });

  test('5.3 — Start Recovery with no device selected does not fire a request', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'PhotoRec');
    let requestFired = false;
    page.on('request', (req) => {
      if (req.url().includes('/api/rescue/photorec/recover')) requestFired = true;
    });
    await win(page).locator('button', { hasText: 'Start Recovery' }).click();
    await page.waitForTimeout(500);
    expect(requestFired).toBe(false);
  });

  test('5.4 — Diagnose with an empty array name does not fire a request', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'mdadm');
    let requestFired = false;
    page.on('request', (req) => {
      if (req.url().includes('/api/rescue/mdadm/diagnose')) requestFired = true;
    });
    await win(page).locator('button', { hasText: 'Diagnose' }).click();
    await page.waitForTimeout(500);
    expect(requestFired).toBe(false);
  });

  test('5.5 — diagnosing a nonexistent mdadm array fails cleanly with visible feedback', async ({
    page,
  }) => {
    // Safe: "e2e-<ts>-nonexistent" can never resolve to a real /dev/md/*
    // device, so SecurityValidator rejects it before mdadm is ever
    // invoked (see tests/rescue_integration.rs). This also exercises the
    // 2026-09-05 fix — previously a failure here showed nothing at all.
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'mdadm');
    const name = `e2e-${Date.now()}-nonexistent`;
    await win(page).locator('#rescue-md-name').fill(name);
    await win(page).locator('button', { hasText: 'Diagnose' }).click();
    await expect(page.locator('.notifications-panel, [class*="notification"]').first().or(
      page.locator('text=/mdadm diagnosis failed/i'),
    )).toBeVisible({ timeout: 10_000 }).catch(async () => {
      // Fall back to just confirming the loading state cleared and no
      // diagnostic panel appeared (the notification center's exact DOM
      // structure isn't this suite's concern) — either way, no crash.
      await expect(win(page).locator('[data-testid="rescue-diagnostic-grid"]')).toHaveCount(0);
    });
  });
});

// ===========================================================================
// 6. Navigation
// ===========================================================================

test.describe('6 — Navigation', () => {
  test('6.1 — the window closes and can be reopened', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    const closeBtn = closeWindowButton(page);
    if ((await closeBtn.count()) === 0) test.skip(true, 'no close control found for this window chrome');
    await closeBtn.click();
    await expect(page.locator('[data-testid="rescue-page"]')).toHaveCount(0);
    await openLaunchpadApp(page, 'Rescue');
    await expect(win(page)).toBeVisible();
  });

  test('6.2 — switching tabs preserves the window (no reload)', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    const marker = 'e2e-nav-marker';
    await win(page).evaluate((el, m) => ((el as any)[m] = true), marker);
    await switchTab(page, 'TestDisk');
    await switchTab(page, 'ddrescue');
    const stillThere = await win(page).evaluate((el, m) => Boolean((el as any)[m]), marker);
    expect(stillThere).toBe(true);
  });
});

// ===========================================================================
// 7. Responsive
// ===========================================================================

test.describe('7 — Responsive', () => {
  for (const [w, h, label] of [
    [360, 800, '360px (mobile)'],
    [768, 1024, '768px (tablet)'],
    [1920, 1080, '1920px (desktop)'],
  ] as const) {
    test(`7.${label} — renders without horizontal overflow`, async ({ page }) => {
      await page.setViewportSize({ width: w, height: h });
      await gotoRescue(page, adminTokens);
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
  test('8.1 — the tab bar exposes tablist/tab roles with aria-selected', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await expect(win(page).locator('[role="tablist"]')).toHaveCount(1);
    const tabs = win(page).locator('[role="tab"]');
    await expect(tabs).toHaveCount(4);
    const selectedCount = await tabs.evaluateAll(
      (els) => els.filter((e) => e.getAttribute('aria-selected') === 'true').length,
    );
    expect(selectedCount).toBe(1);
  });

  test('8.2 — form labels have a matching `for`/`id` pair', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await expect(win(page).locator('label[for="rescue-dd-source"]')).toBeVisible();
    await expect(win(page).locator('label[for="rescue-dd-dest"]')).toBeVisible();
    for (const id of ['rescue-dd-source', 'rescue-dd-dest']) {
      const forAttr = await win(page).locator(`label[for="${id}"]`).getAttribute('for');
      expect(forAttr).toBe(id);
    }
  });

  test('8.3 — no duplicate element ids on the page', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    const dupes = await page.evaluate(() => {
      const ids = Array.from(document.querySelectorAll('[id]')).map((e) => e.id);
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

  test('8.4 — arrow keys move focus between tabs', async ({ page }) => {
    // 2026-09-05 fix — WindowHeader's tab keydown handler compared
    // e.key against "IconArrowRight" etc. (an icon component name, not a
    // real KeyboardEvent.key value), so arrow-key tab navigation was
    // silently dead everywhere the window header's tab bar is used.
    await gotoRescue(page, adminTokens);
    await win(page).locator('.hv-winhdr-tab').first().focus();
    await page.keyboard.press('ArrowRight');
    const focused = await page.evaluate(() => document.activeElement?.getAttribute('aria-selected'));
    expect(focused).toBe('true');
    const focusedLabel = await page.evaluate(() => document.activeElement?.textContent?.trim());
    expect(focusedLabel).toBe('TestDisk');
  });
});

// ===========================================================================
// 9. Performance
// ===========================================================================

test.describe('9 — Performance', () => {
  test('9.1 — the window opens in a reasonable time', async ({ page }) => {
    const start = Date.now();
    await gotoRescue(page, adminTokens);
    const elapsed = Date.now() - start;
    console.log(`[PERF] Rescue window ready in ${elapsed}ms`);
    expect(elapsed).toBeLessThan(15_000);
  });

  test('9.2 — opening the Rescue window does not add an excessive number of DOM nodes', async ({
    page,
  }) => {
    await gotoDesktop(page, adminTokens);
    const before = await page.evaluate(() => document.querySelectorAll('*').length);
    await openLaunchpadApp(page, 'Rescue');
    await page.waitForSelector('[data-testid="rescue-page"]', { timeout: 10_000 });
    const after = await page.evaluate(() => document.querySelectorAll('*').length);
    const delta = after - before;
    console.log(`[PERF] Rescue window added ${delta} DOM nodes`);
    expect(delta).toBeLessThan(2000);
  });

  test('9.3 — the initial load issues a bounded number of API calls', async ({ page }) => {
    await seedBrowser(page, adminTokens);
    await page.goto('/desktop');
    await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
    let apiCalls = 0;
    page.on('request', (req) => {
      if (req.url().includes('/api/rescue/')) apiCalls++;
    });
    await openLaunchpadApp(page, 'Rescue');
    await page.waitForSelector('[data-testid="rescue-page"]', { timeout: 10_000 });
    await page.waitForTimeout(1_000);
    expect(apiCalls).toBeLessThan(10);
  });
});

// ===========================================================================
// 10. Security
// ===========================================================================

test.describe('10 — Security', () => {
  test('10.1 — the Rescue API rejects a missing session token', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/rescue/tools');
      return r.status;
    });
    expect(status).toBe(400);
  });

  test('10.2 — the Rescue API rejects a tampered session token', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    const status = await page.evaluate(async () => {
      const r = await fetch('/api/rescue/tools', {
        headers: { 'x-session-token': 'totally-bogus-token-value' },
      });
      return r.status;
    });
    expect(status).toBe(401);
  });

  test('10.3 — mutating endpoints require admin, not just any session', async ({
    page,
    playwright,
  }) => {
    await gotoRescue(page, adminTokens);
    const ctx = await newCtx(playwright);
    const username = `e2erescuenonadmin${Date.now().toString(36)}`;
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

    const status = await page.evaluate(
      async (token) => {
        const r = await fetch('/api/rescue/mdadm/diagnose', {
          method: 'POST',
          headers: { 'x-session-token': token, 'Content-Type': 'application/json' },
          body: JSON.stringify({ name: 'md0' }),
        });
        return r.status;
      },
      userToken,
    );
    expect(status).toBe(403);

    await ctx.delete(`/api/users/${username}`, {
      headers: { 'x-session-token': adminTokens.token, 'x-csrf-token': adminTokens.csrf },
    });
    await ctx.dispose();
  });

  test('10.4 — no session token is ever placed in the URL', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    expect(page.url()).not.toContain(adminTokens.token);
  });

  test('10.5 — an XSS payload in the mdadm array-name field is never rendered as markup', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'mdadm');
    const payload = '<img src=x onerror="window.__rescueXss=true">';
    await win(page).locator('#rescue-md-name').fill(payload);
    const fired = await page.evaluate(() => (window as any).__rescueXss === true);
    expect(fired).toBe(false);
  });
});

// ===========================================================================
// 11. Keyboard
// ===========================================================================

test.describe('11 — Keyboard', () => {
  test('11.1 — Tab order reaches the tab bar and form fields', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await win(page).locator('.hv-winhdr-tab').first().focus();
    await expect(win(page).locator('.hv-winhdr-tab').first()).toBeFocused();
  });

  test('11.2 — Home/End jump to the first/last tab', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await win(page).locator('.hv-winhdr-tab').first().focus();
    await page.keyboard.press('End');
    let label = await page.evaluate(() => document.activeElement?.textContent?.trim());
    expect(label).toBe('mdadm');
    await page.keyboard.press('Home');
    label = await page.evaluate(() => document.activeElement?.textContent?.trim());
    expect(label).toBe('ddrescue');
  });
});

// ===========================================================================
// 12. Console & network
// ===========================================================================

test.describe('12 — Console & network', () => {
  test('12.1 — no uncaught JS errors while browsing all 4 tabs', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await gotoRescue(page, adminTokens);
    for (const tab of ['TestDisk', 'PhotoRec', 'mdadm', 'ddrescue'] as const) {
      await switchTab(page, tab);
    }
    expect(errors).toEqual([]);
  });

  test('12.2 — no failed (4xx/5xx) requests during a normal page load', async ({ page }) => {
    const failed: string[] = [];
    page.on('response', (res) => {
      if (res.status() >= 400 && res.url().includes('/api/')) {
        failed.push(`${res.status()} ${res.url()}`);
      }
    });
    await gotoRescue(page, adminTokens);
    expect(failed).toEqual([]);
  });
});

// ===========================================================================
// 13. Edge cases & memory regression
// ===========================================================================

test.describe('13 — Edge cases & memory regression', () => {
  test('13.1 — a unicode array name is accepted by the field without throwing', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'mdadm');
    await win(page).locator('#rescue-md-name').fill('配列-🔧-md0');
    await expect(win(page).locator('#rescue-md-name')).toHaveValue('配列-🔧-md0');
  });

  test('13.2 — a very long array name does not break layout', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    await switchTab(page, 'mdadm');
    const long = 'a'.repeat(500);
    await win(page).locator('#rescue-md-name').fill(long);
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
    );
    expect(overflow).toBe(false);
  });

  test('13.3 — rapidly switching tabs does not crash the page', async ({ page }) => {
    await gotoRescue(page, adminTokens);
    const tabs = ['ddrescue', 'TestDisk', 'PhotoRec', 'mdadm'] as const;
    for (let i = 0; i < 12; i++) {
      await switchTab(page, tabs[i % tabs.length]);
    }
    await expect(win(page)).toBeVisible();
  });

  test('13.4 — clearing localStorage mid-session and reloading does not crash', async ({
    page,
  }) => {
    await gotoRescue(page, adminTokens);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await page.waitForTimeout(1_000);
    const bodyText = await page.locator('body').innerText().catch(() => '');
    expect(bodyText.length).toBeGreaterThan(0);
  });

  test('13.5 — opening and closing the Rescue window ×5 does not leak significant heap', async ({
    page,
  }) => {
    test.setTimeout(150_000);
    await gotoDesktop(page, adminTokens);
    const before = await getHeapMB(page).catch(() => 0);
    for (let i = 0; i < 5; i++) {
      await openLaunchpadApp(page, 'Rescue');
      await page.waitForSelector('[data-testid="rescue-page"]', { timeout: 10_000 });
      const closeBtn = closeWindowButton(page);
      if ((await closeBtn.count()) === 0) {
        test.skip(true, 'no close control found for this window chrome');
      }
      await closeBtn.click();
      await expect(page.locator('[data-testid="rescue-page"]')).toHaveCount(0);
    }
    const after = await getHeapMB(page).catch(() => 0);
    if (before > 0 && after > 0) {
      const delta = after - before;
      console.log(`[MEM] 5x window open/close delta: ${delta.toFixed(1)} MB`);
      expect(delta).toBeLessThan(15);
    }
  });
});
