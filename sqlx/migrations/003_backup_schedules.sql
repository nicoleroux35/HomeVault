-- Migration: 003_backup_schedules.sql
-- Backup scheduling, retention policies, and metadata

-- Backup schedules table
CREATE TABLE IF NOT EXISTS backup_schedules (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    source TEXT NOT NULL,
    destination TEXT NOT NULL,
    schedule_type TEXT NOT NULL DEFAULT 'cron',
    cron_expression TEXT NOT NULL,
    encryption_enabled INTEGER NOT NULL DEFAULT 0,
    passphrase TEXT,
    retention_policy_id TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_run_at TEXT,
    next_run_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (retention_policy_id) REFERENCES backup_retention_policies(id) ON DELETE SET NULL
);

-- Backup retention policies table
CREATE TABLE IF NOT EXISTS backup_retention_policies (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    max_count INTEGER,
    max_age_days INTEGER,
    max_size_gb INTEGER,
    apply_to_all INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Backup metadata table
CREATE TABLE IF NOT EXISTS backup_metadata (
    id TEXT PRIMARY KEY,
    backup_path TEXT NOT NULL UNIQUE,
    tags TEXT,
    description TEXT,
    notes TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Create indexes for performance
CREATE INDEX IF NOT EXISTS idx_backup_schedules_enabled ON backup_schedules(enabled);
CREATE INDEX IF NOT EXISTS idx_backup_schedules_retention ON backup_schedules(retention_policy_id);
CREATE INDEX IF NOT EXISTS idx_backup_metadata_path ON backup_metadata(backup_path);
