# Metrics (historical performance)

**Purpose:** Time-series history for CPU / memory / load / network
throughput / max temperature, with 1 h / 24 h / 7 d charts in a dedicated
app. Inspiration: OMV's collectd + RRD perfstats. Live gauges already
existed (Dashboard widgets) — this adds the *history*.

## Backend

- `src/metrics.rs` — collector loop (one sample per minute into
  `metric_samples`, migration `044_metrics_history.sql`) + the pure
  bucketing function. Reads `/proc/stat` (deltas between consecutive
  samples), `/proc/meminfo`, `/proc/loadavg`, `/proc/net/dev` (all
  non-loopback interfaces summed) and `/sys/class/thermal` (max zone).
  Retention: 8 days, pruned on every tick (indexed PK range delete).
- `src/handlers/metrics.rs` — read-only endpoints.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /metrics/history?range=1h\|24h\|7d` | auth | bucketed averages: 60×1 min / 96×15 min / 84×2 h (+ the in-progress trailing bucket) |
| `GET /metrics/latest` | auth | most recent stored sample |

## Invariants

- **No write path.** Samples come only from the collector; the HTTP surface
  is read-only (`require_auth`, same sensitivity class as Dashboard).
- Empty buckets come back as all-`null` points so charts draw a gap instead
  of a lying flat line; point count is stable (`window/bucket + 1`).
- The first sample after a boot has no delta to compute against: CPU falls
  back to the since-boot average and network stores zeros once (counter
  reset) rather than a huge spike.
- Gaps in coverage (server down) are visible as gaps, never interpolated.

## Frontend

- `frontend/src/routes/Metrics.svelte` (app id `metrics`): four SVG line
  charts (CPU, memory, network, temperature) + load, 1 h / 24 h / 7 d
  selector, no chart library (hand-rolled SVG, design-system tokens).
- `frontend/src/lib/services/api.ts`: `getMetricsHistory(range)`,
  `getMetricsLatest`.
- i18n keys: `metrics.*` (4 locales at key parity).

## Tests

- Unit: range parsing/windows, bucketing (averages, gaps, out-of-window
  samples, temp max), /proc parsers on the real host (tolerate missing
  files) — `src/metrics.rs::tests`.
- Integration: `tests/metrics_integration.rs` (auth guards, range
  validation, seeded-sample bucketing round trip, empty DB).
- E2E: `tests/e2e/metrics/metrics.spec.ts` (live server, read-only screen).

## Related

- [Dashboard](./dashboard.md), [Widgets](./widgets.md), [Health](./health.md)
