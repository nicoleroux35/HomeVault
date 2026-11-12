import type { Page } from '@playwright/test';

/**
 * Dismiss the once-per-version "What's New" modal.
 *
 * The modal is fetched after the desktop shell paints, so a test that waits
 * only for `.hv-user-pill` can start clicking while the overlay is still
 * arriving — the click then waits forever on a covered/inert button. Wait a
 * short bounded window for the modal, dismiss it if it appears, and only then
 * proceed.
 */
export async function dismissWhatsNew(page: Page, timeout = 900): Promise<void> {
  const modal = page.locator('[data-testid="whats-new-modal"]');
  await modal.waitFor({ state: 'visible', timeout }).catch(() => undefined);
  if (!(await modal.isVisible().catch(() => false))) return;
  await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => undefined);
  await modal.waitFor({ state: 'hidden', timeout: 5_000 }).catch(() => undefined);
}

/** Wait for the dock's entrance animation, so a click is not rejected as unstable. */
export async function waitForDockSettled(page: Page): Promise<void> {
  await page
    .waitForFunction(
      () => {
        const items = [...document.querySelectorAll('.hv-dock-item')] as HTMLElement[];
        return items.length > 0 && items.every((el) => el.getAnimations().length === 0);
      },
      undefined,
      { timeout: 5_000 },
    )
    .catch(() => undefined);
}

/** Land on an authenticated desktop, clear the What's New overlay, wait for the shell. */
export async function gotoDesktopReady(page: Page): Promise<void> {
  await page.goto('/desktop');
  await dismissWhatsNew(page);
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  await waitForDockSettled(page);
}
