/**
 * HOMEVAULT DESIGN SYSTEM - Chart components
 *
 * Reusable charts built on LayerChart (Svelte 5, d3), themed with `--hv-*`
 * tokens so they render the appliance palette by default. These are page
 * composites (the `charts/` sibling of `app/`), not ui primitives — see
 * `docs/guides/dashboard.md` for usage guidance.
 */

export { default as DonutChart } from './DonutChart.svelte';
export { default as BarsChart } from './BarsChart.svelte';
export { default as TrendLineChart } from './TrendLineChart.svelte';
export type { DonutSegment } from './DonutChart.svelte';
export type { BarsChartItem } from './BarsChart.svelte';
export type { TrendSeries } from './TrendLineChart.svelte';