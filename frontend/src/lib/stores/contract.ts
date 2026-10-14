/* =============================================================================
 * File          : contract.ts
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Client/server contract-mismatch flag: api.ts sets it when the server
 *   answers 426 Upgrade Required (stale cached bundle after an update), and
 *   Desktop.svelte subscribes to show a reload prompt. Idempotent.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   Written by api.ts, subscribed by Desktop.svelte.
 * =============================================================================
 */

import { writable } from 'svelte/store';

/**
 * Set when the server answers `426 Upgrade Required` because the client's
 * compiled contract version no longer matches the server's (a stale cached
 * bundle after an update). `Desktop.svelte` subscribes and shows a reload
 * prompt; `api.ts` sets it from any API response.
 */
export const contractMismatch = writable(false);

/** Record a detected client/server contract mismatch. Idempotent. */
export function notifyContractMismatch() {
  contractMismatch.set(true);
}
