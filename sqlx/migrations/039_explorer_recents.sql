-- Recently opened paths per user, powering the Explorer "Recents" section.
-- Bounded per user (latest 50 kept); rows are personal data with the same
-- retention story as favorites (deleted with the user account, see
-- handle_user_delete).
CREATE TABLE IF NOT EXISTS explorer_recents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL,
    path TEXT NOT NULL,
    last_opened_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(username, path)
);
CREATE INDEX IF NOT EXISTS idx_explorer_recents_user_time
    ON explorer_recents(username, last_opened_at DESC);
