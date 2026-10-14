/* =============================================================================
 * File          : index.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Barrel export of the chart components: page composites (the charts/
 *   sibling of app/) built on LayerChart and themed with the --hv-* tokens
 *   so they render the appliance palette by default.
 *
 * Dependencies:
 *   - none (re-exports only)
 *
 * Usage:
 *   `import { DonutChart, ... } from '$lib/components/charts'`.
 * =============================================================================
 */

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
export { default as AreaChart } from './AreaChart.svelte';
export type { DonutSegment } from './DonutChart.svelte';
export type { BarsChartItem } from './BarsChart.svelte';
export type { TrendSeries } from './TrendLineChart.svelte';
export type { AreaSeries } from './AreaChart.svelte';