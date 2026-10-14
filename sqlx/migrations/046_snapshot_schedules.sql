-- Migration: 046_snapshot_schedules.sql
-- Scheduled pool snapshots with GFS (grandfather-father-son) retention —
-- see docs/guides/snapshots.md.
--
-- One row per (pool, prefix) schedule. The executor (snapshot_scheduler.rs)
-- creates `{prefix}-{YYYYMMDD-HHMMSS}` snapshots on the cron, then prunes
-- ONLY snapshots carrying that prefix — manual and restic snapshots are
-- never touched. Retention counts: keep the newest N hourly / daily /
-- weekly / monthly / yearly generations; 0 disables a tier.
--
-- Note: the old /snapshots/{pool} endpoints were removed (unauthenticated);
-- these schedule endpoints are require_admin end to end.

CREATE TABLE IF NOT EXISTS snapshot_schedules (
    id TEXT PRIMARY KEY,
    pool TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    cron TEXT NOT NULL,
    prefix TEXT NOT NULL DEFAULT 'hv-auto',
    recursive INTEGER NOT NULL DEFAULT 1,
    gfs_hourly INTEGER NOT NULL DEFAULT 24,
    gfs_daily INTEGER NOT NULL DEFAULT 7,
    gfs_weekly INTEGER NOT NULL DEFAULT 4,
    gfs_monthly INTEGER NOT NULL DEFAULT 12,
    gfs_yearly INTEGER NOT NULL DEFAULT 1,
    last_run_at TEXT,
    last_status TEXT,
    last_error TEXT,
    next_run_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_snapshot_schedules_enabled ON snapshot_schedules(enabled);
