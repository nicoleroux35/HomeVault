-- Migration: 061_trash.sql
-- Retention for the Explorer Trash: one appliance-wide policy controlling how
-- long deleted files are kept and how large the trash may grow before a
-- background pass removes the oldest items.
--
-- Until now the only way a trashed file ever left the disk was the user
-- emptying the trash by hand. A forgotten trash with a few large videos in it
-- could quietly consume tens of gigabytes. This singleton row holds the age
-- and size limits; `last_purge_at` lets the scheduler run the pass on its
-- configured cadence instead of on every tick.
--
-- Additive: a fresh install (or an upgraded one) reads the default row from
-- `TrashPolicySettings::default()` — 30 days and 10 GiB — which is the
-- behaviour the settings screen shows before anything is saved.

CREATE TABLE IF NOT EXISTS trash_policy (
    -- Singleton row; the CHECK keeps a second configuration from ever being
    -- written by a buggy query.
    id INTEGER PRIMARY KEY CHECK (id = 1),
    -- Days after which a trashed item is permanently removed. 0 = keep forever.
    retention_days INTEGER NOT NULL DEFAULT 30,
    -- Total size ceiling for one trash root, in bytes. 0 = no ceiling.
    max_bytes INTEGER NOT NULL DEFAULT 10737418240,
    -- When the background pass last ran, RFC 3339. NULL = never.
    last_purge_at TEXT,
    updated_at TEXT NOT NULL
);
