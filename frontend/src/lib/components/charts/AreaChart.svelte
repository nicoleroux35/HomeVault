<!-- ========================================================================
     File          : AreaChart.svelte
     Author        : Nicolas Le Roux
     Created       : 13/10/2026
     Last modified : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Compact SVG area chart for rolling live metrics (CPU/memory over the
       last N samples), supporting stacked composition (one band per app
       summing to the host total) with stable per-series colours.
       Hand-rolled and dependency-free so it renders inside any window.

     Dependencies:
       - svelte (>= 5)

     Usage:
       Dashboard live-usage card.
     ======================================================================== -->
<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - AreaChart
   *
   * A compact SVG area chart for rolling live metrics (CPU / memory over the
   * last N samples). Hand-rolled rather than built on LayerChart because the
   * Dashboard's live-usage card needs *stacked* composition (one band per
   * app, summing to the host total) and a stable per-series colour, which the
   * shared LineChart does not express. Kept dependency-free so it renders
   * identically in every engine and stays cheap to poll.
   *
   * `data` is one object per sample; `x` names the (string) category key and
   * each series reads its numeric value off its own `key`. When `stacked` is
   * true the series are drawn bottom-up and their values must sum to the
   * host total; otherwise each series is drawn against the same baseline.
   *
   * All geometry is computed from the data, so a single sample still renders
   * (as a flat line) instead of an empty box.
   */
  import { cn } from '$lib/utils.js';

  export interface AreaSeries {
    key: string;
    label?: string;
    color?: string;
  }

  let {
    data = [],
    x = 'label',
    series = [],
    height = 120,
    max = undefined,
    stacked = true,
    class: className = '',
    ariaLabel = '',
  }: {
    data?: Record<string, any>[];
    x?: string;
    series?: AreaSeries[];
    height?: number;
    /** Fixed y-axis maximum. When omitted, the data's own max is used (min 1). */
    max?: number;
    stacked?: boolean;
    class?: string;
    ariaLabel?: string;
  } = $props();

  const W = 100; // viewBox width — stretched to the container, so unitless
  const PAD_TOP = 4;

  const resolved = $derived(
    series.length
      ? series
      : [{ key: 'value', label: 'value', color: 'var(--hv-primary)' }]
  );

  /** The y-axis max: an explicit `max`, else the data max (stacked sums the
   *  series per sample), floored at 1 so an all-zero series still renders. */
  const yMax = $derived.by(() => {
    if (typeof max === 'number' && max > 0) return max;
    let m = 0;
    for (const row of data) {
      if (stacked) {
        let sum = 0;
        for (const s of resolved) sum += Number(row[s.key]) || 0;
        m = Math.max(m, sum);
      } else {
        for (const s of resolved) m = Math.max(m, Number(row[s.key]) || 0);
      }
    }
    return Math.max(m, 1);
  });

  /** One path per series. Stacked: each band's lower edge is the sum of the
   *  series below it. Non-stacked: every band sits on the baseline. */
  const bands = $derived.by(() => {
    const n = data.length;
    if (n === 0) return [];
    const stepX = n > 1 ? W / (n - 1) : 0;
    const yFor = (v: number) => PAD_TOP + (1 - Math.min(v, yMax) / yMax) * (100 - PAD_TOP);

    const out: { key: string; color: string; d: string }[] = [];
    const lower: number[] = new Array(n).fill(0);

    for (const s of resolved) {
      const upper = data.map((row, i) => (stacked ? lower[i] + (Number(row[s.key]) || 0) : Number(row[s.key]) || 0));
      const top = upper.map((v, i) => `${(i * stepX).toFixed(2)},${yFor(v).toFixed(2)}`);
      const bottom = lower
        .map((v, i) => `${(i * stepX).toFixed(2)},${yFor(v).toFixed(2)}`)
        .reverse();
      out.push({
        key: s.key,
        color: s.color ?? 'var(--hv-primary)',
        d: `M${top.join('L')}L${bottom.join('L')}Z`,
      });
      if (stacked) for (let i = 0; i < n; i++) lower[i] = upper[i];
    }
    return out;
  });

  /** The topmost edge of the stack, drawn as a bright stroke so the total is
   *  readable even when the bands are similar colours. */
  const topLine = $derived.by(() => {
    const n = data.length;
    if (n === 0) return '';
    const stepX = n > 1 ? W / (n - 1) : 0;
    const yFor = (v: number) => PAD_TOP + (1 - Math.min(v, yMax) / yMax) * (100 - PAD_TOP);
    const pts = data.map((row, i) => {
      const v = stacked
        ? resolved.reduce((acc, s) => acc + (Number(row[s.key]) || 0), 0)
        : Math.max(...resolved.map((s) => Number(row[s.key]) || 0));
      return `${(i * stepX).toFixed(2)},${yFor(v).toFixed(2)}`;
    });
    return `M${pts.join('L')}`;
  });
</script>

<div class={cn('w-full', className)} style="height:{height}px" data-testid="area-chart">
  {#if data.length}
    <svg
      viewBox="0 0 {W} 100"
      preserveAspectRatio="none"
      width="100%"
      height="100%"
      role="img"
      aria-label={ariaLabel || undefined}
      class="block"
    >
      {#each bands as band (band.key)}
        <path d={band.d} fill={band.color} fill-opacity="0.55" stroke={band.color} stroke-width="0.4" />
      {/each}
      <path d={topLine} fill="none" stroke="var(--hv-shell-text-strong)" stroke-width="0.6" stroke-opacity="0.5" vector-effect="non-scaling-stroke" />
    </svg>
  {:else}
    <div class="h-full grid place-items-center text-c-base text-[var(--hv-shell-text-3)]">—</div>
  {/if}
</div>
