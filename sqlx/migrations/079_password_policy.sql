-- Global password policy (tasklist P1-2).
--
-- `expiry_days = 0` is the historical behaviour: a password never expires.
-- A positive value is enforced at sign-in: the account is flagged
-- `must_change_password`, which `middleware::password_change_guard` already
-- limits to the password-change flow.
--
-- `password_changed_at` is deliberately not added to the `User` model: only
-- the expiry check and the two password-writing paths need it, and NULL is a
-- valid legacy value (the expiry falls back to `created_at`).
CREATE TABLE IF NOT EXISTS password_policy (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    expiry_days INTEGER NOT NULL DEFAULT 0
);

INSERT OR IGNORE INTO password_policy (id, expiry_days) VALUES (1, 0);

ALTER TABLE users ADD COLUMN password_changed_at TEXT;

-- Existing rows have no recorded change: the account creation date is the
-- only truthful approximation, and it keeps a freshly upgraded box from
-- expiring every password at once.
UPDATE users SET password_changed_at = created_at WHERE password_changed_at IS NULL;
