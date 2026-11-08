-- Public file requests: a folder owner mints a link anyone can upload to
-- (optionally password-protected, expiring and capped). The destination is
-- stored canonically at creation; the public handler re-checks that the owner
-- still holds a write grant before accepting a file.
CREATE TABLE IF NOT EXISTS explorer_file_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token TEXT NOT NULL UNIQUE,
    username TEXT NOT NULL REFERENCES users(username) ON DELETE CASCADE,
    path TEXT NOT NULL,
    message TEXT NOT NULL DEFAULT '',
    password_hash TEXT,
    expires_at TEXT,
    max_uploads INTEGER,
    upload_count INTEGER NOT NULL DEFAULT 0,
    active_uploads INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_file_requests_username
    ON explorer_file_requests(username);
