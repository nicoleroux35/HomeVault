-- Upload/download transfer quotas, configurable per user or per group from
-- the Users/Groups pages. Distinct from the pre-existing filesystem
-- disk-space quota system in src/quota.rs (which limits how much a group
-- may *store*, enforced via setquota/repquota against a real filesystem) —
-- this instead caps how much data a user or group may *upload*/*download*
-- in total, tracked entirely by HomeVault itself (no OS-level quota tool
-- involved).

CREATE TABLE IF NOT EXISTS transfer_quotas (
    id TEXT PRIMARY KEY,
    subject_type TEXT NOT NULL CHECK (subject_type IN ('user', 'group')),
    subject_name TEXT NOT NULL,
    upload_enabled INTEGER NOT NULL DEFAULT 0,
    upload_limit_bytes INTEGER NOT NULL DEFAULT 0,
    upload_used_bytes INTEGER NOT NULL DEFAULT 0,
    download_enabled INTEGER NOT NULL DEFAULT 0,
    download_limit_bytes INTEGER NOT NULL DEFAULT 0,
    download_used_bytes INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    UNIQUE(subject_type, subject_name)
);
