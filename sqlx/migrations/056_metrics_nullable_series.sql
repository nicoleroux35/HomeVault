-- Migration: 056_metrics_nullable_series.sql
-- ROADMAP 11.1 — a failed /proc read was stored as 0.0.
--
-- `044_metrics_history.sql` declared cpu/mem/load1/net_rx/net_tx `NOT NULL`,
-- so `sample_and_store` had no way to say "this read failed" and wrote a
-- plausible-looking zero instead: a broken sensor, a missing /proc entry and a
-- genuinely idle host all produced the same chart. `temp` was already
-- nullable, which is why the collector could tell the truth about thermal
-- zones and nothing else.
--
-- SQLite cannot drop a NOT NULL constraint in place, so the table is rebuilt.
-- The existing rows are copied verbatim (their values were real readings; only
-- the *future* writes change shape) and the index is recreated, because
-- DROP TABLE takes its indexes with it.

CREATE TABLE metric_samples_new (
    ts       INTEGER PRIMARY KEY, -- unix seconds, one row per sample
    cpu      REAL,                -- overall CPU usage % (NULL = read failed)
    mem      REAL,                -- memory usage % (NULL = read failed)
    load1    REAL,                -- 1-minute load average (NULL = read failed)
    net_rx   REAL,                -- average RX bytes/s over the interval
    net_tx   REAL,                -- average TX bytes/s over the interval
    temp     REAL                 -- max thermal-zone temperature °C
);

INSERT INTO metric_samples_new (ts, cpu, mem, load1, net_rx, net_tx, temp)
SELECT ts, cpu, mem, load1, net_rx, net_tx, temp FROM metric_samples;

DROP TABLE metric_samples;

ALTER TABLE metric_samples_new RENAME TO metric_samples;

CREATE INDEX IF NOT EXISTS idx_metric_samples_ts ON metric_samples (ts DESC);
