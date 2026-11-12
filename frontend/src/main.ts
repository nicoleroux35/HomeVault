/* =============================================================================
 * File          : main.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Frontend entry point: loads the style cascade (fonts, theme tokens,
 *   app shell) in dependency order and mounts the root App component.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   Vite bundle entry (index.html).
 * =============================================================================
 */

import { mount } from 'svelte'
import './lib/styles/fonts.css'
import './lib/styles/theme/index.css'  // Load theme tokens first
import './app.css'
import App from './App.svelte'

const app = mount(App, {
  target: document.getElementById('app')!,
})

export default app
