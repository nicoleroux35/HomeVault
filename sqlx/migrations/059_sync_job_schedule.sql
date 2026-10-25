-- Migration: 059_sync_job_schedule.sql
-- ROADMAP 16.4: give a persisted Cloud Sync (rclone) job a cadence and a
-- server-side run state.
--
-- Until now `sync_jobs` stored the definition only, `POST /rclone/jobs/{id}/run`
-- was the only way a job ran, and the screen's "last run / status" columns were
-- the outcome of the click that had just happened. A nightly sync could not be
-- expressed at all.
--
-- Everything new is nullable and defaults to "no schedule", so an existing
-- appliance behaves exactly as before until an admin sets an interval or a
-- cron: `schedule_kind = 'interval'` with `interval_minutes`, or
-- `schedule_kind = 'cron'` with `cron_expr`. The state columns are written by
-- the server (the scheduler and the run path), never by the client.

ALTER TABLE sync_jobs ADD COLUMN schedule_kind TEXT NOT NULL DEFAULT 'none';
ALTER TABLE sync_jobs ADD COLUMN interval_minutes INTEGER;
ALTER TABLE sync_jobs ADD COLUMN cron_expr TEXT;
ALTER TABLE sync_jobs ADD COLUMN last_run_at TEXT;
ALTER TABLE sync_jobs ADD COLUMN last_status TEXT;
ALTER TABLE sync_jobs ADD COLUMN last_message TEXT;
ALTER TABLE sync_jobs ADD COLUMN next_run_at TEXT;

-- The scheduler tick only ever asks for enabled jobs whose due time has passed.
CREATE INDEX IF NOT EXISTS idx_sync_jobs_next_run ON sync_jobs(next_run_at);
