import { firefox } from 'playwright';

const BASE = 'http://localhost:5173';
const CATS = ['general', 'security', 'appearance', 'widgets', 'notifications', 'ai', 'monitoring', 'system'];
const LABEL = {
  general: 'General', security: 'Security', appearance: 'Appearance', widgets: 'Widgets',
  notifications: 'Notifications', ai: 'AI Assistant', monitoring: 'Monitoring', system: 'System',
};

const browser = await firefox.launch();

async function open(ctx) {
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
  await page.waitForTimeout(700);
  return page;
}

async function measure(page, vpName, cats) {
  for (const c of cats) {
    const item = page.locator('[data-testid="settings-nav-item"]').filter({ hasText: LABEL[c] });
    await item.click();
    await page.waitForTimeout(350);
    const r = await page.evaluate(() => {
      const root = document.querySelector('[data-testid="settings-page"]');
      const panel = document.querySelector('[data-testid="settings-content"]');
      const pr = panel.getBoundingClientRect();
      const pcs = getComputedStyle(panel);
      const kids = [...panel.children].map((el) => {
        const b = el.getBoundingClientRect();
        return { t: el.getAttribute('data-testid') || (el.className || '').toString().slice(0, 24), y: Math.round(b.y), h: Math.round(b.height), w: Math.round(b.width), x: Math.round(b.x) };
      });
      const gaps = [];
      for (let i = 1; i < kids.length; i++) gaps.push(kids[i].y - (kids[i - 1].y + kids[i - 1].h));
      const rows = [];
      for (const card of panel.querySelectorAll('[data-testid="settings-card"]')) {
        for (const row of card.children) {
          if (!(row instanceof HTMLElement)) continue;
          const cs = getComputedStyle(row);
          if (!cs.display.startsWith('flex')) continue;
          const kids2 = [...row.children].map((k) => { const b = k.getBoundingClientRect(); return { tag: k.tagName.toLowerCase(), w: Math.round(b.width), h: Math.round(b.height), x: Math.round(b.x) }; });
          if (kids2.length < 2) continue;
          const a = kids2[0], z = kids2[kids2.length - 1];
          rows.push({ dir: cs.flexDirection, labelW: a.w, controlW: z.w, controlTag: z.tag, space: Math.round(z.x - (a.x + a.w)), rowH: Math.round(row.getBoundingClientRect().height), text: (row.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 28) });
        }
      }
      const hdr = document.querySelector('[data-testid="settings-header-title"]');
      return {
        winW: Math.round(root.getBoundingClientRect().width),
        panelX: Math.round(pr.x), panelW: Math.round(pr.width),
        padL: pcs.paddingLeft, padT: pcs.paddingTop,
        cardW: Math.round((panel.querySelector('[data-testid="settings-card"]')?.getBoundingClientRect().width) || 0),
        firstTop: kids[0] ? kids[0].y - Math.round(pr.y) : null,
        kids: kids.map((k) => `${k.t}@${k.y}+${k.h}`),
        gaps,
        rows,
        hdrClip: hdr ? hdr.scrollWidth - hdr.clientWidth : null,
        panelScrollX: panel.scrollWidth - panel.clientWidth,
        docOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      };
    });
    console.log(JSON.stringify({ vp: vpName, cat: c, ...r }));
  }
}

const VPs = [
  ['wide', { width: 1920, height: 1080 }],
  ['desk', { width: 1280, height: 720 }],
  ['mid', { width: 1024, height: 768 }],
  ['tab', { width: 768, height: 1024 }],
  ['tiny', { width: 360, height: 740 }],
];

for (const [name, vp] of VPs) {
  const ctx = await browser.newContext({ viewport: vp });
  const page = await open(ctx);
  if (name === 'desk') {
    const odd = await page.evaluate(() => {
      const panel = document.querySelector('[data-testid="settings-content"]');
      const pr = panel.getBoundingClientRect();
      const out = [];
      for (const el of panel.querySelectorAll('*')) {
        const b = el.getBoundingClientRect();
        if (b.width > 0 && b.width <= 10 && b.height >= 14 && b.height <= 90 && b.left > pr.left && b.left < pr.left + 34) {
          out.push({ cls: (el.className || '').toString().slice(0, 70), x: Math.round(b.x), y: Math.round(b.y), w: Math.round(b.width), h: Math.round(b.height), bg: getComputedStyle(el).backgroundColor });
        }
      }
      // also whatever sits at that pixel from the whole document
      const probeEl = document.elementFromPoint(pr.left + 8, pr.top + 50);
      return { out, at: probeEl ? { tag: probeEl.tagName, cls: (probeEl.className || '').toString().slice(0, 80), bg: getComputedStyle(probeEl).backgroundColor } : null };
    });
    console.log('ODD ' + JSON.stringify(odd));
  }
  await measure(page, name, name === 'desk' || name === 'tiny' || name === 'tab' ? CATS : ['general', 'system']);
  await ctx.close();
}

await browser.close();
