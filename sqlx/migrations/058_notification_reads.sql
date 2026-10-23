-- Migration: 058_notification_reads.sql
-- Per-user read state for system notifications (ROADMAP 21.2).
--
-- The old `notifications.read_at` column was a single global read timestamp:
-- one admin clicking "mark all read" cleared the badge for every account, and
-- `mark_read` had no user predicate. Read state now lives per
-- (notification, username) in this join table; the global column is dropped.

CREATE TABLE IF NOT EXISTS notification_reads (
    notification_id TEXT NOT NULL REFERENCES notifications(id) ON DELETE CASCADE,
    username        TEXT NOT NULL,
    read_at         TEXT NOT NULL,
    PRIMARY KEY (notification_id, username)
);

CREATE INDEX IF NOT EXISTS idx_notification_reads_username
    ON notification_reads(username);

-- Rebuild `notifications` without the global `read_at` column (SQLite cannot
-- drop a column in place). Historical read state cannot be attributed to a
-- user — the column was global — so it is dropped; previously-read rows
-- simply reappear unread once per account.
CREATE TABLE notifications_new (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    message TEXT NOT NULL,
    severity TEXT NOT NULL,
    category TEXT NOT NULL,
    source TEXT NOT NULL,
    created_at TEXT NOT NULL
);

INSERT INTO notifications_new (id, title, message, severity, category, source, created_at)
    SELECT id, title, message, severity, category, source, created_at
    FROM notifications;

DROP TABLE notifications;

ALTER TABLE notifications_new RENAME TO notifications;

CREATE INDEX IF NOT EXISTS idx_notifications_created_at ON notifications(created_at);
