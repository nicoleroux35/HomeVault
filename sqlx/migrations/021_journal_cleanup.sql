-- Auto-cleanup for HomeVault's own rotating application log files (the
-- "app" source in src/handlers/journal.rs, files under
-- src/config/paths.rs::logs_dir()). User-configurable and off by default —
-- when enabled, a background scheduler (src/journal_cleanup_scheduler.rs)
-- deletes those log files every `interval_days` days.

-- Singleton settings row (id is always 1).
CREATE TABLE IF NOT EXISTS journal_cleanup_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    auto_cleanup_enabled INTEGER NOT NULL DEFAULT 0,
    interval_days INTEGER NOT NULL DEFAULT 30,
    last_cleanup_at TEXT,
    updated_at TEXT NOT NULL
);
