<!-- ========================================================================
     File          : WidgetHeader.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Standard floating-widget title bar: accent icon tile + label, shared
       by every desktop monitor panel so they read as one family.

     Dependencies:
       - svelte (>= 5)

     Usage:
       Top of each desktop monitor widget.
     ======================================================================== -->
<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    /** Glyph for the icon tile (rendered white inside the accent). */
    icon: Snippet;
    /** Panel title — the only text the bar carries. */
    label: string;
    /** Icon-tile background — gradient or flat colour. Defaults to the shared
     *  app accent gradient. */
    color?: string;
  }

  let { icon, label, color = 'var(--hv-app-accent-grad)' }: Props = $props();
</script>

<!--
  WidgetHeader — the standard header of every floating desktop widget panel
  (2026-09-11 widget-kit standardization). Same anatomy as the in-window app
  header (`WindowHeader`): a tinted icon tile + a 14px semibold title on a
  white bar. No close/action buttons (2026-09-15) — monitor visibility is
  managed from Settings > Widgets, so the bar is icon + title only. Owns the
  drag gesture (the panel's onpointerdown / [data-hv-drag] contract).
-->
<div class="hv-widget-header" data-hv-drag>
  <span class="hv-widget-tile" style="background: {color};">
    {@render icon()}
  </span>
  <span class="hv-widget-title">{label}</span>
</div>

<style>
  .hv-widget-header {
    display: flex;
    align-items: center;
    gap: var(--hv-space-3);
    height: var(--hv-widget-header-h);
    box-sizing: border-box;
    margin: calc(-1 * var(--hv-space-4)) calc(-1 * var(--hv-space-4)) var(--hv-space-4);
    padding: 0 var(--hv-space-3);
    background: var(--hv-winhdr-bg);
    border-bottom: 1px solid var(--hv-winhdr-border);
    cursor: grab;
    /* Let the drag gesture drive the pointer handlers instead of the browser
       trying to scroll/pan the page (needed for touch dragging). */
    touch-action: none;
    user-select: none;
    -webkit-user-select: none;
  }

  .hv-widget-header:active {
    cursor: grabbing;
  }

  /* The app-header icon tile: --hv-winhdr-icon-size (28px), glyph clamped to
     the 16px the app header uses, white on the accent background. */
  .hv-widget-tile {
    display: grid;
    place-items: center;
    width: var(--hv-winhdr-icon-size);
    height: var(--hv-winhdr-icon-size);
    border-radius: var(--hv-winhdr-icon-radius);
    color: var(--hv-on-primary);
    flex: none;
  }

  .hv-widget-tile :global(svg) {
    width: 16px;
    height: 16px;
  }

  .hv-widget-title {
    font-size: var(--hv-text-md);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-winhdr-title);
    line-height: 20px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>