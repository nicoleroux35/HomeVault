-- Configurable metrics retention (tasklist P1-3).
--
-- The collector used to prune at a hard-coded 8 days. A singleton row keeps
-- the existing value as the default, so an upgrade changes nothing until an
-- admin chooses 30 or 90 days in the Metrics app.
CREATE TABLE IF NOT EXISTS metrics_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    retention_days INTEGER NOT NULL DEFAULT 8
);

INSERT OR IGNORE INTO metrics_settings (id, retention_days) VALUES (1, 8);
