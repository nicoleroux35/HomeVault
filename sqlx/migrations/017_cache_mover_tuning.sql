-- Mover tuning for the cache tier:
--   * mover_age_days  — only move files modified more than N days ago
--   * mover_excludes  — space/comma separated glob patterns to skip
--   * mover_bwlimit  — rsync bandwidth limit in KB/s (0 = unlimited)

ALTER TABLE cache_settings ADD COLUMN mover_age_days INTEGER NOT NULL DEFAULT 0;
ALTER TABLE cache_settings ADD COLUMN mover_excludes TEXT;
ALTER TABLE cache_settings ADD COLUMN mover_bwlimit INTEGER NOT NULL DEFAULT 0;
