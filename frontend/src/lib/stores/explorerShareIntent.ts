import { writable } from 'svelte/store';

/**
 * Set by the File Explorer's "Share this folder" row action just before it
 * asks the desktop to open the Shares app. Shares reads it once on mount to
 * pre-fill its "create share" form, then clears it.
 */
export const explorerShareIntent = writable<{ path: string } | null>(null);
