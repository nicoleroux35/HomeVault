-- Migration: 037_setup_state.sql
-- Move the first-run setup state out of loose marker files in the data
-- directory and into the SQLite database itself.
--
-- Before this table, "is this appliance configured?" was answered by the
-- presence of `.setup_complete` (a flag file) plus `.system_hash` (the hardware
-- fingerprint), while the administrator account itself lived in the database.
-- The two could disagree: losing the data directory while the DB survived
-- re-opened the Setup Wizard even though an admin still existed, and the
-- reverse dead-ended the wizard on "Setup already complete" after a hardware
-- change. Marking a completed setup also lived outside the transaction that
-- creates the admin row.
--
-- Now the `users` table is the single source of truth for "an admin exists"
-- (see `storage/setup_state_repo.rs::has_admin`) and this singleton row carries
-- the hardware fingerprint, so the account and the setup state travel together
-- with the rest of the identity data.

CREATE TABLE IF NOT EXISTS setup_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    completed_at TEXT NOT NULL,
    system_hash TEXT NOT NULL,
    machine_id TEXT NOT NULL
);
