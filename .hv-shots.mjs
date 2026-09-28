import { firefox } from 'playwright';
import fs from 'node:fs';

const BASE = 'http://localhost:5173';
const OUT = '/tmp/opencode/shots';
fs.mkdirSync(OUT, { recursive: true });

const CATS = ['general', 'security', 'appearance', 'widgets', 'notifications', 'ai', 'monitoring', 'system'];
const LABEL = {
  general: 'General', security: 'Security', appearance: 'Appearance', widgets: 'Widgets',
  notifications: 'Notifications', ai: 'AI Assistant', monitoring: 'Monitoring', system: 'System',
};

const browser = await firefox.launch();

async function openSettings(ctx, vpName) {
  const page = await ctx.newPage();
  await page.addInitScript(({ t, c }) => {
    try {
      localStorage.setItem('session-token', t);
      localStorage.setItem('csrf-token', c);
      localStorage.setItem('hv-language', 'en');
    } catch {}
  }, { t: process.env.TOKEN, c: process.env.CSRF });
  await page.goto(BASE + '/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-dock', { timeout: 30000 });
  await page.addStyleTag({ content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}' }).catch(() => {});
  const dockBtn = page.locator('.hv-dock-item:has(.hv-dock-icon-settings)').first();
  await dockBtn.scrollIntoViewIfNeeded().catch(() => {});
  try { await dockBtn.click({ timeout: 5000 }); } catch { await dockBtn.click({ force: true }); }
  await page.waitForSelector('[data-testid="settings-page"]', { timeout: 20000 });
  await page.locator('[data-testid="settings-nav-item"]').first().waitFor({ state: 'visible', timeout: 20000 });
  await page.waitForTimeout(800);
  console.log('opened', vpName);
  return page;
}

async function shoot(page, vpName, cat) {
  const item = page.locator('[data-testid="settings-nav-item"]').filter({ hasText: LABEL[cat] });
  await item.click();
  await page.waitForTimeout(400);
  const el = page.locator('[data-testid="settings-page"]');
  await el.screenshot({ path: `${OUT}/${vpName}-${cat}.png` });
  console.log('shot', vpName, cat);
}

// Pass 1: every category at the default desktop viewport.
{
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 720 } });
  const page = await openSettings(ctx, 'desk');
  for (const c of CATS) await shoot(page, 'desk', c);
  await ctx.close();
}

// Pass 2: adaptation — wide, intermediate, narrow, tiny, on 3 categories.
for (const [vpName, vp] of [
  ['wide', { width: 1920, height: 1080 }],
  ['mid', { width: 1024, height: 768 }],
  ['tab', { width: 768, height: 1024 }],
  ['tiny', { width: 360, height: 740 }],
]) {
  const ctx = await browser.newContext({ viewport: vp });
  const page = await openSettings(ctx, vpName);
  for (const c of ['general', 'notifications', 'system']) await shoot(page, vpName, c);
  await ctx.close();
}

await browser.close();
console.log('done');
