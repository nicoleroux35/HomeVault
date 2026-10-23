<!-- ========================================================================
     File          : Metrics.svelte
     Author        : Nicolas Le Roux
     Created       : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Metrics app (app id `metrics`): historical performance charts
     (CPU, memory, network throughput, temperature, load) over 1 h / 24 h /
     7 d ranges, rendered as hand-rolled SVG (no chart library). Data from
     GET /api/metrics/history; gaps render as gaps (never flat zero).

     Dependencies:
       - svelte (>= 5), theme + app/ composites, $lib/services/api

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { api, type MetricsPoint } from '$lib/services/api';
  import { onAppVisible } from '$lib/utils/autoRevalidate';
  import { t, languageStore } from '$lib/stores/i18n';
  import { PageLayout, EmptyState, SectionCard } from '$lib/components/app';
  import { IconChartLine } from '@tabler/icons-svelte';

  type Range = '1h' | '24h' | '7d';
  const RANGES: Range[] = ['1h', '24h', '7d'];

  let range: Range = '1h';
  let points: MetricsPoint[] = [];
  let loading = true;
  let loadError = false;
  /** Bumped by every `load()`; only the newest ticket may write the screen. */
  let loadTicket = 0;

  onMount(() => {
    load();
    // Charts are aggregate data — refresh when the window regains
    // attention (same revalidate pattern as every other app) and keep a
    // slow 60 s ticker so the 1 h view visibly rolls.
    const timer = setInterval(() => {
      if (!document.hidden) load();
    }, 60_000);
    const stopVisible = onAppVisible(load);
    return () => {
      clearInterval(timer);
      stopVisible?.();
    };
  });

  async function load() {
    // Only the newest request may write the screen. A slow `24h` answer used
    // to land on top of a newer `1h` one and repaint the charts with the wrong
    // range (ROADMAP 11). Every call takes a ticket; a stale answer is dropped.
    const ticket = ++loadTicket;
    try {
      const res = await api.getMetricsHistory(range);
      if (ticket !== loadTicket) return;
      if (!res.success) {
        loadError = true;
        return;
      }
      points = res.data?.points ?? [];
      loadError = false;
    } catch {
      if (ticket !== loadTicket) return;
      loadError = true;
    } finally {
      if (ticket === loadTicket) loading = false;
    }
  }

  function selectRange(r: Range) {
    range = r;
    loading = true;
    load();
  }

  function formatTime(ts: number): string {
    const d = new Date(ts * 1000);
    try {
      if (range === '1h') return new Intl.DateTimeFormat(get(languageStore), { timeStyle: 'short' }).format(d);
      return new Intl.DateTimeFormat(get(languageStore), { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }).format(d);
    } catch {
      return new Date(ts * 1000).toISOString().slice(11, 16);
    }
  }

  function formatBytesRate(v: number | null): string {
    if (v == null) return '—';
    const units = ['B/s', 'KB/s', 'MB/s', 'GB/s'];
    let value = v;
    let i = 0;
    while (value >= 1024 && i < units.length - 1) { value /= 1024; i++; }
    return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[i]}`;
  }

  /** A missing reading renders as `—`: it is unknown, never a 0. */
  function formatPercent(v: number | null): string {
    return v == null ? '—' : `${v.toFixed(1)} %`;
  }

  function formatNumber(v: number | null, digits = 2): string {
    return v == null ? '—' : v.toFixed(digits);
  }

  /** True when at least one series carries a value in this point. */
  function hasReading(p: MetricsPoint): boolean {
    return (
      p.cpu != null ||
      p.mem != null ||
      p.load1 != null ||
      p.temp != null ||
      p.net_rx != null ||
      p.net_tx != null
    );
  }

  /** SVG polyline points for one nullable series over `pts`. */
  function series(pts: MetricsPoint[], key: 'cpu' | 'mem' | 'load1' | 'temp' | 'net_rx' | 'net_tx', max: number): string {
    const segs: string[] = [];
    let current: string[] = [];
    for (let i = 0; i < pts.length; i++) {
      const v = pts[i][key];
      if (v == null) {
        if (current.length > 1) segs.push(current.join(' '));
        current = [];
        continue;
      }
      const x = (i / Math.max(1, pts.length - 1)) * 100;
      const y = 100 - Math.min(100, (v / max) * 100);
      current.push(`${x.toFixed(2)},${y.toFixed(2)}`);
    }
    if (current.length > 1) segs.push(current.join(' '));
    return segs.join(' ');
  }

  /** Latest non-null value of a series, for the header figure. */
  function latest(key: 'cpu' | 'mem' | 'load1' | 'temp' | 'net_rx' | 'net_tx'): number | null {
    for (let i = points.length - 1; i >= 0; i--) {
      const v = points[i][key];
      if (v != null) return v;
    }
    return null;
  }

  interface ChartDef {
    id: 'cpu' | 'mem' | 'load1' | 'temp' | 'net';
    title: string;
    max: number;
    /** Formatted current value + unit. */
    current(): string;
  }

  const charts: ChartDef[] = [
    { id: 'cpu', title: 'metrics.chart_cpu', max: 100, current: () => formatPercent(latest('cpu')) },
    { id: 'mem', title: 'metrics.chart_mem', max: 100, current: () => formatPercent(latest('mem')) },
    { id: 'load1', title: 'metrics.chart_load', max: 4, current: () => formatNumber(latest('load1')) },
    { id: 'temp', title: 'metrics.chart_temp', max: 100, current: () => (latest('temp') == null ? '—' : `${latest('temp')!.toFixed(0)} °C`) },
    { id: 'net', title: 'metrics.chart_net', max: 0, current: () => `↓ ${formatBytesRate(latest('net_rx'))} · ↑ ${formatBytesRate(latest('net_tx'))}` },
  ];
</script>

<PageLayout
  title={$t('metrics.heading')}
  iconName="chart-line"
  data-testid="metrics-page"
>
  {#snippet icon()}
    <IconChartLine stroke={1.7} />
  {/snippet}

  {#snippet toolbarLeft()}
    <div
      data-testid="metrics-ranges"
      class="flex flex-wrap gap-1 p-1.5 rounded-2xl bg-[var(--hv-shell-fill)] border border-[var(--hv-shell-border)]"
      role="group"
      aria-label={$t('metrics.range_label')}
    >
      {#each RANGES as r (r)}
        <button
          type="button"
          data-testid={`metrics-range-${r}`}
          aria-pressed={range === r}
          onclick={() => selectRange(r)}
          class={range === r
            ? 'inline-flex items-center h-9 px-4 border-none rounded-lg bg-[var(--hv-primary-selected-bg)] text-[var(--hv-primary)] text-c-base font-semibold cursor-pointer'
            : 'inline-flex items-center h-9 px-4 border-none rounded-lg bg-transparent text-[var(--hv-shell-text-2)] text-c-base font-medium cursor-pointer hover:bg-[var(--hv-shell-border-soft)]'}
        >
          {$t(`metrics.range_${r}`)}
        </button>
      {/each}
    </div>
  {/snippet}

  <div class="flex flex-col gap-4 max-w-[1100px]">
    {#if loading && points.length === 0}
      <EmptyState data-testid="metrics-loading" icon="chart-line" title={$t('metrics.loading')} />
    {:else if loadError && points.length === 0}
      <EmptyState data-testid="metrics-error" icon="alert-triangle" title={$t('metrics.error')} />
    {:else if !points.some(hasReading)}
      <EmptyState
        data-testid="metrics-empty"
        icon="chart-line"
        title={$t('metrics.empty_title')}
        description={$t('metrics.empty_desc')}
      />
    {:else}
      {#each charts as chart (chart.id)}
        <SectionCard
          title={$t(chart.title)}
          description={chart.id === 'net' ? '' : formatTime(points[0]?.ts ?? 0) + ' → ' + formatTime(points[points.length - 1]?.ts ?? 0)}
          data-testid={`metrics-card-${chart.id}`}
        >
          {#snippet actions()}
            <span class="text-c-base font-semibold text-[var(--hv-primary)]" data-testid={`metrics-current-${chart.id}`}>
              {chart.current()}
            </span>
          {/snippet}
          {#if chart.id === 'net'}
            <svg
              viewBox="0 0 100 100"
              preserveAspectRatio="none"
              class="w-full h-32"
              role="img"
              aria-label={$t('metrics.chart_net')}
              data-testid="metrics-chart-net"
            >
              <line x1="0" y1="50" x2="100" y2="50" class="metrics-grid" />
              <polyline
                points={series(points, 'net_rx', Math.max(1024, Math.max(...points.map((p) => p.net_rx ?? 0))))}
                class="metrics-line-rx"
              />
              <polyline
                points={series(points, 'net_tx', Math.max(1024, Math.max(...points.map((p) => p.net_tx ?? 0))))}
                class="metrics-line-tx"
              />
            </svg>
            <div class="flex gap-4 mt-2 text-c-label text-[var(--hv-shell-text-2)]">
              <span class="flex items-center gap-1"><span class="metrics-dot metrics-dot-rx"></span>{$t('metrics.net_rx')}</span>
              <span class="flex items-center gap-1"><span class="metrics-dot metrics-dot-tx"></span>{$t('metrics.net_tx')}</span>
            </div>
          {:else}
            <svg
              viewBox="0 0 100 100"
              preserveAspectRatio="none"
              class="w-full h-32"
              role="img"
              aria-label={$t(chart.title)}
              data-testid={`metrics-chart-${chart.id}`}
            >
              <line x1="0" y1="25" x2="100" y2="25" class="metrics-grid" />
              <line x1="0" y1="50" x2="100" y2="50" class="metrics-grid" />
              <line x1="0" y1="75" x2="100" y2="75" class="metrics-grid" />
              <polyline points={series(points, chart.id === 'temp' ? 'temp' : chart.id, chart.max)} class="metrics-line" />
            </svg>
          {/if}
        </SectionCard>
      {/each}
    {/if}
  </div>
</PageLayout>

<style>
  .metrics-grid {
    stroke: var(--hv-shell-border);
    stroke-width: 0.4;
    vector-effect: non-scaling-stroke;
  }
  .metrics-line {
    fill: none;
    stroke: var(--hv-primary);
    stroke-width: 1.6;
    stroke-linejoin: round;
    stroke-linecap: round;
    vector-effect: non-scaling-stroke;
  }
  .metrics-line-rx {
    fill: none;
    stroke: var(--hv-primary);
    stroke-width: 1.6;
    stroke-linejoin: round;
    vector-effect: non-scaling-stroke;
  }
  .metrics-line-tx {
    fill: none;
    stroke: var(--hv-success, #16a34a);
    stroke-width: 1.6;
    stroke-linejoin: round;
    vector-effect: non-scaling-stroke;
  }
  .metrics-dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }
  .metrics-dot-rx { background: var(--hv-primary); }
  .metrics-dot-tx { background: var(--hv-success, #16a34a); }
</style>
