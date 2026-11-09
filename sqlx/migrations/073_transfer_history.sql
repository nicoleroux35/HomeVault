-- Migration: 073_transfer_history.sql
-- Transfer history / audit trail for the File Explorer (HV-ADV-02, DSM's
-- FileStation "completed tasks" list).
--
-- The desktop activity feed only shows work *in progress* (plus a one-minute
-- grace for a failure) — deliberately, so the taskbar never becomes a
-- graveyard. That leaves no answer to "who uploaded this, when, and how was it
-- copied?", which is what an administrator asks when a file appears, a copy
-- looks wrong, or a transfer failed last night. One row per finished
-- upload/download/copy/move answers it.
--
-- Design notes:
--   * `created_at`/`completed_at` are **unix seconds** (the appliance's
--     dominant convention for time-ordered tables), so range queries and
--     retention are plain integer comparisons.
--   * `method` keeps the reflink cascade's own slug (`clone`, `kernel_clone`,
--     `kernel_copy`, `stream`) — reporting the speed-up as "instant" is only
--     honest if it is recorded, and a mixed tree reports its slowest method.
--   * `verified` is set when the transfer's bytes were proven identical
--     (a SHA-256 match on upload, or a read-back hash on the copy stream), so
--     the UI can show "✓ verified" instead of implying it.
--   * Paths are stored as they were requested (already confined to the
--     caller's allowed roots by the handler). A path is not a secret here: the
--     row is only readable by its owner or an administrator.
--   * No foreign key on `username`: an account can be deleted while its
--     history stays useful to the administrator, exactly like the journal.

CREATE TABLE IF NOT EXISTS transfer_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Progress-manager job id when the transfer ran as a tracked job, else NULL.
    job_id TEXT,
    -- Account that started the transfer.
    username TEXT NOT NULL,
    -- 'upload' | 'download' | 'copy' | 'move' | 'checksum' | 'usb_copy'
    operation TEXT NOT NULL,
    -- NULL for an upload (the bytes came from the client, not from a path).
    source_path TEXT,
    -- NULL for a download (the destination is the client).
    dest_path TEXT,
    -- Declared or measured byte count. 0 when unknown (a directory move is
    -- reported as an item count in `error_message`-free rows).
    bytes_total INTEGER NOT NULL DEFAULT 0,
    -- Wall-clock duration, when the transfer ran long enough to measure.
    duration_ms INTEGER,
    -- Reflink cascade slug, or the checksum algorithm ('sha256', …).
    method TEXT,
    -- 1 when the bytes were proven identical (digest matched / read-back hash).
    verified INTEGER NOT NULL DEFAULT 0,
    -- 'complete' | 'failed' | 'cancelled'
    status TEXT NOT NULL,
    -- Failure reason, human-readable, never carries a credential.
    error_message TEXT,
    created_at INTEGER NOT NULL,
    completed_at INTEGER
);

-- The two queries the endpoint actually runs: "this user's recent transfers"
-- and "everything recent, for the administrator's audit view".
CREATE INDEX IF NOT EXISTS idx_transfer_history_user
    ON transfer_history(username, completed_at DESC);
CREATE INDEX IF NOT EXISTS idx_transfer_history_date
    ON transfer_history(completed_at DESC);
