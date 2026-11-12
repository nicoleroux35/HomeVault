/**
 * D2 — automated accessibility gate (WCAG 2.2 AA).
 *
 * The project treats WCAG 2.2 AA / EN 301 549 as a correctness bar, but the
 * only accessibility evidence used to be a manual audit list. This suite runs
 * axe-core against the login screen and every in-app screen (the desktop shell
 * + one window per app), and fails the build on any NEW violation.
 *
 * Existing debt is frozen in `a11y-baseline.json` (a burn-down list). When a
 * screen is fixed, re-run with `HV_A11Y_UPDATE=1` to shrink the baseline; a
 * regression in a previously clean screen fails immediately.
 *
 * Run:
 *   npx playwright test tests/e2e/design/a11y.spec.ts
 *   HV_A11Y_UPDATE=1 npx playwright test tests/e2e/design/a11y.spec.ts
 *
 * Requires a live, seeded server (see TESTING.md / the e2e workflow).
 */
import fs from 'node:fs';
import path from 'node:path';
import { test, expect, request, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import {
  ADMIN_PASS,
  ADMIN_USER,
  APP_IDS,
  apiLogin,
  closeLastWindow,
  gotoDesktop,
  openApp,
  type Tokens,
} from './designGates';

const TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'];
const BASELINE = path.join(process.cwd(), 'tests/e2e/design/a11y-baseline.json');

interface Baseline {
  [screen: string]: string[];
}

function readBaseline(): Baseline {
  try {
    return JSON.parse(fs.readFileSync(BASELINE, 'utf8')) as Baseline;
  } catch {
    return {};
  }
}

/** Stable identity of one violation node: rule id + normalised target. */
function violationKeys(result: Awaited<ReturnType<AxeBuilder['analyze']>>): string[] {
  const keys: string[] = [];
  for (const violation of result.violations) {
    for (const node of violation.nodes) {
      const target = node.target
        .join(' ')
        // Generated ids (window-1, lp-info-…) would make the baseline churn.
        .replace(/#window-\d+/g, '#window-N')
        .replace(/#lp-(info|state)-[^ ]+/g, '#lp-$1-X');
      keys.push(`${violation.id}::${target}`);
    }
  }
  return [...new Set(keys)].sort();
}

async function scan(page: Page, include?: string) {
  let builder = new AxeBuilder({ page }).withTags(TAGS);
  if (include) builder = builder.include(include);
  return builder.analyze();
}

const collected = new Map<string, string[]>();

function compare(screen: string, keys: string[]) {
  collected.set(screen, keys);
  if (process.env.HV_A11Y_UPDATE === '1') return;
  const baseline = readBaseline();
  const known = new Set(baseline[screen] ?? []);
  const added = keys.filter((key) => !known.has(key));

  expect(
    added,
    `${added.length} new axe violation(s) on “${screen}”. Fix them, or — only for ` +
      `pre-existing debt — re-run with HV_A11Y_UPDATE=1 to add them to the baseline.`,
  ).toEqual([]);
}

test.describe.configure({ mode: 'serial' });

let tokens: Tokens;

test.beforeAll(async () => {
  const ctx = await request.newContext({ baseURL: process.env.BASE_URL || 'http://localhost:8080' });
  tokens = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
  await ctx.dispose();
  expect(tokens.token, 'the seeded admin must log in — is the server seeded?').toBeTruthy();
});

test.afterAll(() => {
  if (process.env.HV_A11Y_UPDATE !== '1') return;
  const baseline = readBaseline();
  for (const [screen, keys] of collected) {
    // Union with whatever the other browser project already recorded, so the
    // baseline covers both engines instead of one overwriting the other.
    baseline[screen] = [...new Set([...(baseline[screen] ?? []), ...keys])].sort();
  }
  fs.writeFileSync(BASELINE, `${JSON.stringify(baseline, null, 2)}\n`);
});

test('login screen has no new axe violations', async ({ browser }) => {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  await page.goto('/login');
  await page.waitForSelector('#username', { timeout: 20_000 });
  await page.waitForTimeout(400);

  compare('login', violationKeys(await scan(page)));
  await ctx.close();
});

test('desktop shell has no new axe violations', async ({ page }) => {
  await gotoDesktop(page, tokens);
  await page.waitForTimeout(400);
  compare('desktop', violationKeys(await scan(page)));
});

for (const appId of APP_IDS) {
  test(`${appId} has no new axe violations`, async ({ page }) => {
    await gotoDesktop(page, tokens);
    await openApp(page, appId);
    try {
      compare(appId, violationKeys(await scan(page, '.hv-window')));
    } finally {
      await closeLastWindow(page).catch(() => {});
    }
  });
}
