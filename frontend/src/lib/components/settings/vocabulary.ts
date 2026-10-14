/* =============================================================================
 * File          : vocabulary.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The Settings screen's class vocabulary — one section label, one card,
 *   one row, one set of buttons — shared deliberately between the route
 *   (which builds the categories) and the SettingsCard shell so the two can
 *   never diverge.
 *
 * Dependencies:
 *   - none (Tailwind class strings)
 *
 * Usage:
 *   Imported by routes/Settings.svelte and components/settings/*.
 * =============================================================================
 */

/**
 * The Settings screen's class vocabulary — one section label, one card, one
 * row, one set of buttons.
 *
 * Shared deliberately between `routes/Settings.svelte` (which builds the
 * categories out of it) and `components/settings/SettingsCard.svelte` (the
 * shell the six embedded sub-cards — TLS, Antivirus, Anti-Ransomware,
 * Firewall, UPS, SNMP — are built on). Without one module the two would drift
 * into two slightly different cards on the same screen.
 *
 * Every string is either a Tailwind utility or a `--hv-settings-*` token
 * (see `styles/theme/tokens.css` → SETTINGS SCREEN). Nothing is hardcoded.
 * The `settings-*` class names carried alongside the utilities are the E2E
 * suite's contract — they are assertion hooks, not styles.
 */

/** Vertical spacer above a card: the section label text was removed
 *  (2026-09-14), but `settings-card-title` stays — it is the E2E hook for the
 *  label-sibling-of-card contract, and the styling classes keep 23.5's
 *  computed-style assertions (uppercase, micro-size) valid. The 24px top
 *  padding matches the panel's left/right inset (px-6) and IS the section
 *  rhythm: the empty label contributes exactly `pt-6`, the card below it
 *  carries no margin of its own (see `settingsCard`), so every section opens
 *  on the same 24px — a value on the 4-unit scale, not 24px + a stray margin. */
export const secLabel =
  'shrink-0 px-1 pt-6 text-c-xs font-bold uppercase tracking-[var(--hv-letter-spacing-wider)] leading-[var(--hv-line-height-normal)] text-[var(--hv-settings-label)]';

/** The same spacer for the first section of a category. */
export const secLabelFirst = secLabel;

/** The white hairline card. `shrink-0` is load-bearing: the panel is a
 *  column flexbox, and without it a tall card is squeezed and clips its own
 *  content instead of scrolling the panel (E2E 2.5).

 *  No top margin: the gap above a card belongs to the section label spacer
 *  (24px) or, for the peer sub-cards that carry no label, to the `mt-6` /
 *  `mt-3` their caller passes here — two rules owning one gap would land on
 *  an off-scale 28px and make the two cases disagree by 4px. */
export const settingsCard =
  'shrink-0 rounded-xl bg-[var(--hv-settings-tile)] border-[1.45px] border-[var(--hv-settings-border)] px-4';

/** One row: label on the left, control on the right, hairline between.

 *  Below 480px of panel the row stacks — label (and its hint) over a
 *  full-width control — because a 280px field plus its label stops fitting
 *  once the window gives the panel less than ~465px. The query is
 *  container-scoped (`@container` on `[data-testid="settings-content"]`), so
 *  it follows the width the *window* gives the panel, not the viewport: the
 *  Settings window is a resizable window inside the desktop, and a viewport
 *  breakpoint would fire for a large window the user simply dragged narrow.
 *  Deliberately carries **no** `settings-*` hook: `settingsRow` is used for
 *  control rows too (the `system` category's check-frequency `<select>`), and
 *  `settings.spec.ts` 10.1 counts `.settings-info-row` in that category
 *  expecting exactly 3 — the read-only server/platform/version rows. An
 *  earlier attempt put the hook here and 10.1 counted 4. Use `settingsInfoRow`
 *  below for the read-only rows that 10.1 and `security.spec.ts` 2.1 actually
 *  target. */
export const settingsRow =
  'flex flex-col items-start gap-2 @min-[480px]:flex-row @min-[480px]:items-center @min-[480px]:justify-between @min-[480px]:gap-4 py-3 border-b-[1.45px] border-[var(--hv-settings-hairline)] last:border-b-0';

/** A row that only *shows* a value — label left, value right, no control. Its
 *  `settings-info-row` class is the E2E contract: `security.spec.ts` 2.1 asserts
 *  the Antivirus card's info rows through it, `settings.spec.ts` 10.1 counts the
 *  three `system` ones. Both are read-only rows, which is why the hook lives on
 *  this const and not on `settingsRow`. The sub-cards that migrated onto the
 *  vocabulary lost it (it was a scoped class of their own) and 2.1 went red —
 *  this const is what restores it. */
export const settingsInfoRow = `settings-info-row ${settingsRow}`;

/** A row whose control needs the full width under its label. */
export const settingsRowStacked =
  'flex flex-col gap-2 py-3 border-b-[1.45px] border-[var(--hv-settings-hairline)] last:border-b-0';

/** The row immediately above a card footer. Carries no bottom hairline of
 *  its own — the footer's top border is the separator, and two adjacent
 *  1.45px lines would read as one 3px rule. Same stacking rule as
 *  `settingsRow`: side-by-side while the panel is wide, label over control
 *  once it is not. */
export const settingsRowLast =
  'flex flex-col items-start gap-2 @min-[480px]:flex-row @min-[480px]:items-center @min-[480px]:justify-between @min-[480px]:gap-4 py-3';

/** Row copy. */
export const settingsRowLabel = 'text-sm font-medium leading-5 text-[var(--hv-settings-title)]';
export const settingsRowHint = 'settings-hint mt-0.5 text-xs leading-4 text-[var(--hv-settings-label)]';

/** Standalone label used where a row shows a value rather than a control. */
export const settingsLabel = 'text-sm font-medium leading-5 text-[var(--hv-settings-title)]';
export const settingsHint = 'settings-hint text-xs leading-4 text-[var(--hv-settings-label)]';

/* Every interactive control is exactly `--hv-touch-target-min` (44px) tall —
   the WCAG 2.5.5 / Apple / Material touch floor — so the screen works the
   same with a mouse or a finger. Height lives in the token, never a literal. */
const controlHeight = 'h-[var(--hv-touch-target-min)]';

export const settingsInput =
  `settings-input w-full ${controlHeight} px-3 border-[1.45px] border-[var(--hv-settings-border)] rounded-lg bg-[var(--hv-settings-tile)] text-sm text-[var(--hv-settings-row)] outline-none focus:border-[var(--hv-primary)]`;

/** The control geometry, without a width — the two selects below differ only
 *  in how they take their horizontal space, so they share one declaration. */
const selectBase =
  `settings-select ${controlHeight} px-3 border-[1.45px] border-[var(--hv-settings-border)] rounded-lg bg-[var(--hv-settings-tile)] text-sm text-[var(--hv-settings-row)] outline-none focus:border-[var(--hv-primary)]`;

export const settingsSelect = `w-full ${selectBase}`;

/** A select that hugs its own text instead of filling the row — the mock's
 *  shape for a control holding a short value ("5 s"). `w-auto` rather than a
 *  second width utility on top of `w-full`: two width classes in one string
 *  would be resolved by stylesheet order, not by intent. */
export const settingsSelectCompact = `w-auto ${selectBase}`;
export const settingsTextarea =
  'w-full min-h-[120px] p-3 border-[1.45px] border-[var(--hv-settings-border)] rounded-lg bg-[var(--hv-settings-tile)] text-sm font-[family-name:var(--hv-font-mono)] outline-none focus:border-[var(--hv-primary)]';

/** Footer strip under a card's rows (test-email, check-for-updates). */
export const settingsCardFooter =
  'flex items-center gap-2 py-3 border-t-[1.45px] border-[var(--hv-settings-hairline)]';

/** Buttons — appliance geometry, one step down from the old 44px rows. */
export const settingsBtn =
  `settings-btn inline-flex items-center justify-center gap-2 ${controlHeight} px-4 rounded-lg text-sm font-medium cursor-pointer transition-colors whitespace-nowrap disabled:opacity-50 disabled:cursor-default`;
export const settingsBtnGhost =
  'settings-btn-ghost border-[1.45px] border-[var(--hv-settings-border)] bg-[var(--hv-settings-tile)] text-[var(--hv-settings-title)] hover:bg-[var(--hv-settings-hairline)]';
export const settingsBtnPrimary =
  'settings-btn-primary border-none bg-[var(--hv-primary)] text-[var(--hv-on-primary)] hover:bg-[var(--hv-primary-hover)]';
export const settingsBtnDanger =
  'settings-btn-danger border-none bg-[var(--hv-error)] text-[var(--hv-on-primary)] hover:bg-[var(--hv-error-hover)]';
/** The green confirmation skin of the rail-footer Save button (2026-09-23).
 *  It mirrors the two skins above on purpose — one skin per button state, and
 *  never two `bg-` utilities on the same element (the stylesheet order, not
 *  the class order, would pick the winner). White on `--hv-success` (#1E7E34)
 *  is 5.14:1 ✓ for WCAG 1.4.3; the hover step goes darker, never lighter. */
export const settingsBtnSuccess =
  'settings-btn-success border-none bg-[var(--hv-success)] text-[var(--hv-on-primary)] hover:bg-[var(--hv-success-hover)]';

/** The 36px icon tile of a sub-card header: category colour at 9% behind a
 *  20px glyph in the full colour. Pass the colour as a custom property. */
export const settingsCardIcon = 'grid place-items-center w-9 h-9 rounded-xl flex-none';

/* The category accents, for the sub-cards. A sub-card is rendered from inside
   a category branch of Settings.svelte, so it cannot read `activeCat.accent`
   itself; this constant keeps the "a switch fills with its *category*'s colour
   when on" rule true for the sub-cards too, instead of falling back to the
   brand blue. Only `security` has switches in its sub-cards today (TLS,
   Antivirus, Anti-Ransomware, Firewall) — UPS and SNMP have none. Keep it in
   step with the `--hv-settings-cat-security` token. */
export const SECURITY_ACCENT = 'var(--hv-settings-cat-security)';
