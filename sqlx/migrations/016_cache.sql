-- Cache tier: a designated fast pool that absorbs writes and is later
-- drained to the "array" (a slower target pool) by the mover.

-- Singleton settings row (id is always 1).
CREATE TABLE IF NOT EXISTS cache_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    cache_pool TEXT,
    mover_cron TEXT NOT NULL DEFAULT '0 3 * * *',
    mover_enabled INTEGER NOT NULL DEFAULT 0,
    min_free_percent INTEGER NOT NULL DEFAULT 20,
    mover_last_run_at TEXT,
    updated_at TEXT NOT NULL
);

-- One row per "cache share": a named dataset that lives both on the cache
-- pool and on its target (array) pool, with a cache mode describing how the
-- mover should move data between the two.
CREATE TABLE IF NOT EXISTS cache_shares (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    cache_mode TEXT NOT NULL DEFAULT 'yes',
    target_pool TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- History of mover runs (manual or scheduled).
CREATE TABLE IF NOT EXISTS cache_mover_runs (
    id TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    files_moved INTEGER NOT NULL DEFAULT 0,
    bytes_moved INTEGER NOT NULL DEFAULT 0,
    error_message TEXT
);
