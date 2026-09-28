-- Migration: 002_auth_security.sql
-- Authentication security improvements

-- Track failed login attempts per user
CREATE TABLE IF NOT EXISTS login_attempts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL,
    ip_address TEXT NOT NULL,
    attempted_at TEXT NOT NULL
);

-- Store hashed previous passwords for history
CREATE TABLE IF NOT EXISTS password_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- Store user's TOTP secret (encrypted)
CREATE TABLE IF NOT EXISTS user_2fa (
    username TEXT PRIMARY KEY,
    totp_secret_encrypted TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0,
    backup_codes TEXT,
    created_at TEXT NOT NULL
);

-- Create indexes for performance
CREATE INDEX IF NOT EXISTS idx_login_attempts_username ON login_attempts(username);
CREATE INDEX IF NOT EXISTS idx_login_attempts_at ON login_attempts(attempted_at);
CREATE INDEX IF NOT EXISTS idx_password_history_username ON password_history(username);
