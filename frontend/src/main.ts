import { mount } from 'svelte'
import './lib/styles/fonts.css'
import './lib/styles/theme/index.css'  // Load theme tokens first
import './app.css'
import App from './App.svelte'

// `Icon.svelte` renders every icon as `<use href="#tabler-x">` - a
// same-document fragment reference, which only resolves once the sprite's
// `<symbol>` definitions actually exist somewhere in this page's DOM. Until
// now nothing ever put them there: index.html only <link rel="preload">s
// the sprite file (a resource hint, not a DOM insertion), so every `<Icon>`
// consumer across the app rendered a correctly-sized but empty icon slot.
// Fetched and injected here rather than inlined into index.html - the
// sprite is 4MB (every Phosphor/Tabler glyph), too large to want in
// the initial HTML response - and asynchronously/non-blocking, since `<use>`
// re-resolves live once its target symbol appears in the DOM, so icons
// simply pop in a moment after mount rather than delaying first paint.
async function loadIconSprite() {
  try {
    const res = await fetch('/sprite-unified.svg');
    if (!res.ok) return;
    const svgMarkup = await res.text();
    const container = document.createElement('div');
    container.setAttribute('aria-hidden', 'true');
    container.style.cssText = 'position:absolute;width:0;height:0;overflow:hidden;';
    container.innerHTML = svgMarkup;
    document.body.insertBefore(container, document.body.firstChild);
  } catch {
    // Non-fatal: icons stay empty, everything else keeps working.
  }
}

loadIconSprite();

const app = mount(App, {
  target: document.getElementById('app')!,
})

export default app
