-- Migration: 035_login_attempts_success_user_agent.sql
-- Persist real login outcomes + user agents in the audit history. The admin
-- login-history endpoints previously hardcoded `success = true` /
-- `user_agent = NULL` because the table only stored failed attempts and no
-- browser fingerprint.

ALTER TABLE login_attempts ADD COLUMN success INTEGER NOT NULL DEFAULT 0;
ALTER TABLE login_attempts ADD COLUMN user_agent TEXT;

CREATE INDEX IF NOT EXISTS idx_login_attempts_success ON login_attempts(success);