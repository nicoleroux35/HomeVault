import { firefox } from 'playwright';
const BASE = 'http://localhost:5173';
const browser = await firefox.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 720 } });
const page = await ctx.newPage();
await page.addInitScript(({ t, c }) => {
  try { localStorage.setItem('session-token', t); localStorage.setItem('csrf-token', c); localStorage.setItem('hv-language','en'); } catch {}
}, { t: process.env.TOKEN, c: process.env.CSRF });
await page.goto(BASE + '/desktop', { waitUntil: 'domcontentloaded' });
await page.waitForSelector('.hv-dock', { timeout: 30000 });
await page.addStyleTag({ content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}' }).catch(() => {});
try { await page.locator('.hv-dock-item:has(.hv-dock-icon-settings)').first().click({ timeout: 5000 }); } catch {}
await page.waitForSelector('[data-testid="settings-page"]', { timeout: 20000 });
await page.waitForTimeout(1000);
const out = await page.evaluate(() => {
  const win = document.querySelector('[data-testid="settings-page"]').getBoundingClientRect();
  const res = [];
  for (const el of document.querySelectorAll('*')) {
    const b = el.getBoundingClientRect();
    if (b.width <= 0 || b.height <= 0) continue;
    if (b.width <= 8 && b.height >= 12 && b.height <= 90) {
      const cs = getComputedStyle(el);
      const bg = cs.backgroundColor;
      const solid = bg !== 'rgba(0, 0, 0, 0)' && bg !== 'transparent';
      if (solid) res.push({ rel: { x: Math.round(b.x - win.x), y: Math.round(b.y - win.y) }, w: Math.round(b.width), h: Math.round(b.height), tag: el.tagName.toLowerCase(), cls: (el.className||'').toString().slice(0,70), bg, pos: cs.position, parentCls: (el.parentElement?.className||'').toString().slice(0,50) });
    }
  }
  return { win: { x: Math.round(win.x), y: Math.round(win.y) }, res };
});
console.log(JSON.stringify(out, null, 1));
await browser.close();
