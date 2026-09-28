-- Migration: 014_encrypted_volumes.sql
-- LUKS2-encrypted standalone volumes. The passphrase is never stored here —
-- unlock is always manual, so only the metadata needed to address a device
-- once unlocked is persisted.

CREATE TABLE IF NOT EXISTS encrypted_volumes (
    id TEXT PRIMARY KEY,
    device_path TEXT NOT NULL UNIQUE,
    mapper_name TEXT NOT NULL,
    luks_uuid TEXT NOT NULL,
    filesystem TEXT NOT NULL,
    label TEXT NOT NULL,
    mount_point TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_encrypted_volumes_device ON encrypted_volumes(device_path);
