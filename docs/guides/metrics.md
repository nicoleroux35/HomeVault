# Metrics (historical performance)

**Purpose:** Time-series history for CPU / memory / load / network
throughput / max temperature, with 1 h / 24 h / 7 d charts in a dedicated
app. Inspiration: OMV's collectd + RRD perfstats. Live gauges already
existed (Dashboard widgets) — this adds the *history*.

## Backend

- `src/metrics.rs` — collector loop (one sample per minute into
  `metric_samples`, migrations `044_metrics_history.sql` and
  `056_metrics_nullable_series.sql`) + the pure bucketing and sample math.
  Reads `/proc/stat` (deltas between consecutive samples),
  `/proc/meminfo`, `/proc/loadavg`, `/proc/net/dev` (all non-loopback
  interfaces summed) and `/sys/class/thermal` (max zone) — **each file
  exactly once per tick**. Retention: 8 days, pruned on every tick
  (indexed PK range delete).
- `src/handlers/metrics.rs` — read-only endpoints.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /metrics/history?range=1h\|24h\|7d` | auth | bucketed averages: 60×1 min / 96×15 min / 84×2 h (+ the in-progress trailing bucket) |
| `GET /metrics/latest` | auth | most recent stored sample; `200` with `data: null` when nothing has been collected yet |

## Null semantics — a gap is not a zero

Every series column is **nullable** and `NULL` means *unknown*, which is a
different statement from `0.0` (*the host really was idle*). This is the
whole point of the table:

- A `/proc` read that fails stores `NULL` for **its own series only**; the
  other four keep their values. A broken sensor, a kernel built without
  `/proc/net/dev` and an idle disk no longer produce the same chart.
- Each series averages on its own inside a bucket: a bucket where CPU came
  back twice and the temperature once reports the mean of the two CPU
  readings, not a CPU figure halved by the missing thermal zone.
- Buckets without samples come back as all-`null` points, and the point
  count is stable (`window/bucket + 1`), so charts draw a gap instead of a
  lying flat line and the x-axis stays simple.
- `GET /metrics/latest` on a fresh install is `200` + `data: null`: "nothing
  measured yet" is a first-boot state, not a client error.

## Rates divide by the real elapsed time

The nominal cadence (`SAMPLE_INTERVAL_SECS`, 60 s) is **not** the divisor
for the throughput series. The collector keeps the previous instant
(`PrevSample { at, counters }`) and divides by the wall-clock distance
between the two reads, so a suspend, a stalled tick or a slow tick cannot
inflate a rate. The interval runs with `MissedTickBehavior::Skip`, not the
default `Burst`: after a suspend, `Burst` fires the missed ticks
back-to-back, they all land in the same minute bucket, and `INSERT OR
REPLACE` silently keeps only the last one.

The first sample of a boot has no previous instant, and a reboot makes the
counters go backwards: both cases store **no rate** (`NULL`), never a zero
and never a negative. CPU is a ratio of two counter deltas, so it needs no
elapsed time; without a delta (first sample, counter reset) it falls back to
the since-boot average, which is what the figure means at that point.

## Invariants

- **No write path.** Samples come only from the collector; the HTTP surface
  is read-only (`require_auth`, same sensitivity class as Dashboard).
- Nothing shown as a measurement is a constant or an interpolation: a
  missing reading is `null` on the wire and `—` on the screen.
- Gaps in coverage (server down, failed read) are visible as gaps, never
  interpolated and never zero-filled.

## Frontend

- `frontend/src/routes/Metrics.svelte` (app id `metrics`): five SVG line
  charts (CPU, memory, load, temperature, network), 1 h / 24 h / 7 d
  selector, no chart library (hand-rolled SVG, design-system tokens).
  - The empty state is decided on **every** series: a host whose only
    readable sensor is a thermal zone renders its temperature chart instead
    of "no data".
  - Every `load()` takes a ticket and only the newest one may write the
    screen, so a slow `24h` answer can no longer repaint a newer `1h` range.
  - A series with no value in the window shows `—` in the card header, not
    `0.0`.
- `frontend/src/lib/services/api.ts`: `getMetricsHistory(range)`,
  `getMetricsLatest`.
- i18n keys: `metrics.*` (4 locales at key parity).

## Tests

- Unit: range parsing/windows; bucketing (in-bucket averages, gaps as
  `null`, out-of-window samples, temp max, **per-series averaging**, an
  all-null bucket); `sample_from` (a failed read is `None` never `0.0`, a
  wholly failed tick stores only nulls, the rate divides by the real elapsed
  time, no previous instant / counter reset / frozen clock are `None`, CPU
  delta vs since-boot fallback); the `/proc` parsers on fixtures
  (`stat`, `meminfo`, `loadavg`, `net/dev`, thermal) and the readers on the
  real host (tolerate missing files) — `src/metrics.rs::tests`.
- Integration: `tests/metrics_integration.rs` (auth guards, range
  validation, seeded-sample bucketing round trip, empty DB, **a failed read
  comes back as a gap while a real `0.0` stays a zero**, a null series does
  not drag down its neighbours, `latest` keeps a null series null and
  answers `200` + `data: null` on a fresh install).
- E2E: `tests/e2e/metrics/metrics.spec.ts` (live server, read-only screen;
  §5 stubs the API to pin the two frontend invariants above).

## Related

- [Dashboard](./dashboard.md), [Widgets](./widgets.md), [Health](./health.md)
