/* =============================================================================
 * File          : metrics.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Historical performance metrics: a background collector samples CPU,
 *   memory, load, network throughput and max temperature once a minute into
 *   the metric_samples table (8-day retention, pruned in the same loop),
 *   and exposes the bucketed averages the Metrics app charts.
 *
 *   Every series is nullable and every host read is independently fallible:
 *   a failed /proc read is stored as NULL — a gap on the chart — and never as
 *   a plausible-looking 0.0 (ROADMAP 11.1). Throughput rates are divided by
 *   the real elapsed time between two reads, not by the nominal cadence
 *   (ROADMAP 11.2).
 *
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Collector spawned once at startup in src/web/mod.rs; queries via
 *   handlers/metrics.rs. The bucket math and the sample math are pure and
 *   unit-tested.
 * =============================================================================
 */

//! Metrics collector (60 s cadence) + time-bucketing for the history API.

use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;

/// Nominal sample cadence — one row per minute. This is *not* the divisor for
/// the rate series: `sample_from` uses the real distance between two reads, so
/// a suspend or a missed tick cannot inflate or deflate a throughput figure.
pub const SAMPLE_INTERVAL_SECS: u64 = 60;
/// Samples older than this are pruned (gives the 7-day chart one day of
/// headroom).
const RETENTION_SECS: i64 = 8 * 24 * 3600;

/// A named time range the UI can ask for, with its bucket size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricsRange {
    /// 60 × 1-minute buckets.
    OneHour,
    /// 96 × 15-minute buckets.
    OneDay,
    /// 84 × 2-hour buckets.
    SevenDays,
}

impl MetricsRange {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "1h" => Some(MetricsRange::OneHour),
            "24h" => Some(MetricsRange::OneDay),
            "7d" => Some(MetricsRange::SevenDays),
            _ => None,
        }
    }

    /// (bucket width, lookback window) in seconds.
    pub fn window(&self) -> (i64, i64) {
        match self {
            MetricsRange::OneHour => (60, 3600),
            MetricsRange::OneDay => (15 * 60, 24 * 3600),
            MetricsRange::SevenDays => (2 * 3600, 7 * 24 * 3600),
        }
    }
}

/// One raw stored sample, as read back from `metric_samples`.
///
/// Every series is independently nullable: `NULL` means *unknown* (the read
/// failed), which is a different statement from `0.0` (the host really was
/// idle). The collector writes `NULL` on failure and the bucketing keeps each
/// series' gaps separate, so one unreadable file never drags the other four.
#[derive(Debug, Clone, Copy, PartialEq, sqlx::FromRow)]
pub struct RawSample {
    pub ts: i64,
    pub cpu: Option<f64>,
    pub mem: Option<f64>,
    pub load1: Option<f64>,
    pub net_rx: Option<f64>,
    pub net_tx: Option<f64>,
    pub temp: Option<f64>,
}

/// One bucketed point of a series (averages over the bucket, timestamps in
/// unix seconds — the bucket start).
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsPoint {
    pub ts: i64,
    pub cpu: Option<f64>,
    pub mem: Option<f64>,
    pub load1: Option<f64>,
    pub net_rx: Option<f64>,
    pub net_tx: Option<f64>,
    pub temp: Option<f64>,
}

/// One series' accumulator inside a bucket: only the samples that actually
/// carry a value count, so a gap in one series does not dilute its neighbours.
#[derive(Debug, Default, Clone, Copy)]
struct SeriesAcc {
    sum: f64,
    n: u32,
}

impl SeriesAcc {
    fn push(&mut self, value: Option<f64>) {
        if let Some(v) = value {
            self.sum += v;
            self.n += 1;
        }
    }

    fn mean(&self) -> Option<f64> {
        (self.n > 0).then(|| self.sum / f64::from(self.n))
    }
}

/// Buckets raw samples for one range: `bucket_width`-second buckets covering
/// the last `window` seconds, each the average of the samples it covers. The
/// trailing bucket is the one currently in progress (rolling window), so the
/// chart's right edge stays live. Buckets without samples are returned as
/// all-`None` so the chart draws a gap rather than lying with a flat line —
/// the point count is stable (`window / bucket_width + 1`), which keeps the
/// x-axis simple.
///
/// Each series averages on its own: a bucket where `cpu` came back twice and
/// `temp` once reports the mean of the two CPU readings and the one
/// temperature, not a CPU figure halved by the missing thermal zone.
pub fn bucket_samples(
    samples: &[RawSample],
    bucket_width: i64,
    window: i64,
    now: i64,
) -> Vec<MetricsPoint> {
    let first_bucket = (now - window) / bucket_width;
    let count = (window / bucket_width + 1) as usize;

    // Exact averages: one accumulator per bucket (sums + per-series counts),
    // emitted in stable bucket order below. Per-bucket sample counts are
    // small (≤ bucket_width / 60 s cadence) so a Vec indexed by bucket is
    // plenty and keeps the output ordered by construction.
    #[derive(Default, Clone, Copy)]
    struct Acc {
        cpu: SeriesAcc,
        mem: SeriesAcc,
        load1: SeriesAcc,
        net_rx: SeriesAcc,
        net_tx: SeriesAcc,
        temp: Option<f64>,
    }
    let mut accs: Vec<Acc> = vec![Acc::default(); count];

    for sample in samples {
        let bucket = sample.ts / bucket_width;
        if bucket < first_bucket {
            continue;
        }
        let idx = (bucket - first_bucket) as usize;
        if idx >= count {
            continue;
        }
        let acc = &mut accs[idx];
        acc.cpu.push(sample.cpu);
        acc.mem.push(sample.mem);
        acc.load1.push(sample.load1);
        acc.net_rx.push(sample.net_rx);
        acc.net_tx.push(sample.net_tx);
        acc.temp = match (acc.temp, sample.temp) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, b) => b,
        };
    }

    accs.into_iter()
        .enumerate()
        .map(|(i, acc)| MetricsPoint {
            ts: (first_bucket + i as i64) * bucket_width,
            cpu: acc.cpu.mean(),
            mem: acc.mem.mean(),
            load1: acc.load1.mean(),
            net_rx: acc.net_rx.mean(),
            net_tx: acc.net_tx.mean(),
            temp: acc.temp,
        })
        .collect()
}

// =============================================================================
// Host readings — every parser is pure so it can be tested on fixtures
// =============================================================================

/// One instant of host readings. Each field is independently fallible: one
/// unreadable file leaves its own field `None` and does not invalidate the
/// others.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RawCounters {
    /// (total jiffies, idle jiffies) from `/proc/stat`.
    pub cpu: Option<(u64, u64)>,
    /// Memory usage % from `/proc/meminfo`.
    pub mem: Option<f64>,
    /// 1-minute load average from `/proc/loadavg`.
    pub load1: Option<f64>,
    /// (rx bytes, tx bytes) summed over the non-loopback interfaces.
    pub net: Option<(u64, u64)>,
    /// Hottest thermal zone in °C.
    pub temp: Option<f64>,
}

/// One sample resolved against the previous instant, ready to store.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SampleValues {
    pub cpu: Option<f64>,
    pub mem: Option<f64>,
    pub load1: Option<f64>,
    pub net_rx: Option<f64>,
    pub net_tx: Option<f64>,
    pub temp: Option<f64>,
}

/// Resolves one instant against the previous one.
///
/// `elapsed_secs` is the real wall-clock distance between the two reads, never
/// the nominal `SAMPLE_INTERVAL_SECS`: after a suspend, or after a tick the
/// collector could not serve, the counters moved over the whole gap and
/// dividing by 60 s would over-report throughput by the ratio of the two.
///
/// CPU is a ratio of two counter deltas, so it needs no elapsed time; when
/// there is no delta to work with (first sample after a boot, or counters that
/// went backwards) it falls back to the since-boot average, which is what the
/// figure means at that point.
pub fn sample_from(
    prev: Option<&RawCounters>,
    cur: &RawCounters,
    elapsed_secs: f64,
) -> SampleValues {
    let cpu = match (prev.and_then(|p| p.cpu), cur.cpu) {
        (Some((prev_total, prev_idle)), Some((total, idle))) if total > prev_total => {
            let d_total = (total - prev_total) as f64;
            let d_idle = idle.saturating_sub(prev_idle) as f64;
            (d_total > 0.0).then(|| ((d_total - d_idle) / d_total) * 100.0)
        }
        (_, counters) => counters.and_then(since_boot_percent),
    };

    let (net_rx, net_tx) = match (prev.and_then(|p| p.net), cur.net) {
        // A rate needs two points in time. Without a previous instant, with a
        // clock that did not advance, or after a counter reset (reboot), the
        // throughput is *unknown*: `None` draws a gap, a zero would be a
        // measurement nobody made.
        (Some((prev_rx, prev_tx)), Some((rx, tx)))
            if elapsed_secs > 0.0 && rx >= prev_rx && tx >= prev_tx =>
        {
            (
                Some((rx - prev_rx) as f64 / elapsed_secs),
                Some((tx - prev_tx) as f64 / elapsed_secs),
            )
        }
        _ => (None, None),
    };

    SampleValues {
        cpu,
        mem: cur.mem,
        load1: cur.load1,
        net_rx,
        net_tx,
        temp: cur.temp,
    }
}

/// CPU busy share since boot, from a `(total, idle)` jiffy pair. Used only
/// where a delta is unavailable — it is the average over the whole uptime, not
/// over the last minute.
fn since_boot_percent((total, idle): (u64, u64)) -> Option<f64> {
    if total == 0 {
        return None;
    }
    Some((total.saturating_sub(idle) as f64 / total as f64) * 100.0)
}

/// Parses the aggregate `cpu` line of `/proc/stat` into `(total, idle)`
/// jiffies, `idle` being idle + iowait (the definition the kernel's own tools
/// use).
fn parse_proc_stat(content: &str) -> Option<(u64, u64)> {
    let line = content
        .lines()
        .next()?
        .split_whitespace()
        .collect::<Vec<_>>();
    if line.len() < 5 || line[0] != "cpu" {
        return None;
    }
    let vals: Vec<u64> = line[1..].iter().filter_map(|v| v.parse().ok()).collect();
    let idle = vals.get(3).copied().unwrap_or(0) + vals.get(4).copied().unwrap_or(0);
    let total: u64 = vals.iter().sum();
    Some((total, idle))
}

/// Memory usage % from `/proc/meminfo` (`(MemTotal - MemAvailable) / MemTotal`).
fn parse_meminfo(content: &str) -> Option<f64> {
    let mut total = 0u64;
    let mut available = 0u64;
    for line in content.lines() {
        if let Some(v) = line.strip_prefix("MemTotal:") {
            total = v.trim().trim_end_matches(" kB").parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("MemAvailable:") {
            available = v.trim().trim_end_matches(" kB").parse().unwrap_or(0);
        }
    }
    if total == 0 {
        return None;
    }
    Some(((total.saturating_sub(available)) as f64 / total as f64) * 100.0)
}

/// 1-minute load average from `/proc/loadavg`.
fn parse_loadavg(content: &str) -> Option<f64> {
    content.split_whitespace().next()?.parse().ok()
}

/// (rx_bytes, tx_bytes) summed over all non-loopback interfaces of
/// `/proc/net/dev`. An interface list without any non-loopback entry is a real
/// `(0, 0)` — the host genuinely moved nothing — which is why the "file could
/// not be read" case is signalled by `None` at the caller, not here.
fn parse_net_dev(content: &str) -> (u64, u64) {
    let mut rx = 0u64;
    let mut tx = 0u64;
    for line in content.lines().skip(2) {
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        if iface.trim() == "lo" {
            continue;
        }
        let fields: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|f| f.parse().ok())
            .collect();
        rx += fields.first().copied().unwrap_or(0);
        tx += fields.get(8).copied().unwrap_or(0);
    }
    (rx, tx)
}

/// One thermal-zone `temp` file (milli-degrees Celsius) as °C.
fn parse_thermal_temp(raw: &str) -> Option<f64> {
    raw.trim().parse::<f64>().ok().map(|millis| millis / 1000.0)
}

/// Max temperature across `/sys/class/thermal` zones.
fn max_temperature() -> Option<f64> {
    let mut best: Option<f64> = None;
    if let Ok(entries) = std::fs::read_dir("/sys/class/thermal") {
        for entry in entries.flatten() {
            let path = entry.path().join("temp");
            if let Ok(raw) = std::fs::read_to_string(&path) {
                if let Some(c) = parse_thermal_temp(&raw) {
                    best = Some(best.map_or(c, |b: f64| b.max(c)));
                }
            }
        }
    }
    best
}

/// One instant of host readings — each file is read exactly once. A failed
/// read leaves its field `None`; it neither fails the sample nor becomes a
/// zero.
fn read_counters() -> RawCounters {
    let stat = std::fs::read_to_string("/proc/stat").ok();
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok();
    let loadavg = std::fs::read_to_string("/proc/loadavg").ok();
    let net_dev = std::fs::read_to_string("/proc/net/dev").ok();

    RawCounters {
        cpu: stat.as_deref().and_then(parse_proc_stat),
        mem: meminfo.as_deref().and_then(parse_meminfo),
        load1: loadavg.as_deref().and_then(parse_loadavg),
        net: net_dev.as_deref().map(parse_net_dev),
        temp: max_temperature(),
    }
}

// =============================================================================
// Collector loop
// =============================================================================

/// Collector state between samples: the previous instant of counters and when
/// it was read, so the next throughput divides by the real elapsed time.
#[derive(Debug, Clone, Copy)]
struct PrevSample {
    at: i64,
    counters: RawCounters,
}

/// Runs forever. Intended to be `tokio::spawn`ed once at server startup.
pub async fn run_collector_loop(pool: Arc<SqlitePool>) {
    let mut prev: Option<PrevSample> = None;
    // First sample fires immediately (so the chart isn't empty for a minute),
    // then every SAMPLE_INTERVAL_SECS. `Skip`, not the default `Burst`: after a
    // suspend or a stalled tick, `Burst` fires the missed ticks back-to-back,
    // they all land in the same minute bucket and `INSERT OR REPLACE` keeps
    // only the last one — silent data loss plus a burst of sub-second rates.
    let mut interval = tokio::time::interval(Duration::from_secs(SAMPLE_INTERVAL_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let at = chrono::Utc::now().timestamp();
        let counters = read_counters();
        let elapsed_secs = prev
            .map(|p| (at - p.at).max(0) as f64)
            .unwrap_or(0.0);
        let values = sample_from(prev.as_ref().map(|p| &p.counters), &counters, elapsed_secs);
        if let Err(e) = store_sample(pool.as_ref(), at, &values).await {
            tracing::warn!("Metrics collector tick failed: {}", e);
        }
        prev = Some(PrevSample { at, counters });
    }
}

/// Stores one resolved sample (a `NULL` stays a `NULL`) and prunes beyond
/// retention.
async fn store_sample(pool: &SqlitePool, ts: i64, values: &SampleValues) -> Result<(), String> {
    sqlx::query(
        "INSERT OR REPLACE INTO metric_samples (ts, cpu, mem, load1, net_rx, net_tx, temp) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(ts)
    .bind(values.cpu)
    .bind(values.mem)
    .bind(values.load1)
    .bind(values.net_rx)
    .bind(values.net_tx)
    .bind(values.temp)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // Retention — cheap (indexed PK range delete), run on every tick so the
    // bound holds even if ticks are far apart.
    sqlx::query("DELETE FROM metric_samples WHERE ts < ?")
        .bind(ts - RETENTION_SECS)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shorthand for a stored sample; every series is optional by design.
    fn sample(
        ts: i64,
        cpu: Option<f64>,
        mem: Option<f64>,
        load1: Option<f64>,
        net_rx: Option<f64>,
        net_tx: Option<f64>,
        temp: Option<f64>,
    ) -> RawSample {
        RawSample {
            ts,
            cpu,
            mem,
            load1,
            net_rx,
            net_tx,
            temp,
        }
    }

    #[test]
    fn range_parsing_and_windows() {
        assert_eq!(MetricsRange::parse("1h"), Some(MetricsRange::OneHour));
        assert_eq!(MetricsRange::parse("24h"), Some(MetricsRange::OneDay));
        assert_eq!(MetricsRange::parse("7d"), Some(MetricsRange::SevenDays));
        assert_eq!(MetricsRange::parse("90d"), None);
        let (w, lookback) = MetricsRange::OneHour.window();
        assert_eq!((w, lookback), (60, 3600));
        assert_eq!(MetricsRange::SevenDays.window().0, 7200);
    }

    #[test]
    fn bucketing_averages_within_buckets_and_gaps_are_none() {
        // 3-minute window of 1-minute buckets; samples at t=0 and t=30 share
        // bucket 0 → cpu 20 avg; t=150 alone in bucket 2. Bucket 3 is the
        // in-progress trailing bucket (empty here).
        let samples = vec![
            sample(0, Some(10.0), Some(1.0), Some(0.1), Some(1.0), Some(1.0), None),
            sample(30, Some(30.0), Some(2.0), Some(0.2), Some(2.0), Some(2.0), None),
            sample(
                150,
                Some(50.0),
                Some(3.0),
                Some(0.3),
                Some(3.0),
                Some(3.0),
                Some(45.0),
            ),
        ];
        let out = bucket_samples(&samples, 60, 180, 180);
        assert_eq!(out.len(), 4, "one point per bucket + the in-progress one");
        assert_eq!(out[0].cpu, Some(20.0), "samples in the same bucket average");
        assert_eq!(out[1].cpu, None, "empty bucket is a gap, not zero");
        assert_eq!(out[2].cpu, Some(50.0));
        assert_eq!(out[2].temp, Some(45.0));
        assert_eq!(out[3].cpu, None);
        assert_eq!(out[0].ts, 0, "bucket timestamps are bucket starts");
    }

    #[test]
    fn bucketing_ignores_samples_outside_the_window() {
        let samples = vec![
            sample(0, Some(99.0), Some(99.0), Some(99.0), Some(99.0), Some(99.0), None),
            sample(
                5000,
                Some(10.0),
                Some(10.0),
                Some(1.0),
                Some(1.0),
                Some(1.0),
                None,
            ),
        ];
        let out = bucket_samples(&samples, 60, 3600, 5000);
        assert_eq!(out.len(), 61);
        // The old sample must not leak into the window, and `now`'s own
        // bucket must be present.
        assert_eq!(out[0].cpu, None);
        assert_eq!(out[60].cpu, Some(10.0));
    }

    #[test]
    fn temperature_takes_the_max_within_a_bucket() {
        let samples = vec![
            sample(0, Some(1.0), Some(1.0), Some(0.0), Some(0.0), Some(0.0), Some(40.0)),
            sample(10, Some(1.0), Some(1.0), Some(0.0), Some(0.0), Some(0.0), Some(55.0)),
        ];
        let out = bucket_samples(&samples, 60, 60, 59);
        assert_eq!(out[0].temp, Some(55.0));
    }

    #[test]
    fn a_null_series_averages_on_its_own() {
        // One bucket, two samples: CPU came back on the first read, memory on
        // the second. Each mean must be over the samples that carried a value
        // — halving the CPU figure because the meminfo read failed would be a
        // fabricated measurement (ROADMAP 11.1).
        let samples = vec![
            sample(0, Some(50.0), None, Some(0.5), None, None, None),
            sample(10, None, Some(40.0), None, Some(100.0), Some(200.0), Some(44.0)),
        ];
        let out = bucket_samples(&samples, 60, 60, 59);
        assert_eq!(out[0].cpu, Some(50.0), "one CPU reading, not 50/2");
        assert_eq!(out[0].mem, Some(40.0), "one memory reading, not 40/2");
        assert_eq!(out[0].load1, Some(0.5));
        assert_eq!(out[0].net_rx, Some(100.0));
        assert_eq!(out[0].net_tx, Some(200.0));
        assert_eq!(out[0].temp, Some(44.0));
    }

    #[test]
    fn an_all_null_bucket_is_a_gap_for_every_series() {
        let samples = vec![sample(0, None, None, None, None, None, None)];
        let out = bucket_samples(&samples, 60, 60, 59);
        assert_eq!(out[0].cpu, None);
        assert_eq!(out[0].mem, None);
        assert_eq!(out[0].load1, None);
        assert_eq!(out[0].net_rx, None);
        assert_eq!(out[0].net_tx, None);
        assert_eq!(out[0].temp, None);
    }

    #[test]
    fn a_failed_read_stores_none_never_zero() {
        let prev = RawCounters {
            cpu: Some((1_000, 400)),
            mem: Some(30.0),
            load1: Some(0.5),
            net: Some((1_000, 2_000)),
            temp: Some(40.0),
        };
        // Every file except /proc/stat failed to parse on this tick.
        let cur = RawCounters {
            cpu: Some((2_000, 900)),
            ..RawCounters::default()
        };
        let v = sample_from(Some(&prev), &cur, 60.0);
        assert_eq!(v.cpu, Some(50.0), "the CPU delta still works");
        assert_eq!(v.mem, None, "a failed meminfo read is unknown, not 0 %");
        assert_eq!(v.load1, None, "a failed loadavg read is unknown, not 0");
        assert_eq!(v.net_rx, None, "a failed net/dev read is unknown, not 0 B/s");
        assert_eq!(v.net_tx, None);
        assert_eq!(v.temp, None, "a host without thermal zones has no temperature");
    }

    #[test]
    fn a_wholly_failed_tick_stores_nothing_but_nulls() {
        let v = sample_from(None, &RawCounters::default(), 0.0);
        assert_eq!(
            v,
            SampleValues {
                cpu: None,
                mem: None,
                load1: None,
                net_rx: None,
                net_tx: None,
                temp: None,
            }
        );
    }

    #[test]
    fn network_rate_divides_by_the_real_elapsed_time() {
        let prev = RawCounters {
            net: Some((1_000, 2_000)),
            ..RawCounters::default()
        };
        let cur = RawCounters {
            net: Some((2_200, 4_400)),
            ..RawCounters::default()
        };
        // 1 200 bytes over 120 s (a suspend, not the nominal 60 s) is 10 B/s.
        // Dividing by SAMPLE_INTERVAL_SECS would report 20 B/s.
        let v = sample_from(Some(&prev), &cur, 120.0);
        assert_eq!(v.net_rx, Some(10.0));
        assert_eq!(v.net_tx, Some(20.0));
    }

    #[test]
    fn network_rate_is_unknown_without_a_previous_instant() {
        let cur = RawCounters {
            net: Some((2_000, 4_000)),
            ..RawCounters::default()
        };
        let v = sample_from(None, &cur, 0.0);
        assert_eq!(v.net_rx, None, "no first-sample zero: the rate is unknown");
        assert_eq!(v.net_tx, None);
    }

    #[test]
    fn network_rate_is_unknown_after_a_counter_reset() {
        let prev = RawCounters {
            net: Some((5_000, 6_000)),
            ..RawCounters::default()
        };
        let cur = RawCounters {
            net: Some((10, 20)),
            ..RawCounters::default()
        };
        let v = sample_from(Some(&prev), &cur, 60.0);
        assert_eq!(v.net_rx, None, "a rebooted counter is not a negative rate");
        assert_eq!(v.net_tx, None);
    }

    #[test]
    fn network_rate_is_unknown_when_the_clock_did_not_advance() {
        let prev = RawCounters {
            net: Some((1_000, 1_000)),
            ..RawCounters::default()
        };
        let cur = RawCounters {
            net: Some((2_000, 3_000)),
            ..RawCounters::default()
        };
        let v = sample_from(Some(&prev), &cur, 0.0);
        assert_eq!(v.net_rx, None, "division by a zero interval is unknown");
        assert_eq!(v.net_tx, None);
    }

    #[test]
    fn cpu_uses_the_counter_delta_between_two_reads() {
        let prev = RawCounters {
            cpu: Some((1_000, 400)),
            ..RawCounters::default()
        };
        let cur = RawCounters {
            cpu: Some((2_000, 900)),
            ..RawCounters::default()
        };
        // d_total = 1000, d_idle = 500 → 50 % busy.
        assert_eq!(sample_from(Some(&prev), &cur, 60.0).cpu, Some(50.0));
    }

    #[test]
    fn cpu_falls_back_to_the_since_boot_average_without_a_previous_read() {
        let cur = RawCounters {
            cpu: Some((2_000, 1_000)),
            ..RawCounters::default()
        };
        assert_eq!(sample_from(None, &cur, 0.0).cpu, Some(50.0));
        // Counters that went backwards (a reboot between two reads) take the
        // same branch instead of underflowing.
        let prev = RawCounters {
            cpu: Some((9_000, 8_000)),
            ..RawCounters::default()
        };
        assert_eq!(sample_from(Some(&prev), &cur, 60.0).cpu, Some(50.0));
    }

    #[test]
    fn parse_proc_stat_reads_total_and_idle_plus_iowait() {
        let fixture = "cpu  1000 200 300 4000 500 0 0 0 0 0\ncpu0 1 2 3 4 5\n";
        assert_eq!(parse_proc_stat(fixture), Some((6_000, 4_500)));
        assert_eq!(since_boot_percent((6_000, 4_500)), Some(25.0));
        assert_eq!(parse_proc_stat(""), None, "no aggregate line");
        assert_eq!(parse_proc_stat("intr 12345 0 0"), None);
        assert_eq!(parse_proc_stat("cpu 1 2"), None, "too few fields");
    }

    #[test]
    fn parse_meminfo_is_usage_over_memtotal() {
        let fixture = "MemTotal:       16384000 kB\nMemFree:         1000000 kB\nMemAvailable:    8192000 kB\n";
        assert_eq!(parse_meminfo(fixture), Some(50.0));
        assert_eq!(
            parse_meminfo("MemFree: 10 kB\n"),
            None,
            "no MemTotal → unknown, not 0 %"
        );
        assert_eq!(
            parse_meminfo("MemTotal: 1000 kB\nMemAvailable: 4000 kB\n"),
            Some(0.0),
            "available above total clamps to 0 %, never a negative"
        );
    }

    #[test]
    fn parse_loadavg_takes_the_one_minute_field() {
        assert_eq!(parse_loadavg("0.42 0.51 0.60 2/345 6789\n"), Some(0.42));
        assert_eq!(parse_loadavg(""), None);
        assert_eq!(parse_loadavg("nonsense\n"), None);
    }

    #[test]
    fn parse_net_dev_skips_loopback_and_sums_interfaces() {
        let fixture = concat!(
            "Inter-|   Receive                                                |  Transmit\n",
            " face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n",
            "    lo: 999999 1 0 0 0 0 0 0 888888 1 0 0 0 0 0 0\n",
            "  eth0: 1000 10 0 0 0 0 0 0 2000 10 0 0 0 0 0 0\n",
            "  eth1: 500 5 0 0 0 0 0 0 250 5 0 0 0 0 0 0\n",
        );
        assert_eq!(parse_net_dev(fixture), (1_500, 2_250));
        // Only loopback: a real zero, not an error (the caller signals a read
        // failure with `None`).
        assert_eq!(parse_net_dev("h1\nh2\n    lo: 5 1 0 0 0 0 0 0 6 1 0 0 0 0 0 0\n"), (0, 0));
    }

    #[test]
    fn parse_thermal_temp_converts_millidegrees() {
        assert_eq!(parse_thermal_temp("42500\n"), Some(42.5));
        assert_eq!(parse_thermal_temp("garbage"), None);
        assert_eq!(parse_thermal_temp(""), None);
    }

    #[test]
    fn proc_readers_tolerate_missing_files() {
        // These read the *real* /proc and /sys on the test host — they must
        // return `None`/defaults, never panic, on any Linux box (and in CI
        // containers where thermal zones may not exist).
        let _ = read_counters();
        let _ = max_temperature();
    }
}
