-- Per-share Btrfs compression policy (tasklist P2-11).
--
-- `path` is an absolute directory on a mounted Btrfs filesystem. The policy
-- is applied by `src/btrfs_maintenance.rs` (defragment with the requested
-- zstd level, then `chattr +c` so new files inherit compression). `level` is
-- `zstd:1`…`zstd:15` or `none`.
CREATE TABLE IF NOT EXISTS btrfs_compression_policies (
    path TEXT PRIMARY KEY,
    level TEXT NOT NULL,
    -- 1 = also defragment existing extents; 0 = only future files inherit.
    defrag INTEGER NOT NULL DEFAULT 1,
    last_applied_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_btrfs_compression_policies_level
    ON btrfs_compression_policies(level);
