-- Server-side explorer favorites, per user.
CREATE TABLE IF NOT EXISTS explorer_favorites (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    username   TEXT    NOT NULL,
    path       TEXT    NOT NULL,
    label      TEXT    NOT NULL DEFAULT '',
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE(username, path)
);

CREATE INDEX IF NOT EXISTS idx_explorer_favorites_username ON explorer_favorites(username);
