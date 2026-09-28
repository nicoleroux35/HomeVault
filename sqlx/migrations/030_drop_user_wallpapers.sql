-- The custom wallpaper upload / gallery feature was removed: the desktop
-- wallpaper is now chosen only from the images bundled into the binary
-- (served from /wallpapers/), and the choice is persisted in the global
-- appearance settings (`wallpaper` string), not per user. Drop the now
-- unused table. Migration 007 stays as history.
DROP INDEX IF EXISTS idx_user_wallpapers_active;
DROP INDEX IF EXISTS idx_user_wallpapers_username;
DROP TABLE IF EXISTS user_wallpapers;
