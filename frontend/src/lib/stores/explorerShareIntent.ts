/* =============================================================================
 * File          : explorerShareIntent.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   One-shot handoff between apps: the File Explorer's "Share this folder"
 *   action sets it just before opening the Shares app, which reads it once
 *   on mount to pre-fill its create-share form and clears it.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   Written by Explorer.svelte, consumed once by Shares.svelte.
 * =============================================================================
 */

import { writable } from 'svelte/store';

/**
 * Set by the File Explorer's "Share this folder" row action just before it
 * asks the desktop to open the Shares app. Shares reads it once on mount to
 * pre-fill its "create share" form, then clears it.
 */
export const explorerShareIntent = writable<{ path: string } | null>(null);
