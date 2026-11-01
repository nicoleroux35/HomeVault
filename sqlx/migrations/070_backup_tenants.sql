-- Backup destination role (`docs/guides/backup-target.md`, phase P2.3).
--
-- One row per source appliance allowed to push restic backups into this
-- machine. `token_hash` is a SHA-256 of the bearer token (never the token
-- itself); `repo_password_sealed` is the repository password sealed with the
-- appliance key (`crate::storage::secret_box`), used only to verify the
-- repository and to run the scheduled test restore.
CREATE TABLE IF NOT EXISTS backup_tenants (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    -- bcrypt hash for rest-server's htpasswd file, empty until a token is set.
    auth_hash TEXT NOT NULL DEFAULT '',
    token_hash TEXT NOT NULL DEFAULT '',
    repo_password_sealed TEXT NOT NULL DEFAULT '',
    repo_path TEXT NOT NULL,
    quota_bytes INTEGER NOT NULL DEFAULT 0,
    -- Set when the quota watchdog revoked this tenant's access.
    over_quota INTEGER NOT NULL DEFAULT 0,
    verify_enabled INTEGER NOT NULL DEFAULT 1,
    last_seen_at TEXT,
    last_verified_at TEXT,
    last_verify_status TEXT,
    last_verify_error TEXT,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_backup_tenants_name
    ON backup_tenants(name COLLATE NOCASE);
