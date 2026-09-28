<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - TrendLineChart
   * Time-series line chart built on LayerChart's LineChart (Svelte 5, d3).
   * Compact by default (no axis/legend) for sparkline-style usage; pass
   * `axis`/`legend` to opt back in. `x`/`y` are accessor keys on each data
   * item; multiple series are supported via `series` (each with its own
   * `color`, falling back to `--hv-*` tokens).
   */
  import { LineChart } from "layerchart";
  import { cn } from "$lib/utils.js";

  export type TrendSeries = {
    key: string;
    label?: string;
    color?: string;
  };

  let {
    data = [],
    x = 'label',
    y = 'value',
    series = [],
    height = 120,
    axis = false,
    legend = false,
    class: className = '',
  }: {
    data?: Record<string, unknown>[];
    x?: string;
    y?: string;
    series?: TrendSeries[];
    height?: number;
    axis?: boolean;
    legend?: boolean;
    class?: string;
  } = $props();

  const resolvedSeries = $derived(
    (series as TrendSeries[]).length
      ? (series as TrendSeries[]).map((s) => ({
          key: s.key,
          label: s.label ?? s.key,
          value: y,
          color: s.color ?? 'var(--hv-primary)',
        }))
      : [{ key: y, label: y, value: y, color: 'var(--hv-primary)' }]
  );
</script>

<div class={cn("w-full", className)} style="height:{height}px">
  {#if data.length}
    <LineChart
      {data}
      {x}
      {y}
      series={resolvedSeries}
      {axis}
      {legend}
      tooltipContext={true}
    />
  {/if}
</div>