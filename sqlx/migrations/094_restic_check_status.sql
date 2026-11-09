-- FIAB-14: surface the last `restic check` result in the Backup screen.
-- A scheduled check used to leave its verdict only in the task run history.
ALTER TABLE restic_repos ADD COLUMN last_check_ok INTEGER;
ALTER TABLE restic_repos ADD COLUMN last_check_at TEXT;
ALTER TABLE restic_repos ADD COLUMN last_check_error TEXT;
