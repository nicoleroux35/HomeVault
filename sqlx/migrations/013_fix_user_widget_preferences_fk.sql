-- Fixes a malformed foreign key: `user_widget_preferences.user_id` referenced
-- `users(id)`, a column that has never existed (`users` is keyed by
-- `username TEXT PRIMARY KEY`). With `PRAGMA foreign_keys = ON`, SQLite
-- validates every foreign key referencing a table whenever that table is
-- modified, so this broken constraint caused *any* `UPDATE users ...`
-- statement (password changes, password resets, locking a user, etc.) to
-- fail with "foreign key mismatch", regardless of whether widget
-- preferences were involved at all.
CREATE TABLE user_widget_preferences_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    widget_key TEXT NOT NULL,
    position_x INTEGER NOT NULL DEFAULT 0,
    position_y INTEGER NOT NULL DEFAULT 0,
    is_visible BOOLEAN NOT NULL DEFAULT 1,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(username) ON DELETE CASCADE,
    UNIQUE(user_id, widget_key)
);

INSERT INTO user_widget_preferences_new (id, user_id, widget_key, position_x, position_y, is_visible, created_at, updated_at)
SELECT id, user_id, widget_key, position_x, position_y, is_visible, created_at, updated_at
FROM user_widget_preferences
WHERE user_id IN (SELECT username FROM users);

DROP TABLE user_widget_preferences;
ALTER TABLE user_widget_preferences_new RENAME TO user_widget_preferences;

CREATE INDEX IF NOT EXISTS idx_user_widget_prefs_user_id ON user_widget_preferences(user_id);
CREATE INDEX IF NOT EXISTS idx_user_widget_prefs_widget_key ON user_widget_preferences(widget_key);
