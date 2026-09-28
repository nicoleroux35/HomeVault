-- User home folders configuration
CREATE TABLE IF NOT EXISTS user_homes (
    username TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 0,
    disk TEXT,
    path TEXT,
    created_at TEXT NOT NULL
);
