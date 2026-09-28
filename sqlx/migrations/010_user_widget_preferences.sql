-- Create table for user widget preferences
CREATE TABLE IF NOT EXISTS user_widget_preferences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    widget_key TEXT NOT NULL,
    position_x INTEGER NOT NULL DEFAULT 0,
    position_y INTEGER NOT NULL DEFAULT 0,
    is_visible BOOLEAN NOT NULL DEFAULT 1,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    UNIQUE(user_id, widget_key)
);

-- Create index for faster lookups
CREATE INDEX IF NOT EXISTS idx_user_widget_prefs_user_id ON user_widget_preferences(user_id);
CREATE INDEX IF NOT EXISTS idx_user_widget_prefs_widget_key ON user_widget_preferences(widget_key);
