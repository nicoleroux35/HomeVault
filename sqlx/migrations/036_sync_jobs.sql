-- Migration: 036_sync_jobs.sql
-- Persist Cloud Sync (rclone) job definitions server-side. Previously the
-- frontend kept sync jobs in localStorage, so jobs were per-browser and lost
-- on cache clear. Execution stays one-shot (POST /api/rclone/jobs/{id}/run
-- reuses the `sync` handler); no scheduler is implied by this table.

CREATE TABLE IF NOT EXISTS sync_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    source_type TEXT NOT NULL DEFAULT 'cloud',
    source TEXT NOT NULL,
    source_path TEXT,
    dest_type TEXT NOT NULL DEFAULT 'cloud',
    destination TEXT NOT NULL,
    dest_path TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sync_jobs_created_at ON sync_jobs(created_at);