<!-- ========================================================================
     File          : DonutChart.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Reusable donut/gauge on LayerChart's ArcChart (Svelte 5 / d3): themed
       with the --hv-* tokens by default, per-segment colour override, two
       modes (single gauge or several labelled segments).

     Dependencies:
       - svelte (>= 5), layerchart

     Usage:
       `import { DonutChart } from '$lib/components/charts'`.
     ======================================================================== -->
<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - DonutChart
   * Reusable donut/gauge built on LayerChart's ArcChart (Svelte 5, d3).
   * Colors come from `--hv-*` tokens by default (brand #1E62AE, semantic
   * success/warning/error); pass a `color` per segment to override.
   *
   * Two modes:
   *  - multiple segments: pass several `{ label, value, color? }` items
   *  - single gauge:      pass one item + `maxValue` (the visible arc shows
   *                       value/maxValue, the rest is the track ring)
   */
  import { ArcChart } from "layerchart";
  import { cn } from "$lib/utils.js";

  export type DonutSegment = {
    label: string;
    value: number;
    color?: string;
  };

  let {
    data = [],
    maxValue,
    size = 180,
    thickness = 22,
    padAngle = 0.03,
    caption = '',
    valueLabel = '',
    valueClass = '',
    class: className = '',
  }: {
    data?: DonutSegment[];
    maxValue?: number;
    size?: number;
    thickness?: number;
    padAngle?: number;
    caption?: string;
    valueLabel?: string;
    valueClass?: string;
    class?: string;
  } = $props();

  const segments = $derived(
    (data as DonutSegment[]).map((d) => ({ ...d, color: d.color ?? 'var(--hv-primary)' }))
  );

  const innerRadius = $derived(Math.max(0, size / 2 - thickness));
  const outerRadius = $derived(size / 2);

  const total = $derived(segments.reduce((acc, d) => acc + (d.value || 0), 0));
</script>

<div class={cn("relative mx-auto", className)} style="width:{size}px;height:{size}px">
  <div class="h-full w-full">
    <ArcChart
      data={segments}
      key="label"
      label="label"
      value="value"
      {maxValue}
      {innerRadius}
      {outerRadius}
      trackInnerRadius={innerRadius}
      trackOuterRadius={outerRadius}
      {padAngle}
      tooltipContext={false}
    />
  </div>
  {#if caption || valueLabel}
    <div class="pointer-events-none absolute inset-0 flex flex-col items-center justify-center text-center">
      {#if caption}
        <div class="text-sm text-[var(--hv-shell-text-2)] leading-snug">{caption}</div>
      {/if}
      {#if valueLabel}
        <div
          class={cn(
            "mt-1 font-[family-name:var(--hv-font-mono)] text-3xl font-bold leading-none text-[var(--hv-shell-text-strong)]",
            valueClass
          )}
        >
          {valueLabel}
        </div>
      {/if}
    </div>
  {/if}
</div>