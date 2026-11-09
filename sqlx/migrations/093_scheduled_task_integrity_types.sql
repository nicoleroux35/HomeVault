-- The integrity lot added two task types (`scrub`, `restic_check`) to the
-- generic scheduler, while the service task type arrived with migration 087.
-- SQLite cannot widen a CHECK in place, so rebuild the tables again and keep
-- the run history. This is the union of both migrations:
--   command, snapshot, smart_test, scrub, restic_check, power, service
--
-- The rebuild renames the parent (never drops it while the child references
-- it), copies every row, and recreates the run table with its data so no
-- ON DELETE CASCADE fires.
ALTER TABLE scheduled_tasks RENAME TO scheduled_tasks_old;

CREATE TABLE scheduled_tasks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    task_type TEXT NOT NULL CHECK (task_type IN (
        'command', 'snapshot', 'smart_test', 'scrub', 'restic_check', 'power', 'service'
    )),
    cron TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '{}',
    enabled INTEGER NOT NULL DEFAULT 1,
    timezone TEXT,
    last_run_at TEXT,
    last_status TEXT,
    last_error TEXT,
    next_run_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO scheduled_tasks
    (id, name, task_type, cron, payload, enabled, timezone, last_run_at,
     last_status, last_error, next_run_at, created_at, updated_at)
    SELECT id, name, task_type, cron, payload, enabled, timezone, last_run_at,
           last_status, last_error, next_run_at, created_at, updated_at
    FROM scheduled_tasks_old;

CREATE TABLE scheduled_task_runs_new (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES scheduled_tasks(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('running', 'success', 'failed')),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    output TEXT
);

INSERT INTO scheduled_task_runs_new
    SELECT id, task_id, status, started_at, finished_at, output
    FROM scheduled_task_runs;

DROP TABLE scheduled_task_runs;
ALTER TABLE scheduled_task_runs_new RENAME TO scheduled_task_runs;
DROP TABLE scheduled_tasks_old;

CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_enabled ON scheduled_tasks(enabled);
CREATE INDEX IF NOT EXISTS idx_scheduled_task_runs_task
    ON scheduled_task_runs(task_id, started_at DESC);
