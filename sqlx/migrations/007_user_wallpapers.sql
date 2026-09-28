-- Add user_wallpapers table for wallpaper gallery management
CREATE TABLE IF NOT EXISTS user_wallpapers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL REFERENCES users(username) ON DELETE CASCADE,
    filename TEXT NOT NULL UNIQUE,
    original_name TEXT,
    size_bytes INTEGER NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    uploaded_at TEXT NOT NULL DEFAULT (datetime('now')),
    is_active INTEGER NOT NULL DEFAULT 0,
    CHECK (is_active IN (0, 1))
);

-- Index for efficient queries
CREATE INDEX IF NOT EXISTS idx_user_wallpapers_username ON user_wallpapers(username);
CREATE INDEX IF NOT EXISTS idx_user_wallpapers_active ON user_wallpapers(username, is_active) WHERE is_active = 1;

-- Note: SQLite doesn't support multiple active wallpapers constraint directly
-- The application logic will ensure only one wallpaper is active per user
