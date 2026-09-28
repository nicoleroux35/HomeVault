/**
 * HOMEVAULT DESIGN SYSTEM — page composites (`app/` layer)
 *
 * The standardized page anatomy for every app screen (2026-09-12). These
 * compose the primitives (`theme/`) + framework (`ui/`) + desktop chrome into
 * the shared page skeleton, so no screen hand-rolls its own cards, tables,
 * metrics or forms. See docs/guides/design-system.md §4.
 */

export { default as PageLayout } from './PageLayout.svelte';
export { default as SectionCard } from './SectionCard.svelte';
export { default as StatCard } from './StatCard.svelte';
export { default as DataTable } from './DataTable.svelte';
export { default as EmptyState } from './EmptyState.svelte';
export { default as FormRow } from './FormRow.svelte';
export { default as ConfirmDialog } from './ConfirmDialog.svelte';

// Shared control class constants (audit W5) — see ./controlClasses.ts and
// docs/guides/design-system.md §4.3.
export {
  iconBtn,
  iconBtnDanger,
  tileBtn,
  actionBtn,
  actionIconBtn,
  dangerHover,
  primaryBtn,
  ghostBtn,
  textInput,
} from './controlClasses';