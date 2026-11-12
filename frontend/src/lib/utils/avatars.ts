/* =============================================================================
 * File          : avatars.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Shared avatar-tile palette for the Users and Groups screens: every stop
 *   is a background for white initials and therefore clears the 4.5:1
 *   contrast floor (the two retired stops that failed it are documented in
 *   the code; a test guards the rule). The pick is deterministic — the same
 *   name always lands on the same colour.
 *
 * Dependencies:
 *   - none
 *
 * Usage:
 *   `avatarColorFor(username)` in Users.svelte / Groups.svelte tiles.
 * =============================================================================
 */

/**
 * Shared avatar palette — the single source of truth for the Users and
 * Groups avatar tiles (both used to carry their own copy, which is how the
 * non-compliant stops survived until 2026-09-23).
 *
 * Every stop is a background for WHITE initials, so each must clear
 * WCAG 1.4.3 (≥4.5:1 against #ffffff):
 *
 *   #2E72C9 4.81 · #2560AC 6.16 · #167A4D 5.35
 *   #96601A 5.27 · #0E7F92 4.70 · #6A45D6 6.11
 *
 * The retired stops #4B8AD4 (3.57:1) and #8b5cf6 (4.23:1) failed that
 * floor. Never add a stop below 4.5:1 with white — the regression guard is
 * `tests/e2e/groups/groups.spec.ts` → 16.1.
 */
export const AVATAR_COLORS = [
  '#2E72C9',
  '#2560AC',
  '#167A4D',
  '#96601A',
  '#0E7F92',
  '#6A45D6',
] as const;

/** Deterministic palette pick: the same name yields the same colour in both apps. */
export function avatarColorFor(name: string): string {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return AVATAR_COLORS[h % AVATAR_COLORS.length];
}
