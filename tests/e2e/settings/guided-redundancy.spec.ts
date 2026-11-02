/* =============================================================================
 * File          : guided-redundancy.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Guided redundancy dialog end-to-end tests (Playwright): opening it from the
 *   Storage screen, the empty state, the layout cards with their capacity and
 *   risk figures, keyboard selection of a card, and the honest statement that
 *   creating the array is not available yet.
 *
 *   Every backend call is intercepted with `page.route`, so the suite never
 *   creates an array on the machine that hosts the test run. The real engine
 *   lives in `src/guided_redundancy.rs`; the endpoint, auth matrix and the
 *   `501` contract live in `tests/guided_redundancy_integration.rs` and
 *   `tests/phase4_auth_integration.rs`.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/settings/guided-redundancy.spec.ts
 * =============================================================================
 */

import { test, expect, type Page, type APIRequestContext, type Route } from '@playwright/test';

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

async function seedSession(page: Page, tokens: Tokens) {
  await page.addInitScript((t) => {
    try {
      localStorage.setItem('session-token', t.token);
      localStorage.setItem('csrf-token', t.csrf);
    } catch {
      /* storage unavailable */
    }
  }, tokens);
}

function layout(overrides: Record<string, unknown> = {}) {
  return {
    id: 'raid5',
    name: 'Efficient Redundancy (RAID 5)',
    description: 'Balanced capacity and safety with distributed parity',
    technology: 'mdadm',
    level: '5',
    redundancy: 'single',
    usable_bytes: 2_000_000_000_000,
    efficiency_pct: 66.7,
    score: 80,
    risk: {
      data_loss_risk: 'low',
      failure_tolerance: 1,
      rebuild_time: 'hours_a_few',
      explanation: 'One disk may fail without losing data.',
    },
    layout: {
      groups: [{ role: 'data', disks: ['/dev/sda', '/dev/sdb', '/dev/sdc'] }],
      metadata_bytes: 1_000_000_000,
    },
    warnings: [],
    ...overrides,
  };
}

async function mockStorage(page: Page, recs: any[]) {
  // Storage's own calls: an empty disk/array inventory keeps the screen calm.
  await page.route('**/api/disks', (route: Route) =>
    route.fulfill({ json: { success: true, data: [] } })
  );
  await page.route('**/api/storage/**', async (route: Route) => {
    if (route.request().url().includes('guided-redundancy')) {
      return route.fulfill({
        json: {
          success: true,
          data: {
            recommendations: recs,
            total_disks: 3,
            available_disks: 3,
          },
        },
      });
    }
    return route.fulfill({ json: { success: true, data: [] } });
  });
}

async function gotoDesktop(page: Page) {
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
  const whatsNew = page.locator('[data-testid="whats-new-modal"]');
  if (await whatsNew.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await whatsNew.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

/** Opens Storage, then the guided-redundancy dialog from its empty state. */
async function openGuidedRedundancy(page: Page) {
  await gotoDesktop(page);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const tile = page.locator('.lp-card[data-app-id="storage"]');
  await tile.waitFor({ state: 'visible', timeout: 10_000 });
  await tile.click();

  // Storage opens on the "Disks" tab; the empty-state action that offers the
  // recommendations lives on "Virtual disks".
  await page.getByRole('tab', { name: /virtual disks/i }).click();
  const empty = page.locator('[data-testid="storage-empty"]');
  await empty.waitFor({ state: 'visible', timeout: 20_000 });
  await empty.getByRole('button', { name: /redundan|raid layout|suggestion|raid/i }).first().click();

  await expect(
    page.getByRole('dialog', { name: /RAID layout suggestions/i })
  ).toBeVisible({ timeout: 15_000 });
}

test.describe('Guided redundancy dialog', () => {
  test('the empty state explains why nothing is suggested', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockStorage(page, []);
    await openGuidedRedundancy(page);

    await expect(page.locator('[data-testid="gr-empty"]')).toBeVisible();
    // With nothing to suggest there is no layout to apply, so the Apply button
    // is disabled rather than offering an action that cannot work.
    await expect(page.locator('[data-testid="gr-apply"]')).toBeDisabled();
  });

  test('a recommendation shows capacity, efficiency and tolerated failures', async ({
    page,
    request,
  }) => {
    await seedSession(page, await apiLogin(request));
    await mockStorage(page, [layout()]);
    await openGuidedRedundancy(page);

    const card = page.locator('[data-testid="gr-select-raid5"]').locator('..');
    await expect(card).toContainText('RAID 5');
    await expect(card).toContainText('66.7%');
    await expect(card).toContainText('TB');

    // Creating an array from a recommendation is not implemented; the dialog
    // says so before the user clicks anything.
    await expect(page.locator('[data-testid="gr-apply-unavailable"]')).toBeVisible();
  });

  test('a card is selectable with the keyboard alone', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockStorage(page, [
      layout(),
      layout({ id: 'raid1_spare', name: 'Mirror with Hot Spare', level: '1+spare' }),
    ]);
    await openGuidedRedundancy(page);

    const radio = page.locator('[data-testid="gr-select-raid5"]');
    await radio.focus();
    await page.keyboard.press('Space');
    await expect(radio).toBeChecked();

    // The apply action becomes enabled once something is selected.
    await expect(page.locator('[data-testid="gr-apply"]')).toBeEnabled();
  });

  test('the details panel appears only for the selected layout', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockStorage(page, [
      layout(),
      layout({ id: 'raid0', name: 'Maximum Capacity (RAID 0)', level: '0', redundancy: 'none' }),
    ]);
    await openGuidedRedundancy(page);

    await expect(page.getByText(/One disk may fail without losing data/i)).toHaveCount(0);
    await page.locator('[data-testid="gr-select-raid5"]').check();
    await expect(page.getByText(/One disk may fail without losing data/i)).toBeVisible();
  });

  test('the apply action never claims to have created anything', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockStorage(page, [layout()]);
    await openGuidedRedundancy(page);

    // There is no POST to `apply` at all: the dialog states the limitation and
    // closes. A click that silently did nothing would be worse than a sentence.
    let applied = 0;
    await page.route('**/api/storage/guided-redundancy/apply/**', (route: Route) => {
      applied += 1;
      return route.fulfill({
        status: 501,
        json: { success: false, message: 'not implemented' },
      });
    });

    await page.locator('[data-testid="gr-select-raid5"]').check();
    await page.locator('[data-testid="gr-apply"]').click();
    await expect(
      page.getByRole('dialog', { name: /RAID layout suggestions/i })
    ).toBeHidden({ timeout: 10_000 });
    expect(applied).toBe(0);
  });

  test('the dialog traps focus while it is open', async ({ page, request }) => {
    await seedSession(page, await apiLogin(request));
    await mockStorage(page, [layout()]);
    await openGuidedRedundancy(page);

    // Tabbing enough times must never move focus outside the dialog.
    const dialog = page.getByRole('dialog', { name: /RAID layout suggestions/i });
    for (let i = 0; i < 12; i += 1) {
      await page.keyboard.press('Tab');
    }
    const focusInside = await dialog.evaluate((el) => el.contains(document.activeElement));
    expect(focusInside).toBe(true);
  });
});
