-- Migration: 043_scheduled_tasks.sql
-- Generic scheduled-task engine (see docs/guides/scheduled-tasks.md).
--
-- One table + one background loop (`src/scheduled_tasks_scheduler.rs`)
-- replace what would otherwise be a bespoke scheduler per feature: host
-- power actions, S.M.A.R.T. self-tests, pool snapshots and (heavily
-- validated) admin commands all schedule through here, driven by the same
-- 5-field cron expressions the backup scheduler already parses.
--
-- `payload` is a JSON object whose shape depends on `task_type`; it is
-- validated by `crate::scheduled_tasks_scheduler::TaskAction::from_parts`
-- *before* a task can be created (not at run time), so a malformed row can
-- never reach the executor.
--
-- Run history (`scheduled_task_runs`) keeps the last executions per task so
-- the UI can show what ran, when, and with what output tail, without
-- overloading the broadcast `notifications` table (015).

CREATE TABLE IF NOT EXISTS scheduled_tasks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    task_type TEXT NOT NULL CHECK (task_type IN ('command', 'snapshot', 'smart_test', 'power')),
    cron TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '{}',
    enabled INTEGER NOT NULL DEFAULT 1,
    last_run_at TEXT,
    last_status TEXT,
    last_error TEXT,
    next_run_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS scheduled_task_runs (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES scheduled_tasks(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('running', 'success', 'failed')),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    output TEXT
);

CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_enabled ON scheduled_tasks(enabled);
CREATE INDEX IF NOT EXISTS idx_scheduled_task_runs_task ON scheduled_task_runs(task_id, started_at DESC);
