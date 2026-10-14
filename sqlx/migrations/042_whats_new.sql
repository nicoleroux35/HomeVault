-- Migration: 042_whats_new.sql
-- Per-user "What's New" acknowledgement.
--
-- After an update, the SPA shows a "What's New" modal summarising the release
-- highlights once per version per user. This table is the server-side record
-- of that acknowledgement, so the modal stays dismissed across browsers and
-- devices instead of living only in `localStorage`.
--
-- Broadcast notifications already have their own table (015_notifications.sql);
-- this is deliberately separate because acknowledgement is *per user*, whereas
-- notifications are broadcast-style.

CREATE TABLE IF NOT EXISTS whats_new_seen (
    user_id    TEXT NOT NULL,
    version    TEXT NOT NULL,
    seen_at    TEXT NOT NULL,
    PRIMARY KEY (user_id, version)
);

CREATE INDEX IF NOT EXISTS idx_whats_new_seen_user ON whats_new_seen (user_id);
