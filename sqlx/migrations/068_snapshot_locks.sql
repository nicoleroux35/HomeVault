-- Immutable snapshots (`docs/guides/snapshots.md`, phase P2.5).
--
-- A row is one retention lock HomeVault placed on one ZFS/Btrfs snapshot.
-- While a row is unreleased, the snapshot cannot be deleted through HomeVault:
-- a ZFS hold (`zfs hold`) / a read-only Btrfs subvolume enforces it at the
-- filesystem level, and the row is what the API consults before allowing a
-- delete. `released_at` marks a released lock; the row is kept as an audit
-- trail (who released it, when, and why — including break-glass).
CREATE TABLE IF NOT EXISTS snapshot_locks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    pool TEXT NOT NULL,
    snapshot TEXT NOT NULL,
    -- Filesystem-side lock tag (ZFS hold tag); unused by Btrfs, kept so an
    -- upgrade can find the hold it created even if the row is edited.
    tag TEXT NOT NULL,
    locked_until TEXT NOT NULL,
    reason TEXT,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    released_at TEXT,
    released_by TEXT,
    release_reason TEXT
);

-- One active lock per snapshot: a second unreleased row would make "can this
-- be deleted?" ambiguous. Released rows are excluded, so history is preserved.
CREATE UNIQUE INDEX IF NOT EXISTS idx_snapshot_locks_active
    ON snapshot_locks(pool, snapshot) WHERE released_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_snapshot_locks_pool
    ON snapshot_locks(pool, released_at);
