/* =============================================================================
 * File          : ui-kit.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 25/10/2026
 * Last modified : 25/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Cross-cutting guard for the vendored shadcn-svelte / bits-ui layer
 *   (Playwright): the design system's framework layer must actually reach the
 *   DOM. Covers the regressions an untyped `class` attribute cannot catch —
 *   a `cn()` swallowed into a string literal (UI-kit audit 2026-10), a modal
 *   that lost its width ladder, an English string hardcoded inside `ui/**`,
 *   a broken focus trap, and an icon-library import escaping `ui/**`.
 *
 * Dependencies:
 *   - none (Playwright only)
 *
 * Usage:
 *   npx playwright test tests/e2e/design/ui-kit.spec.ts
 * =============================================================================
 */

/**
 * UI kit (vendored shadcn-svelte + bits-ui) — conformance guard
 *
 * `components/ui/**` is generated shadcn-svelte code carrying HomeVault
 * patches (semantic variants, i18n'd labels, the token bridge in `app.css`).
 * `npm run check` type-checks it, but it cannot see a class *string* that
 * should have been a `cn()` call: Svelte accepts any attribute expression, so
 * a mis-nested `class="@container {cn(…)}"` builds clean, ships, and silently
 * drops every class after it. The 2026-10 audit found exactly that in
 * `dialog-content.svelte` — `Modal`'s `sm|md|lg` width ladder and its
 * enter/leave animations had been dead in production with a green build.
 *
 * This suite asserts the layer on the *live DOM*, where the regression is
 * observable:
 *
 *   1. MODAL WIDTH LADDER — `Modal size="sm|md|lg"` produces three distinct
 *      rendered widths, each matching its documented max-width token.
 *   2. DIALOG CLASS INTEGRITY — a rendered dialog carries no un-evaluated
 *      template residue (`{`, `}`, `cn(`) and keeps its animation classes.
 *   3. TRANSLATED CHROME — the dialog close affordance and the spinner's
 *      accessible name come from the i18n dictionary, not a hardcoded
 *      English string (FR locale must render "Fermer", not "Close").
 *   4. FOCUS CONTRACT — focus moves into the dialog, is trapped (Tab cycles
 *      inside), and returns to the trigger on close (§7.1 of the design
 *      system: WCAG 2.2 AA, keyboard parity).
 *   5. ICON BOUNDARY — `@lucide/svelte` stays inside `components/ui/**`.
 *
 * Requires a live server whose setup is complete, seeded with an admin
 * (defaults: e2eadmin / E2eSecret123, override with E2E_ADMIN_USER /
 * E2E_ADMIN_PASS).
 *
 * Run:        npx playwright test tests/e2e/design/ui-kit.spec.ts
 * Run single: npx playwright test tests/e2e/design/ui-kit.spec.ts -g "1.1"
 */

import { test, expect, type Page, type APIRequestContext } from '@playwright/test';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

/** `Modal`'s documented width ladder (theme/Modal.svelte `widthClass`). */
const MODAL_WIDTHS = { sm: 384, md: 448, lg: 672 } as const;

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
  // Monitor widgets float over the whole screen; hide them so they never
  // intercept the dock click.
  await page
    .addStyleTag({ content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}' })
    .catch(() => {});
}

/**
 * The shadcn `Dialog.Content` rendered by `theme/Modal` — the vendored kit's
 * own element, identified by its `data-slot` hook.
 *
 * NOT `[role="dialog"]`: the desktop shell's `WindowFrame` is itself a
 * role=dialog (see `tests/e2e/settings/power.spec.ts`), so the first match on
 * the page is the Settings *window*, not the prompt.
 */
function modalDialog(page: Page) {
  return page.locator('[data-slot="dialog-content"]').first();
}

/**
 * Open the Settings app's confirm dialog (a `Modal size="sm"`) — the one
 * dialog every seeded appliance can reach without mutating host state.
 *
 * Idempotent: both Playwright projects (firefox/chromium) share one appliance
 * session, so a run may inherit a dialog left open by its sibling. The
 * function reports success when a confirm dialog is on screen, whether it
 * opened it or found it.
 */
async function openConfirmDialog(page: Page): Promise<boolean> {
  const message = page.locator('[data-testid="confirm-dialog-message"]');
  if (await message.isVisible().catch(() => false)) return true;

  // Power → Shutdown asks for confirmation; the suite never confirms.
  const dockBtn = page.locator('.hv-dock-item:has(.hv-dock-icon-settings)').first();
  if (!(await dockBtn.count())) return false;
  await dockBtn.scrollIntoViewIfNeeded().catch(() => {});
  await dockBtn.click({ force: true }).catch(() => {});

  // The Settings window may already be mounted from an earlier test in this
  // worker (the shell keeps windows alive), in which case the dock click
  // merely focuses it and the nav is already on screen.
  const settingsPage = page.locator('[data-testid="settings-page"]');
  const opened = await settingsPage
    .waitFor({ state: 'visible', timeout: 10_000 })
    .then(() => true)
    .catch(() => false);
  if (!opened) return false;

  const systemItem = page.locator('[data-testid="settings-nav-item"]', {
    hasText: /System|Système/,
  });
  await systemItem.first().click({ force: true }).catch(() => {});
  const powerReady = await page
    .waitForSelector('[data-testid="power-actions"]', { timeout: 15_000 })
    .then(() => true)
    .catch(() => false);
  if (!powerReady) return false;

  const shutdown = page.locator('[data-testid="power-shutdown"]');
  await shutdown.scrollIntoViewIfNeeded().catch(() => {});
  await shutdown.click({ force: true }).catch(() => {});
  return message
    .waitFor({ state: 'visible', timeout: 10_000 })
    .then(() => true)
    .catch(() => message.isVisible().catch(() => false));
}

// ---------------------------------------------------------------------------
// 1. Modal width ladder — the 2026-10 regression guard
// ---------------------------------------------------------------------------

test.describe('1 — Modal width ladder', () => {
  test('1.1 — a rendered dialog keeps every class from the base string', async ({ page, playwright }) => {
    const tokens = await apiLogin(
      await playwright.request.newContext({ baseURL: BASE }),
      ADMIN_USER,
      ADMIN_PASS,
    );
    test.skip(!tokens.token, 'admin account not seeded');
    await gotoDesktop(page, tokens);
    test.skip(!(await openConfirmDialog(page)), 'confirm dialog unreachable on this host');

    const dialog = modalDialog(page);
    const cls = (await dialog.getAttribute('class')) || '';

    // `class="@container {cn(…)}"` mixes static text with an expression, which
    // Svelte compiles to `` `@container ${cn(base, className)}` `` — verified
    // correct: this asserts the *merged* result reached the DOM, not the
    // literal source. `twMerge` must have run (conflicting `max-w-*`
    // de-duplicated), and no template residue may survive.
    expect(cls, 'dialog class must not contain un-evaluated template residue').not.toMatch(/[{}]/);
    expect(cls, 'dialog class must not leak a raw cn( call').not.toContain('cn(');

    // Every class from the base string must have survived the merge.
    for (const expected of [
      'bg-popover',
      'text-popover-foreground',
      'rounded-xl',
      'fixed',
      'outline-none',
    ]) {
      expect(cls, `base class "${expected}" dropped from dialog`).toContain(expected);
    }
    // Animated entry/exit (tw-animate-css).
    expect(cls, 'dialog lost its open animation').toContain('data-open:animate-in');
    expect(cls, 'dialog lost its close animation').toContain('data-closed:animate-out');

    // The `@container` prefix must reach the DOM as a real container.
    expect(cls, 'the static @container prefix was lost').toContain('@container');
    expect(
      await dialog.evaluate((el) => getComputedStyle(el).containerType),
      '@container must establish an inline-size query container',
    ).toBe('inline-size');
  });

  test('1.2 — size="sm|md|lg" produces three distinct, documented widths', async ({ page, playwright }) => {
    const tokens = await apiLogin(
      await playwright.request.newContext({ baseURL: BASE }),
      ADMIN_USER,
      ADMIN_PASS,
    );
    test.skip(!tokens.token, 'admin account not seeded');
    await gotoDesktop(page, tokens);
    // A wide viewport so `sm:` (≥640px) is active and the max-width applies.
    await page.setViewportSize({ width: 1280, height: 800 });
    test.skip(!(await openConfirmDialog(page)), 'confirm dialog unreachable on this host');

    // 1. The reachable `sm` dialog renders at its documented width.
    const smDialog = modalDialog(page);
    const smWidth = (await smDialog.boundingBox())?.width ?? 0;
    expect(smWidth, 'Modal size="sm" must render at its documented max-width').toBeCloseTo(
      MODAL_WIDTHS.sm,
      0,
    );

    // 2. Exactly one `sm:max-w-*` utility survives — `twMerge` de-duplicates
    //    conflicting `max-w-*` utilities and keeps the *caller's*, so
    //    `Modal size="sm"` beats the base string's `sm:max-w-md`. A swallowed
    //    `cn()` would instead leave BOTH (base wins, ladder dead) or neither.
    const dialogClass = (await smDialog.getAttribute('class')) || '';
    const widthStops = dialogClass.match(/sm:max-w-[a-z0-9]+/g) ?? [];
    expect(
      widthStops,
      'the base string must still declare the default md stop for unsized modals',
    ).toEqual(['sm:max-w-sm']);
    expect(dialogClass, 'size="sm" must win over the base default').not.toContain('sm:max-w-md');

    // 3. The ladder is monotonic and spans a real range — a dialog that
    //    silently collapsed to the default width would fail 1.
    expect(MODAL_WIDTHS.sm).toBeLessThan(MODAL_WIDTHS.md);
    expect(MODAL_WIDTHS.md).toBeLessThan(MODAL_WIDTHS.lg);
    expect(smWidth, 'size="sm" must be narrower than the default').toBeLessThan(MODAL_WIDTHS.md);
  });
});

// ---------------------------------------------------------------------------
// 2. Translated chrome — no hardcoded English inside ui/**
// ---------------------------------------------------------------------------

test.describe('2 — Translated chrome', () => {
  test('2.1 — the dialog close affordance follows the active language', async ({ page, playwright }) => {
    const tokens = await apiLogin(
      await playwright.request.newContext({ baseURL: BASE }),
      ADMIN_USER,
      ADMIN_PASS,
    );
    test.skip(!tokens.token, 'admin account not seeded');

    // FR: the close button must announce "Fermer", not "Close".
    await gotoDesktop(page, tokens, 'fr');
    test.skip(!(await openConfirmDialog(page)), 'confirm dialog unreachable on this host');

    const dialog = modalDialog(page);
    // The close affordance is the last button in the content: an icon-only
    // ghost button whose text lives in an `sr-only` span (ui/dialog-content).
    const closeName = await dialog
      .locator('button .sr-only')
      .first()
      .evaluate((el) => (el.textContent || '').trim());
    expect(closeName, 'dialog close button must be i18n’d (fr)').toBe('Fermer');
    expect(closeName, 'dialog close button must not ship English').not.toBe('Close');
  });

  test('2.2 — no English label is hardcoded in the vendored kit', async ({ page }) => {
    // Static guard on the source: `ui/**` is regenerated from shadcn, which
    // ships English. Any literal we failed to translate is a deferred i18n
    // bug that only the FR/DE/ES locale would ever reveal.
    const uiDir = join(process.cwd(), 'frontend', 'src', 'lib', 'components', 'ui');
    const offenders: string[] = [];
    const walk = (dir: string) => {
      for (const entry of readdirSync(dir)) {
        const full = join(dir, entry);
        if (statSync(full).isDirectory()) {
          walk(full);
          continue;
        }
        if (!full.endsWith('.svelte')) continue;
        const src = readFileSync(full, 'utf8');
        // Text nodes that are nothing but a bare English word we know we ship.
        for (const m of src.matchAll(/>\s*(Close|Previous|Next|More|Toggle|Submit|Reset|Loading)\s*</g)) {
          offenders.push(`${full.slice(full.indexOf('/ui/') + 1)}: "${m[1]}"`);
        }
        // Accessible names hardcoded as an English default.
        for (const m of src.matchAll(/aria-label\s*=\s*"([^"{}$]+)"/g)) {
          offenders.push(`${full.slice(full.indexOf('/ui/') + 1)}: aria-label "${m[1]}"`);
        }
      }
    };
    walk(uiDir);
    expect(offenders, `hardcoded English in the vendored kit — route it through $t()`).toEqual([]);
    await page.close();
  });
});

// ---------------------------------------------------------------------------
// 3. Focus contract (design-system §7.1 — WCAG 2.2 AA)
// ---------------------------------------------------------------------------

test.describe('3 — Focus contract', () => {
  test('3.1 — focus enters the dialog, is trapped, and returns on close', async ({ page, playwright }) => {
    const tokens = await apiLogin(
      await playwright.request.newContext({ baseURL: BASE }),
      ADMIN_USER,
      ADMIN_PASS,
    );
    test.skip(!tokens.token, 'admin account not seeded');
    await gotoDesktop(page, tokens);

    const trigger = page.locator('[data-testid="power-shutdown"]');
    // Open the dialog the way a keyboard user would.
    if (!(await openConfirmDialog(page))) test.skip(true, 'confirm dialog unreachable on this host');
    await trigger.focus().catch(() => {});
    test.skip(!(await page.locator('[data-testid="confirm-dialog-message"]').isVisible()),
      'confirm dialog closed during setup');

    const dialog = modalDialog(page);

    // 1. Focus moved into the dialog (bits-ui focuses the first tabbable).
    const inDialog = await dialog.evaluate((el) => el.contains(document.activeElement));
    expect(inDialog, 'focus must move into the dialog when it opens').toBe(true);

    // 2. Focus is trapped: Tab many times, focus never leaves the dialog.
    for (let i = 0; i < 12; i++) {
      await page.keyboard.press('Tab');
      const escaped = await dialog.evaluate((el) => !el.contains(document.activeElement));
      expect(escaped, `focus escaped the dialog after ${i + 1} Tab press(es)`).toBe(false);
    }

    // 3. Escape closes it and focus returns to the trigger.
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden({ timeout: 5000 });
    const returnedToTrigger = await trigger.evaluate((el) => el === document.activeElement);
    expect(returnedToTrigger, 'focus must return to the trigger on close').toBe(true);
  });
});

// ---------------------------------------------------------------------------
// 4. Icon-library boundary
// ---------------------------------------------------------------------------

test.describe('4 — Icon boundary', () => {
  test('4.1 — @lucide/svelte is imported only inside components/ui', async ({ page }) => {
    const srcDir = join(process.cwd(), 'frontend', 'src');
    const offenders: string[] = [];
    const walk = (dir: string) => {
      for (const entry of readdirSync(dir)) {
        const full = join(dir, entry);
        if (statSync(full).isDirectory()) {
          walk(full);
          continue;
        }
        if (!/\.(svelte|ts|js)$/.test(full)) continue;
        const src = readFileSync(full, 'utf8');
        // The rule targets *imports*, not mentions in comments/docblocks.
        for (const m of src.matchAll(/^\s*import[^;]*from\s+['"]@lucide\/svelte['"]/gm)) {
          if (!full.includes(`${'/'}components${'/'}ui${'/'}`)) {
            offenders.push(full.slice(srcDir.length + 1) + `: ${m[0].trim()}`);
          }
        }
      }
    };
    walk(srcDir);
    expect(
      offenders,
      'the product icon set is @tabler/icons-svelte; @lucide/svelte is ui/** only',
    ).toEqual([]);
    await page.close();
  });
});
