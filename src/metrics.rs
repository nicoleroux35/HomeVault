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
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Collector spawned once at startup in src/web/mod.rs; queries via
 *   handlers/metrics.rs. Bucket math is pure and unit-tested.
 * =============================================================================
 */

//! Metrics collector (60 s cadence) + time-bucketing for the history API.

use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;

/// Sample cadence — one row per minute.
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

/// One raw stored sample: (ts, cpu, mem, load1, net_rx, net_tx, temp).
pub type RawSample = (i64, f64, f64, f64, f64, f64, Option<f64>);

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

/// Buckets raw samples for one range: `bucket_width`-second buckets covering
/// the last `window` seconds, each the average of the samples it covers. The
/// trailing bucket is the one currently in progress (rolling window), so the
/// chart's right edge stays live. Buckets without samples are returned as
/// all-`None` so the chart draws a gap rather than lying with a flat line —
/// the point count is stable (`window / bucket_width + 1`), which keeps the
/// x-axis simple.
pub fn bucket_samples(
    samples: &[RawSample],
    bucket_width: i64,
    window: i64,
    now: i64,
) -> Vec<MetricsPoint> {
    let first_bucket = (now - window) / bucket_width;
    let count = (window / bucket_width + 1) as usize;

    // Exact averages: one accumulator per bucket (sums + sample count),
    // emitted in stable bucket order below. Per-bucket sample counts are
    // small (≤ bucket_width / 60 s cadence) so a Vec indexed by bucket is
    // plenty and keeps the output ordered by construction.
    struct Acc {
        cpu: f64,
        mem: f64,
        load1: f64,
        net_rx: f64,
        net_tx: f64,
        temp: Option<f64>,
        n: u32,
    }
    let mut accs: Vec<Acc> = (0..count)
        .map(|_| Acc {
            cpu: 0.0,
            mem: 0.0,
            load1: 0.0,
            net_rx: 0.0,
            net_tx: 0.0,
            temp: None,
            n: 0,
        })
        .collect();

    for (ts, cpu, mem, load1, net_rx, net_tx, temp) in samples {
        let (ts, cpu, mem, load1, net_rx, net_tx, temp) =
            (*ts, *cpu, *mem, *load1, *net_rx, *net_tx, *temp);
        let bucket = ts / bucket_width;
        if bucket < first_bucket {
            continue;
        }
        let idx = (bucket - first_bucket) as usize;
        if idx >= count {
            continue;
        }
        let acc = &mut accs[idx];
        acc.cpu += cpu;
        acc.mem += mem;
        acc.load1 += load1;
        acc.net_rx += net_rx;
        acc.net_tx += net_tx;
        acc.temp = match (acc.temp, temp) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, b) => b,
        };
        acc.n += 1;
    }

    accs.into_iter()
        .enumerate()
        .map(|(i, acc)| {
            let n = acc.n as f64;
            MetricsPoint {
                ts: (first_bucket + i as i64) * bucket_width,
                cpu: (acc.n > 0).then(|| acc.cpu / n),
                mem: (acc.n > 0).then(|| acc.mem / n),
                load1: (acc.n > 0).then(|| acc.load1 / n),
                net_rx: (acc.n > 0).then(|| acc.net_rx / n),
                net_tx: (acc.n > 0).then(|| acc.net_tx / n),
                temp: acc.temp,
            }
        })
        .collect()
}

/// Reads overall CPU usage from /proc/stat (one shot — the *percentage over
/// the whole period since boot*, which is why the collector computes deltas
/// between consecutive samples instead of using this alone).
fn cpu_usage_percent_now() -> Option<f64> {
    let content = std::fs::read_to_string("/proc/stat").ok()?;
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
    if total == 0 {
        return None;
    }
    Some(((total - idle) as f64 / total as f64) * 100.0)
}

/// (total, available) memory in KiB from /proc/meminfo.
fn memory_usage_percent() -> Option<f64> {
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
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
    Some(((total - available) as f64 / total as f64) * 100.0)
}

/// 1-minute load average from /proc/loadavg.
fn load1() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// (rx_bytes, tx_bytes) summed over all non-loopback interfaces.
fn network_counters() -> (u64, u64) {
    let Ok(content) = std::fs::read_to_string("/proc/net/dev") else {
        return (0, 0);
    };
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

/// Max temperature across /sys/class/thermal zones (milli°C files).
fn max_temperature() -> Option<f64> {
    let mut best: Option<f64> = None;
    if let Ok(entries) = std::fs::read_dir("/sys/class/thermal") {
        for entry in entries.flatten() {
            let path = entry.path().join("temp");
            if let Ok(raw) = std::fs::read_to_string(&path) {
                if let Ok(millis) = raw.trim().parse::<f64>() {
                    let c = millis / 1000.0;
                    best = Some(best.map_or(c, |b: f64| b.max(c)));
                }
            }
        }
    }
    best
}

/// Collector state between samples: the /proc counters the first sample
/// needs a delta against.
struct PrevCounters {
    cpu: Option<(u64, u64)>, // (total, idle)
    net: Option<(u64, u64)>, // (rx, tx)
}

/// Runs forever. Intended to be `tokio::spawn`ed once at server startup.
pub async fn run_collector_loop(pool: Arc<SqlitePool>) {
    let mut prev = PrevCounters {
        cpu: read_cpu_counters(),
        net: Some(network_counters()),
    };
    // First sample fires immediately (so the chart isn't empty for a
    // minute), then every SAMPLE_INTERVAL_SECS.
    let mut interval = tokio::time::interval(Duration::from_secs(SAMPLE_INTERVAL_SECS));
    loop {
        interval.tick().await;
        if let Err(e) = sample_and_store(pool.as_ref(), &mut prev).await {
            tracing::warn!("Metrics collector tick failed: {}", e);
        }
    }
}

fn read_cpu_counters() -> Option<(u64, u64)> {
    let content = std::fs::read_to_string("/proc/stat").ok()?;
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

/// Takes one sample (computing the rate-based series from the previous
/// counters), stores it and prunes beyond retention.
async fn sample_and_store(pool: &SqlitePool, prev: &mut PrevCounters) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp();

    let cpu = match (read_cpu_counters(), prev.cpu) {
        (Some((total, idle)), Some((ptotal, pidle))) if total > ptotal => {
            let d_total = (total - ptotal) as f64;
            let d_idle = (idle.saturating_sub(pidle)) as f64;
            if d_total > 0.0 {
                Some(((d_total - d_idle) / d_total) * 100.0)
            } else {
                None
            }
        }
        _ => cpu_usage_percent_now(), // fallback for the very first sample
    };
    if let Some((t, i)) = read_cpu_counters() {
        prev.cpu = Some((t, i));
    }

    let mem = memory_usage_percent().unwrap_or(0.0);
    let load1 = load1().unwrap_or(0.0);

    let (net_rx, net_tx) = match (network_counters(), prev.net) {
        ((rx, tx), Some((prx, ptx))) if rx >= prx && tx >= ptx => {
            let dt = SAMPLE_INTERVAL_SECS as f64;
            ((rx - prx) as f64 / dt, (tx - ptx) as f64 / dt)
        }
        ((rx, tx), _) => {
            // Counter reset (reboot) or first sample: store zeros this round
            // and seed the counters.
            prev.net = Some((rx, tx));
            (0.0, 0.0)
        }
    };
    prev.net = Some(network_counters());

    let temp = max_temperature();

    sqlx::query(
        "INSERT OR REPLACE INTO metric_samples (ts, cpu, mem, load1, net_rx, net_tx, temp) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(now)
    .bind(cpu.unwrap_or(0.0))
    .bind(mem)
    .bind(load1)
    .bind(net_rx)
    .bind(net_tx)
    .bind(temp)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // Retention — cheap (indexed PK range delete), run on every tick so the
    // bound holds even if ticks are far apart.
    sqlx::query("DELETE FROM metric_samples WHERE ts < ?")
        .bind(now - RETENTION_SECS)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
            (0i64, 10.0, 1.0, 0.1, 1.0, 1.0, None),
            (30, 30.0, 2.0, 0.2, 2.0, 2.0, None),
            (150, 50.0, 3.0, 0.3, 3.0, 3.0, Some(45.0)),
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
            (0, 99.0, 99.0, 99.0, 99.0, 99.0, None), // far before the window
            (5000, 10.0, 10.0, 1.0, 1.0, 1.0, None), // inside (in-progress bucket)
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
            (0, 1.0, 1.0, 0.0, 0.0, 0.0, Some(40.0)),
            (10, 1.0, 1.0, 0.0, 0.0, 0.0, Some(55.0)),
        ];
        let out = bucket_samples(&samples, 60, 60, 59);
        assert_eq!(out[0].temp, Some(55.0));
    }

    #[test]
    fn proc_parsers_handle_missing_files_gracefully() {
        // These read the *real* /proc on the test host — they must return
        // None/defaults, never panic, on any Linux box (and in CI containers
        // where thermal zones may not exist).
        let _ = cpu_usage_percent_now();
        let _ = memory_usage_percent();
        let _ = load1();
        let _ = network_counters();
        let _ = max_temperature();
        let _ = read_cpu_counters();
    }
}
