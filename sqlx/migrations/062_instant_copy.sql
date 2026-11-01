-- Migration: 062_instant_copy.sql
-- Appliance-wide switch for the Explorer's instant copy (ROADMAP-DSM-QUICKWINS
-- QW-03, DSM's File Station "ReflinkCopy").
--
-- A copy on a copy-on-write filesystem (Btrfs, XFS with reflink=1, ZFS with
-- block cloning) can share the source's extents instead of rewriting every
-- byte: `FICLONE` makes a 40 GB copy complete in milliseconds and cost no
-- extra space until one side is modified. This singleton row decides whether
-- the Explorer tries that at all.
--
--   auto — try to clone every file; fall back to a kernel range copy and then
--          the verified byte stream when the filesystem refuses. This is the
--          behaviour on every filesystem, so it is the default.
--   off  — never clone, always the verified stream (for an administrator who
--          wants predictable allocation, or to rule the cascade out when
--          diagnosing a storage problem).
--
-- Additive: an install without a row reads `InstantCopyMode::default()`
-- (auto), which is exactly what the settings screen shows before saving.

CREATE TABLE IF NOT EXISTS instant_copy_policy (
    -- Singleton row; the CHECK keeps a second configuration from ever being
    -- written by a buggy query.
    id INTEGER PRIMARY KEY CHECK (id = 1),
    -- 'auto' | 'off'. Unrecognised values are read back as 'auto'.
    mode TEXT NOT NULL DEFAULT 'auto',
    updated_at TEXT NOT NULL
);
