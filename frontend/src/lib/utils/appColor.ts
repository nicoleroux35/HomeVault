/**
 * Deterministic per-app colour.
 *
 * The Dashboard's live-usage card draws one stacked band per running app and
 * a matching colour chip in its list. The colour must be **stable across
 * polls and reloads** — a colour that reshuffled every 5 s would make the
 * stacked bands unreadable and the chips meaningless — so it is derived from
 * the container name with a hash, never from a render index.
 *
 * This is the offline equivalent of extracting a colour from the app icon:
 * no Canvas, no network, identical on every engine and every load. The hue
 * comes from the hash; saturation and lightness are fixed so every chip
 * clears the 3:1 non-text contrast bar (WCAG 1.4.11) against the card
 * background and the bands stay mutually distinguishable.
 *
 * Kept as a pure function (no Svelte rune context) so it is unit-testable.
 */
export function appColor(name: string): string {
  let h = 5381;
  for (let i = 0; i < name.length; i++) {
    // djb2, kept in unsigned 32-bit range
    h = ((h << 5) + h + name.charCodeAt(i)) >>> 0;
  }
  return `hsl(${h % 360} 62% 52%)`;
}
