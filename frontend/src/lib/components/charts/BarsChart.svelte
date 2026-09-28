<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - BarsChart
   * Categorical bar chart built on LayerChart's BarChart (Svelte 5, d3).
   * Compact by default (no axis/grid/legend) so it drops into a StatCard or a
   * `--hv-shell-fill` panel like the old hand-rolled SVG core bars; pass
   * `axis`/`grid`/`rule` to opt back in for a full chart.
   */
  import { BarChart } from "layerchart";
  import { cn } from "$lib/utils.js";

  export type BarsChartItem = {
    label: string;
    value: number;
    color?: string;
  };

  let {
    data = [],
    height = 120,
    color = 'var(--hv-primary)',
    axis = false,
    grid = false,
    rule = false,
    class: className = '',
  }: {
    data?: BarsChartItem[];
    height?: number;
    color?: string;
    axis?: boolean;
    grid?: boolean;
    rule?: boolean;
    class?: string;
  } = $props();

  const items = $derived(
    (data as BarsChartItem[]).map((d) => ({ ...d, color: d.color ?? color }))
  );

  const colorRange = $derived(items.map((d) => d.color));
</script>

<div class={cn("w-full", className)} style="height:{height}px">
  {#if items.length}
    <BarChart
      data={items}
      x="label"
      y="value"
      {axis}
      {grid}
      {rule}
      legend={false}
      bandPadding={0.35}
      cRange={colorRange}
      tooltipContext={true}
    />
  {/if}
</div>