-- Migration: 011_restic_repos.sql
-- Restic repository configuration and backup job tracking

CREATE TABLE IF NOT EXISTS restic_repos (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    repository TEXT NOT NULL,
    password TEXT NOT NULL,
    retention_policy_id TEXT,
    source_paths TEXT NOT NULL DEFAULT '[]',
    exclude_patterns TEXT,
    tags TEXT,
    snapshot_source TEXT NOT NULL DEFAULT 'directory',
    snapshot_pool TEXT,
    snapshot_dataset TEXT,
    snapshot_create_before_backup INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (retention_policy_id) REFERENCES backup_retention_policies(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS restic_backup_jobs (
    id TEXT PRIMARY KEY,
    repo_id TEXT NOT NULL,
    snapshot_id TEXT,
    source TEXT NOT NULL,
    backup_type TEXT NOT NULL DEFAULT 'manual',
    status TEXT NOT NULL DEFAULT 'running',
    files_new INTEGER DEFAULT 0,
    files_changed INTEGER DEFAULT 0,
    files_unmodified INTEGER DEFAULT 0,
    data_added_bytes INTEGER DEFAULT 0,
    total_files_processed INTEGER DEFAULT 0,
    total_bytes INTEGER DEFAULT 0,
    duration_secs INTEGER,
    error_message TEXT,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (repo_id) REFERENCES restic_repos(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS restic_schedules (
    id TEXT PRIMARY KEY,
    repo_id TEXT NOT NULL,
    name TEXT NOT NULL,
    cron_expression TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_run_at TEXT,
    next_run_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (repo_id) REFERENCES restic_repos(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_restic_backup_jobs_repo ON restic_backup_jobs(repo_id);
CREATE INDEX IF NOT EXISTS idx_restic_backup_jobs_status ON restic_backup_jobs(status);
CREATE INDEX IF NOT EXISTS idx_restic_schedules_repo ON restic_schedules(repo_id);
CREATE INDEX IF NOT EXISTS idx_restic_schedules_enabled ON restic_schedules(enabled);
