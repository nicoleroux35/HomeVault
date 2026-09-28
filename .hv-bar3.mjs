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
  const root = document.querySelector('[data-testid="settings-page"]').getBoundingClientRect();
  const res = [];
  for (const el of document.querySelectorAll('*')) {
    for (const pseudo of ['::before', '::after']) {
      const cs = getComputedStyle(el, pseudo);
      if (cs.content === 'none' || cs.content === 'normal') continue;
      const w = parseFloat(cs.width), h = parseFloat(cs.height);
      if (!(w > 0 && w <= 10 && h >= 12 && h <= 90)) continue;
      const b = el.getBoundingClientRect();
      res.push({
        pseudo, tag: el.tagName.toLowerCase(), cls: (el.className||'').toString().slice(0,70),
        ownerRel: { x: Math.round(b.x - root.x), y: Math.round(b.y - root.y), w: Math.round(b.width), h: Math.round(b.height) },
        w: Math.round(w), h: Math.round(h), pos: cs.position, bg: cs.backgroundColor,
        inset: `${cs.top} ${cs.right} ${cs.bottom} ${cs.left}`,
      });
    }
  }
  return res;
});
console.log(JSON.stringify(out, null, 1));
await browser.close();
