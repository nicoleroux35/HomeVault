/* =============================================================================
 * File          : explorerIntent.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   One-shot handoff into the File Explorer: "open this folder, and select
 *   this entry". The owner is the desktop (it opens the window); the Explorer
 *   reads it once on mount and clears it, so reopening the app later never
 *   jumps to a stale folder.
 *
 *   Distinct from `explorerShareIntent`, which targets the Shares app. Same
 *   convention, different destination — one file per intent keeps each one
 *   honest about who consumes it.
 *
 * Dependencies:
 *   - svelte (>= 5), writable
 *
 * Usage:
 *   `fileProvider.openHitInExplorer()` (the ⌘K file results) sets it and asks
 *   the desktop to open the Explorer; `routes/Explorer.svelte` consumes it.
 * =============================================================================
 */

import { writable } from 'svelte/store';

/**
 * `path` is the folder to open. `select`, when present, is the entry to
 * highlight inside it — always a child of `path` (the caller derives it from
 * the hit's own path, so the two cannot disagree).
 */
export interface ExplorerIntent {
  path: string;
  select?: string;
}

export const explorerIntent = writable<ExplorerIntent | null>(null);
