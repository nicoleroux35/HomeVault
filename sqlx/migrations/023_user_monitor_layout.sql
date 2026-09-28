-- Per-account desktop monitor layout (position + visibility of the floating
-- Resource/RAID/Weather/... monitor panels), stored server-side so the same
-- account converges on the same layout across browser origins (e.g. a LAN
-- IP and a public IP forwarded to the same server), which the frontend's
-- per-origin localStorage alone cannot provide.
CREATE TABLE IF NOT EXISTS user_monitor_layout (
    username TEXT PRIMARY KEY NOT NULL,
    layout_json TEXT NOT NULL,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (username) REFERENCES users(username) ON DELETE CASCADE
);
