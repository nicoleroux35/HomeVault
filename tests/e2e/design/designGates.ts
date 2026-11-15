/**
 * Shared helpers for the cross-cutting design gates (accessibility, visual
 * regression, browser compatibility).
 *
 * Every screen is a desktop app window, so these helpers seed a real session,
 * land on `/desktop`, and open/close one app at a time the way a user does
 * (launchpad tile → window close button). Keeping the list in one place means
 * a new app is covered by every gate in one edit.
 */
import { expect, request, type APIRequestContext, type Page } from '@playwright/test';

export const BASE = process.env.BASE_URL || 'http://localhost:8080';
export const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
export const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

export interface Tokens {
  token: string;
  csrf: string;
}

/** Every in-app screen rendered through the window manager. */
export const APP_IDS = [
  'dashboard',
  'storage',
  'usb-copy',
  'raid',
  'explorer',
  'docker',
  'network',
  'vpn',
  'backup',
  'destinations',
  'replication',
  'traffic-control',
  'iscsi',
  'directory',
  'users',
  'groups',
  'quotas',
  'shares',
  'rclone',
  'syncthing',
  'health',
  'chat',
  'downloader',
  'rescue',
  'widgets',
  'edit-widgets',
  'journal',
  'virtual-machines',
  'tasks',
  'metrics',
  'settings',
] as const;

export type AppId = (typeof APP_IDS)[number];

/** Apps that open from a pinned dock button rather than a launchpad tile. */
export const DOCK_APPS: Partial<Record<AppId, string>> = {
  settings: '[data-testid="dock-settings"]',
};

/** The accent the product ships with (`DEFAULT_ACCENT` in handlers/settings.rs). */
export const SHIPPED_ACCENT = '#2E72C9';

/**
 * Restore the shipped appearance before a gate measures colours.
 *
 * The settings spec saves a custom accent to prove the server value wins over
 * this origin's localStorage, and the server keeps it. Before accents were
 * clamped on write, a pale one made a gate report the active window tab
 * (`color: var(--hv-primary)`) as a contrast failure that does not exist with
 * the shipped accent — exactly what happened to the Storage screen. The gates
 * measure the look the product ships, so they reset it first.
 */
export async function resetShippedAppearance(tokens: Tokens): Promise<void> {
  const ctx = await request.newContext({ baseURL: BASE });
  try {
    await ctx.put('/api/settings/appearance', {
      headers: { 'x-session-token': tokens.token, 'x-csrf-token': tokens.csrf },
      data: { accent_color: SHIPPED_ACCENT, wallpaper: '' },
    });
  } finally {
    await ctx.dispose();
  }
}

export async function apiLogin(ctx: APIRequestContext, user = ADMIN_USER, pass = ADMIN_PASS): Promise<Tokens> {
  const res = await ctx.post(`${BASE}/api/auth/login`, { data: { username: user, password: pass } });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

/** Seed the session into localStorage and land on the desktop shell. */
export async function gotoDesktop(page: Page, tokens: Tokens, lang = 'en'): Promise<void> {
  await page.addInitScript(
    ({ t, l }) => {
      try {
        localStorage.setItem('session-token', t.token);
        localStorage.setItem('csrf-token', t.csrf);
        localStorage.setItem('hv-language', l);
      } catch {
        /* storage unavailable — the assertion below surfaces it */
      }
    },
    { t: tokens, l: lang },
  );
  await page.goto('/desktop');
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await page.locator('.hv-dock').waitFor({ state: 'visible', timeout: 10_000 });

  // The release-notes modal opens over a fresh account and swallows pointer
  // events; dismiss it before any interaction.
  const dismiss = page.locator('[data-testid="whats-new-dismiss"]');
  if (await dismiss.count()) await dismiss.first().click().catch(() => {});

  // Monitor widgets float over the whole desktop and intercept dock clicks.
  await page
    .addStyleTag({ content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}' })
    .catch(() => {});
}

/** Open one app window and wait until its chrome is on screen. */
export async function openApp(page: Page, appId: AppId): Promise<void> {
  if (DOCK_APPS[appId]) {
    await page.locator(DOCK_APPS[appId]!).click();
  } else {
    await page.locator('[data-testid="dock-launchpad"]').click();
    const tile = page.locator(`[data-app-id="${appId}"]`);
    await expect(tile).toBeVisible({ timeout: 10_000 });
    await tile.click();
  }
  await expect(page.locator('.hv-window').last()).toBeVisible({ timeout: 15_000 });
  await settleWindow(page);
  // Give the window's first API round-trip a chance to settle so axe measures
  // the loaded state, not the skeleton. Bounded, never networkidle (the app
  // keeps SSE/polling sockets open).
  await page.waitForTimeout(600);
}

/**
 * Wait until the window is fully opaque and no animation is running.
 *
 * axe measures the colours it finds *right now*: a window caught mid-fade (or
 * a launchpad scrim still fading over it) blends every text colour with the
 * backdrop, and the accessibility gate then reports colour-contrast
 * violations that do not exist once the screen has settled. The CI runner
 * hit exactly that — one `color-contrast::.hv-winhdr-tab-active` on Storage
 * with the fixed 600 ms wait, 8 of them when the wait was removed locally.
 */
export async function settleWindow(page: Page): Promise<void> {
  await page
    .waitForFunction(
      () => document.getAnimations().every((animation) => animation.playState !== 'running'),
      undefined,
      { timeout: 5_000 },
    )
    .catch(() => {});
  await page
    .waitForFunction(
      () => {
        const windows = Array.from(document.querySelectorAll('.hv-window'));
        return windows.every((win) => getComputedStyle(win).opacity === '1');
      },
      undefined,
      { timeout: 5_000 },
    )
    .catch(() => {});
}

/** Close the most recently opened window. */
export async function closeLastWindow(page: Page): Promise<void> {
  const windows = page.locator('.hv-window');
  const before = await windows.count();
  if (before === 0) return;
  const last = windows.nth(before - 1);
  await last.locator('[data-testid="win-close"]').click();
  await expect(windows).toHaveCount(before - 1, { timeout: 10_000 });
}
