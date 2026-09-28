import { firefox } from 'playwright';
const BASE = 'http://localhost:5173';
const browser = await firefox.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 720 } });
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
try { await dockBtn.click({ timeout: 5000 }); } catch { await dockBtn.click({ force: true }); }
await page.waitForSelector('[data-testid="settings-page"]', { timeout: 20000 });
await page.waitForTimeout(1200);

const out = await page.evaluate(() => {
  const panel = document.querySelector('[data-testid="settings-content"]');
  const pr = panel.getBoundingClientRect();
  const hits = [];
  for (const el of document.querySelectorAll('*')) {
    const b = el.getBoundingClientRect();
    if (b.width <= 0 || b.height <= 0) continue;
    if (b.left >= pr.left - 2 && b.left <= pr.left + 26 && b.width <= 12 && b.height >= 15 && b.height <= 90) {
      const cs = getComputedStyle(el);
      hits.push({
        tag: el.tagName.toLowerCase(),
        cls: (el.className || '').toString().slice(0, 80),
        parent: (el.parentElement?.className || '').toString().slice(0, 60),
        x: Math.round(b.x), y: Math.round(b.y), w: Math.round(b.width), h: Math.round(b.height),
        bg: cs.backgroundColor, pos: cs.position, pe: cs.pointerEvents, z: cs.zIndex,
      });
    }
  }
  // stacked hit test exactly where the bar is
  const stack = document.elementsFromPoint(pr.left + 8, pr.top + 50).map((e) => ({
    tag: e.tagName.toLowerCase(), cls: (e.className || '').toString().slice(0, 70),
  }));
  // also: any element whose rect *contains* that point and paints a colour
  const containing = [];
  for (const el of document.querySelectorAll('*')) {
    const b = el.getBoundingClientRect();
    if (b.left <= pr.left + 8 && b.right >= pr.left + 8 && b.top <= pr.top + 50 && b.bottom >= pr.top + 50) {
      const cs = getComputedStyle(el);
      if (cs.backgroundColor !== 'rgba(0, 0, 0, 0)' || cs.borderLeftWidth !== '0px' && cs.borderLeftColor !== 'rgba(0, 0, 0, 0)') {
        containing.push({ tag: el.tagName.toLowerCase(), cls: (el.className || '').toString().slice(0, 60), bg: cs.backgroundColor, bl: `${cs.borderLeftWidth} ${cs.borderLeftColor}` });
      }
    }
  }
  return { panel: { x: Math.round(pr.x), y: Math.round(pr.y) }, hits, stack, containing: containing.slice(0, 20) };
});
console.log(JSON.stringify(out, null, 2));
await browser.close();
