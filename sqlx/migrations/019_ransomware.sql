-- Anti-ransomware protection: honeypot decoy files + mass file-operation
-- rate detection on user-configured protected paths. Heuristic/best-effort,
-- not a guarantee — see docs/CLAUDE.md scope notes for this feature.

-- Singleton settings row (id is always 1).
CREATE TABLE IF NOT EXISTS ransomware_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0,
    honeypots_enabled INTEGER NOT NULL DEFAULT 1,
    rate_threshold_events INTEGER NOT NULL DEFAULT 50,
    rate_threshold_seconds INTEGER NOT NULL DEFAULT 10,
    auto_response TEXT NOT NULL DEFAULT 'alert_only',
    updated_at TEXT NOT NULL
);

-- User-configured paths (typically shares) watched by the fs watcher.
-- share_name/share_type identify the associated Share for the
-- 'disable_share' auto-response since Share has no numeric id (src/shares.rs).
CREATE TABLE IF NOT EXISTS ransomware_protected_paths (
    id TEXT PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    share_name TEXT,
    share_type TEXT,
    honeypot_filename TEXT,
    created_at TEXT NOT NULL
);

-- Detected suspicious activity (honeypot touch, mass modify/rename/delete).
CREATE TABLE IF NOT EXISTS ransomware_events (
    id TEXT PRIMARY KEY,
    protected_path_id TEXT REFERENCES ransomware_protected_paths(id) ON DELETE SET NULL,
    event_type TEXT NOT NULL,
    detail TEXT,
    suspected_username TEXT,
    response_taken TEXT NOT NULL DEFAULT 'alert_only',
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ransomware_events_path ON ransomware_events(protected_path_id);
