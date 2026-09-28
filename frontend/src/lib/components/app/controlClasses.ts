/**
 * HOMEVAULT DESIGN SYSTEM — shared control class constants (audit W5).
 *
 * The recurring hand-rolled button/input shapes, defined once instead of
 * being re-declared per route (Docker, Users, Groups, Shares, VirtualMachines
 * and Downloader each shipped their own copy with slightly different sizes
 * and colours).
 *
 * `<theme/Button>` remains the standard way to render a *text* action button —
 * it emits the documented `.hv-btn hv-btn-*` variants from `components.css`.
 * These constants cover what `Button` does not: compact icon buttons, tiled
 * icon buttons, the h-10/h-11 shell action buttons and the shell text input.
 *
 * Compose with `cn()` and append overrides; conflicting Tailwind classes are
 * resolved last-wins by tailwind-merge. Every compact icon button constant
 * already carries the shared `.hv-ic` 44px hit square (see
 * docs/guides/design-system.md §4.3 and §7.1).
 */
import { cn } from '$lib/utils.js';

/** 36px compact icon button — grids, table rows and inline actions. */
export const iconBtn =
  'grid place-items-center w-9 h-9 flex-none rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] cursor-pointer transition-colors hover:bg-[var(--hv-shell-border)] hv-ic';

/** Destructive hover tint (error bg / text / border) for icon + action buttons. */
export const dangerHover =
  'hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)] hover:border-[var(--hv-error)]/40';

/** `iconBtn` pre-tinted for destructive actions (trash / remove). */
export const iconBtnDanger = cn(iconBtn, dangerHover);

/** 44px tiled icon button — detail-pane back/close actions. */
export const tileBtn =
  'grid place-items-center w-11 h-11 flex-none border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)] cursor-pointer transition-colors hover:bg-[var(--hv-shell-border)] hv-ic';

/** h-10 labelled shell action button — toolbars (Stop / Start / Restart …). */
export const actionBtn =
  'inline-flex items-center justify-center gap-1.5 h-10 px-4 rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] text-c-base font-semibold cursor-pointer transition-colors hover:bg-[var(--hv-shell-border)]';

/** `actionBtn` shrunk to a 40px square for icon-only use (no label). */
export const actionIconBtn = cn(actionBtn, 'grid place-items-center w-10 h-10 p-0 hv-ic');

/** h-11 brand-blue primary action (filled). */
export const primaryBtn =
  'inline-flex items-center gap-2 h-11 px-4 border-none rounded-lg bg-[var(--hv-primary)] text-white text-c-base font-semibold cursor-pointer transition-colors hover:bg-[var(--hv-primary-hover)] disabled:opacity-55 disabled:cursor-default';

/** h-11 bordered secondary action (shell fill). */
export const ghostBtn =
  'inline-flex items-center gap-2 h-11 px-4 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] text-c-base font-semibold cursor-pointer transition-colors hover:bg-[var(--hv-shell-border)] disabled:opacity-55 disabled:cursor-default';

/** Standard shell text input (h-11); the caller owns label + aria wiring. */
export const textInput =
  'w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]';
