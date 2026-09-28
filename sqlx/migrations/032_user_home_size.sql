-- Per-user home directory size limit (disk quota).
-- NULL or 0 means "unlimited" (no quota applied).
ALTER TABLE user_homes ADD COLUMN size_limit_bytes INTEGER;
