-- Password Recovery Codes
-- Stores an Argon2 hash of each user's numeric recovery code (8 groups of 4
-- digits, ~106 bits of entropy), shown once at setup and used to reset a
-- forgotten password without email. The plaintext code is never stored.

CREATE TABLE IF NOT EXISTS password_recovery_codes (
    username TEXT PRIMARY KEY,
    code_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (username) REFERENCES users(username) ON DELETE CASCADE
);
