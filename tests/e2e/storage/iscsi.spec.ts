/* =============================================================================
 * File          : iscsi.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   iSCSI screen end-to-end tests (Playwright): opening the app, the host
 *   status (usable and unusable), creating a disk, creating an export with a
 *   LUN and a CHAP initiator, the guards that keep an in-use disk from being
 *   forgotten, and apply-now.
 *
 *   Every backend call is intercepted with `page.route`, so the suite never
 *   runs `targetcli` on the machine that hosts the test run and the
 *   assertions are deterministic. The real endpoint, auth matrix, validation
 *   and sealing live in `tests/iscsi_integration.rs`; the command pipeline
 *   lives in the `iscsi.rs` unit tests.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/storage/iscsi.spec.ts
 * =============================================================================
 */

import { test, expect, type Page, type APIRequestContext, type Route } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

interface Tokens {
  token: string;
  csrf: string;
}

async function apiLogin(ctx: APIRequestContext): Promise<Tokens> {
  const res = await ctx.post('/api/auth/login', {
    data: { username: ADMIN_USER, password: ADMIN_PASS },
  });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

async function gotoDesktop(page: Page, tokens: Tokens) {
  await page.addInitScript((t) => {
    try {
      localStorage.setItem('session-token', t.token);
      localStorage.setItem('csrf-token', t.csrf);
    } catch {
      /* storage unavailable — the test will surface it */
    }
  }, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  // Monitor widgets overlap the dock on this layout — hide them so clicks land.
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

/** `[data-testid="iscsi-page"]` is unique to Iscsi.svelte. */
function win(page: Page) {
  return page.locator('[data-testid="iscsi-page"]');
}

/** A freshly seeded server answers `GET /api/whats-new` with `seen: false`, so
 *  the desktop opens a bits-ui modal whose full-screen overlay swallows the
 *  first dock click. Close it if it is there. */
async function dismissWhatsNew(page: Page) {
  const modal = page.locator('[data-testid="whats-new-modal"]');
  if (await modal.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await modal.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

const STORE = {
  id: 1,
  name: 'data',
  kind: 'fileio',
  path: '/srv/homevault/data/iscsi/data.img',
  size_bytes: 10 * 1024 ** 3,
  readonly: false,
  created_at: '2026-11-01T10:00:00Z',
  updated_at: '2026-11-01T10:00:00Z',
  lun_count: 1,
};

const SPARE = {
  id: 2,
  name: 'spare',
  kind: 'fileio',
  path: '/srv/homevault/data/iscsi/spare.img',
  size_bytes: 5 * 1024 ** 3,
  readonly: false,
  created_at: '2026-11-01T10:00:00Z',
  updated_at: '2026-11-01T10:00:00Z',
  lun_count: 0,
};

const TARGET = {
  id: 5,
  iqn: 'iqn.2026-11.local.homevault:disk0',
  enabled: true,
  created_at: '2026-11-01T10:00:00Z',
  updated_at: '2026-11-01T10:00:00Z',
  portals: [{ id: 1, target_id: 5, address: '0.0.0.0', port: 3260 }],
  luns: [{ id: 1, target_id: 5, lun: 0, backstore_id: 1, backstore_name: 'data' }],
  acls: [
    {
      id: 1,
      target_id: 5,
      initiator_iqn: 'iqn.2026-11.local.client:laptop',
      userid: 'homevault',
      has_secret: true,
    },
  ],
};

function usableCaps() {
  return {
    kernel_available: true,
    configfs_available: true,
    targetcli_available: true,
    modules_loaded: ['target_core_mod', 'iscsi_target_mod'],
    modules_missing: [],
    usable: true,
  };
}

function unusableCaps() {
  return {
    kernel_available: false,
    configfs_available: false,
    targetcli_available: false,
    modules_loaded: [],
    modules_missing: ['target_core_mod', 'iscsi_target_mod'],
    usable: false,
  };
}

interface Fixture {
  enabled?: boolean;
  caps?: ReturnType<typeof usableCaps> | ReturnType<typeof unusableCaps>;
  stores?: any[];
  targets?: any[];
  pending?: boolean;
  /** Thin-volume support: which ZFS pools the host offers. */
  zvol?: { available: boolean; pools: string[] };
  /** Snapshots returned for the zvol snapshot panel. */
  snapshots?: any[];
  /** Overrides the status returned by the next mutation. */
  refuse?: { status: number; message: string };
}

interface Recorder {
  storeCreates: any[];
  targetCreates: any[];
  settings: any[];
  deletes: string[];
  applies: number;
}

function buildPayload(
  f: Required<
    Pick<Fixture, 'enabled' | 'caps' | 'stores' | 'targets' | 'pending' | 'zvol'>
  >,
) {
  return {
    settings: {
      enabled: f.enabled,
      applied_at: f.pending ? null : '2026-11-01T10:00:00Z',
      updated_at: '2026-11-01T10:00:00Z',
    },
    capabilities: f.caps,
    backstores: f.stores,
    targets: f.targets,
    defaults: {
      portal_address: '0.0.0.0',
      portal_port: 3260,
      iqn: 'iqn.2026-11.local.homevault:disk1',
    },
    paths: {
      fileio_root: '/srv/homevault/data/iscsi',
      allowed_roots: ['/srv/homevault/data', '/mnt/pool1'],
    },
    zvol: {
      available: f.zvol.available,
      pools: f.zvol.pools,
      parent: 'homevault-iscsi',
    },
    state: {
      usable: f.caps.usable,
      enabled: f.enabled,
      pending: f.pending,
      applied_at: f.pending ? null : '2026-11-01T10:00:00Z',
      exported_targets: f.targets.filter((t) => t.enabled).length,
    },
  };
}

/** Intercepts every iSCSI call and records what the page sent. */
async function mockApi(page: Page, fixture: Fixture = {}): Promise<Recorder> {
  const recorder: Recorder = {
    storeCreates: [],
    targetCreates: [],
    settings: [],
    deletes: [],
    applies: 0,
  };
  const state = {
    enabled: fixture.enabled ?? true,
    caps: fixture.caps ?? usableCaps(),
    stores: fixture.stores ?? [STORE, SPARE],
    targets: fixture.targets ?? [TARGET],
    pending: fixture.pending ?? false,
    zvol: fixture.zvol ?? { available: false, pools: [] },
    snapshots: fixture.snapshots ?? [],
  };

  const ok = (extra: any = {}) => ({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({
      success: true,
      data: { ...buildPayload(state), ...extra },
    }),
  });
  const refuse = (status: number, message: string) => ({
    status,
    contentType: 'application/json',
    body: JSON.stringify({ success: false, message, data: null }),
  });

  await page.route('**/api/storage/iscsi**', async (route: Route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();

    if (method === 'GET' && path.endsWith('/api/storage/iscsi')) {
      return route.fulfill(ok());
    }

    if (fixture.refuse) {
      const { status, message } = fixture.refuse;
      fixture.refuse = undefined;
      return route.fulfill(refuse(status, message));
    }

    if (method === 'PUT' && path.endsWith('/settings')) {
      const body = request.postDataJSON();
      recorder.settings.push(body);
      state.enabled = !!body.enabled;
      return route.fulfill(ok());
    }

    if (method === 'POST' && path.endsWith('/apply')) {
      recorder.applies += 1;
      state.pending = false;
      return route.fulfill(
        ok({
          apply: {
            applied: true,
            reason: null,
            reason_code: null,
            commands: ['sudo targetcli /iscsi create iqn.2026-11.local.homevault:disk0'],
            failed: null,
          },
        }),
      );
    }

    // Thin volumes: snapshots and clones.
    if (path.includes('/snapshots')) {
      if (method === 'GET') {
        return route.fulfill(ok({ snapshots: state.snapshots }));
      }
      if (method === 'POST') {
        const body = request.postDataJSON();
        state.snapshots = [
          ...state.snapshots,
          { name: `tank/homevault-iscsi/disk1@${body.name}`, short_name: body.name, created: '1762800000' },
        ];
        return route.fulfill(ok({ snapshot: body.name }));
      }
      return route.fulfill(ok({ deleted: true }));
    }
    if (method === 'POST' && path.endsWith('/clone')) {
      recorder.storeCreates.push(request.postDataJSON());
      return route.fulfill(ok());
    }

    if (method === 'POST' && path.endsWith('/backstores')) {
      const body = request.postDataJSON();
      recorder.storeCreates.push(body);
      const created = {
        id: 9,
        name: body.name,
        kind: body.kind,
        path: body.path,
        size_bytes: body.size_bytes,
        readonly: !!body.readonly,
        created_at: '2026-11-01T11:00:00Z',
        updated_at: '2026-11-01T11:00:00Z',
        lun_count: 0,
      };
      state.stores = [...state.stores, created];
      return route.fulfill(ok());
    }

    if (method === 'POST' && path.endsWith('/targets')) {
      const body = request.postDataJSON();
      recorder.targetCreates.push(body);
      state.targets = [
        ...state.targets,
        {
          id: 6,
          iqn: body.iqn,
          enabled: body.enabled,
          created_at: '2026-11-01T11:00:00Z',
          updated_at: '2026-11-01T11:00:00Z',
          portals: body.portals.map((p: any, i: number) => ({ id: i + 1, target_id: 6, ...p })),
          luns: body.luns.map((l: any, i: number) => ({
            id: i + 1,
            target_id: 6,
            lun: l.lun,
            backstore_id: l.backstore_id,
            backstore_name: state.stores.find((s) => s.id === l.backstore_id)?.name ?? '?',
          })),
          acls: body.acls.map((a: any, i: number) => ({
            id: i + 1,
            target_id: 6,
            initiator_iqn: a.initiator_iqn,
            userid: a.userid,
            has_secret: !!a.secret,
          })),
        },
      ];
      return route.fulfill(ok());
    }

    if (method === 'DELETE') {
      recorder.deletes.push(request.url());
      if (path.includes('/backstores/')) {
        const id = Number(path.split('/').pop());
        state.stores = state.stores.filter((s) => s.id !== id);
      } else if (path.includes('/targets/')) {
        const id = Number(path.split('/').pop());
        state.targets = state.targets.filter((t) => t.id !== id);
      }
      return route.fulfill(ok());
    }

    return route.fulfill(ok());
  });

  return recorder;
}

let tokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  const ctx = await playwright.request.newContext({ baseURL: BASE });
  tokens = await apiLogin(ctx);
  if (tokens.token) {
    // Acknowledge the release once, so the What's New overlay never sits
    // between a test and its first dock click.
    await ctx
      .post('/api/whats-new/seen', {
        headers: { 'X-Session-Token': tokens.token, 'X-CSRF-Token': tokens.csrf },
        data: {},
      })
      .catch(() => null);
  }
  await ctx.dispose();
  if (!tokens.token) {
    throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  }
});

test.beforeEach(() => {
  test.setTimeout(60_000);
});

async function openIscsi(page: Page) {
  await gotoDesktop(page, tokens);
  await dismissWhatsNew(page);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[data-app-id="iscsi"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="iscsi-page"]', { timeout: 15_000 });
  await expect(win(page).locator('[data-testid="iscsi-content"]')).toBeVisible({ timeout: 15_000 });
}

// ===========================================================================
// 1. Load and status
// ===========================================================================

test.describe('1 — Load and status', () => {
  test('1.1 — the app opens from the launcher and lists disks and exports', async ({ page }) => {
    await mockApi(page);
    await openIscsi(page);

    await expect(win(page).locator('[data-testid="iscsi-backstore-1"]')).toContainText('data');
    await expect(win(page).locator('[data-testid="iscsi-backstore-1"]')).toContainText('10 GiB');
    await expect(win(page).locator('[data-testid="iscsi-backstore-1"]')).toContainText(
      'used by 1 LUN',
    );
    await expect(win(page).locator('[data-testid="iscsi-target-5"]')).toContainText(
      TARGET.iqn,
    );
  });

  test('1.2 — the status region is a live region reporting the reachable exports', async ({
    page,
  }) => {
    await mockApi(page);
    await openIscsi(page);
    const status = win(page).locator('[data-testid="iscsi-status"]');
    await expect(status).toHaveAttribute('role', 'status');
    await expect(status).toContainText('1 export(s) reachable');
  });

  test('1.3 — saved-but-not-applied changes say so', async ({ page }) => {
    await mockApi(page, { pending: true });
    await openIscsi(page);
    const status = win(page).locator('[data-testid="iscsi-status"]');
    await expect(status).toContainText('Changes are saved, not applied yet');
  });

  test('1.4 — a host without the LIO stack hides the editors and names what is missing', async ({
    page,
  }) => {
    await mockApi(page, { caps: unusableCaps(), stores: [], targets: [], enabled: false });
    await openIscsi(page);

    await expect(win(page).locator('[data-testid="iscsi-unusable"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="iscsi-missing-targetcli"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="iscsi-missing-configfs"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="iscsi-missing-modules"]')).toContainText(
      'target_core_mod',
    );
    // The editors are gone, not merely disabled: there is nothing to save.
    await expect(win(page).locator('[data-testid="iscsi-backstores"]')).toHaveCount(0);
    await expect(win(page).locator('[data-testid="iscsi-targets"]')).toHaveCount(0);
    await expect(win(page).locator('[data-testid="iscsi-apply"]')).toBeDisabled();
  });
});

// ===========================================================================
// 2. Creating a disk
// ===========================================================================

test.describe('2 — Disks', () => {
  test('2.1 — adding a disk sends the size in bytes', async ({ page }) => {
    const recorder = await mockApi(page);
    await openIscsi(page);

    await win(page).locator('[data-testid="iscsi-add-backstore"]').click();
    const modal = page.locator('[data-testid="iscsi-backstore-modal"]');
    await expect(modal).toBeVisible();

    await page.locator('[data-testid="iscsi-backstore-name"]').fill('backup');
    await page
      .locator('[data-testid="iscsi-backstore-path"]')
      .fill('/srv/homevault/data/iscsi/backup.img');
    await page.locator('[data-testid="iscsi-backstore-size"]').fill('250');
    await page.locator('[data-testid="iscsi-backstore-save"]').click();

    await expect.poll(() => recorder.storeCreates.length).toBe(1);
    const body = recorder.storeCreates[0];
    expect(body.name).toBe('backup');
    expect(body.kind).toBe('fileio');
    expect(body.path).toBe('/srv/homevault/data/iscsi/backup.img');
    expect(body.size_bytes).toBe(250 * 1024 ** 3);
    expect(body.readonly).toBe(false);

    await expect(modal).toBeHidden();
    await expect(win(page).locator('[data-testid="iscsi-backstore-9"]')).toContainText('backup');
  });

  test('2.2 — a refused save keeps the form open and shows the reason', async ({ page }) => {
    const recorder = await mockApi(page, {
      refuse: { status: 400, message: "'backup.img' is already used by another disk" },
    });
    await openIscsi(page);

    await win(page).locator('[data-testid="iscsi-add-backstore"]').click();
    await page.locator('[data-testid="iscsi-backstore-name"]').fill('backup');
    await page
      .locator('[data-testid="iscsi-backstore-path"]')
      .fill('/srv/homevault/data/iscsi/backup.img');
    await page.locator('[data-testid="iscsi-backstore-save"]').click();

    await expect(page.locator('[data-testid="iscsi-backstore-error"]')).toContainText(
      'is already used by another disk',
    );
    await expect(page.locator('[data-testid="iscsi-backstore-modal"]')).toBeVisible();
    expect(recorder.storeCreates).toHaveLength(0);
  });
});

// ===========================================================================
// 3. Creating an export
// ===========================================================================

test.describe('3 — Exports', () => {
  test('3.1 — a new export prefills the portal and sends LUN and CHAP', async ({ page }) => {
    const recorder = await mockApi(page);
    await openIscsi(page);

    await win(page).locator('[data-testid="iscsi-add-target"]').click();
    const modal = page.locator('[data-testid="iscsi-target-modal"]');
    await expect(modal).toBeVisible();

    // The IQN and the first portal come from the server defaults.
    await expect(page.locator('[data-testid="iscsi-target-iqn"]')).toHaveValue(
      'iqn.2026-11.local.homevault:disk1',
    );
    await expect(page.locator('[data-testid="iscsi-portal-port-0"]')).toHaveValue('3260');

    await page.locator('[data-testid="iscsi-lun-add"]').click();
    await expect(page.locator('[data-testid="iscsi-lun-backstore-0"]')).toHaveValue('1');

    await page.locator('[data-testid="iscsi-acl-add"]').click();
    await page
      .locator('[data-testid="iscsi-acl-initiator-0"]')
      .fill('iqn.2026-11.local.client:desktop');
    await page.locator('[data-testid="iscsi-acl-userid-0"]').fill('homevault');
    await page.locator('[data-testid="iscsi-acl-secret-0"]').fill('TwelveChars!');

    await page.locator('[data-testid="iscsi-target-save"]').click();

    await expect.poll(() => recorder.targetCreates.length).toBe(1);
    const body = recorder.targetCreates[0];
    expect(body.iqn).toBe('iqn.2026-11.local.homevault:disk1');
    expect(body.enabled).toBe(true);
    expect(body.portals).toEqual([{ address: '0.0.0.0', port: 3260 }]);
    expect(body.luns).toEqual([{ lun: 0, backstore_id: 1 }]);
    expect(body.acls).toEqual([
      {
        initiator_iqn: 'iqn.2026-11.local.client:desktop',
        userid: 'homevault',
        secret: 'TwelveChars!',
      },
    ]);

    await expect(modal).toBeHidden();
    await expect(win(page).locator('[data-testid="iscsi-target-6"]')).toContainText(
      'iqn.2026-11.local.homevault:disk1',
    );
  });

  test('3.2 — an export with no initiator sends an empty list (everyone allowed)', async ({
    page,
  }) => {
    const recorder = await mockApi(page);
    await openIscsi(page);

    await win(page).locator('[data-testid="iscsi-add-target"]').click();
    await page.locator('[data-testid="iscsi-lun-add"]').click();
    await page.locator('[data-testid="iscsi-target-save"]').click();

    await expect.poll(() => recorder.targetCreates.length).toBe(1);
    expect(recorder.targetCreates[0].acls).toEqual([]);
  });

  test('3.3 — the LUN editor says a disk is needed first', async ({ page }) => {
    await mockApi(page, { stores: [], targets: [] });
    await openIscsi(page);

    await win(page).locator('[data-testid="iscsi-add-target"]').click();
    await expect(page.locator('[data-testid="iscsi-lun-no-backstores"]')).toBeVisible();
    await expect(page.locator('[data-testid="iscsi-lun-add"]')).toBeDisabled();
  });
});

// ===========================================================================
// 4. Delete guards
// ===========================================================================

test.describe('4 — Deleting', () => {
  test('4.1 — a disk a LUN still serves is refused without a request', async ({ page }) => {
    const recorder = await mockApi(page);
    await openIscsi(page);

    await win(page).locator('[data-testid="iscsi-backstore-delete-1"]').click();

    const toast = page.locator('[data-sonner-toast][data-type="error"]').first();
    await expect(toast).toBeVisible({ timeout: 8_000 });
    await expect(toast).toContainText('still used by 1 LUN');
    await expect(page.locator('[data-testid="iscsi-delete-store-modal"]')).toHaveCount(0);
    expect(recorder.deletes).toHaveLength(0);
  });

  test('4.2 — deleting the backing file needs the typed name', async ({ page }) => {
    const recorder = await mockApi(page);
    await openIscsi(page);

    await win(page).locator('[data-testid="iscsi-backstore-delete-2"]').click();
    const modal = page.locator('[data-testid="iscsi-delete-store-modal"]');
    await expect(modal).toBeVisible();

    const confirm = page.locator('[data-testid="iscsi-delete-store-confirm"]');
    await expect(confirm).toBeEnabled();

    // Asking for the file too arms the typed confirmation.
    await modal.getByRole('checkbox').click();
    await expect(confirm).toBeDisabled();

    await page.locator('[data-testid="iscsi-delete-confirm-name"]').fill('spare');
    await expect(confirm).toBeEnabled();
    await confirm.click();

    await expect.poll(() => recorder.deletes.length).toBe(1);
    expect(recorder.deletes[0]).toContain('/backstores/2');
    expect(recorder.deletes[0]).toContain('delete_file=true');
    expect(recorder.deletes[0]).toContain('confirm=spare');
    await expect(modal).toBeHidden();
    await expect(win(page).locator('[data-testid="iscsi-backstore-2"]')).toHaveCount(0);
  });
});

// ===========================================================================
// 5. Apply
// ===========================================================================

test.describe('5 — Apply', () => {
  test('5.1 — apply-now publishes the pending exports', async ({ page }) => {
    const recorder = await mockApi(page, { pending: true });
    await openIscsi(page);

    await expect(win(page).locator('[data-testid="iscsi-pending"]')).toBeVisible();
    await win(page).locator('[data-testid="iscsi-apply"]').click();

    await expect.poll(() => recorder.applies).toBe(1);
    await expect(win(page).locator('[data-testid="iscsi-active"]')).toBeVisible();
  });

  test('5.2 — the feature switch saves immediately', async ({ page }) => {
    const recorder = await mockApi(page);
    await openIscsi(page);

    await win(page).locator('[data-testid="iscsi-enable-toggle"]').click();
    await expect.poll(() => recorder.settings.length).toBe(1);
    expect(recorder.settings[0]).toEqual({ enabled: false });
  });
});

// ===========================================================================
// 6. Accessibility
// ===========================================================================

test.describe('6 — Accessibility', () => {
  test('6.1 — row actions are named, not icon-only mysteries', async ({ page }) => {
    await mockApi(page);
    await openIscsi(page);

    await expect(win(page).locator('[data-testid="iscsi-backstore-edit-1"]')).toHaveAttribute(
      'aria-label',
      'Edit the disk data',
    );
    await expect(win(page).locator('[data-testid="iscsi-target-delete-5"]')).toHaveAttribute(
      'aria-label',
      'Delete the export iqn.2026-11.local.homevault:disk0',
    );
  });

  test('6.2 — every editor field is labelled', async ({ page }) => {
    await mockApi(page);
    await openIscsi(page);
    await win(page).locator('[data-testid="iscsi-add-target"]').click();

    for (const id of ['iscsi-target-iqn', 'iscsi-portal-address-0', 'iscsi-portal-port-0']) {
      const name = await page.locator(`[data-testid="${id}"]`).evaluate((el) => {
        const input = el as HTMLInputElement;
        return input.labels?.[0]?.textContent?.trim() ?? input.getAttribute('aria-label') ?? '';
      });
      expect(name.length, `${id} must have a label`).toBeGreaterThan(0);
    }
  });

  test('6.3 — the editor traps focus and returns it on Escape', async ({ page }) => {
    await mockApi(page);
    await openIscsi(page);

    const trigger = win(page).locator('[data-testid="iscsi-target-edit-5"]');
    await trigger.click();
    const modal = page.locator('[data-testid="iscsi-target-modal"]');
    await expect(modal).toBeVisible();

    // Focus starts inside the dialog, never behind it.
    await expect
      .poll(async () => modal.evaluate((el) => el.contains(document.activeElement)))
      .toBe(true);

    await page.keyboard.press('Escape');
    await expect(modal).toBeHidden();
    await expect(trigger).toBeFocused();
  });
});

// ===========================================================================
// 7 — Thin volumes (zvol): creation, snapshots and clones
//
// The API is mocked, so nothing runs `zfs` on the host: the tests prove the
// dialog offers the option only when a ZFS pool is reported, that a zvol sends
// its pool and size (and no path), and that the snapshot panel lists what the
// server returns.
// ===========================================================================

const ZVOL_STORE = {
  id: 3,
  name: 'disk1',
  kind: 'zvol',
  path: '/dev/zvol/tank/homevault-iscsi/disk1',
  size_bytes: 20 * 1024 ** 3,
  readonly: false,
  created_at: '2026-11-01T10:00:00Z',
  updated_at: '2026-11-01T10:00:00Z',
  lun_count: 0,
};

test.describe('7 — Thin volumes (zvol)', () => {
  test('7.1 — the ZFS option appears only when a pool is reported, and sends pool + size', async ({
    page,
  }) => {
    const recorder = await mockApi(page, { zvol: { available: true, pools: ['tank'] } });
    await openIscsi(page);

    await win(page).locator('[data-testid="iscsi-add-backstore"]').click();
    const kind = page.locator('[data-testid="iscsi-backstore-kind"]');
    await expect(kind).toBeVisible();

    // The option exists because the payload reports a pool.
    await kind.selectOption('zvol');
    await page.locator('[data-testid="iscsi-backstore-name"]').fill('disk1');
    await page.locator('[data-testid="iscsi-backstore-pool"]').selectOption('tank');
    await page.locator('[data-testid="iscsi-backstore-size"]').fill('20');
    // No path field for a zvol: the dataset is derived server-side.
    await expect(page.locator('[data-testid="iscsi-backstore-path"]')).toHaveCount(0);
    await page.locator('[data-testid="iscsi-backstore-save"]').click();

    await expect.poll(() => recorder.storeCreates.length).toBe(1);
    const body = recorder.storeCreates[0];
    expect(body.kind).toBe('zvol');
    expect(body.pool).toBe('tank');
    expect(body.path).toBe('');
    expect(body.size_bytes).toBe(20 * 1024 ** 3);
    expect(body.sparse).toBe(true);
  });

  test('7.2 — without a ZFS pool the option is absent (no dead choice)', async ({ page }) => {
    await mockApi(page, { zvol: { available: false, pools: [] } });
    await openIscsi(page);
    await win(page).locator('[data-testid="iscsi-add-backstore"]').click();
    const kind = page.locator('[data-testid="iscsi-backstore-kind"]');
    await expect(kind).toBeVisible();
    await expect(kind.locator('option[value="zvol"]')).toHaveCount(0);
  });

  test('7.3 — a ZFS volume lists its snapshots and clones one into a new disk', async ({ page }) => {
    const recorder = await mockApi(page, {
      zvol: { available: true, pools: ['tank'] },
      stores: [ZVOL_STORE],
      snapshots: [
        {
          name: 'tank/homevault-iscsi/disk1@before-upgrade',
          short_name: 'before-upgrade',
          created: '1762800000',
        },
      ],
    });
    await openIscsi(page);

    await win(page).locator('[data-testid="iscsi-backstore-snapshots-3"]').click();
    const modal = page.locator('[data-testid="iscsi-snapshots"]');
    await expect(modal).toBeVisible();

    // The existing snapshot is listed with its short name.
    await expect(modal).toContainText('before-upgrade');

    // Cloning sends the new name and the snapshot it comes from.
    await page.locator('[data-testid="iscsi-clone-name"]').fill('disk1-copy');
    await modal.locator('[data-testid="iscsi-snapshot-clone-before-upgrade"]').click();
    await expect.poll(() => recorder.storeCreates.length).toBe(1);
    expect(recorder.storeCreates[0]).toEqual({
      name: 'disk1-copy',
      snapshot: 'before-upgrade',
    });
  });
});
