-- Per-account desktop weather-widget location, stored server-side so the
-- same account converges on the same chosen city across browser origins
-- (e.g. a LAN IP and a public IP forwarded to the same server), which the
-- frontend's per-origin localStorage alone cannot provide. Mirrors
-- 023_user_monitor_layout.sql's shape/reasoning but stays a separate table,
-- matching weatherLocation.ts's own deliberate separation from monitorStore.
CREATE TABLE IF NOT EXISTS user_weather_location (
    username TEXT PRIMARY KEY NOT NULL,
    location_json TEXT NOT NULL,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (username) REFERENCES users(username) ON DELETE CASCADE
);
