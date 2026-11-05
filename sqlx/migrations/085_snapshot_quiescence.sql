-- Application quiescence hooks for snapshots (tasklist P2-13).
--
-- A hook runs before and after a snapshot so databases/VMs can flush. `pre`
-- and `post` are JSON arrays of argv strings (no shell). Hooks are opt-in and
-- global: every enabled hook is applied to every scheduled snapshot.
CREATE TABLE IF NOT EXISTS snapshot_quiescence_hooks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    pre_json TEXT NOT NULL,
    post_json TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_snapshot_quiescence_hooks_enabled
    ON snapshot_quiescence_hooks(enabled);
