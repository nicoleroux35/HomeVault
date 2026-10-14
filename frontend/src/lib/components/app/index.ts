/* =============================================================================
 * File          : index.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Barrel export of the page composites: the standardized page anatomy
 *   (layout, cards, table, forms, states) shared by every app screen so no
 *   screen hand-rolls its own skeleton. See docs/guides/design-system.md.
 *
 * Dependencies:
 *   - none (re-exports only)
 *
 * Usage:
 *   `import { PageLayout, DataTable, ... } from '$lib/components/app'`.
 * =============================================================================
 */

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