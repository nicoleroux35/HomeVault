-- Per-share outcome of a cache mover run (ROADMAP 2.2).
--
-- A run row alone cannot answer "which shares are now split across the two
-- tiers?" — one failing share used to abort the whole run silently. One row per
-- enabled share per run records what happened, so the UI can name the shares
-- that were not moved.
CREATE TABLE IF NOT EXISTS cache_mover_run_shares (
    run_id TEXT NOT NULL,
    share_name TEXT NOT NULL,
    mode TEXT NOT NULL,
    -- 'moved' | 'skipped' | 'failed'
    status TEXT NOT NULL,
    files_moved INTEGER NOT NULL DEFAULT 0,
    bytes_moved INTEGER NOT NULL DEFAULT 0,
    message TEXT,
    PRIMARY KEY (run_id, share_name)
);
