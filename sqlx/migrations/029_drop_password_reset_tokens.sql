-- Remove the email password-reset feature.
-- Superseded by the numeric recovery code (migration 028). The
-- email/token "forgot password" flow and all its code were deleted;
-- this drops the now-unused table.

DROP INDEX IF EXISTS idx_password_reset_tokens_token;
DROP INDEX IF EXISTS idx_password_reset_tokens_username;
DROP INDEX IF EXISTS idx_password_reset_tokens_expires_at;
DROP TABLE IF EXISTS password_reset_tokens;
