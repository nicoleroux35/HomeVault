<!-- ========================================================================
     File          : WidgetHeader.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Standard floating-widget title bar: circular accent icon tile + label,
       shared by every desktop monitor panel so they read as one family.

     Dependencies:
       - svelte (>= 5)

     Usage:
       Top of each desktop monitor widget.
     ======================================================================== -->
<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    /** Glyph for the icon tile (rendered in the widget's accent). */
    icon: Snippet;
    /** Panel title. */
    label: string;
    /** The widget's accent. The header derives a 12% tint for the icon pill
     *  and uses this value as the glyph colour. Defaults to the brand accent. */
    color?: string;
    /** Optional trailing status (v2 widget headers: "live", "2 / 3 up",
     *  "Protected", a location, a count…). Aligned right, never interactive. */
    meta?: Snippet;
  }

  let { icon, label, color = 'var(--hv-primary)', meta }: Props = $props();
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
  <span class="hv-widget-tile" style="--hv-widget-accent: {color};">
    {@render icon()}
  </span>
  <span class="hv-widget-title">{label}</span>
  {#if meta}
    <span class="hv-widget-meta">{@render meta()}</span>
  {/if}
</div>

<style>
  .hv-widget-header {
    display: flex;
    align-items: center;
    gap: var(--hv-space-3);
    height: var(--hv-widget-header-h);
    box-sizing: border-box;
    /* v2 card header: icon pill + title, no rule under it. The panel owns the
       16px inset, so the header spans it without a divider. */
    margin: calc(-1 * var(--hv-space-4)) calc(-1 * var(--hv-space-4)) var(--hv-space-3);
    padding: 0 var(--hv-space-4);
    background: transparent;
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

  /* v2 icon pill: a 30px squircle carrying a 12% tint of the widget's own
     accent, with the accent itself as the glyph colour. Never a solid tile —
     the colour is a marker, not a banner (DESIGN §2.1 rule 3). */
  .hv-widget-tile {
    display: grid;
    place-items: center;
    width: var(--hv-winhdr-icon-size);
    height: var(--hv-winhdr-icon-size);
    border-radius: 9px;
    background: color-mix(in srgb, var(--hv-widget-accent, var(--hv-primary)) 12%, white);
    color: var(--hv-widget-accent, var(--hv-primary));
    flex: none;
  }

  /* The glyph size is owned HERE, exactly like `.hv-winhdr-icon :global(svg)`
     in the in-window header: 16px in the 28px tile, i.e. the same glyph the
     app header shows. A snippet must therefore not pass `size` — a stylesheet
     beats the SVG width/height attribute, so the prop is dead noise (the 11
     monitor snippets carried `size={16}` until 2026-10-18 and had it removed).
     `stroke` is NOT clamped by any rule and stays explicit. */
  .hv-widget-tile :global(svg) {
    width: 16px;
    height: 16px;
  }

  .hv-widget-title {
    font-size: var(--hv-text-md);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-winhdr-title);
    line-height: 20px;
    min-width: 0;
    flex: 0 1 auto;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* v2 header trailing slot (live dot, counts, status badges, location). */
  .hv-widget-meta {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: var(--hv-space-1-5);
    flex: 0 1 auto;
    min-width: 0;
    max-width: 50%;
    font-size: var(--hv-text-xs);
    color: var(--hv-shell-text-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>