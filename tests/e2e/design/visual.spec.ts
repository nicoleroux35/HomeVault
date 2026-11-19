/**
 * D3 — visual regression at three widths (360 / 768 / 1280).
 *
 * Every app window is screenshotted at a phone width, a tablet width and a
 * desktop width. A layout that only collapses at 360px (the W15 family) now
 * fails here instead of being found by eye. Baselines are committed; update
 * them intentionally with:
 *
 *   npx playwright test --config=playwright.visual.config.ts --update-snapshots
 *
 * and review the diff before committing.
 */
import { test, expect, request } from '@playwright/test';
import {
  ADMIN_PASS,
  ADMIN_USER,
  APP_IDS,
  apiLogin,
  closeLastWindow,
  gotoDesktop,
  openApp,
  resetShippedAppearance,
  type Tokens,
} from './designGates';

test.describe.configure({ mode: 'serial' });

let tokens: Tokens;

test.beforeAll(async () => {
  const ctx = await request.newContext({ baseURL: process.env.BASE_URL || 'http://localhost:8080' });
  tokens = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  await ctx.dispose();
  expect(tokens.token, 'the seeded admin must log in — is the server seeded?').toBeTruthy();
  // Screenshots must show the shipped look, not an accent a previous spec saved.
  await resetShippedAppearance(tokens);
});

/** Freeze everything that animates or ticks so two runs are comparable. */
async function stabilise(page: import('@playwright/test').Page) {
  await page.addStyleTag({
    content: `
      *,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}
      [class*="-monitor"]{display:none!important;pointer-events:none!important}
    `,
  });
  await page.waitForTimeout(500);
}

/**
 * Some screens render live, host-owned data. A strict pixel ratio would make
 * those baselines flaky without catching a layout regression, so they get a
 * wider tolerance:
 *
 * - `journal` — the log rows grow as the host logs;
 * - `dashboard` — live CPU/RAM gauges and container counts;
 * - `storage` — the disk/partition table mirrors this machine's devices;
 * - `docker` — image/container/volume counts come from the daemon;
 * - `network` / `vpn` — interface names, states and addresses belong to the
 *   runner (a container's `eth0`/`lo` differ from a VM's `ens5`, and the
 *   default route/MTU change with them);
 * - `traffic-control` — the "Shaping is stopped" notice adds one line naming
 *   the `tc` tool (`iproute2`) when the runner does not have it;
 * - `rclone` — the remote cards are the *host's* `/etc/rclone.conf`: a runner
 *   with two configured remotes shows two cards where the CI container shows
 *   the "no remotes" empty state;
 * - `shares` — the list is the daemon's own config, so a host with Samba
 *   installed lists its distro default `[printers]` / `[print$]` next to the
 *   app's own shares, while the CI container lists none;
 *
 * Two screens are deliberately NOT in the budget below because their body is
 * not host *data* but host *capability*: `backup` and `destinations` switch
 * between "Restic Not Installed" install hints and the real empty state
 * depending on whether `restic` / `rest-server` are on the server's PATH, and
 * that swaps the whole card (a taller notice stack shifts everything below it,
 * so no pixel budget can absorb it without hiding a reflow). The committed
 * baselines are the CI ones — a root container with neither tool — so a
 * workstation that has restic installed must run the server without it
 * (`PATH=/usr/local/bin:/usr/bin:/bin` before `hv-test.sh serve`) to match.
 *
 * The baselines were captured on a freshly seeded database (`Reset server +
 * seed before the visual gates`), so what remains is the host itself. A layout
 * regression moves far more than these budgets.
 */
const TOLERANCE: Partial<Record<(typeof APP_IDS)[number], number>> = {
  journal: 0.12,
  dashboard: 0.08,
  storage: 0.08,
  docker: 0.08,
  network: 0.08,
  vpn: 0.08,
  'traffic-control': 0.1,
  rclone: 0.1,
  shares: 0.1,
};

/**
 * Default pixel budget. 2 % keeps the gate honest for layout (a moved card, a
 * clipped table or a missing control covers far more) while absorbing the
 * few-per-hundred-pixels of host-dependent text (uptimes, counters, file
 * sizes) that every screen can pick up.
 */
const DEFAULT_TOLERANCE = 0.02;

test('desktop shell at this width', async ({ page }) => {
  await gotoDesktop(page, tokens);
  await stabilise(page);
  // The clock and the live resource gauges tick; mask them so the baseline
  // asserts layout, not the second the screenshot was taken.
  await expect(page).toHaveScreenshot('desktop.png', {
    mask: [page.locator('.hv-menubar'), page.locator('[class*="live"], [class*="gauge"]')],
    maxDiffPixelRatio: 0.01,
  });
});

for (const appId of APP_IDS) {
  test(`${appId} at this width`, async ({ page }) => {
    await gotoDesktop(page, tokens);
    await openApp(page, appId);
    await stabilise(page);
    try {
      await expect(page.locator('.hv-window').last()).toHaveScreenshot(`${appId}.png`, {
        maxDiffPixelRatio: TOLERANCE[appId] ?? DEFAULT_TOLERANCE,
      });
    } finally {
      await closeLastWindow(page).catch(() => {});
    }
  });
}
