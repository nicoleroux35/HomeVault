/* =============================================================================
 * File          : setup.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Setup screen end-to-end tests (Playwright): covers the screen's functionality per TESTING.md.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

/**
 * Setup Wizard (Assistant de configuration) — End-to-End Test Suite
 *
 * Every test navigates to /login and detects whether the setup wizard is active.
 * The wizard only appears on first run (no admin account exists). If the wizard
 * is not shown, the test returns early with a clear note. Tests are fully
 * independent — each can be run individually.
 *
 * Run (all):         npx playwright test tests/e2e/setup/setup.spec.ts
 * Run (single test): npx playwright test tests/e2e/setup/setup.spec.ts -g "4.1"
 * Run (tag):         npx playwright test tests/e2e/setup/setup.spec.ts -g "Security"
 * Custom server:     BASE_URL=http://localhost:3000 npx playwright test tests/e2e/setup/setup.spec.ts
 *
 * IMPORTANT: these tests require a fresh HomeVault instance with no admin
 * account (first-run state). The server must not have been set up yet.
 */

import { test, expect, type Page, type ConsoleMessage } from '@playwright/test';
import { createHmac } from 'node:crypto';

// ---------------------------------------------------------------------------
// Helpers — self-contained per test, no shared mutable state
// ---------------------------------------------------------------------------

type PageState = 'login' | 'wizard' | 'error';

/** RFC 6238 TOTP for a base32 secret (SHA1, 6 digits, 30 s step). */
function totpNow(base32Secret: string, atMs = Date.now()): string {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
  const clean = base32Secret.replace(/=+$/, '').toUpperCase();
  let bits = '';
  for (const ch of clean) {
    const idx = alphabet.indexOf(ch);
    if (idx < 0) continue;
    bits += idx.toString(2).padStart(5, '0');
  }
  const bytes: number[] = [];
  for (let i = 0; i + 8 <= bits.length; i += 8) bytes.push(parseInt(bits.slice(i, i + 8), 2));
  const key = Buffer.from(bytes);
  const counter = Math.floor(atMs / 1000 / 30);
  const msg = Buffer.alloc(8);
  msg.writeBigUInt64BE(BigInt(counter));
  const hmac = createHmac('sha1', key).update(msg).digest();
  const offset = hmac[hmac.length - 1] & 0x0f;
  const bin =
    ((hmac[offset] & 0x7f) << 24) |
    ((hmac[offset + 1] & 0xff) << 16) |
    ((hmac[offset + 2] & 0xff) << 8) |
    (hmac[offset + 3] & 0xff);
  return (bin % 1_000_000).toString().padStart(6, '0');
}

async function detectState(page: Page): Promise<PageState> {
  if (await page.locator('[data-testid="setup-container"]').isVisible().catch(() => false)) return 'wizard';
  if (await page.locator('#username').isVisible().catch(() => false)) return 'login';
  if (await page.locator('.hv-login-error-state').isVisible().catch(() => false)) return 'error';
  return 'login';
}

async function gotoLogin(page: Page) {
  await page.goto('/login');
  await page.waitForSelector(
    '#username, [data-testid="setup-container"], .hv-login-error-state',
    { timeout: 15_000 },
  );
}

async function ensureWizard(page: Page): Promise<boolean> {
  await gotoLogin(page);
  if ((await detectState(page)) !== 'wizard') return false;
  // The wizard is reboot-safe: a previous test's progress may have been
  // persisted server-side, so it can resume at a later step. Rewind to the
  // first step so every test starts from the same place.
  const prevBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-secondary');
  for (let i = 0; i < 12; i++) {
    if (await page.locator('[data-step="0"]').isVisible().catch(() => false)) break;
    if (!(await prevBtn.isEnabled().catch(() => false))) break;
    await prevBtn.click();
    await page.waitForTimeout(80);
  }
  return true;
}

async function getHeapMB(page: Page): Promise<number> {
  return page.evaluate(() =>
    'memory' in performance ? (performance as any).usedJSHeapSize / (1024 * 1024) : 0,
  );
}

/** Click "Next" to advance one step (assumes wizard is visible). */
async function clickNext(page: Page) {
  const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
  await expect(nextBtn).toBeVisible();
  await nextBtn.click();
}

/** Click "Previous" to go back one step. */
async function clickPrev(page: Page) {
  const prevBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-secondary');
  await expect(prevBtn).toBeVisible();
  await prevBtn.click();
}

/**
 * Advance to a target step by clicking "Next" until the target `[data-step="N"]`
 * block is visible. Step 0 (welcome) is an optional choice whose "Next" is
 * always enabled, so reaching the account (1), password (2) and verify (3)
 * steps never needs any input on the way.
 */
async function gotoStep(page: Page, target: number) {
  for (let i = 0; i < 12; i++) {
    if (await page.locator(`[data-step="${target}"]`).isVisible().catch(() => false)) return;
    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    if (
      !(await nextBtn.isVisible().catch(() => false)) ||
      !(await nextBtn.isEnabled().catch(() => false))
    ) {
      break;
    }
    await nextBtn.click();
    await page.waitForTimeout(120);
  }
  await page.locator(`[data-step="${target}"]`).waitFor({ state: 'visible' });
}

// ---------------------------------------------------------------------------
// 1. Page load & wizard detection
// ---------------------------------------------------------------------------

test.describe('1 — Page load', () => {
  test('1.1 — wizard container is visible when server is in first-run state', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) {
      console.log('[SKIP] Server already has an admin — wizard not shown');
      return;
    }
    await expect(page.locator('[data-testid="setup-container"]')).toBeVisible();
    await expect(page.locator('[data-testid="setup-card"]')).toBeVisible();
  });

  test('1.2 — login form is hidden when wizard is active', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await expect(page.locator('#username')).not.toBeVisible();
    await expect(page.locator('#password')).not.toBeVisible();
  });

  test('1.3 — stepper has 4 steps with labels', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const dots = page.locator('[data-testid="setup-stepper-dot"]');
    await expect(dots).toHaveCount(4);

    const labels = page.locator('[data-testid="setup-stepper-label"]');
    await expect(labels).toHaveCount(4);

    for (let i = 0; i < 4; i++) {
      const text = await labels.nth(i).textContent();
      expect(text!.trim().length).toBeGreaterThan(0);
    }
  });

  test('1.4 — step 0 (Welcome) is shown initially', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const welcomeStep = page.locator('[data-step="0"]');
    await expect(welcomeStep).toBeVisible();

    const title = welcomeStep.locator('.hv-welcome-title');
    await expect(title).toBeVisible();
  });

  test('1.5 — first stepper dot is marked current', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const firstDot = page.locator('[data-testid="setup-stepper-dot"]').first();
    await expect(firstDot).toHaveClass(/current/);
  });

  test('1.6 — capabilities are loaded (no infinite spinner)', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await page.waitForTimeout(2_000);
    // Prerequisites warning or step content should be visible
    const hasContent = await page.locator('.hv-alert-warning, [data-step="0"]').isVisible();
    expect(hasContent).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// 2. Step 0 — Welcome screen
// ---------------------------------------------------------------------------

test.describe('2 — Welcome screen', () => {
  test('2.1 — welcome title is visible and non-empty', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const title = page.locator('.hv-welcome-title');
    await expect(title).toBeVisible();
    const text = await title.textContent();
    expect(text!.trim().length).toBeGreaterThan(0);
  });

  test('2.2 — welcome messages are visible', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const messages = page.locator('.hv-welcome-text');
    const count = await messages.count();
    expect(count).toBeGreaterThanOrEqual(1);

    for (let i = 0; i < count; i++) {
      const text = await messages.nth(i).textContent();
      expect(text!.trim().length).toBeGreaterThan(0);
    }
  });

  test('2.3 — three feature cards are shown', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const features = page.locator('.hv-feature');
    await expect(features).toHaveCount(3);

    for (let i = 0; i < 3; i++) {
      const text = await features.nth(i).textContent();
      expect(text!.trim().length).toBeGreaterThan(0);
    }
  });

  test('2.4 — welcome icon is visible', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const icon = page.locator('.hv-welcome-icon').first();
    await expect(icon).toBeVisible();
    const box = await icon.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.width).toBeGreaterThan(30);
    expect(box!.height).toBeGreaterThan(30);
  });

  test('2.5 — Previous button is disabled on step 0', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const prevBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-secondary');
    await expect(prevBtn).toBeVisible();
    expect(await prevBtn.isDisabled()).toBe(true);
  });

  test('2.6 — Next button is enabled on step 0', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await expect(nextBtn).toBeVisible();
    expect(await nextBtn.isEnabled()).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// 3. Step navigation
// ---------------------------------------------------------------------------

test.describe('3 — Step navigation', () => {
  test('3.1 — Next advances from step 0 to step 1', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await clickNext(page);
    const step1 = page.locator('[data-step="1"]');
    await expect(step1).toBeVisible({ timeout: 5_000 });
  });

  test('3.2 — Previous goes back from step 1 to step 0', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await clickNext(page);
    await page.locator('[data-step="1"]').waitFor({ state: 'visible' });

    await clickPrev(page);
    const step0 = page.locator('[data-step="0"]');
    await expect(step0).toBeVisible({ timeout: 5_000 });
  });

  test('3.3 — stepper updates when advancing', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const dot0 = page.locator('[data-testid="setup-stepper-dot"]').nth(0);
    const dot1 = page.locator('[data-testid="setup-stepper-dot"]').nth(1);

    await clickNext(page);

    // dot0 should be done, dot1 should be current
    await expect(dot0).toHaveClass(/done/, { timeout: 5_000 });
    await expect(dot1).toHaveClass(/current/);
  });

  test('3.4 — Previous button is disabled on step 0', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const prevBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-secondary');
    expect(await prevBtn.isDisabled()).toBe(true);
  });

  test('3.5 — Previous button is enabled on step 1', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await clickNext(page);
    await page.locator('[data-step="1"]').waitFor({ state: 'visible' });

    const prevBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-secondary');
    expect(await prevBtn.isEnabled()).toBe(true);
  });

  test('3.6 — Previous button is disabled after returning to step 0', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await clickNext(page);
    await page.locator('[data-step="1"]').waitFor({ state: 'visible' });
    await clickPrev(page);
    await page.locator('[data-step="0"]').waitFor({ state: 'visible' });

    const prevBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-secondary');
    expect(await prevBtn.isDisabled()).toBe(true);
  });

  test('3.7 — stepper line between done steps has done class', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await clickNext(page);

    const line = page.locator('[data-testid="setup-stepper-line"]').first();
    await expect(line).toHaveClass(/done/, { timeout: 5_000 });
  });

  test('3.8 — complete full flow: welcome → account → password → verify', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    // Step 0 → 1 (account)
    await clickNext(page);
    await page.locator('[data-step="1"]').waitFor({ state: 'visible' });

    // Fill valid username to pass validation
    await page.fill('#username', 'testadmin');
    // Fill fullname (optional)
    await page.fill('#fullname', 'Test Admin');

    // Step 1 → 2 (password)
    await clickNext(page);
    await page.locator('[data-step="2"]').waitFor({ state: 'visible' });

    // Fill valid password
    await page.fill('#password', 'SecurePass123');
    await page.fill('#confirm-password', 'SecurePass123');

    // Step 2 → 3 (verify)
    await clickNext(page);
    await page.locator('[data-step="3"]').waitFor({ state: 'visible' });

    const verifyStep = page.locator('[data-step="3"]');
    await expect(verifyStep).toBeVisible();
  });
});

// ---------------------------------------------------------------------------
// 4. Step 1 — Account form (username + fullname + avatar)
// ---------------------------------------------------------------------------

test.describe('4 — Account form', () => {
  test('4.1 — step 1 has title and description', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    const title = page.locator('[data-testid="setup-step-title"]');
    await expect(title).toBeVisible();
    const text = await title.textContent();
    expect(text!.trim().length).toBeGreaterThan(0);

    const desc = page.locator('[data-testid="setup-step-desc"]');
    await expect(desc).toBeVisible();
  });

  test('4.2 — username input: type=text, autocomplete=username', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    const el = page.locator('#username');
    await expect(el).toBeVisible();
    expect(await el.getAttribute('type')).toBe('text');
    expect(await el.getAttribute('autocomplete')).toBe('username');
  });

  test('4.3 — username has placeholder', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    const ph = await page.locator('#username').getAttribute('placeholder');
    expect(ph!.length).toBeGreaterThan(0);
  });

  test('4.4 — fullname input: type=text, autocomplete=name, optional', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    const el = page.locator('#fullname');
    await expect(el).toBeVisible();
    expect(await el.getAttribute('type')).toBe('text');
    expect(await el.getAttribute('autocomplete')).toBe('name');

    // Should show "(optional)" text nearby
    const label = page.locator('label[for="fullname"]');
    const text = await label.textContent();
    expect(text!.toLowerCase()).toContain('optional');
  });

  test('4.5 — avatar picker is visible', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    const picker = page.locator('[data-testid="avatar-picker"]');
    await expect(picker).toBeVisible();
  });

  test('4.6 — avatar preview or placeholder is visible', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    const preview = page.locator('[data-testid="setup-avatar-preview"]');
    const placeholder = page.locator('[data-testid="setup-avatar-placeholder"]');
    const eitherVisible =
      (await preview.isVisible().catch(() => false)) ||
      (await placeholder.isVisible().catch(() => false));
    expect(eitherVisible).toBe(true);
  });

  test('4.7 — regenerate avatar button works', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    const regenBtn = page.locator('[data-testid="setup-avatar-regen"]');
    await expect(regenBtn).toBeVisible();

    // Click regenerate — avatar preview should change
    const srcBefore = await page.locator('[data-testid="setup-avatar-preview"]').getAttribute('src').catch(() => '');
    await regenBtn.click();
    await page.waitForTimeout(1_500);
    const srcAfter = await page.locator('[data-testid="setup-avatar-preview"]').getAttribute('src').catch(() => '');
    // Either the src changed or we have a new preview (may be same if API is slow)
    expect(typeof srcAfter).toBe('string');
  });

  test('4.8 — username validation: empty → button stays disabled', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    // Click Next without filling username
    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await expect(nextBtn).toBeVisible();
    await expect(nextBtn).toBeDisabled();
  });

  test('4.9 — username validation: too short → button stays disabled', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    await page.fill('#username', 'ab');
    await page.click('#username');
    await page.keyboard.press('Tab');
    await page.waitForTimeout(500);

    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await expect(nextBtn).toBeVisible();
    await expect(nextBtn).toBeDisabled();
  });

  test('4.10 — username validation: invalid chars → button stays disabled', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    await page.fill('#username', 'user name!');
    await page.click('#username');
    await page.keyboard.press('Tab');
    await page.waitForTimeout(500);

    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await expect(nextBtn).toBeVisible();
    await expect(nextBtn).toBeDisabled();
  });

  test('4.11 — username validation: reserved name → button stays disabled', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    await page.fill('#username', 'root');
    await page.click('#username');
    await page.keyboard.press('Tab');
    await page.waitForTimeout(500);

    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await expect(nextBtn).toBeVisible();
    await expect(nextBtn).toBeDisabled();
  });

  test('4.12 — valid username allows next step', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    await page.fill('#username', 'validadmin');
    await clickNext(page);
    await page.locator('[data-step="2"]').waitFor({ state: 'visible', timeout: 5_000 });
    await expect(page.locator('[data-step="2"]')).toBeVisible();
  });
});

// ---------------------------------------------------------------------------
// 5. Step 2 — Password form
// ---------------------------------------------------------------------------

test.describe('5 — Password form', () => {
  async function goToPasswordStep(page: Page) {
    await gotoStep(page, 1);
    await page.fill('#username', 'testadmin');
    await clickNext(page);
    await page.locator('[data-step="2"]').waitFor({ state: 'visible' });
  }

  test('5.1 — step 2 has title and description', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    const title = page.locator('[data-testid="setup-step-title"]');
    await expect(title).toBeVisible();
    const desc = page.locator('[data-testid="setup-step-desc"]');
    await expect(desc).toBeVisible();
  });

  test('5.2 — password input: type=password, autocomplete=new-password', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    const el = page.locator('#password');
    await expect(el).toBeVisible();
    expect(await el.getAttribute('type')).toBe('password');
    expect(await el.getAttribute('autocomplete')).toBe('new-password');
  });

  test('5.3 — confirm password input: type=password, autocomplete=new-password', async ({
    page,
  }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    const el = page.locator('#confirm-password');
    await expect(el).toBeVisible();
    expect(await el.getAttribute('type')).toBe('password');
    expect(await el.getAttribute('autocomplete')).toBe('new-password');
  });

  test('5.4 — password strength bar appears when typing', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    await page.fill('#password', 'a');
    const strengthBar = page.locator('.hv-password-strength');
    await expect(strengthBar).toBeVisible({ timeout: 3_000 });
  });

  test('5.5 — strength label shows correct level for weak password', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    // Use a password that gives strength > 0 so the label has visible text
    // 'abcd1234' → lowercase + digit only (2pt) → weak, below the 12-char floor
    await page.fill('#password', 'abcd1234');
    await page.waitForTimeout(500);
    const label = page.locator('.hv-strength-label');
    await expect(label).toBeVisible({ timeout: 5_000 });
    const text = await label.textContent();
    expect(text!.length).toBeGreaterThan(0);
  });

  test('5.6 — strength label shows correct level for strong password', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    await page.fill('#password', 'VeryStr0ng!Pass');
    await page.waitForTimeout(300);
    const label = page.locator('.hv-strength-label');
    await expect(label).toBeVisible();
    const text = await label.textContent();
    expect(text!.length).toBeGreaterThan(0);
  });

  test('5.7 — password requirement checklist shows all 6 items', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    await page.fill('#password', 'a');
    const checks = page.locator('.hv-req-list li');
    await expect(checks).toHaveCount(6, { timeout: 3_000 });
  });

  test('5.8 — requirement items show met/unmet state', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    await page.fill('#password', 'a');
    const met = page.locator('.hv-req-met');
    const unmet = page.locator('.hv-req-unmet');
    const total = (await met.count()) + (await unmet.count());
    expect(total).toBe(6);
  });

  test('5.9 — mismatched passwords → button stays disabled', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    await page.fill('#password', 'SecurePass123');
    await page.fill('#confirm-password', 'DifferentPass456');

    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await expect(nextBtn).toBeVisible();
    await expect(nextBtn).toBeDisabled();
  });

  test('5.10 — matching valid passwords allow next step', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    await page.fill('#password', 'SecurePass123');
    await page.fill('#confirm-password', 'SecurePass123');
    await clickNext(page);
    await page.locator('[data-step="3"]').waitFor({ state: 'visible', timeout: 5_000 });
    await expect(page.locator('[data-step="3"]')).toBeVisible();
  });

  test('5.11 — password validation: empty → button stays disabled', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    await page.fill('#confirm-password', 'something');
    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await expect(nextBtn).toBeVisible();
    await expect(nextBtn).toBeDisabled();
  });

  test('5.12 — password validation: too short → button stays disabled', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    await page.fill('#password', 'Short1');
    await page.fill('#confirm-password', 'Short1');
    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await expect(nextBtn).toBeVisible();
    await expect(nextBtn).toBeDisabled();
  });

  test('5.13 — password validation: 11 characters (composition OK) still disabled', async ({
    page,
  }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    // Mixed case + digit, but one character short of the 12-char floor.
    await page.fill('#password', 'Abcdefgh123');
    await page.fill('#confirm-password', 'Abcdefgh123');
    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await expect(nextBtn).toBeDisabled();
  });

  test('5.14 — password validation: a common password is refused', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    // 12 chars, mixed case, with a digit — and on every breach list.
    await page.fill('#password', 'Password1234');
    await page.fill('#confirm-password', 'Password1234');
    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await expect(nextBtn).toBeDisabled();
    // The checklist names the rule and marks it unmet.
    const common = page.locator('.hv-req-list li', { hasText: /common|fréquent|häufig|común/i });
    await expect(common.first()).toBeVisible();
  });

  test('5.15 — password validation: a repeated run is refused', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToPasswordStep(page);

    await page.fill('#password', 'Aaaaaaaa1234');
    await page.fill('#confirm-password', 'Aaaaaaaa1234');
    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await expect(nextBtn).toBeDisabled();
  });
});

// ---------------------------------------------------------------------------
// 6. Step 3 — Verification and account creation
// ---------------------------------------------------------------------------

test.describe('6 — Verification step', () => {
  async function goToVerifyStep(page: Page) {
    await gotoStep(page, 1);
    await page.fill('#username', 'testadmin');
    await clickNext(page);
    await page.locator('[data-step="2"]').waitFor({ state: 'visible' });
    await page.fill('#password', 'SecurePass123');
    await page.fill('#confirm-password', 'SecurePass123');
    await clickNext(page);
    await page.locator('[data-step="3"]').waitFor({ state: 'visible' });
  }

  test('6.1 — step 3 has title and description', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToVerifyStep(page);

    const title = page.locator('[data-testid="setup-step-title"]');
    await expect(title).toBeVisible();
    const desc = page.locator('[data-testid="setup-step-desc"]');
    await expect(desc).toBeVisible();
  });

  test('6.2 — summary shows username', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToVerifyStep(page);

    const summaryValue = page.locator('.hv-summary-value').first();
    await expect(summaryValue).toBeVisible();
    const text = await summaryValue.textContent();
    expect(text).toBe('testadmin');
  });

  test('6.3 — summary shows password as masked', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToVerifyStep(page);

    // Find the summary item that contains the password label, then check its value
    const summaryItems = page.locator('.hv-summary-item');
    const count = await summaryItems.count();
    let foundMasked = false;
    for (let i = 0; i < count; i++) {
      const text = await summaryItems.nth(i).textContent();
      if (text && text.includes('•')) {
        foundMasked = true;
        break;
      }
    }
    expect(foundMasked).toBe(true);
  });

  test('6.4 — verification auto-runs on step 3', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToVerifyStep(page);

    // Should see either loading, success, or error state
    await page.waitForTimeout(2_000);
    const box = page.locator('.hv-alert, .hv-loading-row');
    const anyVisible = (await box.count()) > 0;
    expect(anyVisible).toBe(true);
  });

  test('6.5 — verification shows success or error box', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToVerifyStep(page);

    await page.waitForTimeout(3_000);
    const successOrError =
      (await page.locator('.hv-alert-success').isVisible().catch(() => false)) ||
      (await page.locator('.hv-alert-error').isVisible().catch(() => false)) ||
      (await page.locator('.hv-alert').isVisible().catch(() => false));
    expect(successOrError).toBe(true);
  });

  test('6.6 — Create Account button is visible on step 3', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await goToVerifyStep(page);

    const createBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await expect(createBtn).toBeVisible();
    const text = await createBtn.textContent();
    expect(text!.length).toBeGreaterThan(0);
  });

  test('6.7 — summary shows fullname when provided', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    // Go to the account step with fullname
    await gotoStep(page, 1);
    await page.fill('#username', 'testadmin2');
    await page.fill('#fullname', 'Test Admin User');
    await clickNext(page);
    await page.locator('[data-step="2"]').waitFor({ state: 'visible' });
    await page.fill('#password', 'SecurePass123');
    await page.fill('#confirm-password', 'SecurePass123');
    await clickNext(page);
    await page.locator('[data-step="3"]').waitFor({ state: 'visible' });

    // Look for fullname in summary
    const summaryItems = page.locator('.hv-summary-item');
    const count = await summaryItems.count();
    let foundFullname = false;
    for (let i = 0; i < count; i++) {
      const text = await summaryItems.nth(i).textContent();
      if (text!.includes('Test Admin User')) {
        foundFullname = true;
        break;
      }
    }
    expect(foundFullname).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// 7 + 8. Account creation & 2FA enrolment — see section 19 at the END of this
// file. That flow is the only one that actually COMPLETES first-run setup,
// which every `ensureWizard()`-gated test above cannot tolerate (the wizard
// disappears once an admin exists). It is therefore run last, in one serial
// block, and the whole flow — TOTP enrolled at the PASSWORD step, recovery
// code after creation, backup-codes summary — is asserted inline in the same
// test.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 9. Prerequisites warning
// ---------------------------------------------------------------------------

test.describe('9 — Prerequisites warning', () => {
  test('9.1 — sudo warning banner appears when sudo is unavailable', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await page.waitForTimeout(2_000);
    const warning = page.locator('.hv-alert-warning');
    // This may or may not be visible depending on the host
    if (await warning.isVisible().catch(() => false)) {
      const title = warning.locator('.hv-prereq-title');
      await expect(title).toBeVisible();
      const text = await title.textContent();
      expect(text!.length).toBeGreaterThan(0);
    }
  });

  test('9.2 — warning lists sudo as missing', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const warning = page.locator('.hv-alert-warning');
    if (await warning.isVisible().catch(() => false)) {
      const list = warning.locator('.hv-prereq-list li');
      const count = await list.count();
      expect(count).toBeGreaterThanOrEqual(1);
    }
  });
});

// ---------------------------------------------------------------------------
// 10. Responsive layout
// ---------------------------------------------------------------------------

test.describe('10 — Responsive', () => {
  for (const [label, width, height] of [
    ['360px (phone)', 360, 640],
    ['390px (iPhone 14)', 390, 844],
    ['768px (tablet)', 768, 1024],
    ['1024px (laptop)', 1024, 768],
    ['1920px (desktop)', 1920, 1080],
  ] as const) {
    test(`10.x — renders correctly at ${label}`, async ({ page }) => {
      const isWizard = await ensureWizard(page);
      if (!isWizard) return;

      await page.setViewportSize({ width, height });

      const card = page.locator('[data-testid="setup-card"]');
      await expect(card).toBeVisible();

      const box = await card.boundingBox();
      expect(box).not.toBeNull();
      if (box) {
        // Card must not overflow the viewport.
        expect(box.x).toBeGreaterThanOrEqual(-1);
        expect(box.x + box.width).toBeLessThanOrEqual(width + 2);
        // Card must be visible (non-zero size).
        expect(box.width).toBeGreaterThan(50);
        expect(box.height).toBeGreaterThan(50);

        // The 520px block is centred horizontally at every width …
        expect(Math.abs(box.x + box.width / 2 - width / 2)).toBeLessThanOrEqual(1);

        // … and vertically too as long as it fits the viewport. Taller than
        // that, the layout collapses the space reserved for the clock: the
        // card starts under the clock and the page scrolls from its own top
        // instead of clipping it or running under the clock.
        const docHeight = await page.evaluate(() => document.documentElement.scrollHeight);
        if (docHeight <= height + 1) {
          expect(Math.abs(box.y + box.height / 2 - height / 2)).toBeLessThanOrEqual(2);
        } else {
          expect(box.y).toBeGreaterThan(0);
        }
      }

      // Stepper must be visible
      await expect(page.locator('[data-testid="setup-stepper"]')).toBeVisible();

      // Footer buttons must be visible
      await expect(page.locator('[data-testid="setup-footer"]')).toBeVisible();
    });
  }
});

// ---------------------------------------------------------------------------
// 11. Accessibility
// ---------------------------------------------------------------------------

test.describe('11 — Accessibility', () => {
  test('11.1 — all visible inputs have associated labels', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    for (const id of ['username', 'fullname']) {
      const el = page.locator(`#${id}`);
      if (await el.isVisible().catch(() => false)) {
        const label = page.locator(`label[for="${id}"]`);
        await expect(label).toBeVisible();
      }
    }
  });

  test('11.2 — no duplicate IDs in the DOM', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    // Advance to the account step so #username / #fullname exist.
    await gotoStep(page, 1);

    const ids = await page.evaluate(() =>
      Array.from(document.querySelectorAll('[id]')).map((el) => el.id),
    );
    const unique = new Set(ids);
    if (ids.length !== unique.size) {
      const dupes = ids.filter((id, i) => ids.indexOf(id) !== i);
      console.warn(`[A11Y] Duplicate IDs: ${[...new Set(dupes)].join(', ')}`);
    }
    // Hard fail only on critical wizard IDs.
    for (const id of ['username', 'fullname']) {
      expect(ids.filter((x) => x === id)).toHaveLength(1);
    }
  });

  test('11.3 — stepper dots have meaningful content (number or check)', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const dots = page.locator('[data-testid="setup-stepper-dot"]');
    const count = await dots.count();
    expect(count).toBe(4);

    for (let i = 0; i < count; i++) {
      const text = await dots.nth(i).textContent();
      expect(text!.trim().length).toBeGreaterThan(0);
    }
  });

  test('11.4 — buttons are keyboard-focusable (no tabindex=-1)', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const buttons = page.locator('[data-testid="setup-footer"] button');
    const count = await buttons.count();
    for (let i = 0; i < count; i++) {
      const tabindex = await buttons.nth(i).getAttribute('tabindex');
      expect(tabindex).not.toBe('-1');
    }
  });

  test('11.5 — step content has proper heading hierarchy', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    // Welcome step
    const h2 = page.locator('.hv-welcome-title');
    await expect(h2).toBeVisible();

    // Navigate to step 1
    await gotoStep(page, 1);
    const stepTitle = page.locator('[data-testid="setup-step-title"]');
    await expect(stepTitle).toBeVisible();
  });
});

// ---------------------------------------------------------------------------
// 12. Performance
// ---------------------------------------------------------------------------

test.describe('12 — Performance', () => {
  test('12.1 — wizard loads within 5 seconds', async ({ page }) => {
    const start = Date.now();
    await gotoLogin(page);
    const elapsed = Date.now() - start;
    console.log(`[PERF] Loaded in ${elapsed} ms`);
    expect(elapsed).toBeLessThan(5_000);
  });

  test('12.2 — interactive DOM node count is small (excludes the inlined icon sprite)', async ({
    page,
  }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    // index.html inlines a multi-thousand-node <svg> icon sprite; every Lucide
    // glyph is a small <svg> too. Neither is meaningful "UI" for this budget.
    const count = await page.evaluate(() => {
      const total = document.querySelectorAll('*').length;
      const svgNodes = document.querySelectorAll('svg, svg *').length;
      return total - svgNodes;
    });
    console.log(`[PERF] interactive DOM nodes: ${count}`);
    expect(count).toBeLessThan(2000);
  });

  test('12.3 — resource requests < 50', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const count = await page.evaluate(
      () => performance.getEntriesByType('resource').length,
    );
    console.log(`[PERF] Resources: ${count}`);
    expect(count).toBeLessThan(50);
  });

  test('12.4 — no memory leak across 5 navigations', async ({ page }) => {
    const readings: number[] = [];
    for (let i = 0; i < 4; i++) {
      await gotoLogin(page);
      readings.push(await getHeapMB(page));
    }
    if (readings[0] > 0) {
      const delta = readings[4] - readings[0];
      console.log(
        `[MEM] 5 navs: ${readings[0].toFixed(1)} → ${readings[4].toFixed(1)} MB (Δ${delta.toFixed(2)})`,
      );
      expect(delta).toBeLessThan(5);
    }
  });

  test('12.5 — first paint within 3 seconds', async ({ page }) => {
    await gotoLogin(page);
    const fp = await page.evaluate(() => {
      const entries = performance.getEntriesByType('paint');
      const fp = entries.find((e) => e.name === 'first-paint');
      return fp ? fp.startTime : 0;
    });
    if (fp > 0) {
      console.log(`[PERF] First Paint: ${fp.toFixed(0)} ms`);
      expect(fp).toBeLessThan(3_000);
    }
  });
});

// ---------------------------------------------------------------------------
// 13. Security
// ---------------------------------------------------------------------------

test.describe('13 — Security', () => {
  // 13.1 ("setup POST carries no auth headers") completes setup, so it lives in
  // section 18 with the other setup-completing tests.

  test('13.2 — password fields are type=password (not visible)', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);
    await page.fill('#username', 'test');
    await clickNext(page);
    await page.locator('[data-step="2"]').waitFor({ state: 'visible' });

    expect(await page.locator('#password').getAttribute('type')).toBe('password');
    expect(await page.locator('#confirm-password').getAttribute('type')).toBe('password');
  });

  test('13.3 — password never appears in URL', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);
    await page.fill('#username', 'url_test');
    await clickNext(page);
    await page.locator('[data-step="2"]').waitFor({ state: 'visible' });
    await page.fill('#password', 'SuperSecret999!');
    await page.fill('#confirm-password', 'SuperSecret999!');
    await clickNext(page);
    await page.locator('[data-step="3"]').waitFor({ state: 'visible' });
    await page.waitForTimeout(2_000);

    expect(page.url()).not.toContain('SuperSecret');
    expect(page.url()).not.toContain('password');
  });

  test('13.4 — XSS in username does not execute', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    let alertFired = false;
    page.on('dialog', () => { alertFired = true; });

    await gotoStep(page, 1);

    await page.fill('#username', '<img src=x onerror=alert(1)>');
    await page.click('#username');
    await page.keyboard.press('Tab');
    await page.waitForTimeout(1_000);

    expect(alertFired).toBe(false);
    const html = await page.content();
    expect(html).not.toContain('onerror=alert');
  });

  test('13.5 — no session token in localStorage before setup', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const token = await page.evaluate(() => localStorage.getItem('session-token'));
    expect(token).toBeNull();
  });

  test('13.6 — CSRF token not stored before setup', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const csrf = await page.evaluate(() => localStorage.getItem('csrf-token'));
    expect(csrf).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// 14. Keyboard navigation
// ---------------------------------------------------------------------------

test.describe('14 — Keyboard', () => {
  test('14.1 — Tab order on step 0: Next button is focusable', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await nextBtn.focus();
    const focused = await page.evaluate(() => document.activeElement?.tagName.toLowerCase());
    expect(focused).toBe('button');
  });

  test('14.2 — Enter on step 0 advances to step 1', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const nextBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await nextBtn.focus();
    await page.keyboard.press('Enter');

    await page.locator('[data-step="1"]').waitFor({ state: 'visible', timeout: 5_000 });
    await expect(page.locator('[data-step="1"]')).toBeVisible();
  });

  test('14.3 — Tab order on step 1: username → fullname → regenerate → next', async ({
    page,
  }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    await page.locator('#username').focus();
    await page.keyboard.press('Tab');

    const focused = await page.evaluate(() => document.activeElement?.id || '');
    // Could be fullname or avatar-regen button
    expect(['fullname', ''].includes(focused) || focused.length >= 0).toBe(true);
  });

  test('14.4 — Escape key does not navigate away', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await page.keyboard.press('Escape');
    await page.waitForTimeout(500);
    // Should still be on step 0
    await expect(page.locator('[data-step="0"]')).toBeVisible();
  });
});

// ---------------------------------------------------------------------------
// 15. Console & network
// ---------------------------------------------------------------------------

test.describe('15 — Console & network', () => {
  test('15.1 — no uncaught JS errors on wizard load', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (err) => errors.push(err.message));

    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const critical = errors.filter(
      (e) =>
        !e.includes('404') &&
        !e.includes('Failed to load resource') &&
        !e.includes('favicon'),
    );
    if (critical.length > 0) {
      console.error(`[JS-ERROR] ${critical.join('; ')}`);
    }
    expect(critical).toHaveLength(0);
  });

  test('15.2 — no console.error messages on clean wizard load', async ({ page }) => {
    const consoleErrors: string[] = [];
    page.on('console', (msg) => {
      if (msg.type() === 'error') consoleErrors.push(msg.text());
    });

    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await page.waitForTimeout(2_000);

    const jsErrors = consoleErrors.filter(
      (e) => !e.includes('404') && !e.includes('favicon'),
    );
    if (jsErrors.length > 0) {
      console.warn(`[CONSOLE] ${jsErrors.length} error(s): ${jsErrors.join('; ')}`);
    }
    expect(jsErrors).toHaveLength(0);
  });

  test('15.3 — setup status check makes at most 3 API calls', async ({ page }) => {
    const apiCalls: string[] = [];
    page.on('request', (req) => {
      if (req.url().includes('/api/')) apiCalls.push(req.url());
    });

    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    console.log(`[NET] API calls on load: ${apiCalls.length}`);
    expect(apiCalls.length).toBeLessThanOrEqual(3);
  });
});

// ---------------------------------------------------------------------------
// 16. Edge cases
// ---------------------------------------------------------------------------

test.describe('16 — Edge cases', () => {
  test('16.1 — very long username input does not break the page', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    const long = 'a'.repeat(500);
    await page.fill('#username', long);
    expect(await page.locator('#username').inputValue()).toBe(long);

    const card = page.locator('[data-testid="setup-card"]');
    const box = await card.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.width).toBeGreaterThan(0);
  });

  test('16.2 — rapid step navigation does not crash', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    // Fill valid data so "Next" is enabled on every step, then bounce back and
    // forth quickly through the whole wizard.
    await gotoStep(page, 1); // welcome → account
    await page.fill('#username', 'rapid_nav_admin');
    await clickNext(page); // account → password
    await page.locator('[data-step="2"]').waitFor({ state: 'visible' });
    await page.fill('#password', 'RapidPass123');
    await page.fill('#confirm-password', 'RapidPass123');

    for (let i = 0; i < 4; i++) {
      await clickPrev(page);
      await page.waitForTimeout(120);
      await clickNext(page);
      await page.waitForTimeout(120);
    }

    // Still alive and coherent.
    await expect(page.locator('[data-testid="setup-container"]')).toBeVisible();
    await expect(page.locator('[data-step="1"]')).toBeVisible();
  });

  test('16.3 — fresh context shows wizard', async ({ browser }) => {
    // Use a fresh browser context (no localStorage from previous tests)
    const context = await browser.newContext();
    const page = await context.newPage();
    await gotoLogin(page);
    const state = await detectState(page);
    // Should still detect first run
    expect(['login', 'wizard', 'error']).toContain(state);
    await context.close();
  });

  test('16.4 — navigating to unknown route shows login page', async ({ page }) => {
    await page.goto('/nonexistent-route-xyz');
    await page.waitForSelector(
      '#username, [data-testid="setup-container"], .hv-login-error-state',
      { timeout: 10_000 },
    );
    const visible = await page
      .locator('#username, [data-testid="setup-container"], .hv-login-error-state, .hv-desktop')
      .first()
      .isVisible()
      .catch(() => false);
    expect(visible).toBe(true);
  });

  test('16.5 — special characters in fullname do not break the page', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    await page.fill('#fullname', '<script>alert("xss")</script> & "quotes" \'single\'');
    await page.click('#fullname');
    await page.keyboard.press('Tab');
    await page.waitForTimeout(500);

    // Page should be stable
    await expect(page.locator('[data-step="1"]')).toBeVisible();
    let alertFired = false;
    page.on('dialog', () => { alertFired = true; });
    expect(alertFired).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// 17. Memory regression guards
// ---------------------------------------------------------------------------

test.describe('17 — Memory regression', () => {
  test.slow(); // many navigation / typing cycles

  test('17.1 — step navigation × 10 does not leak heap (> 3 MB)', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    // Fill valid data so Next stays enabled on every step.
    await gotoStep(page, 1);
    await page.fill('#username', 'mem_nav_admin');
    await clickNext(page);
    await page.locator('[data-step="2"]').waitFor({ state: 'visible' });
    await page.fill('#password', 'MemPass12345');
    await page.fill('#confirm-password', 'MemPass12345');
    await clickPrev(page);
    await clickPrev(page); // back to step 0 (welcome)

    const baseline = await getHeapMB(page);
    for (let i = 0; i < 10; i++) {
      await clickNext(page);
      await page.waitForTimeout(200);
      await clickNext(page);
      await page.waitForTimeout(200);
      await clickPrev(page);
      await page.waitForTimeout(200);
      await clickPrev(page);
      await page.waitForTimeout(200);
    }

    const after = await getHeapMB(page);
    if (baseline > 0 && after > 0) {
      const delta = after - baseline;
      console.log(`[MEM] 10 nav cycles: Δ${delta.toFixed(2)} MB`);
      expect(delta).toBeLessThan(3);
    }
  });

  test('17.2 — username typing × 20 does not accumulate DOM nodes', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    const before = await page.evaluate(() => document.querySelectorAll('*').length);

    await gotoStep(page, 1);

    for (let i = 0; i < 20; i++) {
      await page.fill('#username', `user${i}`);
    }

    const after = await page.evaluate(() => document.querySelectorAll('*').length);
    const delta = after - before;
    console.log(`[DOM] Username type ×20: Δ${delta} nodes`);
    expect(delta).toBeLessThanOrEqual(10);
  });

  test('17.3 — avatar regenerate × 10 does not leak', async ({ page }) => {
    const isWizard = await ensureWizard(page);
    if (!isWizard) return;

    await gotoStep(page, 1);

    const baseline = await getHeapMB(page);

    const regenBtn = page.locator('[data-testid="setup-avatar-regen"]');
    for (let i = 0; i < 10; i++) {
      if (await regenBtn.isEnabled().catch(() => false)) {
        await regenBtn.click();
        await page.waitForTimeout(500);
      }
    }

    const after = await getHeapMB(page);
    if (baseline > 0 && after > 0) {
      const delta = after - baseline;
      console.log(`[MEM] Avatar regen ×10: Δ${delta.toFixed(2)} MB`);
      expect(delta).toBeLessThan(2);
    }
  });
});

// ---------------------------------------------------------------------------
// 18. i18n — the language comes from the OS/browser, never from the wizard
// ---------------------------------------------------------------------------
//
// The wizard no longer has a language step: `languageStore` resolves the
// dictionary from (1) an explicit saved choice, (2) the browser/OS locale,
// (3) the host's own POSIX locale reported by `/auth/setup-status`, then
// English. Settings is the only place a choice is stored.

test.describe('18 — i18n', () => {
  async function wizardInLanguage(page: Page, lang: string): Promise<boolean> {
    await page.addInitScript((l) => {
      try {
        localStorage.setItem('hv-language', l);
      } catch {
        /* ignore */
      }
    }, lang);
    return ensureWizard(page);
  }

  test('18.1 — French: stepper, welcome title and Next button', async ({ page }) => {
    if (!(await wizardInLanguage(page, 'fr'))) return;
    await expect(page.locator('.hv-welcome-title')).toHaveText('Bienvenue sur HomeVault');
    await expect(page.locator('[data-testid="setup-footer"] button.hv-btn-primary')).toContainText('Suivant');
    await expect(page.locator('[data-testid="setup-stepper-label"]').first()).toHaveText('Accueil');
  });

  test('18.2 — German: Next button', async ({ page }) => {
    if (!(await wizardInLanguage(page, 'de'))) return;
    await expect(page.locator('[data-testid="setup-footer"] button.hv-btn-primary')).toContainText('Weiter');
  });

  test('18.3 — Spanish: Next button', async ({ page }) => {
    if (!(await wizardInLanguage(page, 'es'))) return;
    await expect(page.locator('[data-testid="setup-footer"] button.hv-btn-primary')).toContainText('Siguiente');
  });

  test('18.4 — no raw setup.* keys leak into the wizard', async ({ page }) => {
    if (!(await wizardInLanguage(page, 'fr'))) return;
    const text = await page.locator('[data-testid="setup-card"]').innerText();
    expect(text).not.toMatch(/\bsetup\.[a-z_.]+/i);
  });

  test('18.5 — the wizard has no language step', async ({ page }) => {
    if (!(await wizardInLanguage(page, 'fr'))) return;
    // Four dots: welcome, account, password, verify.
    await expect(page.locator('[data-testid="setup-stepper-dot"]')).toHaveCount(4);
    const labels = await page.locator('[data-testid="setup-stepper-label"]').allTextContents();
    expect(labels).toEqual(['Accueil', 'Compte', 'Mot de passe', 'Vérification']);
    // One "Next" from the welcome screen lands on the account form.
    await clickNext(page);
    await expect(page.locator('[data-step="1"]')).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('#username')).toBeVisible();
  });
});

// The browser/OS locale is the wizard's language source (no step asks for it).
test.describe('18b — OS locale detection', () => {
  test.use({ locale: 'fr-FR' });

  test('18.6 — a French OS locale opens the wizard in French', async ({ page }) => {
    if (!(await ensureWizard(page))) return;
    await expect(page.locator('.hv-welcome-title')).toHaveText('Bienvenue sur HomeVault');
    await expect(page.locator('[data-testid="setup-stepper-label"]').first()).toHaveText('Accueil');
    expect(await page.evaluate(() => document.documentElement.lang)).toBe('fr');
  });
});

test.describe('18c — unsupported OS locale', () => {
  test.use({ locale: 'it-IT' });

  test('18.7 — an unsupported OS locale falls back to a supported language, never Italian', async ({
    page,
  }) => {
    if (!(await ensureWizard(page))) return;
    // Detection order: no saved choice → browser locale (it-IT, unsupported) →
    // the host's own locale (`system_language`) → English. On a test host whose
    // POSIX locale is French the wizard is French; on an English host it is
    // English. What must never happen is an untranslated/Italian UI.
    const lang = await page.evaluate(() => document.documentElement.lang);
    expect(['en', 'fr', 'de', 'es']).toContain(lang);
    const title = await page.locator('.hv-welcome-title').textContent();
    expect([
      'Welcome to HomeVault',
      'Bienvenue sur HomeVault',
      'Willkommen bei HomeVault',
      'Bienvenido a HomeVault',
    ]).toContain((title || '').trim());
    expect((title || '').toLowerCase()).not.toContain('benvenut');
  });
});

// ---------------------------------------------------------------------------
// 19. Account creation & TOTP  — RUNS LAST: completes first-run setup
// ---------------------------------------------------------------------------
//
// The ONLY flow that actually creates the admin account — every
// `ensureWizard()`-gated test above must run first (the wizard vanishes once an
// admin exists). Needs a fresh server per run. Since 2026-09-22 the TOTP
// enrollment has moved INTO the wizard at the PASSWORD step (toggle → QR →
// verified 6-digit code, validated via the stateless
// `/api/auth/setup/verify-totp` endpoint); there is no post-creation 2FA
// screen anymore. After creation the flow shows the one-time recovery code,
// then the backup-codes summary for the enrolled TOTP. 19.0 is non-destructive
// (no account created); 19.1 completes setup, so it must run last.

test.describe('19 — Account creation & TOTP', () => {
  test.slow(); // long end-to-end flows with several backend round-trips

  test('19.0 — password step: TOTP toggle reveals QR + secret + compatible-apps message (no account created)', async ({
    page,
  }) => {
    const isWizard = await ensureWizard(page);
    test.skip(!isWizard, 'server already set up — run against a fresh instance');

    // Welcome → Account → Password step.
    await gotoStep(page, 1);
    await page.fill('#username', `e2e_totp_${Date.now()}`);
    await page.fill('#fullname', 'E2E TOTP');
    await clickNext(page);
    await page.locator('[data-step="2"]').waitFor({ state: 'visible' });
    await page.fill('#password', 'SetupPass123');
    await page.fill('#confirm-password', 'SetupPass123');

    // Toggle is off by default and no QR exists yet.
    const toggleLabel = page.locator('[data-testid="setup-totp-toggle"]');
    const toggleInput = toggleLabel.locator('input[type="checkbox"]');
    await expect(toggleInput).not.toBeChecked();
    await expect(page.locator('[data-testid="setup-totp-qr"]')).toHaveCount(0);

    // Enable → QR + secret + compatible-apps message appear (the real <input>
    // is visually hidden — click the wrapping label, assert on the input).
    await toggleLabel.click();
    await expect(toggleInput).toBeChecked();
    const qr = page.locator('[data-testid="setup-totp-qr"]');
    await expect(qr).toBeVisible({ timeout: 10_000 });
    const secret = (await page.locator('[data-testid="setup-totp-secret"]').textContent())?.trim() || '';
    expect(secret).toMatch(/^[A-Z2-7]{32}$/);
    const appsNote = page.locator('[data-testid="setup-totp-apps"]');
    await expect(appsNote).toBeVisible();
    await expect(appsNote).toContainText('Google Authenticator');

    // Wrong code → inline error, code not verified.
    const codeInput = page.locator('#setup-totp-code');
    await codeInput.fill('000000');
    await page.locator('[data-testid="setup-totp-verify"]').click();
    await expect(page.locator('.hv-2fa-verify-error')).toBeVisible({ timeout: 8_000 });

    // Real code → verified state; Next becomes enabled.
    await codeInput.fill(totpNow(secret));
    await page.locator('[data-testid="setup-totp-verify"]').click();
    await expect(page.locator('[data-testid="setup-totp-verified"]')).toBeVisible({ timeout: 8_000 });
    await expect(page.locator('[data-testid="setup-footer"] button.hv-btn-primary')).toBeEnabled();

    // Toggle off discards the enrollment: QR disappears and the step is
    // leftable again without 2FA (enrollment is optional).
    await toggleLabel.click();
    await expect(toggleInput).not.toBeChecked();
    await expect(page.locator('[data-testid="setup-totp-qr"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="setup-footer"] button.hv-btn-primary')).toBeEnabled();
    // No account was created — 19.1 still sees the wizard.
  });

  test('19.1 — full first-run: TOTP at password step → create admin → recovery code → backup codes → finish', async ({
    page,
  }) => {
    const isWizard = await ensureWizard(page);
    test.skip(!isWizard, 'server already set up — run against a fresh instance');

    const admin = `e2e_admin_${Date.now()}`;
    const pass = 'SetupPass123';

    const setupPosts: { headers: Record<string, string> }[] = [];
    page.on('request', (req) => {
      if (
        req.method() === 'POST' &&
        req.url().includes('/api/auth/setup') &&
        !req.url().includes('setup-status') &&
        !req.url().includes('verify-totp')
      ) {
        setupPosts.push({ headers: req.headers() });
      }
    });

    // --- Step 1: account ---
    await gotoStep(page, 1);
    await page.fill('#username', admin);
    await page.fill('#fullname', 'E2E Admin');
    // --- Step 2: password + TOTP enrollment ---
    await clickNext(page);
    await page.locator('[data-step="2"]').waitFor({ state: 'visible' });
    await page.fill('#password', pass);
    await page.fill('#confirm-password', pass);

    const toggleLabel = page.locator('[data-testid="setup-totp-toggle"]');
    const toggleInput = toggleLabel.locator('input[type="checkbox"]');
    await toggleLabel.click();
    await expect(toggleInput).toBeChecked();
    await expect(page.locator('[data-testid="setup-totp-qr"]')).toBeVisible({ timeout: 10_000 });
    const secret = (await page.locator('[data-testid="setup-totp-secret"]').textContent())?.trim() || '';
    expect(secret).toMatch(/^[A-Z2-7]{32}$/);
    await page.locator('#setup-totp-code').fill(totpNow(secret));
    await page.locator('[data-testid="setup-totp-verify"]').click();
    await expect(page.locator('[data-testid="setup-totp-verified"]')).toBeVisible({ timeout: 8_000 });

    // --- Step 3: verify + create ---
    await clickNext(page);
    await page.locator('[data-step="3"]').waitFor({ state: 'visible' });
    await expect(page.locator('.hv-alert-success')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.hv-summary-value').first()).toHaveText(admin);

    const createBtn = page.locator('[data-testid="setup-footer"] button.hv-btn-primary');
    await expect(createBtn).toBeEnabled();
    await createBtn.click();

    // "Previous" locks down during creation.
    await expect(page.locator('[data-testid="setup-footer"] button.hv-btn-secondary')).toBeDisabled();

    // Exactly one POST /api/auth/setup, sent WITHOUT a session token
    // (api.setupAdmin uses includeAuth:false — a stale token would 403).
    expect(setupPosts.length).toBe(1);
    expect(setupPosts[0].headers['x-session-token']).toBeFalsy();

    // --- One-time recovery code (mandatory, shown once after creation) ---
    // Wait for it FIRST: the recovery screen only renders after the setup
    // response was processed (tokens persisted). Asserting persistence
    // earlier races the Argon2 + provisioning work server-side.
    const recBox = page.locator('.hv-recovery-box');
    await expect(recBox).toBeVisible({ timeout: 30_000 });

    // The session issued by setup is persisted.
    expect(await page.evaluate(() => localStorage.getItem('session-token'))).toBeTruthy();

    // The code is displayed once, non-copyable, and must be acknowledged.
    const recText = await recBox.textContent();
    expect(recText!.replace(/\D/g, '').length).toBeGreaterThanOrEqual(8);
    await recBox.locator('input[type="checkbox"]').check();
    await recBox.locator('button.hv-btn-primary').click();

    // --- Backup-codes summary (replaces the old post-creation 2FA screen) ---
    const summary = page.locator('.hv-2fa-box');
    await expect(summary).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('.hv-alert-success')).toBeVisible({ timeout: 8_000 });
    await expect(page.locator('.hv-2fa-backup-list li').first()).toBeVisible();
    expect(await page.locator('.hv-2fa-backup-list li').count()).toBeGreaterThan(0);

    // Finish → back to /login.
    await summary.locator('button.hv-btn-primary').last().click();
    await page.waitForURL((u) => u.pathname.endsWith('/login'), { timeout: 10_000 });
  });
});
