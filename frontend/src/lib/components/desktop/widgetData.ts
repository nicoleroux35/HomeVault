/* =============================================================================
 * File          : widgetData.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Shared data-loading machinery for desktop widgets: a discriminated
 *   result type covering every load phase, plus a small polling helper, so
 *   widgets never reinvent the load/start/stop logic.
 *
 * Dependencies:
 *   - $lib/services/api
 *
 * Usage:
 *   Imported by every desktop monitor widget.
 * =============================================================================
 */

/**
 * Shared data-loading helpers for desktop widgets.
 * Provides a discriminated result type and a small polling helper so that
 * widgets don't reinvent the same load / start / stop machinery.
 */

import { api } from '$lib/services/api';

/** Discriminated union that captures every phase a widget's data can be in. */
export type WidgetLoadResult<T> =
  | { status: 'idle' }
  | { status: 'loading' }
  | { status: 'error'; error: string }
  | { status: 'ready'; data: T };

/** CPU widget data for Desktop.svelte */
export interface CpuData {
  usage: number;
  loadavg_1m: number;
  loadavg_5m: number;
  loadavg_15m: number;
  cores: number;
}

/** RAM widget data for Desktop.svelte */
export interface RamData {
  usage_percent: number;
  total_bytes: number;
  used_bytes: number;
  free_bytes: number;
  available_bytes: number;
}

/** CPU Temperature widget data for Desktop.svelte */
export interface TempData {
  temp: number;
  label: string;
}

/** Network traffic widget data for Desktop.svelte */
export interface NetworkData {
  rx_mbps: number;
  tx_mbps: number;
  rx_bytes_per_sec: number;
  tx_bytes_per_sec: number;
}



/* Translate raw API types to frontend widget types */
function translateCpu(raw: { usage_percent: number; loadavg_1m: number; loadavg_5m: number; loadavg_15m: number; cores: number }): CpuData {
  return { usage: raw.usage_percent, loadavg_1m: raw.loadavg_1m, loadavg_5m: raw.loadavg_5m, loadavg_15m: raw.loadavg_15m, cores: raw.cores };
}

function translateRam(raw: { usage_percent: number; total_bytes: number; used_bytes: number; free_bytes: number; available_bytes: number }): RamData {
  return { usage_percent: raw.usage_percent, total_bytes: raw.total_bytes, used_bytes: raw.used_bytes, free_bytes: raw.free_bytes, available_bytes: raw.available_bytes };
}

function translateTemp(raw: { temp_celsius: number; label: string }): TempData {
  return { temp: raw.temp_celsius, label: raw.label };
}

function translateNetwork(raw: { rx_bytes_per_sec: number; tx_bytes_per_sec: number; rx_mbps: number; tx_mbps: number }): NetworkData {
  return { rx_mbps: raw.rx_mbps, tx_mbps: raw.tx_mbps, rx_bytes_per_sec: raw.rx_bytes_per_sec, tx_bytes_per_sec: raw.tx_bytes_per_sec };
}

/**
 * Single attempt to load the latest data for a widget. Maps the API
 * response (success flag + nullable data) into the discriminated union above.
 */
export async function loadWidgetData<T>(widgetId: string): Promise<WidgetLoadResult<T>> {
  try {
    switch (widgetId) {
      case 'cpu-usage': {
        const res = await api.getWidgetCpu();
        if (!res.success || !res.data) return { status: 'error', error: 'Failed to load' };
        return { status: 'ready', data: translateCpu(res.data) as T };
      }
      case 'ram-usage': {
        const res = await api.getWidgetRam();
        if (!res.success || !res.data) return { status: 'error', error: 'Failed to load' };
        return { status: 'ready', data: translateRam(res.data) as T };
      }
      case 'network-traffic': {
        const res = await api.getWidgetNetworkTraffic();
        if (!res.success || !res.data) return { status: 'error', error: 'Failed to load' };
        return { status: 'ready', data: translateNetwork(res.data) as T };
      }
      case 'cpu-temp': {
        const res = await api.getWidgetCpuTemp();
        if (!res.success || !res.data) return { status: 'error', error: 'Failed to load' };
        return { status: 'ready', data: translateTemp(res.data) as T };
      }
      /* app-update and server-update share the same endpoint. */
      case 'app-update':
      case 'server-update': {
        const res = await api.getWidgetUpdateInfo();
        if (!res.success || !res.data) return { status: 'error', error: 'Failed to load' };
        return { status: 'ready', data: res.data as T };
      }

      default:
        return { status: 'error', error: `Unknown widget: ${widgetId}` };
    }
  } catch (e) {
    return { status: 'error', error: e instanceof Error ? e.message : 'Failed to load' };
  }
}

export interface WidgetPoller<T> {
  load: () => Promise<void>;
  start: () => void;
  stop: () => void;
}

/**
 * Creates a polling controller for a widget.
 *
 * The caller is expected to invoke `start()` inside `onMount` and `stop()`
 * inside `onDestroy`. The poller feeds the latest WidgetLoadResult into the
 * provided `set` callback (typically a Svelte store setter or a plain
 * variable assignment) after every attempt.
 */
export function createWidgetPolling<T>(
  widgetId: string,
  intervalMs: number,
  set: (result: WidgetLoadResult<T>) => void
): WidgetPoller<T> {
  let timer: ReturnType<typeof setInterval> | undefined;

  /* Fires one load cycle: signals 'loading', awaits the data, then pushes the result. */
  const load = async () => {
    set({ status: 'loading' });
    const result = await loadWidgetData<T>(widgetId);
    set(result);
  };

  /* Begins polling — does an immediate load, then repeats on the given interval.
     Guarded so calling start() multiple times doesn't stack timers. */
  const start = () => {
    if (timer) return;
    load();
    timer = setInterval(load, intervalMs);
  };

  /* Stops the interval timer. Safe to call even if already stopped. */
  const stop = () => {
    if (timer) {
      clearInterval(timer);
      timer = undefined;
    }
  };

  return { load, start, stop };
}

/**
 * Reference-counted shared poller registry, keyed by `widgetId:intervalMs`.
 *
 * The desktop shell (`Desktop.svelte`) and every individually-placed
 * `Widget.svelte`/`StandardWidget.svelte` instance previously each ran their
 * own independent `setInterval` against the same underlying endpoints
 * (cpu-usage, ram-usage, ...) — placing N widgets on the desktop meant N+1
 * redundant HTTP requests every tick for identical data. `subscribeWidget`
 * makes every caller share a single interval/fetch per (widgetId, interval)
 * pair: the first subscriber starts it, later subscribers just get added to
 * the fan-out list, and the interval is torn down once the last subscriber
 * unsubscribes.
 */
interface SharedPollerEntry {
  timer: ReturnType<typeof setInterval>;
  subscribers: Set<(result: WidgetLoadResult<unknown>) => void>;
}

const sharedPollers = new Map<string, SharedPollerEntry>();

/**
 * Subscribes to a shared poller for `widgetId` at `intervalMs`. Returns an
 * unsubscribe function — call it from `onDestroy` (or equivalent cleanup).
 * Safe to call from any number of components at once; the underlying
 * `setInterval`/fetch is deduplicated across all of them.
 */
export function subscribeWidget<T>(
  widgetId: string,
  intervalMs: number,
  onUpdate: (result: WidgetLoadResult<T>) => void
): () => void {
  const key = `${widgetId}:${intervalMs}`;
  let entry = sharedPollers.get(key);

  if (!entry) {
    const subscribers = new Set<(result: WidgetLoadResult<unknown>) => void>();
    const tick = async () => {
      const result = await loadWidgetData<unknown>(widgetId);
      for (const cb of subscribers) cb(result);
    };
    const timer = setInterval(tick, intervalMs);
    entry = { timer, subscribers };
    sharedPollers.set(key, entry);
    void tick();
  }

  entry.subscribers.add(onUpdate as (result: WidgetLoadResult<unknown>) => void);

  return () => {
    const current = sharedPollers.get(key);
    if (!current) return;
    current.subscribers.delete(onUpdate as (result: WidgetLoadResult<unknown>) => void);
    if (current.subscribers.size === 0) {
      clearInterval(current.timer);
      sharedPollers.delete(key);
    }
  };
}