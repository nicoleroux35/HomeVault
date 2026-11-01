-- Snapshot replication jobs (`docs/guides/replication.md`, phase P2.4).
--
-- One row per scheduled push of a ZFS/Btrfs dataset to another dataset (local
-- or over SSH). The job carries no plaintext credential: authentication is the
-- dedicated SSH key HomeVault generates for the `homevault` account.
CREATE TABLE IF NOT EXISTS replication_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    source_dataset TEXT NOT NULL,
    -- 'zfs' | 'btrfs'
    backend TEXT NOT NULL,
    -- 'local' (same host, another dataset) or 'ssh'
    destination_kind TEXT NOT NULL,
    destination_dataset TEXT NOT NULL,
    destination_host TEXT,
    destination_user TEXT,
    destination_port INTEGER NOT NULL DEFAULT 22,
    -- Idle minutes between runs; 0 disables the schedule (run-now only).
    interval_minutes INTEGER NOT NULL DEFAULT 1440,
    -- How many HomeVault-created destination snapshots to keep (destination-
    -- only deletion; the source is never pruned by this job).
    keep_last INTEGER NOT NULL DEFAULT 7,
    -- Optional `pv -L <kbps>` throttle; 0 = unthrottled.
    bandwidth_kbps INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 0,
    next_run_at TEXT,
    last_run_at TEXT,
    -- The source snapshot the last successful run replicated; the next run
    -- sends an increment from it (`zfs send -i`/`btrfs send -p`).
    last_snapshot TEXT,
    last_status TEXT,
    last_error TEXT,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_replication_jobs_name
    ON replication_jobs(name COLLATE NOCASE);
