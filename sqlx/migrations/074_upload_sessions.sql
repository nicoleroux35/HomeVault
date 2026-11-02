-- Migration: 074_upload_sessions.sql
-- Durable resumable-upload sessions (HV-UX-03).
--
-- ROADMAP 10.3 gave the Explorer chunked, resumable uploads, but the sessions
-- lived in one process-wide `HashMap`: restarting HomeVault dropped every
-- partial upload, and the client's next chunk got "session not found" while
-- the `.hvupl-*` temp file was still sitting on the pool. The row here is the
-- durable half; the map stays as a write-through cache on the chunk path.
--
-- Notes:
--   * `offset` only ever moves on a full chunk (see
--     `src/upload_session.rs::update_offset`) — the resumable protocol's
--     invariant.
--   * `last_chunk_at` is the idle clock the 30-minute TTL is measured on, so a
--     slow multi-gigabyte upload is not expired mid-flight, and the sweep has a
--     column to range over.
--   * `tmp_path` is the hidden sibling the chunks are written into. It is
--     stored so a resumed session appends to the file it started, never a new
--     one (which would leave an orphan and restart the upload).
--   * `expected_sha256` is the optional digest the client promised; it is
--     checked at `complete` and the file is refused on a mismatch.
--   * No foreign key on `username`: the row is deleted with the session (or by
--     the TTL sweep), and an account deleted mid-upload must not make the
--     session row un-deletable.

CREATE TABLE IF NOT EXISTS upload_sessions (
    session_id TEXT PRIMARY KEY,
    username TEXT NOT NULL,
    dir_path TEXT NOT NULL,
    filename TEXT NOT NULL,
    tmp_path TEXT NOT NULL,
    offset INTEGER NOT NULL DEFAULT 0,
    total_bytes INTEGER NOT NULL,
    -- 'overwrite' | 'keep_both' | 'skip'
    strategy TEXT NOT NULL DEFAULT 'overwrite',
    expected_sha256 TEXT,
    created_at INTEGER NOT NULL,
    last_chunk_at INTEGER NOT NULL,
    -- Throughput of the last accepted chunk (bytes/second), or NULL before the
    -- first one. The tray turns this into "12 MB/s · 4 min left"; it is a
    -- measurement, never an extrapolation from the session's start.
    speed_bps INTEGER
);

-- The TTL sweep, the per-user cap and the tray's per-user list all range on
-- these two columns.
CREATE INDEX IF NOT EXISTS idx_upload_sessions_idle
    ON upload_sessions(last_chunk_at);
CREATE INDEX IF NOT EXISTS idx_upload_sessions_user
    ON upload_sessions(username, last_chunk_at);
