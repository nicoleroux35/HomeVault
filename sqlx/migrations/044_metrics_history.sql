-- Migration: 044_metrics_history.sql
-- Historical performance metrics (see docs/guides/metrics.md).
--
-- One row per sample (60 s cadence from src/metrics.rs) with the four
-- series the Metrics app charts: CPU %, memory %, load average and network
-- throughput. Temps are optional — hosts without thermal zones just leave
-- them NULL.
--
-- Retention: the collector prunes rows older than 8 days on every tick, so
-- the table stays bounded (~11.5k rows) without a separate cleanup job.
-- Longer ranges (7 d) are served by bucketed averages at query time — at
-- this row count SQLite aggregate queries stay well under a millisecond.

CREATE TABLE IF NOT EXISTS metric_samples (
    ts       INTEGER PRIMARY KEY, -- unix seconds, one row per minute
    cpu      REAL NOT NULL,       -- overall CPU usage %
    mem      REAL NOT NULL,       -- memory usage %
    load1    REAL NOT NULL,       -- 1-minute load average
    net_rx   REAL NOT NULL,       -- average RX bytes/s over the interval
    net_tx   REAL NOT NULL,       -- average TX bytes/s over the interval
    temp     REAL                 -- max thermal-zone temperature °C (nullable)
);

CREATE INDEX IF NOT EXISTS idx_metric_samples_ts ON metric_samples (ts DESC);
