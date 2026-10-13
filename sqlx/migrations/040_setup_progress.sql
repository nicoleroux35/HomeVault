-- Reboot-safe first-run progress: the Setup Wizard persists the current step
-- plus the operator's storage/RAID/restore choices so a reboot or browser
-- restart resumes where it left off instead of restarting at step 0.
-- Singleton row (id = 1). Only the choices are stored here — never secrets:
-- the encrypted-restore passphrase is kept in memory only and entered again
-- when the restore is applied after the admin account exists.
CREATE TABLE IF NOT EXISTS setup_progress (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    step INTEGER NOT NULL DEFAULT 0,
    data_json TEXT NOT NULL DEFAULT '{}',
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
INSERT OR IGNORE INTO setup_progress (id, step, data_json) VALUES (1, 0, '{}');
