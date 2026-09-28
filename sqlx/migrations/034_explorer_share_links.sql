-- Explorer file link sharing ("share by link"): one row per public download
-- link a user created from the Explorer. The token is the only thing a
-- recipient needs, so it is high-entropy and never logged.
CREATE TABLE IF NOT EXISTS explorer_share_links (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    token      TEXT    NOT NULL UNIQUE,
    username   TEXT    NOT NULL,
    path       TEXT    NOT NULL,
    mode       TEXT    NOT NULL DEFAULT 'anyone',
    expires_at TEXT,
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_explorer_share_links_token ON explorer_share_links(token);
CREATE INDEX IF NOT EXISTS idx_explorer_share_links_username ON explorer_share_links(username);